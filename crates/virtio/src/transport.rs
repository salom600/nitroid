//! Virtio transport — the bus / connection layer between guest and device.
//!
//! In real virtio there are three transports: PCI, MMIO, and CCW. The
//! Android-x86 kernel boots with PCI by default, so we focus on PCI here.
//! The transport owns the doorbells (kick/notify) and the configuration
//! space layout.

use std::sync::Arc;

use parking_lot::Mutex;

/// Virtio device IDs as defined by the virtio 1.2 spec.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceId {
    Invalid = 0,
    Net = 1,
    Blk = 2,
    Console = 3,
    Rng = 4,
    Balloon = 5,
    IoBalloon = 6,
    RPMsg = 7,
    ScsiHost = 8,
    _9P = 9,
    Mac80211 = 10,
    RprocSerial = 11,
    Caif = 12,
    MemoryBalloon = 13,
    Gpu = 16,
    Input = 18,
    Socket = 19,
    Fs = 26,
    Pmem = 27,
}

/// A PCI transport for a virtio device. Wraps the configuration space and
/// the doorbell registers the guest writes to kick queues.
pub struct PciTransport {
    pub device_id: DeviceId,
    pub features: Arc<Mutex<u64>>,
    pub queue_select: Mutex<u32>,
    pub status: Mutex<u8>,
}

impl PciTransport {
    pub fn new(device_id: DeviceId) -> Self {
        Self {
            device_id,
            features: Arc::new(Mutex::new(0)),
            queue_select: Mutex::new(0),
            status: Mutex::new(0),
        }
    }
}

/// The transport trait — a way to deliver notifications from the guest to
/// the host-side device implementation.
pub trait VirtioTransport: Send + Sync {
    /// Notify the host that the guest has updated an avail ring.
    fn notify(&self, queue_idx: usize);

    /// Read a 32-bit value from device-specific configuration space.
    fn read_config(&self, offset: u32) -> u32;

    /// Write a 32-bit value to device-specific configuration space.
    fn write_config(&self, offset: u32, value: u32);
}
