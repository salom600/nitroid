//! Native ISO 9660 (ECMA-119) parser — reads Android-x86 ISO images and
//! extracts the kernel + initrd.
//!
//! ## Format overview
//!
//! An ISO 9660 image is laid out as:
//!
//! ```text
//! 0x0000 - 0x7FFF : System area (unused)
//! 0x8000 - 0x8007 : Primary Volume Descriptor (PVD)
//! 0x8800 - ...   : Root directory + files
//! ```
//!
//! The PVD is at offset 0x8000 (sector 16). Sector size is always 2048 bytes.
//! The root directory entry inside the PVD points to the root directory
//! extent, which contains entries for every file at the top level.
//!
//! ## Limitations
//!
//! This parser implements the subset of ISO 9660 needed for Android-x86
//! images:
//!
//! - Primary Volume Descriptor only (no Joliet / Rock Ridge extensions)
//! - Plain 8.3 filenames (UPPERCASE, no extensions past `;1`)
//! - Single extent files (no multi-extent)
//! - No interleaving
//!
//! This is sufficient for `/boot/vmlinuz` and `/boot/initrd.img` on
//! Android-x86 ISOs.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use tracing::{debug, info};

/// ISO 9660 sector size — always 2048 bytes.
pub const SECTOR_SIZE: u64 = 2048;

/// Offset of the Primary Volume Descriptor.
pub const PVD_OFFSET: u64 = 0x8000;

/// Magic bytes at offset 0x8001 of an ISO 9660 image ("CD001").
pub const ISO_MAGIC: &[u8] = b"CD001";

/// Volume descriptor types.
pub const VDT_BOOT_RECORD: u8 = 0;
pub const VDT_PRIMARY: u8 = 1;
pub const VDT_SUPPLEMENTARY: u8 = 2;
pub const VDT_PARTITION: u8 = 3;
pub const VDT_TERMINATOR: u8 = 255;

/// A directory record inside an ISO 9660 filesystem.
#[derive(Debug, Clone)]
pub struct DirectoryRecord {
    /// Length of this record (including all extensions).
    pub length: u8,
    /// Extended attribute record length (usually 0).
    pub ext_attr_length: u8,
    /// Location of the file's first data extent (LBA, in sectors).
    pub extent_lba: u32,
    /// Size of the file's data in bytes.
    pub size: u32,
    /// Flags (directory, hidden, etc.).
    pub flags: u8,
    /// File unit size (interleaved files only — 0 for us).
    pub file_unit_size: u8,
    /// Interleave gap size (0 for us).
    pub interleave_gap: u8,
    /// Volume sequence number (always 1 for us).
    pub volume_seq: u16,
    /// Identifier (filename). 8.3 format, uppercase.
    pub identifier: String,
}

impl DirectoryRecord {
    /// Whether this record represents a directory.
    pub fn is_directory(&self) -> bool {
        self.flags & 0x02 != 0
    }

    /// Whether this record is the "." (current dir) entry.
    pub fn is_dot(&self) -> bool {
        self.identifier == "\u{00}"
    }

    /// Whether this record is the ".." (parent dir) entry.
    pub fn is_dotdot(&self) -> bool {
        self.identifier == "\u{01}"
    }
}

/// Primary Volume Descriptor (PVD) — parsed form.
#[derive(Debug, Clone)]
pub struct PrimaryVolumeDescriptor {
    pub system_id: String,
    pub volume_id: String,
    pub volume_space_size: u32,
    pub volume_set_size: u16,
    pub volume_sequence_number: u16,
    pub logical_block_size: u16,
    pub path_table_size: u32,
    pub root_directory: DirectoryRecord,
    pub volume_set_id: String,
    pub publisher_id: String,
    pub data_preparer_id: String,
    pub application_id: String,
}

/// Top-level ISO reader. Owns the file handle.
pub struct IsoReader {
    file: File,
    pvd: PrimaryVolumeDescriptor,
}

impl IsoReader {
    /// Open an ISO 9660 image for reading.
    pub fn open(path: &Path) -> Result<IsoReader, ParseError> {
        let mut file = File::open(path).map_err(ParseError::Io)?;
        let pvd = read_pvd(&mut file)?;
        info!(
            volume_id = %pvd.volume_id.trim(),
            block_size = pvd.logical_block_size,
            space_size = pvd.volume_space_size,
            "opened ISO 9660 image"
        );
        Ok(Self { file, pvd })
    }

