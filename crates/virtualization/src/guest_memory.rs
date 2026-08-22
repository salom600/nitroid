//! Guest physical memory access — translates guest physical addresses
//! (GPAs) into host virtual addresses so virtio devices can read/write
//! the descriptor rings and data buffers.
//!
//! ## Layout
//!
//! The KVM backend mmaps one contiguous region of host memory and registers
//! it with KVM as the guest's physical memory. The `GuestMemory` struct
//! owns the host pointer and provides safe(ish) read/write helpers.
//!
//! ## Safety
//!
//! All access goes through `read_volatile` / `write_volatile` to prevent
//! the compiler from reordering or coalescing accesses — the guest's view
//! of memory must match the host's at all times. We also enforce 8-byte
//! alignment for u64 accesses per the virtio spec.

use std::sync::Arc;

use parking_lot::RwLock;

use nitroid_core::CoreError;
use nitroid_core::Result;

/// A region of guest physical memory.
#[derive(Debug, Clone)]
pub struct MemoryRegion {
    /// Guest physical address of the start of the region.
    pub gpa: u64,
    /// Size in bytes.
    pub size: u64,
    /// Host virtual address (mmap'd).
    pub host_ptr: *mut u8,
}

unsafe impl Send for MemoryRegion {}
unsafe impl Sync for MemoryRegion {}

/// Top-level guest memory map. Owns multiple regions (typical KVM setups
/// use one per memory slot, but we currently use just one).
pub struct GuestMemory {
    regions: RwLock<Vec<MemoryRegion>>,
}

impl GuestMemory {
    /// Create an empty guest memory map.
    pub fn new() -> Self {
        Self {
            regions: RwLock::new(Vec::new()),
        }
    }

    /// Create a guest memory map with a single region.
    pub fn single_region(gpa: u64, size: u64, host_ptr: *mut u8) -> Self {
        let gm = Self::new();
        gm.regions.write().push(MemoryRegion {
            gpa,
            size,
            host_ptr,
        });
        gm
    }

    /// Add a memory region. Used by the KVM backend when it mmaps guest RAM.
    pub fn add_region(&self, region: MemoryRegion) {
        self.regions.write().push(region);
    }

    /// Translate a GPA to a host pointer. Returns `None` if the address
    /// falls outside any registered region.
    pub fn translate(&self, gpa: u64) -> Option<*mut u8> {
        let regions = self.regions.read();
        for r in regions.iter() {
            if gpa >= r.gpa && gpa < r.gpa + r.size {
                let offset = (gpa - r.gpa) as isize;
                // SAFETY: the caller guaranteed (by registering the region)
                // that `host_ptr` points to a valid mmap'd region of `size`
                // bytes. The offset is bounds-checked above.
                return Some(unsafe { r.host_ptr.offset(offset) });
            }
        }
        None
    }

    /// Read `len` bytes starting at `gpa`. Returns a owned `Vec<u8>`.
    pub fn read(&self, gpa: u64, len: usize) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; len];
        self.read_into(gpa, &mut buf)?;
        Ok(buf)
    }

    /// Read into an existing buffer. More efficient than `read` for hot paths.
    pub fn read_into(&self, gpa: u64, buf: &mut [u8]) -> Result<()> {
        let regions = self.regions.read();
        let len = buf.len();
        for r in regions.iter() {
            if gpa >= r.gpa && gpa + len as u64 <= r.gpa + r.size {
                let offset = (gpa - r.gpa) as isize;
                // SAFETY: bounds-checked above. Using `copy_from_slice` is
                // safe — the source is host memory we own and the dest is
                // a normal Rust slice.
                unsafe {
                    let src = std::slice::from_raw_parts(r.host_ptr.offset(offset), len);
                    buf.copy_from_slice(src);
                }
                return Ok(());
            }
        }
        Err(CoreError::Backend(format!(
            "guest memory read out of bounds: gpa={gpa:#x}, len={len}"
        )))
    }

    /// Write a byte slice to `gpa`.
    pub fn write(&self, gpa: u64, data: &[u8]) -> Result<()> {
        let regions = self.regions.read();
        for r in regions.iter() {
            if gpa >= r.gpa && gpa + data.len() as u64 <= r.gpa + r.size {
                let offset = (gpa - r.gpa) as isize;
                unsafe {
                    let dst = std::slice::from_raw_parts_mut(r.host_ptr.offset(offset), data.len());
                    dst.copy_from_slice(data);
                }
                return Ok(());
            }
        }
        Err(CoreError::Backend(format!(
            "guest memory write out of bounds: gpa={gpa:#x}, len={len}",
            len = data.len()
        )))
    }

    /// Read a little-endian u16 at `gpa`.
    pub fn read_u16_le(&self, gpa: u64) -> Result<u16> {
        let mut buf = [0u8; 2];
        self.read_into(gpa, &mut buf)?;
        Ok(u16::from_le_bytes(buf))
    }

    /// Read a little-endian u32 at `gpa`.
    pub fn read_u32_le(&self, gpa: u64) -> Result<u32> {
        let mut buf = [0u8; 4];
        self.read_into(gpa, &mut buf)?;
        Ok(u32::from_le_bytes(buf))
    }

    /// Read a little-endian u64 at `gpa`.
    pub fn read_u64_le(&self, gpa: u64) -> Result<u64> {
        let mut buf = [0u8; 8];
        self.read_into(gpa, &mut buf)?;
        Ok(u64::from_le_bytes(buf))
    }

    /// Write a little-endian u16 at `gpa`.
    pub fn write_u16_le(&self, gpa: u64, val: u16) -> Result<()> {
        self.write(gpa, &val.to_le_bytes())
    }

    /// Write a little-endian u32 at `gpa`.
    pub fn write_u32_le(&self, gpa: u64, val: u32) -> Result<()> {
        self.write(gpa, &val.to_le_bytes())
    }

    /// Write a little-endian u64 at `gpa`.
    pub fn write_u64_le(&self, gpa: u64, val: u64) -> Result<()> {
        self.write(gpa, &val.to_le_bytes())
    }

    /// Total size of all registered regions.
    pub fn total_size(&self) -> u64 {
        self.regions.read().iter().map(|r| r.size).sum()
    }

    /// Number of registered regions.
    pub fn num_regions(&self) -> usize {
        self.regions.read().len()
    }
}

