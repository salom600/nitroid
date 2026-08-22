//! System image registry — represents a pre-built Android image (Android-x86 /
//! Bliss OS) that can be attached to an emulator instance as the system disk.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::CpuArch;
use crate::error::{CoreError, Result};

/// A registered pre-built Android system image. The image file itself is
/// **not** stored in the registry — only its path, fingerprint, and metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemImage {
    /// Friendly name shown in the UI (e.g. "Bliss OS 16").
    pub name: String,
    /// Absolute path to the image file on disk.
    pub path: PathBuf,
    /// Architecture the image was built for.
    pub arch: CpuArch,
    /// BLAKE3 hash of the image (computed lazily on first registration).
    pub fingerprint: String,
    /// Image size in bytes.
    pub size_bytes: u64,
    /// Optional human-readable version string (e.g. "14.0-r6").
    pub version: Option<String>,
    /// Optional vendor / distributor (e.g. "Android-x86 Project").
    pub vendor: Option<String>,
}

impl SystemImage {
    /// Register a new system image from a file path. The BLAKE3 fingerprint is
    /// computed lazily — for large images this can take a few seconds.
    pub fn register(path: impl AsRef<Path>, arch: CpuArch) -> Result<Self> {
        let path = path.as_ref();
        let metadata = std::fs::metadata(path).map_err(|_| {
            CoreError::ImageNotFound(format!("image file not found: {}", path.display()))
        })?;
        if !metadata.is_file() {
            return Err(CoreError::InvalidImage(format!(
                "path is not a regular file: {}",
                path.display()
            )));
        }
        let size_bytes = metadata.len();
        if size_bytes < 512 * 1024 * 1024 {
            return Err(CoreError::InvalidImage(format!(
                "image file is suspiciously small ({} bytes, expected >= 512 MiB)",
                size_bytes
            )));
        }

        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_string();

        let fingerprint = fingerprint_file(path)?;

        Ok(Self {
            name,
            path: path.canonicalize().unwrap_or_else(|_| path.to_path_buf()),
            arch,
            fingerprint,
            size_bytes,
            version: None,
            vendor: None,
        })
    }

    /// Verify the image on disk still matches the stored fingerprint. Returns
    /// `false` if the file has been modified, deleted, or replaced.
    pub fn verify(&self) -> bool {
        if !self.path.exists() {
            return false;
        }
        match fingerprint_file(&self.path) {
            Ok(actual) => actual == self.fingerprint,
            Err(_) => false,
        }
    }
}

fn fingerprint_file(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut hasher = blake3::Hasher::new();
    let mut file = std::fs::File::open(path)?;
    let mut buf = [0u8; 1 << 20]; // 1 MiB buffer
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}