    /// The parsed Primary Volume Descriptor.
    pub fn pvd(&self) -> &PrimaryVolumeDescriptor {
        &self.pvd
    }

    /// List the root directory entries.
    pub fn list_root(&mut self) -> Result<Vec<DirectoryRecord>, ParseError> {
        let root = &self.pvd.root_directory;
        read_directory(&mut self.file, root.extent_lba, root.size)
    }

    /// Find a file by path (e.g. "BOOT/VMlinuz"). Path separators are
    /// case-insensitive; the parser normalises both sides to uppercase.
    pub fn find(&mut self, path: &str) -> Result<Option<DirectoryRecord>, ParseError> {
        let normalized = path.to_ascii_uppercase();
        let segments: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
        let mut current = self.pvd.root_directory.clone();
        for (i, seg) in segments.iter().enumerate() {
            let children = read_directory(&mut self.file, current.extent_lba, current.size)?;
            let target_name = strip_version_suffix(seg);
            let found = children.into_iter().find(|c| {
                if c.is_dot() || c.is_dotdot() {
                    return false;
                }
                let cid = strip_version_suffix(&c.identifier);
                cid == target_name
            });
            match found {
                Some(record) => {
                    current = record;
                    let _ = i;
                }
                None => return Ok(None),
            }
        }
        Ok(Some(current))
    }

    /// Read a file's contents into a `Vec<u8>`.
    pub fn read_file(&mut self, record: &DirectoryRecord) -> Result<Vec<u8>, ParseError> {
        let mut buf = vec![0u8; record.size as usize];
        let offset = record.extent_lba as u64 * SECTOR_SIZE;
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(ParseError::Io)?;
        self.file.read_exact(&mut buf).map_err(ParseError::Io)?;
        Ok(buf)
    }

    /// Convenience: find a file by path and read its contents.
    pub fn read_path(&mut self, path: &str) -> Result<Option<Vec<u8>>, ParseError> {
        match self.find(path)? {
            Some(record) => Ok(Some(self.read_file(&record)?)),
            None => Ok(None),
        }
    }
}

/// Strip the `;1` version suffix that ISO 9660 appends to every filename.
fn strip_version_suffix(s: &str) -> String {
    s.split(';').next().unwrap_or(s).to_string()
}

/// Read and parse the Primary Volume Descriptor from the file.
fn read_pvd(file: &mut File) -> Result<PrimaryVolumeDescriptor, ParseError> {
    let mut sector = [0u8; SECTOR_SIZE as usize];
    // Walk the volume descriptor set starting at sector 16 (offset 0x8000).
    // The set is terminated by a descriptor with type 255.
    let mut sector_idx: u64 = 16;
    loop {
        file.seek(SeekFrom::Start(sector_idx * SECTOR_SIZE))
            .map_err(ParseError::Io)?;
        file.read_exact(&mut sector).map_err(ParseError::Io)?;

        // Check the magic at offset 1.
        if &sector[1..6] != ISO_MAGIC {
            return Err(ParseError::NotIso9660);
        }

        let vdt_type = sector[0];
        match vdt_type {
            VDT_PRIMARY => {
                return parse_pvd(&sector);
            }
            VDT_TERMINATOR => {
                return Err(ParseError::NoPrimaryDescriptor);
            }
            _ => {
                // Skip boot record / supplementary / partition descriptors.
                sector_idx += 1;
                if sector_idx > 64 {
                    // Sanity limit — real ISOs have at most a handful.
                    return Err(ParseError::NoPrimaryDescriptor);
                }
            }
        }
    }
}

