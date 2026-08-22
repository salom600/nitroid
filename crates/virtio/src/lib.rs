//! Virtio device layer — the bridge between the host's virtio framework
//! and the guest Android system.
//!
//! ## Why virtio?
//!
//! Virtio is the standard para-virtualised I/O framework used by KVM, WHPX,
//! and every modern hypervisor. The guest sees a "virtio-blk" PCI device,
//! sends it disk requests, and the host services those requests against a
//! real file. Same pattern for virtio-input (touch events), virtio-gpu
//! (graphics), virtio-net (network).
//!
//! ## Architecture
//!
//! Each virtio device implements the [`VirtioDevice`] trait. The KVM/WHPX
//! run loop dispatches virtqueue traffic to the device, which produces
//! responses consumed by the next vCPU entry.
//!
//! ```text
//!     ┌───────────────┐         ┌──────────────────────┐
//!     │ Guest kernel  │  ──►    │  virtio PCI transport │
//!     │  (Android)    │         │  (handled by KVM)     │
//!     └───────────────┘         └──────────────────────┘
//!                                        │
//!                                        ▼
//!                               ┌────────────────────┐
//!                               │  VirtioDevice trait │
//!                               └────────────────────┘
//!                                        │
//!                       ┌────────────────┼────────────────┐
//!                       │                │                │
//!                       ▼                ▼                ▼
//!                ┌────────────┐  ┌────────────┐  ┌──────────────┐
//!                │ virtio-blk │  │virtio-input│  │  virtio-gpu  │
//!                │ (disk IO) │  │(keymapping)│  │ (framebuffer)│
//!                └────────────┘  └────────────┘  └──────────────┘
//! ```

pub mod blk;
pub mod gpu;
pub mod input;
pub mod queue;
pub mod transport;

pub use blk::VirtioBlk;
pub use gpu::VirtioGpu;
pub use input::VirtioInput;
pub use queue::{VirtQueue, VirtQueueError};
pub use transport::{DeviceId, PciTransport, VirtioTransport};

use nitroid_core::Result;
use nitroid_virtualization::GuestMemory;

/// A virtio device. Every device implements this trait so the run loop can
/// dispatch uniformly.
pub trait VirtioDevice: Send + Sync {
    /// The virtio device ID (e.g. 2 = block, 18 = gpu).
    fn device_id(&self) -> DeviceId;

    /// The list of feature bits the device supports.
    fn features(&self) -> u64 {
        0
    }

    /// Number of virtqueues this device exposes.
    fn num_queues(&self) -> usize;

    /// Process incoming virtqueue traffic. Called by the run loop after
    /// each vCPU exit. Returns the number of descriptors processed.
    /// `queue` is the queue at index `queue_idx`, with access to the guest
    /// memory backing the descriptor rings.
    fn process_queue(
        &self,
        queue_idx: usize,
        queue: &mut VirtQueue,
        mem: &GuestMemory,
    ) -> Result<usize>;

    /// Reset the device to its initial state.
    fn reset(&self);
}
