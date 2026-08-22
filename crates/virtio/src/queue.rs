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
//!
//! ## Memory layout
//!
//! All three rings live in guest physical memory, at addresses the guest
//! wrote to the device's PCI configuration space. The host accesses them
//! via the [`GuestMemory`] layer.

use std::sync::atomic::{fence, Ordering};
use std::sync::Arc;

use nitroid_core::CoreError;
use nitroid_core::Result;
use nitroid_virtualization::GuestMemory;

/// Virtio 1.2 split virtqueue layout. The `desc`, `avail`, and `used` rings
/// live in guest physical memory.
#[derive(Debug)]
pub struct VirtQueue {
    pub size: u16,
    /// GPA of the descriptor table (array of `Descriptor`).
    pub desc_addr: u64,
    /// GPA of the available ring.
    pub avail_addr: u64,
    /// GPA of the used ring.
    pub used_addr: u64,
    /// Host-side last_avail_idx — the next descriptor the host will consume.
    pub last_avail_idx: u16,
    /// Host-side last_used_idx — the next slot in `used` the host will write.
    pub last_used_idx: u16,
    /// Whether the guest has enabled notifications (VIRTIO_RING_F_EVENT_IDX).
    pub event_idx: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum VirtQueueError {
    #[error("invalid descriptor index {0}")]
    InvalidIndex(u16),
    #[error("descriptor chain too long (max {max}, got {actual})")]
    ChainTooLong { max: u16, actual: u16 },
    #[error("buffer length mismatch: expected {expected}, got {actual}")]
    BufferLength { expected: u32, actual: u32 },
    #[error("guest memory access error: {0}")]
    GuestMemory(String),
}

/// A single descriptor in the descriptor table.
///
/// Layout per virtio 1.2 spec, section 2.6.5:
///
/// ```text
/// +------------------+----+
/// | addr (LE u64)    |  8 |
/// +------------------+----+
/// | len  (LE u32)    |  4 |
/// +------------------+----+
/// | flags (LE u16)   |  2 |
/// +------------------+----+
/// | next  (LE u16)   |  2 |
/// +------------------+----+
/// ```
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct Descriptor {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

pub const VIRTQ_DESC_F_NEXT: u16 = 1;
pub const VIRTQ_DESC_F_WRITE: u16 = 2;
pub const VIRTQ_DESC_F_INDIRECT: u16 = 4;

/// Available ring header (lives at `avail_addr`).
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct AvailRingHeader {
    pub flags: u16,
    pub idx: u16,
}

/// Used ring header (lives at `used_addr`).
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct UsedRingHeader {
    pub flags: u16,
    pub idx: u16,
}

/// Used ring element — pushed by the host when a buffer is consumed.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct UsedElement {
    pub id: u32,
    pub len: u32,
}

impl VirtQueue {
    /// Create a new virtqueue of the given `size` pointing at the given
    /// guest physical addresses.
    pub fn new(size: u16, desc_addr: u64, avail_addr: u64, used_addr: u64) -> Result<Self> {
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
            desc_addr,
            avail_addr,
            used_addr,
            last_avail_idx: 0,
            last_used_idx: 0,
            event_idx: false,
        })
    }

    /// Read a descriptor from the descriptor table by index.
    pub fn read_descriptor(&self, mem: &GuestMemory, idx: u16) -> Result<Descriptor> {
        if idx >= self.size {
            return Err(CoreError::Backend(format!(
                "descriptor index {idx} out of range (size={})",
                self.size
            )));
        }
        let gpa = self.desc_addr + (idx as u64) * 16;
        let mut buf = [0u8; 16];
        mem.read_into(gpa, &mut buf)?;
        Ok(Descriptor {
            addr: u64::from_le_bytes(buf[0..8].try_into().unwrap()),
            len: u32::from_le_bytes(buf[8..12].try_into().unwrap()),
            flags: u16::from_le_bytes(buf[12..14].try_into().unwrap()),
            next: u16::from_le_bytes(buf[14..16].try_into().unwrap()),
        })
    }

    /// Read the current `avail.idx` from guest memory.
    pub fn avail_idx(&self, mem: &GuestMemory) -> Result<u16> {
        // flags + idx = 4 bytes
        let gpa = self.avail_addr + 2; // skip flags
        mem.read_u16_le(gpa)
    }

    /// Read the descriptor index at position `pos` in the avail ring.
    pub fn avail_ring_entry(&self, mem: &GuestMemory, pos: u16) -> Result<u16> {
        let gpa = self.avail_addr + 4 + (pos as u64) * 2;
        mem.read_u16_le(gpa)
    }

    /// How many descriptors are waiting to be processed?
    pub fn pending(&self, mem: &GuestMemory) -> Result<u16> {
        let avail = self.avail_idx(mem)?;
        Ok(avail.wrapping_sub(self.last_avail_idx))
    }

    /// Pop the next available descriptor head index. Returns `None` if the
    /// queue is empty.
    pub fn pop_avail(&mut self, mem: &GuestMemory) -> Result<Option<u16>> {
        let avail = self.avail_idx(mem)?;
        if self.last_avail_idx == avail {
            return Ok(None);
        }
        let pos = self.last_avail_idx % self.size;
        let head = self.avail_ring_entry(mem, pos)?;
        self.last_avail_idx = self.last_avail_idx.wrapping_add(1);
        Ok(Some(head))
    }

    /// Push a used element to the used ring. The guest will see it on the
    /// next kick.
    pub fn push_used(&mut self, mem: &GuestMemory, id: u32, len: u32) -> Result<()> {
        let pos = self.last_used_idx % self.size;
        let elem_gpa = self.used_addr + 4 + (pos as u64) * 8;
        // Write id (u32) + len (u32) — 8 bytes total.
        mem.write(elem_gpa, &id.to_le_bytes())?;
        mem.write(elem_gpa + 4, &len.to_le_bytes())?;
        // Increment last_used_idx.
        self.last_used_idx = self.last_used_idx.wrapping_add(1);
        // Update the used ring's idx field.
        let used_idx_gpa = self.used_addr + 2;
        mem.write_u16_le(used_idx_gpa, self.last_used_idx)?;
        // Memory barrier so the guest sees the element before the idx update.
        fence(Ordering::Release);
        Ok(())
    }

    /// Reset the queue to its initial state.
    pub fn reset(&mut self) {
        self.last_avail_idx = 0;
        self.last_used_idx = 0;
        self.event_idx = false;
    }
}

