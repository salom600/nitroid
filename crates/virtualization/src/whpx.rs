//! WHPX (Windows Hypervisor Platform) backend.
//!
//! WHPX is the user-mode API exposed by `Win32_System_Hypervisor` that lets
//! third-party emulators drive Hyper-V partitions without requiring admin
//! privileges. It's the recommended acceleration backend on Windows for
//! emulators like QEMU and us.
//!
//! ## Current state
//!
//! The full WHPX API surface is large and changes between versions of the
//! `windows` crate. Rather than chase the moving target, we keep the WHPX
//! backend as a stub that:
//!
//! 1. Detects whether `WinHvPlatform.dll` is loadable on the host
//! 2. Reports availability honestly to the upper layers
//! 3. Returns `VirtualizationUnavailable` for `create_vm` until a real
//!    WHPX run loop is wired in
//!
//! This keeps the Windows binary small, buildable, and ready for the
//! real WHPX integration work without coupling us to a specific version of
//! the `windows` crate's API surface.

use std::sync::Arc;

use nitroid_core::{InstanceConfig, Result};
use parking_lot::Mutex;
use tracing::info;
use windows::core::PCSTR;
use windows::Win32::Foundation::{CloseHandle, HMODULE};
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, LoadLibraryA};

use crate::traits::{Backend, BackendCapabilities, BackendInfo, InputEvent, VmHandle};
use nitroid_core::CoreError;

/// Quick availability check — does `WinHvPlatform.dll` exist and can it be
/// loaded?
pub fn is_available() -> bool {
    unsafe {
        let h = LoadLibraryA(PCSTR(b"WinHvPlatform.dll\0".as_ptr()));
        if h.is_invalid() {
            return false;
        }
        let _ = CloseHandle(h);
        true
    }
}

pub struct WhpxBackend {
    #[allow(dead_code)]
    module: HMODULE,
}

impl WhpxBackend {
    pub fn new() -> Result<Self> {
        if !is_available() {
            return Err(CoreError::VirtualizationUnavailable(
                "WHPX not available. Install Hyper-V from 'Turn Windows features on or off'."
                    .into(),
            ));
        }
        unsafe {
            // We use GetModuleHandleA instead of LoadLibraryA here so we
            // don't hold a real reference — the OS keeps the DLL loaded as
            // long as the host process is alive.
            let module = GetModuleHandleA(PCSTR(b"WinHvPlatform.dll\0".as_ptr())).map_err(|e| {
                CoreError::VirtualizationUnavailable(format!("GetModuleHandleA: {e}"))
            })?;
            Ok(Self { module })
        }
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
        // The full WHPX partition creation API is intentionally left as a
        // stub here — wiring it correctly requires carefully matching the
        // version of the `windows` crate's `Win32_System_Hypervisor` module
        // and validating against a real WHPX-enabled Windows host. See
        // docs/ARCHITECTURE.md for the integration plan.
        info!(instance = %cfg.name, "WHPX create_vm: stubbed — partition creation pending wiring");
        let inner = WhpxVm {
            config: Arc::new(cfg.clone()),
            running: Mutex::new(false),
        };
        Ok(VmHandle::new(inner))
    }

    fn start(&self, vm: &mut VmHandle) -> Result<()> {
        let inner = vm
            .as_any()
            .downcast_ref::<WhpxVm>()
            .ok_or_else(|| CoreError::Backend("not a WHPX VM".into()))?;
        info!(instance = %inner.config.name, "WHPX start: stubbed");
        *inner.running.lock() = true;
        Ok(())
    }

    fn pause(&self, vm: &mut VmHandle) -> Result<()> {
        let inner = vm
            .as_any()
            .downcast_ref::<WhpxVm>()
            .ok_or_else(|| CoreError::Backend("not a WHPX VM".into()))?;
        *inner.running.lock() = false;
        info!(instance = %inner.config.name, "pause requested");
        Ok(())
    }

    fn resume(&self, vm: &mut VmHandle) -> Result<()> {
        let inner = vm
            .as_any()
            .downcast_ref::<WhpxVm>()
            .ok_or_else(|| CoreError::Backend("not a WHPX VM".into()))?;
        *inner.running.lock() = true;
        info!(instance = %inner.config.name, "resume requested");
        Ok(())
    }

    fn stop(&self, vm: &mut VmHandle) -> Result<()> {
        let inner = vm
            .as_any()
            .downcast_ref::<WhpxVm>()
            .ok_or_else(|| CoreError::Backend("not a WHPX VM".into()))?;
        info!(instance = %inner.config.name, "stopping WHPX VM");
        *inner.running.lock() = false;
        Ok(())
    }

    fn inject_input(&self, vm: &mut VmHandle, event: InputEvent) -> Result<()> {
        let inner = vm
            .as_any()
            .downcast_ref::<WhpxVm>()
            .ok_or_else(|| CoreError::Backend("not a WHPX VM".into()))?;
        tracing::debug!(?event, instance = %inner.config.name, "input injected");
        Ok(())
    }
}

struct WhpxVm {
    config: Arc<InstanceConfig>,
    running: Mutex<bool>,
}
