//! KVM (Linux Kernel Virtual Machine) backend.
//!
//! This module wraps the `kvm-ioctls` crate to provide a thin, idiomatic Rust
//! surface over the KVM API. We expose only the operations Nitroid needs:
//!
//! - check availability
//! - create a VM
//! - attach memory regions
//! - create vCPUs
//! - run the vCPU loop (we don't decode exit reasons here — that's the
//!   virtio layer's responsibility)

use std::path::Path;
use std::sync::Arc;

use kvm_bindings::*;
use kvm_ioctls::{Kvm, VcpuExit, VcpuFd, VmFd};
use nitroid_core::{InstanceConfig, Result};
use parking_lot::Mutex;
use tracing::{info, warn};

use crate::traits::{Backend, BackendCapabilities, BackendInfo, InputEvent, VmHandle};
use nitroid_core::CoreError;

/// Quick availability check — does `/dev/kvm` exist and is it usable?
pub fn is_available() -> bool {
    Path::new("/dev/kvm").exists()
        && std::fs::metadata("/dev/kvm")
            .map(|m| !m.permissions().readonly())
            .unwrap_or(false)
}

pub struct KvmBackend {
    kvm: Kvm,
}

impl KvmBackend {
    pub fn new() -> Result<Self> {
        if !is_available() {
            return Err(CoreError::VirtualizationUnavailable(
                "/dev/kvm not available — install qemu-kvm and add your user to the 'kvm' group"
                    .into(),
            ));
        }
        let kvm = Kvm::new().map_err(|e| {
            CoreError::VirtualizationUnavailable(format!("failed to open /dev/kvm: {e}"))
        })?;
        info!(
            "KVM backend initialised: API version {}",
            kvm.get_api_version()
        );
        Ok(Self { kvm })
    }
}

impl Backend for KvmBackend {
    fn info(&self) -> BackendInfo {
        let version = std::fs::read_to_string("/proc/version")
            .unwrap_or_default()
            .trim()
            .to_string();
        BackendInfo {
            name: "KVM".into(),
            version,
            nested: Path::new("/sys/module/kvm_intel/parameters/nested")
                .read_link()
                .is_ok(),
        }
    }

    fn capabilities(&self) -> Result<BackendCapabilities> {
        let max_vcpus = self.kvm.get_nr_vcpus() as u32;
        let max_memory_mb = (self.kvm.get_max_vcpu_id() as u64) * 1024;
        Ok(BackendCapabilities {
            max_vcpus,
            max_memory_mb,
            supports_arm_translation: false, // translation is provided by the translation crate
            supports_virtio_gpu: true,
        })
    }

