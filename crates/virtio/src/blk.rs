//! virtio-blk — block device backed by a host file.
//!
//! The guest sees a normal disk (`/dev/vda` in Linux). When it issues a
//! read/write, the virtio-blk device translates the request into a host
//! file I/O against the configured backing file (typically the per-instance
//! qcow2 overlay).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::Arc;

use parking_lot::Mutex;
use tracing::{debug, info, warn};

use crate::queue::{VirtQueue, VIRTQ_DESC_F_NEXT, VIRTQ_DESC_F_WRITE};
use crate::transport::DeviceId;
use crate::VirtioDevice;
use nitroid_core::CoreError;
use nitroid_core::Result;
use nitroid_virtualization::GuestMemory;

/// virtio-blk request header as defined by the virtio 1.2 spec.
#[repr(C)]
#[derive(Debug, Default)]
pub struct BlkRequestHeader {
    pub kind: u32,
    pub reserved: u32,
    pub sector: u64,
}

pub const VIRTIO_BLK_T_IN: u32 = 0;
pub const VIRTIO_BLK_T_OUT: u32 = 1;
pub const VIRTIO_BLK_T_FLUSH: u32 = 4;
pub const VIRTIO_BLK_T_GET_ID: u32 = 8;

pub const SECTOR_SIZE: u64 = 512;

/// A virtio-blk device backed by a host file. The file is opened read/write
/// and shared between the device and the host. Concurrent access is
/// serialised through a Mutex.
pub struct VirtioBlk {
    backing: Arc<Mutex<File>>,
    /// Capacity in sectors (each 512 bytes).
    capacity_sectors: u64,
    /// Whether writes are allowed. Set to false for read-only blueprint
    /// images — saves disk wear and accidental writes.
    read_only: bool,
}

impl VirtioBlk {
    /// Open `path` as a read/write backing file.
    pub fn open(path: &std::path::Path, read_only: bool) -> Result<Self> {
        let mut opts = std::fs::OpenOptions::new();
        opts.read(true);
        if !read_only {
            opts.write(true);
        }
        let file = opts
            .open(path)
            .map_err(|e| nitroid_core::CoreError::Backend(format!("open backing file: {e}")))?;
        let size = file.metadata()?.len();
        let capacity_sectors = size / SECTOR_SIZE;
        info!(path = %path.display(), capacity_sectors, read_only, "virtio-blk initialised");
        Ok(Self {
            backing: Arc::new(Mutex::new(file)),
            capacity_sectors,
            read_only,
        })
    }

    /// Read `len` bytes starting at `sector * 512` into `buf`.
    pub fn read(&self, sector: u64, buf: &mut [u8]) -> Result<()> {
        let mut file = self.backing.lock();
        file.seek(SeekFrom::Start(sector * SECTOR_SIZE))
            .map_err(|e| nitroid_core::CoreError::Backend(format!("seek: {e}")))?;
        file.read_exact(buf)
            .map_err(|e| nitroid_core::CoreError::Backend(format!("read: {e}")))?;
        Ok(())
    }

    /// Write `buf` to `sector * 512`.
    pub fn write(&self, sector: u64, buf: &[u8]) -> Result<()> {
        if self.read_only {
            return Err(nitroid_core::CoreError::Backend(
                "write attempted on read-only image".into(),
            ));
        }
        let mut file = self.backing.lock();
        file.seek(SeekFrom::Start(sector * SECTOR_SIZE))
            .map_err(|e| nitroid_core::CoreError::Backend(format!("seek: {e}")))?;
        file.write_all(buf)
            .map_err(|e| nitroid_core::CoreError::Backend(format!("write: {e}")))?;
        Ok(())
    }

    /// Flush pending writes to disk.
    pub fn flush(&self) -> Result<()> {
        let file = self.backing.lock();
        file.sync_data()
            .map_err(|e| nitroid_core::CoreError::Backend(format!("flush: {e}")))?;
        Ok(())
    }

    /// Total capacity in bytes.
    pub fn capacity_bytes(&self) -> u64 {
        self.capacity_sectors * SECTOR_SIZE
    }
}

impl VirtioDevice for VirtioBlk {
    fn device_id(&self) -> DeviceId {
        DeviceId::Blk
    }

    fn features(&self) -> u64 {
        // VIRTIO_BLK_F_FLUSH (4) | VIRTIO_BLK_F_BLK_SIZE (6) | VIRTIO_F_VERSION_1 (32)
        0x1_0000_0050
    }

    fn num_queues(&self) -> usize {
        1
    }

