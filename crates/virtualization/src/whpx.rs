//! WHPX (Windows Hypervisor Platform) backend.
//!
//! WHPX is the user-mode API exposed by `Win32_System_Hypervisor` that lets
//! third-party emulators drive Hyper-V partitions without requiring admin
//! privileges.
//!
//! ## Current state
//!
//! This module exposes the real WHPX partition creation API surface
//! (`WHvCreatePartition`, `WHvMapGpaRange`, `WHvCreateVirtualProcessor`).
//! The vCPU run loop is wired but the exit-reason dispatch is intentionally
//! simplified — the `windows` crate's WHPX bindings have evolved between
//! versions and exit-reason union access is brittle. Full exit dispatch
//! (MMIO, I/O, interrupts) requires runtime debugging on a real Windows host
//! with WHPX enabled, which is out of scope for CI.

use std::sync::Arc;

use nitroid_core::{InstanceConfig, Result};
use parking_lot::Mutex;
use tracing::{info, warn};
use windows::core::PCSTR;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Hypervisor::{
    WHvCreatePartition, WHvCreateVirtualProcessor, WHvDeletePartition,
    WHvDeleteVirtualProcessor, WHvMapGpaRange, WHvPartitionPropertyCodeProcessorCount,
    WHvRunVirtualProcessor, WHvSetPartitionProperty, WHvMapGpaRangeFlagRead,
    WHvMapGpaRangeFlagWrite, WHvMapGpaRangeFlagExecute, WHV_PARTITION_HANDLE,
    WHV_PARTITION_PROPERTY,
};
use windows::Win32::System::LibraryLoader::LoadLibraryA;

use crate::traits::{Backend, BackendCapabilities, BackendInfo, InputEvent, VmHandle};
use nitroid_core::CoreError;

/// Quick availability check — does `WinHvPlatform.dll` exist and can it be
/// loaded?
pub fn is_available() -> bool {
    unsafe { LoadLibraryA(PCSTR(b"WinHvPlatform.dll\0".as_ptr())).is_ok() }
}

pub struct WhpxBackend {
    _loaded: bool,
}

impl WhpxBackend {
    pub fn new() -> Result<Self> {
        if !is_available() {
            return Err(CoreError::VirtualizationUnavailable(
                "WHPX not available. Install Hyper-V from 'Turn Windows features on or off'.".into(),
            ));
        }
        Ok(Self { _loaded: true })
    }
}

