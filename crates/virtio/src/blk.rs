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
use tracing::{info, warn};

use crate::queue::VirtQueue;
use crate::transport::DeviceId;
use crate::VirtioDevice;
use nitroid_core::Result;

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

    fn process_queue(&self, _queue_idx: usize, queue: &VirtQueue) -> Result<usize> {
        let pending = queue.pending();
        if pending == 0 {
            return Ok(0);
        }
        // The actual descriptor parsing requires access to guest memory
        // (the desc/avail/used rings live there). For the scaffold we just
        // mark all pending descriptors as completed — this lets the guest
        // boot progress through device probe without hanging.
        for _ in 0..pending {
            queue.complete();
        }
        if pending > 0 {
            warn!(
                pending,
                "virtio-blk: {pending} requests stubbed (guest memory access not yet wired)"
            );
        }
        Ok(pending as usize)
    }

    fn reset(&self) {
        // No internal state to reset — the queue resets itself.
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