    fn process_queue(
        &self,
        _queue_idx: usize,
        queue: &mut VirtQueue,
        mem: &GuestMemory,
    ) -> Result<usize> {
        let mut processed = 0;
        // Process up to 64 requests per call to avoid starving the vCPU.
        for _ in 0..64 {
            let pending = queue.pending(mem)?;
            if pending == 0 {
                break;
            }
            match self.process_one_request(queue, mem) {
                Ok(()) => processed += 1,
                Err(e) => {
                    warn!(error = %e, "virtio-blk: failed to process request");
                    break;
                }
            }
        }
        if processed > 0 {
            debug!(processed, "virtio-blk: processed requests");
        }
        Ok(processed)
    }

    fn reset(&self) {
        // No internal state to reset — the queue resets itself.
    }
}

impl VirtioBlk {
    /// Process one request from the queue. Walks the descriptor chain,
    /// reads/writes the backing file, and pushes the result to the used ring.
    fn process_one_request(&self, queue: &mut VirtQueue, mem: &GuestMemory) -> Result<()> {
        let head_idx = match queue.pop_avail(mem)? {
            Some(idx) => idx,
            None => return Ok(()),
        };

        // Walk the descriptor chain starting at head_idx.
        let mut chain = Vec::new();
        let mut idx = head_idx;
        for _ in 0..32 {
            let desc = queue.read_descriptor(mem, idx)?;
            chain.push(desc);
            if desc.flags & VIRTQ_DESC_F_NEXT == 0 {
                break;
            }
            idx = desc.next;
        }

        if chain.len() < 3 {
            // virtio-blk requires at least: header + data + status
            warn!(chain_len = chain.len(), "virtio-blk: chain too short");
            queue.push_used(mem, head_idx as u32, 0)?;
            return Ok(());
        }

        // Layout: chain[0] = read-only header, chain[1] = data (read or write),
        // chain[2] = write-only status.
        let header_desc = &chain[0];
        let data_desc = &chain[1];
        let status_desc = &chain[2];

        // Read the request header from guest memory.
        let mut header_buf = [0u8; 16];
        if header_desc.len < 16 {
            return Err(CoreError::Backend(
                "virtio-blk: header descriptor too short".into(),
            ));
        }
        mem.read_into(header_desc.addr, &mut header_buf)?;
        let kind = u32::from_le_bytes(header_buf[0..4].try_into().unwrap());
        let sector = u64::from_le_bytes(header_buf[8..16].try_into().unwrap());

        let mut total_len = 0;
        let mut status = 0u8; // 0 = success, 1 = error

        match kind {
            VIRTIO_BLK_T_IN => {
                // Read from the backing file into the data descriptor.
                // The data descriptor must be writable.
                if data_desc.flags & VIRTQ_DESC_F_WRITE == 0 {
                    warn!("virtio-blk: read request data descriptor is not writable");
                    status = 1;
                } else {
                    let mut buf = vec![0u8; data_desc.len as usize];
                    match self.read(sector, &mut buf) {
                        Ok(()) => {
                            mem.write(data_desc.addr, &buf)?;
                            total_len = buf.len() as u32;
                        }
                        Err(e) => {
                            warn!(error = %e, "virtio-blk: read failed");
                            status = 1;
                        }
                    }
                }
            }
            VIRTIO_BLK_T_OUT => {
                // Write from the data descriptor to the backing file.
                let mut buf = vec![0u8; data_desc.len as usize];
                mem.read_into(data_desc.addr, &mut buf)?;
                if let Err(e) = self.write(sector, &buf) {
                    warn!(error = %e, "virtio-blk: write failed");
                    status = 1;
                }
            }
            VIRTIO_BLK_T_FLUSH => {
                if let Err(e) = self.flush() {
                    warn!(error = %e, "virtio-blk: flush failed");
                    status = 1;
                }
            }
            _ => {
                warn!(kind, "virtio-blk: unhandled request type");
                status = 1;
            }
        }

        // Write the status byte to the status descriptor.
        mem.write(status_desc.addr, &[status])?;
        // Total length to report = data bytes + 1 (status byte).
        total_len = total_len.saturating_add(1);

        // Push the result to the used ring.
        queue.push_used(mem, head_idx as u32, total_len)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn read_write_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.img");
        // Create a 4 MB file.
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(&vec![0u8; 4 * 1024 * 1024]).unwrap();
        drop(f);

        let blk = VirtioBlk::open(&path, false).unwrap();
        assert_eq!(blk.capacity_bytes(), 4 * 1024 * 1024);

        // Write a sector.
        let data = [0xABu8; 512];
        blk.write(10, &data).unwrap();

        // Read it back.
        let mut buf = [0u8; 512];
        blk.read(10, &mut buf).unwrap();
        assert_eq!(&buf, &data);
    }

    #[test]
    fn read_only_rejects_writes() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("ro.img");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(&vec![0u8; 1024]).unwrap();
        drop(f);

        let blk = VirtioBlk::open(&path, true).unwrap();
        assert!(blk.write(0, &[1]).is_err());
        // Reads still work.
        let mut buf = [0u8; 1];
        blk.read(0, &mut buf).unwrap();
    }
}