/// Parse a Primary Volume Descriptor from a 2048-byte buffer.
fn parse_pvd(buf: &[u8]) -> Result<PrimaryVolumeDescriptor, ParseError> {
    if buf.len() < 2048 {
        return Err(ParseError::Truncated);
    }
    if &buf[1..6] != ISO_MAGIC {
        return Err(ParseError::NotIso9660);
    }
    if buf[0] != VDT_PRIMARY {
        return Err(ParseError::UnexpectedDescriptorType(buf[0]));
    }

    // Field offsets per ECMA-119:
    //   8  - 40  : system identifier (32 bytes, ASCII)
    //   40 - 72  : volume identifier (32 bytes, ASCII)
    //   72 - 80  : unused (zeros)
    //   80 - 84  : volume space size (LE u32 + BE u32)
    //   84 - 88  : (BE part of above)
    //   88 - 120 : unused
    //   120 - 122: logical block size (LE u16 + BE u16)
    //   122 - 124: (BE part of above)
    //   124 - 128: path table size (LE u32 + BE u32)
    //   ...
    //   156 - 190: root directory record (34 bytes)

    let system_id = read_ascii(&buf[8..40]);
    let volume_id = read_ascii(&buf[40..72]);
    let volume_space_size = read_le_u32(&buf[80..84]);
    let logical_block_size = read_le_u16(&buf[128..130]);
    let path_table_size = read_le_u32(&buf[132..136]);
    let volume_set_size = read_le_u16(&buf[120..122]);
    let volume_sequence_number = read_le_u16(&buf[124..126]);

    let root = parse_directory_record(&buf[156..190])?;

    Ok(PrimaryVolumeDescriptor {
        system_id,
        volume_id,
        volume_space_size,
        volume_set_size,
        volume_sequence_number,
        logical_block_size,
        path_table_size,
        root_directory: root,
        volume_set_id: read_ascii(&buf[190..318]),
        publisher_id: read_ascii(&buf[318..446]),
        data_preparer_id: read_ascii(&buf[446..574]),
        application_id: read_ascii(&buf[574..702]),
    })
}

/// Parse a directory record starting at the beginning of `buf`.
fn parse_directory_record(buf: &[u8]) -> Result<DirectoryRecord, ParseError> {
    if buf.is_empty() {
        return Err(ParseError::Truncated);
    }
    let length = buf[0];
    if length == 0 {
        // A length of 0 means the rest of the sector is padding.
        return Err(ParseError::EmptyRecord);
    }
    if (length as usize) > buf.len() {
        return Err(ParseError::Truncated);
    }
    let buf = &buf[..length as usize];

    let ext_attr_length = buf[1];
    let extent_lba = read_le_u32(&buf[2..6]);
    let size = read_le_u32(&buf[10..14]);
    let flags = buf[25];
    let file_unit_size = buf[26];
    let interleave_gap = buf[27];
    let volume_seq = read_le_u16(&buf[28..30]);
    let id_len = buf[32] as usize;
    if 33 + id_len > buf.len() {
        return Err(ParseError::Truncated);
    }
    let identifier = String::from_utf8_lossy(&buf[33..33 + id_len]).to_string();

    Ok(DirectoryRecord {
        length,
        ext_attr_length,
        extent_lba,
        size,
        flags,
        file_unit_size,
        interleave_gap,
        volume_seq,
        identifier,
    })
}

/// Read all directory records from the directory extent starting at `lba`.
fn read_directory(
    file: &mut File,
    lba: u32,
    size: u32,
) -> Result<Vec<DirectoryRecord>, ParseError> {
    let mut buf = vec![0u8; size as usize];
    file.seek(SeekFrom::Start(lba as u64 * SECTOR_SIZE))
        .map_err(ParseError::Io)?;
    file.read_exact(&mut buf).map_err(ParseError::Io)?;

    let mut records = Vec::new();
    let mut offset = 0;
    while offset < buf.len() {
        let remaining = &buf[offset..];
        if remaining.is_empty() || remaining[0] == 0 {
            // End of sector — skip to the next 2048-byte boundary.
            let next_sector = (offset / SECTOR_SIZE as usize + 1) * SECTOR_SIZE as usize;
            if next_sector >= buf.len() {
                break;
            }
            offset = next_sector;
            continue;
        }
        let length = remaining[0] as usize;
        if length == 0 || offset + length > buf.len() {
            break;
        }
        match parse_directory_record(&buf[offset..offset + length]) {
            Ok(rec) => records.push(rec),
            Err(ParseError::EmptyRecord) => {
                // Skip padding.
            }
            Err(e) => return Err(e),
        }
        offset += length;
        // Records are padded to even offsets.
        if offset % 2 == 1 {
            offset += 1;
        }
    }
    debug!(count = records.len(), "parsed directory records");
    Ok(records)
}