impl Backend for WhpxBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            name: "WHPX (Windows Hypervisor Platform)".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            nested: false,
        }
    }

    fn capabilities(&self) -> Result<BackendCapabilities> {
        Ok(BackendCapabilities {
            max_vcpus: 64,
            max_memory_mb: 1 << 20,
            supports_arm_translation: false,
            supports_virtio_gpu: true,
        })
    }

    fn create_vm(&self, cfg: &InstanceConfig) -> Result<VmHandle> {
        cfg.validate()?;
        info!(instance = %cfg.name, vcpus = cfg.cpu_count, "WHPX create_vm: creating real partition");

        unsafe {
            // 1. Create the partition.
            let mut partition: WHV_PARTITION_HANDLE = std::mem::zeroed();
            let create_result = WHvCreatePartition(&mut partition);
            if create_result.is_err() {
                return Err(CoreError::Backend(format!(
                    "WHvCreatePartition failed: {:?}",
                    create_result
                )));
            }

            // 2. Set the processor count via the partition property. We
            //    write the property as raw bytes since the `windows` crate's
            //    union access for `WHV_PARTITION_PROPERTY` varies between
            //    versions.
            let cpu_count = cfg.cpu_count;
            let mut prop_bytes = [0u8; std::mem::size_of::<WHV_PARTITION_PROPERTY>()];
            // ProcessorCount is the first u32 in the union.
            prop_bytes[0..4].copy_from_slice(&cpu_count.to_le_bytes());
            let prop_ptr = &prop_bytes as *const _ as *const WHV_PARTITION_PROPERTY;
            let set_result = WHvSetPartitionProperty(
                partition,
                WHvPartitionPropertyCodeProcessorCount,
                prop_ptr,
                std::mem::size_of::<WHV_PARTITION_PROPERTY>() as u32,
            );
            if set_result.is_err() {
                let _ = WHvDeletePartition(partition);
                return Err(CoreError::Backend(format!(
                    "WHvSetPartitionProperty(ProcessorCount) failed: {:?}",
                    set_result
                )));
            }

            // 3. Allocate guest physical memory using VirtualAlloc.
            let mem_size = (cfg.memory_mb as usize) * 1024 * 1024;
            let host_ptr = windows::Win32::System::Memory::VirtualAlloc(
                None,
                mem_size,
                windows::Win32::System::Memory::MEM_COMMIT
                    | windows::Win32::System::Memory::MEM_RESERVE,
                windows::Win32::System::Memory::PAGE_READWRITE,
            );
            if host_ptr.is_null() {
                let _ = WHvDeletePartition(partition);
                return Err(CoreError::Backend(
                    "VirtualAlloc failed for guest memory".into(),
                ));
            }

            // 4. Map the GPA range into the partition with read/write/exec.
            let gpa_flags_raw =
                WHvMapGpaRangeFlagRead.0 | WHvMapGpaRangeFlagWrite.0 | WHvMapGpaRangeFlagExecute.0;
            let map_result = WHvMapGpaRange(
                partition,
                host_ptr,
                0,
                mem_size as u64,
                windows::Win32::System::Hypervisor::WHV_MAP_GPA_RANGE_FLAGS(gpa_flags_raw),
            );
            if map_result.is_err() {
                let _ = windows::Win32::System::Memory::VirtualFree(
                    host_ptr,
                    0,
                    windows::Win32::System::Memory::MEM_RELEASE,
                );
                let _ = WHvDeletePartition(partition);
                return Err(CoreError::Backend(format!(
                    "WHvMapGpaRange failed: {:?}",
                    map_result
                )));
            }

            // 5. Create one virtual processor per configured CPU.
            for vcpu_id in 0..cfg.cpu_count {
                let create_vp = WHvCreateVirtualProcessor(partition, vcpu_id as u32, 0);
                if create_vp.is_err() {
                    warn!(vcpu_id, error = ?, create_vp, "failed to create vCPU");
                    let _ = WHvDeletePartition(partition);
                    return Err(CoreError::Backend(format!(
                        "WHvCreateVirtualProcessor({vcpu_id}) failed: {:?}",
                        create_vp
                    )));
                }
            }

            let inner = Arc::new(WhpxVm {
                partition: Mutex::new(partition),
                host_ptr: Mutex::new(host_ptr),
                mem_size,
                config: Arc::new(cfg.clone()),
                running: Mutex::new(false),
                thread_handles: Mutex::new(Vec::new()),
            });
            Ok(VmHandle::new(inner))
        }
    }

    fn start(&self, vm: &mut VmHandle) -> Result<()> {
        let inner = vm
            .as_any()
            .downcast_ref::<Arc<WhpxVm>>()
            .ok_or_else(|| CoreError::Backend("not a WHPX VM".into()))?
            .clone();
        info!(instance = %inner.config.name, "WHPX start: spawning vCPU threads");

        {
            let mut running = inner.running.lock();
            if *running {
                return Err(CoreError::Backend("VM already running".into()));
            }
            *running = true;
        }

        let mut handles = Vec::with_capacity(inner.config.cpu_count as usize);
        for vcpu_id in 0..inner.config.cpu_count {
            let inner_for_thread = inner.clone();
            let handle = std::thread::Builder::new()
                .name(format!("whpx-vcpu-{}-{}", inner.config.name, vcpu_id))
                .spawn(move || run_whpx_vcpu(inner_for_thread, vcpu_id))
                .map_err(|e| {
                    CoreError::Backend(format!("failed to spawn WHPX vCPU thread: {e}"))
                })?;
            handles.push(handle);
        }
        *inner.thread_handles.lock() = handles;
        Ok(())
    }

    fn pause(&self, vm: &mut VmHandle) -> Result<()> {
        let inner = vm
            .as_any()
            .downcast_ref::<Arc<WhpxVm>>()
            .ok_or_else(|| CoreError::Backend("not a WHPX VM".into()))?
            .clone();
        *inner.running.lock() = false;
        info!(instance = %inner.config.name, "pause requested");
        Ok(())
    }

    fn resume(&self, vm: &mut VmHandle) -> Result<()> {
        let inner = vm
            .as_any()
            .downcast_ref::<Arc<WhpxVm>>()
            .ok_or_else(|| CoreError::Backend("not a WHPX VM".into()))?
            .clone();
        *inner.running.lock() = true;
        info!(instance = %inner.config.name, "resume requested");
        Ok(())
    }

    fn stop(&self, vm: &mut VmHandle) -> Result<()> {
        let inner = vm
            .as_any()
            .downcast_ref::<Arc<WhpxVm>>()
            .ok_or_else(|| CoreError::Backend("not a WHPX VM".into()))?
            .clone();
        info!(instance = %inner.config.name, "stopping WHPX VM");
        *inner.running.lock() = false;
        let handles: Vec<_> = inner.thread_handles.lock().drain(..).collect();
        for handle in handles {
            let _ = handle.join();
        }
        unsafe {
            let partition = *inner.partition.lock();
            for vcpu_id in 0..inner.config.cpu_count {
                let _ = WHvDeleteVirtualProcessor(partition, vcpu_id as u32);
            }
            let _ = WHvDeletePartition(partition);
            let ptr = *inner.host_ptr.lock();
            if !ptr.is_null() {
                let _ = windows::Win32::System::Memory::VirtualFree(
                    ptr,
                    0,
                    windows::Win32::System::Memory::MEM_RELEASE,
                );
            }
        }
        Ok(())
    }

    fn inject_input(&self, vm: &mut VmHandle, event: InputEvent) -> Result<()> {
        let inner = vm
            .as_any()
            .downcast_ref::<Arc<WhpxVm>>()
            .ok_or_else(|| CoreError::Backend("not a WHPX VM".into()))?
            .clone();
        tracing::debug!(?event, instance = %inner.config.name, "input injected");
        Ok(())
    }
}

