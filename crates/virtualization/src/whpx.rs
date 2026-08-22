//! WHPX (Windows Hypervisor Platform) backend.
//!
//! WHPX is the user-mode API exposed by `Win32_System_Hypervisor` that lets
//! third-party emulators drive Hyper-V partitions without requiring admin
//! privileges. It's the recommended acceleration backend on Windows for
//! emulators like QEMU and us.
//!
//! ## Architecture
//!
//! A WHPX-backed VM has:
//!
//! 1. A partition (`WHV_PARTITION_HANDLE`) — the VM container
//! 2. Mapped guest physical memory ranges (`WHvMapGpaRange`)
//! 3. One or more virtual processors (`WHvCreateVirtualProcessor`)
//! 4. A run loop (`WHvRunVirtualProcessor`) that dispatches exit reasons
//!
//! Each vCPU runs on its own OS thread. The thread loops calling
//! `WHvRunVirtualProcessor`, which blocks until the guest exits (MMIO,
//! I/O, halt, etc.). The exit reason is dispatched to the appropriate
//! handler, then the loop continues.
//!
//! ## Caveats
//!
//! The `windows` crate's `Win32_System_Hypervisor` module is still evolving
//! between versions — different versions of the crate expose slightly
//! different function signatures and enum names. This module is written
//! defensively so it compiles across the 0.58 series. If CI breaks after
//! a `windows` crate bump, this is the first place to look.

use std::sync::Arc;

use nitroid_core::{InstanceConfig, Result};
use parking_lot::Mutex;
use tracing::{info, warn};
use windows::core::PCSTR;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Hypervisor::{
    WHvCreatePartition, WHvCreateVirtualProcessor, WHvDeletePartition, WHvDeleteVirtualProcessor,
    WHvGetPartitionCounters, WHvMapGpaRange, WHvMapGpaRangeFlagExecute, WHvMapGpaRangeFlagRead,
    WHvMapGpaRangeFlagWrite, WHvPartitionPropertyCodeProcessorCount, WHvRunVirtualProcessor,
    WHvSetPartitionProperty, WHV_MAP_GPA_RANGE_FLAGS, WHV_PARTITION_HANDLE, WHV_PARTITION_PROPERTY,
    WHV_RUN_VP_EXIT_REASON, WHV_VP_EXIT_REASON_TYPE,
};
use windows::Win32::System::LibraryLoader::LoadLibraryA;

use crate::traits::{Backend, BackendCapabilities, BackendInfo, InputEvent, VmHandle};
use nitroid_core::CoreError;

/// Quick availability check — does `WinHvPlatform.dll` exist and can it be
/// loaded? We don't keep the loaded handle around (the OS will keep the DLL
/// loaded for the lifetime of the process once we touch it), which lets the
/// backend be `Send + Sync` without an `unsafe impl` block.
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
                "WHPX not available. Install Hyper-V from 'Turn Windows features on or off'."
                    .into(),
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
            max_memory_mb: 1 << 20, // 1 TiB cap
            supports_arm_translation: false,
            supports_virtio_gpu: true,
        })
    }

    fn create_vm(&self, cfg: &InstanceConfig) -> Result<VmHandle> {
        cfg.validate()?;
        info!(instance = %cfg.name, vcpus = cfg.cpu_count, "WHPX create_vm: creating real partition");

        // SAFETY: All WHPX calls are unsafe because they're FFI into
        // WinHvPlatform.dll. We rely on the WHPX API's own validation
        // for parameter correctness — invalid parameters return errors,
        // they don't crash.
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

            // 2. Set the processor count.
            let mut prop = WHV_PARTITION_PROPERTY::default();
            *prop.u1.ProcessorCount_mut() = cfg.cpu_count;
            let set_result = WHvSetPartitionProperty(
                partition,
                WHvPartitionPropertyCodeProcessorCount,
                &prop,
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
            // We map one large region of cfg.memory_mb megabytes.
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

            // 4. Map the GPA range into the partition.
            let gpa_flags = WHV_MAP_GPA_RANGE_FLAGS(
                WHvMapGpaRangeFlagRead.0 | WHvMapGpaRangeFlagWrite.0 | WHvMapGpaRangeFlagExecute.0,
            );
            let map_result = WHvMapGpaRange(partition, host_ptr, 0, mem_size as u64, gpa_flags);
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

        // Spawn one thread per vCPU.
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
        // Wait for all vCPU threads to exit.
        let handles: Vec<_> = inner.thread_handles.lock().drain(..).collect();
        for handle in handles {
            let _ = handle.join();
        }
        // Delete the partition + unmap memory.
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
fn run_whpx_vcpu(vm: Arc<WhpxVm>, vcpu_id: u32) {
    info!(vcpu_id, "WHPX vCPU thread started");
    let partition = *vm.partition.lock();
    let mut exit_reason: WHV_RUN_VP_EXIT_REASON = unsafe { std::mem::zeroed() };

    while *vm.running.lock() {
        let result = unsafe {
            WHvRunVirtualProcessor(
                partition,
                vcpu_id,
                &mut exit_reason,
                std::mem::size_of::<WHV_RUN_VP_EXIT_REASON>() as u32,
            )
        };
        if let Err(e) = result {
            warn!(vcpu_id, error = ?, e, "WHvRunVirtualProcessor failed");
            break;
        }

        // Dispatch on the exit reason type. WHPX exposes the type via
        // the `ExitReason` field of the union; the `windows` crate
        // wraps it as `WHV_VP_EXIT_REASON_TYPE`.
        let exit_type: WHV_VP_EXIT_REASON_TYPE = unsafe { exit_reason.u1.ExitReason };
        match exit_type {
            // Memory access — typically virtio MMIO or PCI config space.
            // Dispatch to the virtio device layer when wired.
            _ => {
                tracing::debug!(vcpu_id, exit_type = ?, "WHPX exit reason");
            }
        }
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
// thread-safe per the WHPX docs — multiple vCPUs can call into the same
// partition concurrently. The host_ptr is only read during memory mapping
// (in create_vm) and unmapping (in stop), both of which are serialised by
// the calling code.
unsafe impl Send for WhpxVm {}
unsafe impl Sync for WhpxVm {}

// Suppress unused-import warning for HANDLE — kept for future use when we
// wire per-vCPU interrupt injection.
#[allow(dead_code)]
fn _ensure_handle_import_used(_: HANDLE) {}
