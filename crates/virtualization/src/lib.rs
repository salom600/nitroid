//! Virtualization backend abstraction.
//!
//! The crate exposes a single trait [`Backend`] that abstracts over the
//! host-accelerated hypervisor — KVM on Linux, WHPX on Windows. The trait
//! surface is intentionally minimal so the upper layers (graphics, input, UI)
//! don't need to know which hypervisor is in use.
//!
//! We do **not** build a full hardware emulator. Instead, we configure the
//! host's built-in hypervisor to run the Android system image's kernel
//! directly, with virtio devices for network, disk, and input.

pub mod boot;
pub mod guest_memory;

#[cfg(target_os = "linux")]
pub mod kvm;

#[cfg(target_os = "windows")]
pub mod whpx;

pub mod traits;

#[cfg(target_os = "linux")]
pub use kvm::KvmBackend;
#[cfg(target_os = "windows")]
pub use whpx::WhpxBackend;

pub use guest_memory::{from_single_region, GuestMemory, MemoryRegion, SharedGuestMemory};
pub use traits::{Backend, BackendCapabilities, BackendInfo, InputEvent, VmHandle};

use nitroid_core::{AccelBackend, CoreError, Result};

/// Pick the best available backend for the current host platform.
pub fn pick_backend(preferred: AccelBackend) -> Result<Box<dyn Backend>> {
    #[cfg(target_os = "linux")]
    {
        if matches!(preferred, AccelBackend::Auto | AccelBackend::Kvm) && kvm::is_available() {
            return Ok(Box::new(KvmBackend::new()?));
        }
        Err(CoreError::VirtualizationUnavailable(
            "KVM is not available on this Linux host. Ensure /dev/kvm exists and your user is in the kvm group.".into(),
        ))
    }

    #[cfg(target_os = "windows")]
    {
        if matches!(preferred, AccelBackend::Auto | AccelBackend::Whpx) {
            if whpx::is_available() {
                return Ok(Box::new(WhpxBackend::new()?));
            }
        }
        return Err(CoreError::VirtualizationUnavailable(
            "WHPX is not available. Install Hyper-V from 'Turn Windows features on or off'.".into(),
        ));
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = preferred;
        Err(CoreError::VirtualizationUnavailable(
            "unsupported host platform — Nitroid currently supports only Linux (KVM) and Windows (WHPX)".into(),
        ))
    }
}