/// The per-vCPU run loop for WHPX. Runs on its own OS thread.
///
/// We allocate a buffer for the exit reason and call `WHvRunVirtualProcessor`
/// in a loop. The exit reason dispatch is intentionally simple — full
/// dispatch (MMIO, I/O, interrupts) requires runtime debugging on a real
/// Windows host with WHPX enabled.
fn run_whpx_vcpu(vm: Arc<WhpxVm>, vcpu_id: u32) {
    info!(vcpu_id, "WHPX vCPU thread started");
    let partition = *vm.partition.lock();

    // The exit reason buffer is large enough for any exit reason variant.
    // 256 bytes is over-provisioned but safe.
    let mut exit_buf = [0u8; 256];

    while *vm.running.lock() {
        let result = unsafe {
            WHvRunVirtualProcessor(
                partition,
                vcpu_id,
                exit_buf.as_mut_ptr() as *mut _,
                exit_buf.len() as u32,
            )
        };
        if let Err(e) = result {
            warn!(vcpu_id, error = ?, e, "WHvRunVirtualProcessor failed");
            break;
        }

        // The exit reason type is the first u32 of the exit reason struct.
        // The exact layout depends on the windows crate version, but the
        // ExitReason field is always near the start. We log it for diagnostics
        // but don't dispatch on specific types yet — that's the follow-up
        // work that requires Windows desktop debugging.
        let exit_type = u32::from_le_bytes([
            exit_buf[0],
            exit_buf[1],
            exit_buf[2],
            exit_buf[3],
        ]);
        tracing::debug!(vcpu_id, exit_type, "WHPX exit reason");
    }
    info!(vcpu_id, "WHPX vCPU thread exiting");
}

struct WhpxVm {
    partition: Mutex<WHV_PARTITION_HANDLE>,
    host_ptr: Mutex<*mut std::ffi::c_void>,
    mem_size: usize,
    config: Arc<InstanceConfig>,
    running: Mutex<bool>,
    thread_handles: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

// SAFETY: WHV_PARTITION_HANDLE is a void* under the hood. The partition is
// thread-safe per the WHPX docs.
unsafe impl Send for WhpxVm {}
unsafe impl Sync for WhpxVm {}

#[allow(dead_code)]
fn _ensure_handle_import_used(_: HANDLE) {}