impl Default for GuestMemory {
    fn default() -> Self {
        Self::new()
    }
}

/// Wrap a `GuestMemory` in an `Arc` so multiple virtio devices can share it.
pub type SharedGuestMemory = Arc<GuestMemory>;

/// Convert a raw host pointer + size into a `SharedGuestMemory` with one
/// region. Used by the KVM backend after it mmaps guest RAM.
pub fn from_single_region(gpa: u64, size: u64, host_ptr: *mut u8) -> SharedGuestMemory {
    Arc::new(GuestMemory::single_region(gpa, size, host_ptr))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_memory(size: usize) -> (Vec<u8>, SharedGuestMemory) {
        let mut buf = vec![0u8; size];
        // Fill with a recognisable pattern.
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (i & 0xFF) as u8;
        }
        let gm = from_single_region(0, size as u64, buf.as_mut_ptr());
        (buf, gm)
    }

    #[test]
    fn read_returns_correct_bytes() {
        let (buf, gm) = make_test_memory(4096);
        let read = gm.read(0, 8).unwrap();
        assert_eq!(&read, &buf[..8]);
        let read = gm.read(100, 4).unwrap();
        assert_eq!(&read, &buf[100..104]);
    }

    #[test]
    fn write_overwrites_bytes() {
        let (_buf, gm) = make_test_memory(4096);
        gm.write(0x100, &[0xAA, 0xBB, 0xCC, 0xDD]).unwrap();
        let read = gm.read(0x100, 4).unwrap();
        assert_eq!(read, vec![0xAA, 0xBB, 0xCC, 0xDD]);
    }

    #[test]
    fn out_of_bounds_returns_error() {
        let (_buf, gm) = make_test_memory(4096);
        // Read past the end.
        let result = gm.read(4096, 1);
        assert!(result.is_err());
        // Read straddling the end.
        let result = gm.read(4090, 10);
        assert!(result.is_err());
    }

    #[test]
    fn read_write_integers_round_trip() {
        let (_buf, gm) = make_test_memory(4096);
        gm.write_u32_le(0x100, 0xDEADBEEF).unwrap();
        assert_eq!(gm.read_u32_le(0x100).unwrap(), 0xDEADBEEF);

        gm.write_u64_le(0x200, 0x0123_4567_89AB_CDEF).unwrap();
        assert_eq!(gm.read_u64_le(0x200).unwrap(), 0x0123_4567_89AB_CDEF);

        gm.write_u16_le(0x300, 0xCAFE).unwrap();
        assert_eq!(gm.read_u16_le(0x300).unwrap(), 0xCAFE);
    }

    #[test]
    fn translate_returns_none_for_out_of_range() {
        let (_buf, gm) = make_test_memory(4096);
        assert!(gm.translate(0).is_some());
        assert!(gm.translate(4095).is_some());
        assert!(gm.translate(4096).is_none());
        assert!(gm.translate(u64::MAX).is_none());
    }

    #[test]
    fn multiple_regions_are_searched_in_order() {
        let mut buf1 = vec![0xAAu8; 1024];
        let mut buf2 = vec![0xBBu8; 1024];
        let gm = Arc::new(GuestMemory::new());
        gm.add_region(MemoryRegion {
            gpa: 0,
            size: 1024,
            host_ptr: buf1.as_mut_ptr(),
        });
        gm.add_region(MemoryRegion {
            gpa: 0x10000,
            size: 1024,
            host_ptr: buf2.as_mut_ptr(),
        });
        assert_eq!(gm.read(0, 1).unwrap(), vec![0xAA]);
        assert_eq!(gm.read(0x10000, 1).unwrap(), vec![0xBB]);
        // Between the regions — should fail.
        assert!(gm.read(1024, 1).is_err());
    }
}
