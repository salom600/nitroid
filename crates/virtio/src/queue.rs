//! VirtQueue — the data structure used to pass buffers between the guest
//! and the host.
//!
//! A virtqueue consists of three rings:
//!
//! - `desc` — array of descriptors, each pointing to a guest buffer
//! - `avail` — ring of descriptor indices the guest has made available
//! - `used` — ring of descriptor indices the host has consumed
//!
//! The host polls `avail.idx` against its own `last_avail_idx` to detect
//! new work. When the host finishes a buffer, it pushes the descriptor index
//! to `used` and increments `used.idx`.

use std::sync::atomic::{AtomicU16, Ordering};

use nitroid_core::CoreError;
use nitroid_core::Result;

/// In-memory representation of a virtqueue. The actual rings live in guest
/// memory — this struct holds the indices the host tracks.
#[derive(Debug)]
pub struct VirtQueue {
    pub size: u16,
    pub last_avail_idx: AtomicU16,
    pub last_used_idx: AtomicU16,
}

#[derive(Debug, thiserror::Error)]
pub enum VirtQueueError {
    #[error("invalid descriptor index {0}")]
    InvalidIndex(u16),
    #[error("descriptor chain too long (max {max}, got {actual})")]
    ChainTooLong { max: u16, actual: u16 },
    #[error("buffer length mismatch: expected {expected}, got {actual}")]
    BufferLength { expected: u32, actual: u32 },
}

impl VirtQueue {
    /// Create a new virtqueue of the given `size` (must be a power of 2,
    /// 1 <= size <= 32768).
    pub fn new(size: u16) -> Result<Self> {
        if size == 0 || !size.is_power_of_two() {
            return Err(CoreError::Backend(format!(
                "invalid virtqueue size {size}: must be a power of 2"
            )));
        }
        if size > 32768 {
            return Err(CoreError::Backend(format!(
                "virtqueue size {size} exceeds virtio 1.2 max of 32768"
            )));
        }
        Ok(Self {
            size,
            last_avail_idx: AtomicU16::new(0),
            last_used_idx: AtomicU16::new(0),
        })
    }

    /// How many descriptors are waiting to be processed?
    pub fn pending(&self) -> u16 {
        let avail = self.last_avail_idx.load(Ordering::Acquire);
        let processed = self.last_used_idx.load(Ordering::Acquire);
        avail.wrapping_sub(processed)
    }

    /// Mark a descriptor as processed. The guest will see it in the `used`
    /// ring on the next kick.
    pub fn complete(&self) {
        self.last_used_idx.fetch_add(1, Ordering::Release);
    }

    /// Reset the queue to its initial state (used during device reset).
    pub fn reset(&self) {
        self.last_avail_idx.store(0, Ordering::Release);
        self.last_used_idx.store(0, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_size_validation() {
        assert!(VirtQueue::new(0).is_err());
        assert!(VirtQueue::new(3).is_err());
        assert!(VirtQueue::new(256).is_ok());
        assert!(VirtQueue::new(1).is_ok());
        assert!(VirtQueue::new(32768).is_ok());
    }

    #[test]
    fn pending_starts_at_zero() {
        let q = VirtQueue::new(256).unwrap();
        assert_eq!(q.pending(), 0);
    }

    #[test]
    fn complete_increments_used() {
        let q = VirtQueue::new(256).unwrap();
        // Simulate guest making 3 buffers available.
        q.last_avail_idx.store(3, Ordering::Release);
        assert_eq!(q.pending(), 3);
        q.complete();
        q.complete();
        assert_eq!(q.pending(), 1);
    }

    #[test]
    fn wrap_around() {
        let q = VirtQueue::new(256).unwrap();
        q.last_avail_idx.store(u16::MAX, Ordering::Release);
        q.last_used_idx.store(u16::MAX - 1, Ordering::Release);
        assert_eq!(q.pending(), 1);
        q.complete();
        // Both wrap to 0 — pending should still be 0.
        assert_eq!(q.pending(), 0);
    }
}