/// Convenience type used by devices that need to walk a descriptor chain.
pub struct ChainWalker<'a> {
    queue: &'a mut VirtQueue,
    mem: Arc<GuestMemory>,
    max_chain: u16,
}

impl<'a> ChainWalker<'a> {
    pub fn new(queue: &'a mut VirtQueue, mem: Arc<GuestMemory>) -> Self {
        Self {
            queue,
            mem,
            max_chain: 32, // virtio spec recommends this limit
        }
    }

    /// Walk the chain starting at `head_idx`. Returns the list of descriptors.
    pub fn walk_chain(&mut self, head_idx: u16) -> Result<Vec<Descriptor>> {
        let mut chain = Vec::with_capacity(4);
        let mut idx = head_idx;
        for _ in 0..self.max_chain {
            let desc = self.queue.read_descriptor(&self.mem, idx)?;
            chain.push(desc);
            if desc.flags & VIRTQ_DESC_F_NEXT == 0 {
                return Ok(chain);
            }
            idx = desc.next;
        }
        Err(CoreError::Backend(format!(
            "descriptor chain starting at {head_idx} exceeded max length of {}",
            self.max_chain
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nitroid_virtualization::from_single_region;

    fn make_test_memory(size: usize) -> (Vec<u8>, Arc<GuestMemory>) {
        let mut buf = vec![0u8; size];
        let gm = from_single_region(0, size as u64, buf.as_mut_ptr());
        (buf, gm)
    }

    #[test]
    fn queue_size_validation() {
        assert!(VirtQueue::new(0, 0, 0, 0).is_err());
        assert!(VirtQueue::new(3, 0, 0, 0).is_err());
        assert!(VirtQueue::new(256, 0, 0, 0).is_ok());
        assert!(VirtQueue::new(1, 0, 0, 0).is_ok());
        assert!(VirtQueue::new(32768, 0, 0, 0).is_ok());
    }

    #[test]
    fn read_descriptor_round_trips() {
        let (mut buf, gm) = make_test_memory(4096);
        // Write a descriptor at offset 0x100.
        let desc = Descriptor {
            addr: 0xDEADBEEF,
            len: 1024,
            flags: VIRTQ_DESC_F_NEXT,
            next: 5,
        };
        // Write a descriptor at offset 0x100. The on-disk layout is:
        // addr (u64 LE) + len (u32 LE) + flags (u16 LE) + next (u16 LE) = 16 bytes.
        let mut bytes = [0u8; 16];
        bytes[0..8].copy_from_slice(&desc.addr.to_le_bytes());
        bytes[8..12].copy_from_slice(&desc.len.to_le_bytes());
        bytes[12..14].copy_from_slice(&desc.flags.to_le_bytes());
        bytes[14..16].copy_from_slice(&desc.next.to_le_bytes());
        buf[0x100..0x100 + 16].copy_from_slice(&bytes);

        let q = VirtQueue::new(16, 0x100, 0, 0).unwrap();
        let read = q.read_descriptor(&gm, 0).unwrap();
        assert_eq!(read.addr, 0xDEADBEEF);
        assert_eq!(read.len, 1024);
        assert_eq!(read.flags, VIRTQ_DESC_F_NEXT);
        assert_eq!(read.next, 5);
    }

    #[test]
    fn pending_starts_at_zero() {
        let (mut buf, gm) = make_test_memory(4096);
        // Set avail.idx = 0.
        buf[2] = 0;
        buf[3] = 0;
        let q = VirtQueue::new(256, 0, 0, 0).unwrap();
        assert_eq!(q.pending(&gm).unwrap(), 0);
    }

    #[test]
    fn pop_avail_walks_ring() {
        let (mut buf, gm) = make_test_memory(4096);
        // Set avail.idx = 2 (two pending descriptors).
        buf[2] = 2;
        buf[3] = 0;
        // Set ring[0] = 7, ring[1] = 19.
        buf[4] = 7;
        buf[5] = 0;
        buf[6] = 19;
        buf[7] = 0;

        let mut q = VirtQueue::new(256, 0, 0, 0).unwrap();
        let first = q.pop_avail(&gm).unwrap().unwrap();
        assert_eq!(first, 7);
        let second = q.pop_avail(&gm).unwrap().unwrap();
        assert_eq!(second, 19);
        assert!(q.pop_avail(&gm).unwrap().is_none());
    }

    #[test]
    fn push_used_writes_to_guest_memory() {
        // Allocate enough memory for the used ring header + a few elements.
        // We use 8 KiB so the used ring at offset 0x1000 has room.
        let (buf, gm) = make_test_memory(8192);
        let mut q = VirtQueue::new(256, 0, 0, 0x1000).unwrap();
        q.push_used(&gm, 42, 1024).unwrap();

        // The used ring element should be at used_addr + 4.
        let elem_gpa = 0x1000 + 4;
        let mut id_buf = [0u8; 4];
        gm.read_into(elem_gpa, &mut id_buf).unwrap();
        assert_eq!(u32::from_le_bytes(id_buf), 42);

        let mut len_buf = [0u8; 4];
        gm.read_into(elem_gpa + 4, &mut len_buf).unwrap();
        assert_eq!(u32::from_le_bytes(len_buf), 1024);

        // The used ring idx should have been incremented to 1.
        let idx = gm.read_u16_le(0x1000 + 2).unwrap();
        assert_eq!(idx, 1);

        let _ = buf;
    }
}