fn read_ascii(buf: &[u8]) -> String {
    let s = String::from_utf8_lossy(buf);
    s.trim_matches(|c: char| c == '\0' || c == ' ').to_string()
}

fn read_le_u16(buf: &[u8]) -> u16 {
    u16::from_le_bytes([buf[0], buf[1]])
}

fn read_le_u32(buf: &[u8]) -> u32 {
    u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]])
}

/// Errors returned by the ISO 9660 parser.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("not an ISO 9660 image — missing CD001 magic")]
    NotIso9660,

    #[error("no primary volume descriptor found")]
    NoPrimaryDescriptor,

    #[error("unexpected volume descriptor type: {0}")]
    UnexpectedDescriptorType(u8),

    #[error("truncated record")]
    Truncated,

    #[error("empty directory record")]
    EmptyRecord,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build a minimal in-memory ISO 9660 image with a PVD + a root directory
    /// containing a single file "HELLO.TXT" containing "hello world".
    fn build_test_iso() -> Vec<u8> {
        let sector_size = SECTOR_SIZE as usize;
        let mut buf = vec![0u8; 32 * sector_size];

        // Volume descriptor set starts at sector 16.
        let pvd_sector = 16;
        let pvd_offset = pvd_sector * sector_size;
        buf[pvd_offset] = VDT_PRIMARY; // Type
        buf[pvd_offset + 1..pvd_offset + 6].copy_from_slice(ISO_MAGIC);
        buf[pvd_offset + 6] = 1; // Version
                                 // System ID and Volume ID — both 32-byte ASCII fields, space-padded.
        let mut sys_id = [b' '; 32];
        sys_id[..7].copy_from_slice(b"TESTSYS");
        buf[pvd_offset + 8..pvd_offset + 40].copy_from_slice(&sys_id);
        let mut vol_id = [b' '; 32];
        vol_id[..7].copy_from_slice(b"TESTVOL");
        buf[pvd_offset + 40..pvd_offset + 72].copy_from_slice(&vol_id);

        // Volume space size = 32 sectors (LE + BE)
        let space_size: u32 = 32;
        buf[pvd_offset + 80..pvd_offset + 84].copy_from_slice(&space_size.to_le_bytes());
        buf[pvd_offset + 84..pvd_offset + 88].copy_from_slice(&space_size.to_be_bytes());

        // Logical block size = 2048 (LE u16 at offset 128, BE u16 at 130)
        let block_size: u16 = 2048;
        buf[pvd_offset + 128..pvd_offset + 130].copy_from_slice(&block_size.to_le_bytes());
        buf[pvd_offset + 130..pvd_offset + 132].copy_from_slice(&block_size.to_be_bytes());

        // Root directory record at offset 156 within PVD sector.
        // Root dir lives at sector 19, size = 1 sector.
        let root_lba: u32 = 19;
        let root_size: u32 = sector_size as u32;
        let root_record = build_directory_record(34, root_lba, root_size, 0x02, "\u{00}");
        buf[pvd_offset + 156..pvd_offset + 156 + root_record.len()].copy_from_slice(&root_record);

        // Volume set ID, publisher, etc. — leave blank.

        // Volume descriptor set terminator at sector 17.
        let term_offset = 17 * sector_size;
        buf[term_offset] = VDT_TERMINATOR;
        buf[term_offset + 1..term_offset + 6].copy_from_slice(ISO_MAGIC);
        buf[term_offset + 6] = 1;

        // Write the root directory at sector 19. Put a single file in it.
        let root_dir_offset = 19 * sector_size;
        let file_lba: u32 = 21;
        let file_contents = b"hello world";
        let file_size = file_contents.len() as u32;
        let file_record = build_directory_record(
            33 + b"HELLO.TXT;1".len() as u8,
            file_lba,
            file_size,
            0x00,
            "HELLO.TXT;1",
        );
        buf[root_dir_offset..root_dir_offset + file_record.len()].copy_from_slice(&file_record);

        // Write the file contents at sector 21.
        let file_offset = 21 * sector_size;
        buf[file_offset..file_offset + file_contents.len()].copy_from_slice(file_contents);

        buf
    }

    /// Build a directory record. We pad to even length per the spec.
    fn build_directory_record(len: u8, lba: u32, size: u32, flags: u8, id: &str) -> Vec<u8> {
        let id_bytes = id.as_bytes();
        let mut rec = vec![0u8; len as usize];
        rec[0] = len;
        rec[1] = 0; // ext attr length
        rec[2..6].copy_from_slice(&lba.to_le_bytes());
        rec[6..10].copy_from_slice(&lba.to_be_bytes());
        rec[10..14].copy_from_slice(&size.to_le_bytes());
        rec[14..18].copy_from_slice(&size.to_be_bytes());
        // Recording date at 18..25 (7 bytes) — zeros
        rec[25] = flags;
        rec[26] = 0; // file unit size
        rec[27] = 0; // interleave gap
        rec[28..30].copy_from_slice(&1u16.to_le_bytes()); // volume seq LE
        rec[30..32].copy_from_slice(&1u16.to_be_bytes()); // volume seq BE
        rec[32] = id_bytes.len() as u8;
        rec[33..33 + id_bytes.len()].copy_from_slice(id_bytes);
        rec
    }

    #[test]
    fn parses_test_iso_pvd() {
        let iso = build_test_iso();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.iso");
        let mut f = File::create(&path).unwrap();
        f.write_all(&iso).unwrap();
        drop(f);

        let reader = IsoReader::open(&path).unwrap();
        let pvd = reader.pvd();
        assert_eq!(pvd.system_id, "TESTSYS");
        assert_eq!(pvd.volume_id, "TESTVOL");
        assert_eq!(pvd.logical_block_size, 2048);
        assert_eq!(pvd.volume_space_size, 32);
    }

    #[test]
    fn lists_root_directory() {
        let iso = build_test_iso();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.iso");
        let mut f = File::create(&path).unwrap();
        f.write_all(&iso).unwrap();
        drop(f);

        let mut reader = IsoReader::open(&path).unwrap();
        let entries = reader.list_root().unwrap();
        // Should contain HELLO.TXT (and possibly the . / .. entries).
        let names: Vec<&str> = entries.iter().map(|e| e.identifier.as_str()).collect();
        assert!(names.iter().any(|n| n.starts_with("HELLO.TXT")));
    }

    #[test]
    fn reads_file_contents() {
        let iso = build_test_iso();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.iso");
        let mut f = File::create(&path).unwrap();
        f.write_all(&iso).unwrap();
        drop(f);

        let mut reader = IsoReader::open(&path).unwrap();
        let bytes = reader.read_path("HELLO.TXT").unwrap();
        assert_eq!(bytes.as_deref(), Some(b"hello world".as_slice()));
    }

    #[test]
    fn find_is_case_insensitive() {
        let iso = build_test_iso();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.iso");
        let mut f = File::create(&path).unwrap();
        f.write_all(&iso).unwrap();
        drop(f);

        let mut reader = IsoReader::open(&path).unwrap();
        // Mixed case should still match the uppercased on-disk name.
        assert!(reader.find("hello.txt").unwrap().is_some());
        assert!(reader.find("HELLO.TXT").unwrap().is_some());
        assert!(reader.find("nonexistent.txt").unwrap().is_none());
    }

    #[test]
    fn rejects_non_iso_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-an-iso.bin");
        // Write a 200 KiB file of zeros — large enough to walk all candidate
        // VDT sectors without hitting EOF, but missing the CD001 magic.
        let buf = vec![0u8; 200 * 1024];
        std::fs::write(&path, &buf).unwrap();
        drop(buf);
        let result = IsoReader::open(&path);
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("expected error, got Ok"),
        };
        assert!(
            matches!(
                err,
                ParseError::NotIso9660 | ParseError::NoPrimaryDescriptor
            ),
            "expected NotIso9660 or NoPrimaryDescriptor, got: {err:?}"
        );
    }

    #[test]
    fn strip_version_suffix_works() {
        assert_eq!(strip_version_suffix("HELLO.TXT;1"), "HELLO.TXT");
        assert_eq!(strip_version_suffix("HELLO.TXT"), "HELLO.TXT");
        assert_eq!(strip_version_suffix(";1"), "");
    }
}
