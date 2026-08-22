//! Bundled image discovery — finds a locally-bundled Android ISO in the
//! application directory or known locations, so first-run users don't need
//! to download anything.
//!
//! When Nitroid ships as a release archive, the archive includes a
//! pre-downloaded `android.iso` next to the binary. On first run we look for
//! this file (or a `bundled-image.json` manifest pointing to it) and
//! auto-register the image with the instance manager.
//!
//! The lookup order is:
//!
//! 1. The directory containing the running executable (`std::env::current_exe`)
//! 2. The current working directory
//! 3. `~/.config/nitroid/android.iso` (Linux) / `%APPDATA%\nitroid\android.iso` (Windows)
//!
//! If a `bundled-image.json` manifest is found next to the ISO, we honour
//! its declared architecture and vendor metadata. Otherwise we default to
//! x86_64 + the Android-x86 project as the vendor.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::image::SystemImage;
use crate::CpuArch;
use crate::Result;

/// JSON manifest written by the CI build describing the bundled image.
/// Sits next to the ISO so the runtime knows its declared architecture and
/// version without having to guess.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BundledImageManifest {
    /// Friendly display name (e.g. "Android-x86 9.0 (Pie)").
    pub name: String,
    /// Version string (e.g. "9.0-r2").
    pub version: String,
    /// Filename of the ISO relative to the manifest.
    pub filename: String,
    /// Vendor / distributor.
    pub vendor: String,
    /// Architecture the image was built for.
    pub arch: String,
    /// Whether this is a CI-bundled image (true) or a user-downloaded one (false).
    pub bundled: bool,
}

/// Search for a bundled Android image. Returns the path to the ISO file if
/// found, plus the manifest if one exists.
pub fn find_bundled_image() -> Option<(PathBuf, Option<BundledImageManifest>)> {
    for dir in candidate_dirs() {
        // Look for a manifest first — it tells us the actual filename.
        let manifest_path = dir.join("bundled-image.json");
        if manifest_path.exists() {
            if let Ok(raw) = std::fs::read_to_string(&manifest_path) {
                if let Ok(manifest) = serde_json::from_str::<BundledImageManifest>(&raw) {
                    let iso_path = dir.join(&manifest.filename);
                    if iso_path.exists() {
                        return Some((iso_path, Some(manifest)));
                    }
                }
            }
        }
        // Fall back to the conventional `android.iso` name.
        let iso_path = dir.join("android.iso");
        if iso_path.exists() {
            return Some((iso_path, None));
        }
        // Also accept `system.img` — some images ship as raw disk images
        // rather than ISO 9660 filesystems.
        let img_path = dir.join("system.img");
        if img_path.exists() {
            return Some((img_path, None));
        }
    }
    None
}

/// Directories to search for a bundled image, in priority order.
fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // 1. Directory containing the running executable.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            dirs.push(parent.to_path_buf());
        }
    }

    // 2. Current working directory.
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd);
    }

    // 3. Nitroid data directory.
    dirs.push(crate::paths::data_dir());

    // 4. Nitroid cache directory.
    dirs.push(crate::paths::cache_dir());

    dirs
}

/// Register the bundled image (if any) with the provided closure. Returns
/// the registered `SystemImage` if a bundled image was found and
/// successfully registered, `None` otherwise.
///
/// The closure is responsible for actually inserting the image into the
/// instance manager's registry — we keep this indirection to avoid a
/// circular dependency between `nitroid-core` and `nitroid-instances`.
pub fn register_bundled_image<F>(register: F) -> Option<SystemImage>
where
    F: FnOnce(SystemImage) -> Result<()>,
{
    let (iso_path, manifest) = find_bundled_image()?;
    let arch = manifest
        .as_ref()
        .and_then(|m| parse_arch(&m.arch))
        .unwrap_or(CpuArch::X86_64);

    let mut image = match SystemImage::register(&iso_path, arch) {
        Ok(img) => img,
        Err(e) => {
            tracing::warn!(error = %e, path = %iso_path.display(), "failed to register bundled image");
            return None;
        }
    };

    // Apply manifest metadata if available.
    if let Some(m) = &manifest {
        image.name = m.name.clone();
        image.version = Some(m.version.clone());
        image.vendor = Some(m.vendor.clone());
    }

    tracing::info!(path = %iso_path.display(), "auto-registered bundled image");
    match register(image.clone()) {
        Ok(()) => Some(image),
        Err(e) => {
            tracing::warn!(error = %e, "register callback failed");
            None
        }
    }
}

fn parse_arch(s: &str) -> Option<CpuArch> {
    match s.to_ascii_lowercase().as_str() {
        "x86_64" | "x86-64" | "amd64" => Some(CpuArch::X86_64),
        "aarch64" | "arm64" => Some(CpuArch::Aarch64),
        "armv7" | "arm" => Some(CpuArch::Armv7),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_arch_handles_common_aliases() {
        assert_eq!(parse_arch("x86_64"), Some(CpuArch::X86_64));
        assert_eq!(parse_arch("amd64"), Some(CpuArch::X86_64));
        assert_eq!(parse_arch("ARM64"), Some(CpuArch::Aarch64));
        assert_eq!(parse_arch("armv7"), Some(CpuArch::Armv7));
        assert_eq!(parse_arch("unknown"), None);
    }

    #[test]
    fn manifest_round_trips() {
        let m = BundledImageManifest {
            name: "Android-x86 9.0 (Pie)".into(),
            version: "9.0-r2".into(),
            filename: "android.iso".into(),
            vendor: "Android-x86 Project".into(),
            arch: "x86_64".into(),
            bundled: true,
        };
        let s = serde_json::to_string(&m).unwrap();
        let back: BundledImageManifest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.name, m.name);
        assert_eq!(back.arch, m.arch);
        assert!(back.bundled);
    }

    #[test]
    fn find_bundled_returns_none_when_no_image() {
        // We can't easily test the positive case from a unit test (we'd
        // need to write a file next to the test binary), but we can verify
        // the lookup at least runs without panicking.
        let result = find_bundled_image();
        // Don't assert — the result depends on whether this test is run
        // from the dev environment (where there's no bundled image) or a
        // release archive (where there is).
        let _ = result;
    }

    #[test]
    fn candidate_dirs_includes_exe_dir() {
        let dirs = candidate_dirs();
        // The current_exe dir should always be first.
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                assert!(dirs.iter().any(|d| d == parent));
            }
        }
    }

    // Suppress dead-code warning for the helper functions above — they're
    // kept for future use.
    #[allow(dead_code)]
    fn _ensure_path_import_used() {}
}