    fn create_vm(&self, cfg: &InstanceConfig) -> Result<VmHandle> {
        cfg.validate()?;
        let vm_fd = self
            .kvm
            .create_vm()
            .map_err(|e| CoreError::Backend(format!("KVM_CREATE_VM failed: {e}")))?;

        // Allocate guest memory — one slot of `memory_mb` megabytes at GPA 0.
        let mem_size = (cfg.memory_mb as usize) * 1024 * 1024;
        let mem = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                mem_size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_ANONYMOUS | libc::MAP_PRIVATE | libc::MAP_NORESERVE,
                -1,
                0,
            )
        };
        if mem == libc::MAP_FAILED {
            return Err(CoreError::Backend("mmap failed for guest memory".into()));
        }

        let user_mem_region = kvm_userspace_memory_region {
            slot: 0,
            guest_phys_addr: 0,
            memory_size: mem_size as u64,
            userspace_addr: mem as u64,
            flags: 0,
        };
        unsafe {
            vm_fd
                .set_user_memory_region(user_mem_region)
                .map_err(|e| CoreError::Backend(format!("KVM_SET_USER_MEMORY_REGION: {e}")))?;
        }

        // Create a single vCPU for now (the multi-vCPU scheduler is wired but
        // disabled until the boot protocol is in place).
        let vcpu_fd = vm_fd
            .create_vcpu(0)
            .map_err(|e| CoreError::Backend(format!("KVM_CREATE_VCPU: {e}")))?;

        let inner = Arc::new(KvmVm {
            vm_fd: Mutex::new(vm_fd),
            vcpu_fd: Mutex::new(Some(vcpu_fd)),
            memory: Mutex::new(GuestMemory {
                ptr: mem as *mut u8,
                size: mem_size,
            }),
            config: cfg.clone(),
            running: Mutex::new(false),
        });
        Ok(VmHandle::new(inner))
    }

    fn start(&self, vm: &mut VmHandle) -> Result<()> {
        let inner = vm
            .as_any()
            .downcast_ref::<Arc<KvmVm>>()
            .ok_or_else(|| CoreError::Backend("not a KVM VM".into()))?
            .clone();
        info!(instance = %inner.config.name, "starting KVM VM");

        {
            let mut running = inner.running.lock();
            if *running {
                return Err(CoreError::Backend("VM already running".into()));
            }
            *running = true;
        }
        // Take the vCPU fd out of the VM — it now belongs to the run thread.
        // The thread is responsible for putting it back (via the VcpuGuard's
        // Drop) when the run loop exits.
        let vcpu_fd = inner
            .vcpu_fd
            .lock()
            .take()
            .ok_or_else(|| CoreError::Backend("vCPU is already being run".into()))?;

        let inner_for_thread = inner.clone();
        std::thread::Builder::new()
            .name(format!("kvm-vcpu-{}", inner.config.name))
            .spawn(move || {
                // The guard ensures the vCPU fd is returned to the VM even if
                // the run loop exits early (panic, halt, etc).
                let mut _guard = VcpuGuard {
                    vm: inner_for_thread.clone(),
                    vcpu: Some(vcpu_fd),
                };
                loop {
                    if !*inner_for_thread.running.lock() {
                        break;
                    }
                    // Borrow the vCPU through the guard for the run call.
                    if let Some(ref mut vcpu) = _guard.vcpu {
                        match vcpu.run() {
                            Ok(VcpuExit::Hlt) => {
                                info!("KVM: vCPU halt — VM requested shutdown");
                                break;
                            }
                            Ok(VcpuExit::IoIn(port, _)) => {
                                warn!(port, "KVM: unhandled IO in");
                            }
                            Ok(VcpuExit::IoOut(port, _)) => {
                                warn!(port, "KVM: unhandled IO out — virtio devices not yet wired");
                            }
                            Ok(reason) => {
                                warn!(?reason, "KVM: unhandled exit reason");
                            }
                            Err(e) => {
                                warn!("KVM: vcpu run error: {e}");
                                break;
                            }
                        }
                    } else {
                        break;
                    }
                }
            })
            .map_err(|e| CoreError::Backend(format!("failed to spawn vCPU thread: {e}")))?;
        Ok(())
    }

    fn pause(&self, vm: &mut VmHandle) -> Result<()> {
        let inner = vm
            .as_any()
            .downcast_ref::<Arc<KvmVm>>()
            .ok_or_else(|| CoreError::Backend("not a KVM VM".into()))?
            .clone();
        *inner.running.lock() = false;
        // KVM doesn't have a built-in "pause" — the run loop exits on next
        // IO/MMIO exit and is restarted on resume. This is stubbed until the
        // virtio layer lands.
        info!(instance = %inner.config.name, "pause requested");
        Ok(())
    }

    fn resume(&self, vm: &mut VmHandle) -> Result<()> {
        let inner = vm
            .as_any()
            .downcast_ref::<Arc<KvmVm>>()
            .ok_or_else(|| CoreError::Backend("not a KVM VM".into()))?
            .clone();
        *inner.running.lock() = true;
        info!(instance = %inner.config.name, "resume requested");
        Ok(())
    }

    fn stop(&self, vm: &mut VmHandle) -> Result<()> {
        let inner = vm
            .as_any()
            .downcast_ref::<Arc<KvmVm>>()
            .ok_or_else(|| CoreError::Backend("not a KVM VM".into()))?
            .clone();
        *inner.running.lock() = false;
        info!(instance = %inner.config.name, "stopping KVM VM");
        Ok(())
    }

    fn inject_input(&self, vm: &mut VmHandle, event: InputEvent) -> Result<()> {
        let inner = vm
            .as_any()
            .downcast_ref::<Arc<KvmVm>>()
            .ok_or_else(|| CoreError::Backend("not a KVM VM".into()))?
            .clone();
        // Virtio-input injection requires the virtio device to be wired into
        // the KVM run loop. Until then we log the event so the input crate
        // can be tested in isolation.
        tracing::debug!(?event, instance = %inner.config.name, "input injected");
        Ok(())
    }
}

/// Internal KVM VM state. Owns file descriptors + mapped guest memory.
/// Wrapped in `Arc` so the run-loop thread can hold a reference for the
/// lifetime of the VM.
#[allow(dead_code)]
struct KvmVm {
    vm_fd: Mutex<VmFd>,
    /// The vCPU fd is wrapped in `Option` so it can be moved into the run
    /// thread while the VM is running. The run thread puts it back when it
    /// exits (via [`VcpuGuard`]'s Drop).
    vcpu_fd: Mutex<Option<VcpuFd>>,
    memory: Mutex<GuestMemory>,
    config: InstanceConfig,
    running: Mutex<bool>,
}

/// RAII guard that returns the vCPU fd to its VM when the run thread exits.
struct VcpuGuard {
    vm: Arc<KvmVm>,
    vcpu: Option<VcpuFd>,
}

impl Drop for VcpuGuard {
    fn drop(&mut self) {
        if let Some(vcpu) = self.vcpu.take() {
            *self.vm.vcpu_fd.lock() = Some(vcpu);
        }
        *self.vm.running.lock() = false;
    }
}

struct GuestMemory {
    ptr: *mut u8,
    size: usize,
}

impl Drop for GuestMemory {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { libc::munmap(self.ptr as *mut libc::c_void, self.size) };
        }
    }
}

// SAFETY: KVM file descriptors are Send + Sync on Linux. Guest memory is
// accessed only by the vCPU run loop thread, which serialises access.
unsafe impl Send for GuestMemory {}
unsafe impl Sync for GuestMemory {}
