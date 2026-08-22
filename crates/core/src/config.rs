//! Configuration model — persisted to `~/.config/nitroid/config.toml`.
//!
//! The configuration is intentionally minimal: hardware acceleration backend,
//! graphics backend, CPU architecture target, and per-instance memory/CPU
//! defaults. Everything else is derived.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// CPU architecture the guest Android system was compiled for. We forward
/// ARM-compatible images through the translation layer when running on x86_64
/// hosts; x86_64 images run natively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CpuArch {
    /// Android-x86 / Bliss OS x86_64 images — native execution.
    #[default]
    X86_64,
    /// ARM64 Android images — forwarded through the translation bridge.
    Aarch64,
    /// ARMv7 Android images — forwarded through the translation bridge.
    Armv7,
}

/// Which host graphics API the WGPU renderer should target. `Auto` picks the
/// best available backend per platform (Vulkan on Linux, DX12 on Windows,
/// Metal on macOS).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum GraphicsBackend {
    /// Let WGPU choose (recommended).
    #[default]
    Auto,
    Vulkan,
    Dx12,
    Metal,
    OpenGl,
}

/// Hardware acceleration backend. `Auto` selects KVM on Linux, WHPX on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AccelBackend {
    #[default]
    Auto,
    Kvm,
    Whpx,
    /// Software fallback — slow, only for debugging on machines without virt.
    Tcg,
}

/// Global emulator configuration. Persisted as TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmulatorConfig {
    pub version: u32,
    pub accel: AccelBackend,
    pub graphics: GraphicsBackend,
    pub default_arch: CpuArch,
    pub default_memory_mb: u32,
    pub default_cpu_count: u32,
    pub default_dpi: u32,
    pub default_refresh_rate: u32,
    pub force_translation: bool,
    pub telemetry: bool,
}

impl Default for EmulatorConfig {
    fn default() -> Self {
        Self {
            version: 1,
            accel: AccelBackend::Auto,
            graphics: GraphicsBackend::Auto,
            default_arch: CpuArch::X86_64,
            default_memory_mb: 4096,
            default_cpu_count: 4,
            default_dpi: 320,
            default_refresh_rate: 60,
            force_translation: false,
            telemetry: false,
        }
    }
}

impl EmulatorConfig {
    /// Load the configuration from `~/.config/nitroid/config.toml`. If the
    /// file does not exist a default is written and returned.
    pub fn load_or_create(config_path: &Path) -> Result<Self> {
        if config_path.exists() {
            let raw = std::fs::read_to_string(config_path)?;
            let cfg: EmulatorConfig = toml::from_str(&raw).map_err(|e| {
                CoreError::Config(format!("failed to parse {}: {e}", config_path.display()))
            })?;
            Ok(cfg)
        } else {
            let cfg = EmulatorConfig::default();
            cfg.save(config_path)?;
            Ok(cfg)
        }
    }

    /// Persist the configuration as TOML.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let s = toml::to_string_pretty(self)
            .map_err(|e| CoreError::Config(format!("failed to serialize config: {e}")))?;
        std::fs::write(path, s)?;
        Ok(())
    }
}

/// Helper used by the UI and CLI to validate user-provided memory values.
pub fn validate_memory_mb(mb: u32) -> Result<()> {
    if mb < 1024 {
        return Err(CoreError::Config(format!(
            "memory must be at least 1024 MB, got {mb}"
        )));
    }
    if mb > 32 * 1024 {
        return Err(CoreError::Config(format!(
            "memory cap is 32768 MB (32 GB), got {mb}"
        )));
    }
    Ok(())
}

/// Helper used to validate CPU count.
pub fn validate_cpu_count(n: u32) -> Result<()> {
    let max = num_cpus();
    if n == 0 || n > max {
        return Err(CoreError::Config(format!(
            "cpu count must be in 1..={max}, got {n}"
        )));
    }
    Ok(())
}

/// Cheap host CPU count detection — used as a sanity bound for new instances.
pub fn num_cpus() -> u32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(4)
}

/// Resolve the directory in which to store the global config.
pub fn resolve_config_dir() -> PathBuf {
    if let Some(d) = directories::ProjectDirs::from("", crate::ORG_NAME, crate::APP_NAME) {
        d.config_dir().to_path_buf()
    } else {
        PathBuf::from(".nitroid")
    }
}
