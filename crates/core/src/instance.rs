//! Instance definitions — the in-memory representation of an emulator instance.
//!
//! An "instance" is a logical Android environment identified by a stable UUID
//! and a friendly name. It binds together:
//!
//! - the system image to boot from
//! - the per-instance overlay disk (writable, layered on top of the read-only image)
//! - per-instance CPU / memory / DPI / display settings
//! - per-instance input keymap
//! - the current lifecycle state (`Stopped` → `Booting` → `Running` → `Paused`)

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::{validate_cpu_count, validate_memory_mb, CpuArch};
use crate::error::{CoreError, Result};
use crate::image::SystemImage;

/// Stable identifier for an emulator instance.
pub type InstanceId = String;

/// Lifecycle states visible to the UI / control panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum InstanceState {
    #[default]
    Stopped,
    Booting,
    Running,
    Paused,
    Crashed,
    Saving,
}

/// Per-instance configuration. Persisted to the instance registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceConfig {
    /// Stable UUID-style identifier.
    pub id: InstanceId,
    /// Friendly display name (e.g. "PUBG main").
    pub name: String,
    /// Fingerprint of the bound system image (looked up in the image registry).
    pub image_fingerprint: String,
    /// Architecture override — defaults to the image's arch.
    pub arch: CpuArch,
    /// Memory in megabytes.
    pub memory_mb: u32,
    /// CPU count exposed to the guest.
    pub cpu_count: u32,
    /// Display DPI.
    pub dpi: u32,
    /// Display width in pixels.
    pub width: u32,
    /// Display height in pixels.
    pub height: u32,
    /// Refresh rate in Hz.
    pub refresh_rate: u32,
    /// Path to the per-instance overlay disk file (writable layer).
    pub overlay_path: PathBuf,
    /// Path to the per-instance keymap file. Falls back to global keymap if `None`.
    pub keymap_path: Option<PathBuf>,
    /// Optional "parent" instance this one was cloned from (for shared blueprints).
    pub cloned_from: Option<InstanceId>,
    /// Force ARM translation even when running an x86_64 image.
    pub force_translation: bool,
}

impl InstanceConfig {
    /// Create a new instance configuration. The instance ID is generated.
    pub fn new(name: impl Into<String>, image: &SystemImage) -> Result<Self> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(CoreError::Config("instance name must not be empty".into()));
        }
        let id = new_instance_id();
        let overlay_path = crate::paths::instances_dir().join(format!("{id}.overlay.qcow2"));

        validate_memory_mb(crate::config::EmulatorConfig::default().default_memory_mb)?;
        Ok(Self {
            id,
            name,
            image_fingerprint: image.fingerprint.clone(),
            arch: image.arch,
            memory_mb: crate::config::EmulatorConfig::default().default_memory_mb,
            cpu_count: crate::config::EmulatorConfig::default().default_cpu_count,
            dpi: crate::config::EmulatorConfig::default().default_dpi,
            width: 1280,
            height: 720,
            refresh_rate: 60,
            overlay_path,
            keymap_path: None,
            cloned_from: None,
            force_translation: false,
        })
    }

    /// Validate the configuration before launching the VM.
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(CoreError::Config("instance name is empty".into()));
        }
        if self.width == 0 || self.height == 0 {
            return Err(CoreError::Config("invalid display resolution".into()));
        }
        validate_memory_mb(self.memory_mb)?;
        validate_cpu_count(self.cpu_count)?;
        Ok(())
    }
}

/// Generate a short URL-safe instance ID. Uses BLAKE3 on random bytes for
/// collision resistance without depending on the `uuid` crate.
fn new_instance_id() -> InstanceId {
    let mut seed = [0u8; 16];
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    seed[..8].copy_from_slice(&nanos.to_le_bytes()[..8]);
    // u32 PID is 4 bytes — pad with a thread-local counter for uniqueness
    // across concurrent calls in the same process.
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let extra = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid_bytes = std::process::id().to_le_bytes();
    seed[8..12].copy_from_slice(&pid_bytes);
    seed[12..16].copy_from_slice(&extra.to_le_bytes());
    let hash = blake3::hash(&seed);
    let hex = hash.to_hex().to_string();
    // 16-char ID, dashes for readability.
    format!("{}-{}", &hex[..8], &hex[8..12])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_ids_are_unique() {
        let a = new_instance_id();
        let b = new_instance_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 13);
    }

    #[test]
    fn validation_rejects_empty_name() {
        let mut cfg = InstanceConfig {
            id: "x".into(),
            name: "  ".into(),
            image_fingerprint: "x".into(),
            arch: CpuArch::X86_64,
            memory_mb: 2048,
            cpu_count: 2,
            dpi: 320,
            width: 1280,
            height: 720,
            refresh_rate: 60,
            overlay_path: PathBuf::new(),
            keymap_path: None,
            cloned_from: None,
            force_translation: false,
        };
        assert!(cfg.validate().is_err());
        cfg.name = "test".into();
        assert!(cfg.validate().is_ok());
    }
}
