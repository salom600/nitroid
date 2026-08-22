//! Standardised, per-platform directory layout used by every Nitroid crate.
//!
//! All persistent state lives under one of three roots:
//!
//! - `data_dir`         — configuration and registry files (small, frequently read)
//! - `cache_dir`        — downloaded system images (large, can be deleted safely)
//! - `instances_dir`    — per-instance overlay + snapshots (large, never auto-deleted)
//!
//! On Linux this maps to `~/.config/nitroid`, `~/.cache/nitroid`, `~/.local/share/nitroid`
//! On Windows this maps to `%APPDATA%\nitroid`, `%LOCALAPPDATA%\nitroid\cache`, `%LOCALAPPDATA%\nitroid\instances`

use std::path::PathBuf;

/// Return the data directory (configuration, registry). Created on first call.
pub fn data_dir() -> PathBuf {
    let dir = directories::ProjectDirs::from("", crate::ORG_NAME, crate::APP_NAME)
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".nitroid"));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Return the cache directory (downloaded images). Created on first call.
pub fn cache_dir() -> PathBuf {
    let dir = directories::ProjectDirs::from("", crate::ORG_NAME, crate::APP_NAME)
        .map(|d| d.cache_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".nitroid/cache"));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Return the instances directory. Created on first call.
pub fn instances_dir() -> PathBuf {
    let dir = data_dir().join("instances");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Return the directory used to store per-instance snapshots.
pub fn snapshots_dir() -> PathBuf {
    let dir = data_dir().join("snapshots");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Return the path to the global config file.
pub fn config_file() -> PathBuf {
    data_dir().join("config.toml")
}

/// Return the path to the system image registry file.
pub fn image_registry_file() -> PathBuf {
    data_dir().join("images.json")
}

/// Return the path to the instance registry file.
pub fn instance_registry_file() -> PathBuf {
    data_dir().join("instances.json")
}

/// Return the path to the global keymapping file.
pub fn keymap_file() -> PathBuf {
    data_dir().join("keymap.json")
}
