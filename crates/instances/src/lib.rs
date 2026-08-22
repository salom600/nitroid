//! Multi-instance manager.
//!
//! Lets you run several isolated Android environments simultaneously while
//! sharing the read-only system image (the "blueprint"). Only the per-instance
//! overlay disk (writable, diff layer) is duplicated on disk.
//!
//! ## Storage model
//!
//! ```text
//! system-image.img         ← read-only, shared by every instance
//! instances/
//!   <id>.overlay.qcow2     ← per-instance writable diff layer
//!   <id>.meta.json         ← instance config + runtime state
//! ```
//!
//! The overlay is a copy-on-write layer that records only the blocks the
//! instance has modified since boot. A fresh overlay is a few kilobytes; a
//! heavily-used instance might accumulate 200-500 MB after months of play.

use std::path::{Path, PathBuf};

use dashmap::DashMap;
use nitroid_core::{
    InstanceConfig, InstanceId, InstanceState, SystemImage,
};
use nitroid_core::paths;
use nitroid_core::Result;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// The in-memory + on-disk registry of system images and instances.
pub struct InstanceManager {
    /// Registered system images, indexed by their fingerprint.
    images: RwLock<Vec<SystemImage>>,
    /// All known instance configurations (including stopped ones).
    instances: DashMap<InstanceId, ManagedInstance>,
    /// Path to the persisted registry file.
    registry_path: PathBuf,
}

struct ManagedInstance {
    config: InstanceConfig,
    state: parking_lot::Mutex<InstanceState>,
}

impl InstanceManager {
    /// Create a new manager backed by the default Nitroid data directory.
    pub fn new() -> Result<Self> {
        Self::at(paths::instance_registry_file(), paths::image_registry_file())
    }

    /// Create a manager backed by the given registry files. Used by tests.
    pub fn at(instance_registry: PathBuf, image_registry: PathBuf) -> Result<Self> {
        let mgr = Self {
            images: RwLock::new(load_or_default(&image_registry)?),
            instances: DashMap::new(),
            registry_path: instance_registry,
        };
        mgr.load_instances()?;
        Ok(mgr)
    }

    /// Register a system image. Returns its fingerprint.
    pub fn register_image(&self, image: SystemImage) -> Result<String> {
        let mut images = self.images.write();
        // Deduplicate by fingerprint.
        if let Some(existing) = images.iter().find(|i| i.fingerprint == image.fingerprint) {
            info!(fingerprint = %existing.fingerprint, "image already registered");
            return Ok(existing.fingerprint.clone());
        }
        let fp = image.fingerprint.clone();
        images.push(image);
        drop(images);
        self.save_images()?;
        Ok(fp)
    }

    /// List all registered images.
    pub fn list_images(&self) -> Vec<SystemImage> {
        self.images.read().clone()
    }

    /// Create a new instance bound to the given image.
    pub fn create_instance(
        &self,
        name: impl Into<String>,
        image_fingerprint: &str,
    ) -> Result<InstanceId> {
        let image = self
            .images
            .read()
            .iter()
            .find(|i| i.fingerprint == image_fingerprint)
            .cloned()
            .ok_or_else(|| {
                nitroid_core::CoreError::ImageNotFound(
                    format!("no image with fingerprint {image_fingerprint}"),
                )
            })?;

        let config = InstanceConfig::new(name, &image)?;
        let id = config.id.clone();
        self.ensure_overlay(&config.overlay_path)?;
        self.instances.insert(
            id.clone(),
            ManagedInstance {
                config,
                state: parking_lot::Mutex::new(InstanceState::Stopped),
            },
        );
        self.save_instances()?;
        info!(instance_id = %id, "instance created");
        Ok(id)
    }

    /// Clone an existing instance — creates a new instance sharing the same
    /// image but with its own overlay initialised from a snapshot of the
    /// source instance's current overlay.
    pub fn clone_instance(&self, source_id: &str, new_name: impl Into<String>) -> Result<InstanceId> {
        let source_cfg = self
            .get_config(source_id)
            .ok_or_else(|| nitroid_core::CoreError::InstanceNotFound(source_id.into()))?;

        let image = self
            .images
            .read()
            .iter()
            .find(|i| i.fingerprint == source_cfg.image_fingerprint)
            .cloned()
            .ok_or_else(|| {
                nitroid_core::CoreError::ImageNotFound(source_cfg.image_fingerprint.clone())
            })?;

        let mut new_cfg = InstanceConfig::new(new_name, &image)?;
        new_cfg.cloned_from = Some(source_id.into());
        // Copy the source overlay as the starting point.
        if source_cfg.overlay_path.exists() {
            std::fs::copy(&source_cfg.overlay_path, &new_cfg.overlay_path).map_err(|e| {
                nitroid_core::CoreError::Backend(format!(
                    "failed to clone overlay: {e}"
                ))
            })?;
        } else {
            self.ensure_overlay(&new_cfg.overlay_path)?;
        }

        let id = new_cfg.id.clone();
        self.instances.insert(
            id.clone(),
            ManagedInstance {
                config: new_cfg,
                state: parking_lot::Mutex::new(InstanceState::Stopped),
            },
        );
        self.save_instances()?;
        info!(source = %source_id, cloned_to = %id, "instance cloned");
        Ok(id)
    }

    /// Delete an instance. The shared image is preserved.
    pub fn delete_instance(&self, id: &str) -> Result<()> {
        let entry = self
            .instances
            .remove(id)
            .ok_or_else(|| nitroid_core::CoreError::InstanceNotFound(id.into()))?;
        // Best-effort overlay deletion.
        if entry.1.config.overlay_path.exists() {
            if let Err(e) = std::fs::remove_file(&entry.1.config.overlay_path) {
                warn!(error = %e, "failed to remove overlay");
            }
        }
        self.save_instances()?;
        info!(instance_id = %id, "instance deleted");
        Ok(())
    }

    /// List all known instances with their current state.
    pub fn list_instances(&self) -> Vec<(InstanceConfig, InstanceState)> {
        self.instances
            .iter()
            .map(|e| (e.config.clone(), *e.state.lock()))
            .collect()
    }

    /// Get the config for a specific instance.
    pub fn get_config(&self, id: &str) -> Option<InstanceConfig> {
        self.instances.get(id).map(|e| e.config.clone())
    }

    /// Get the current state of an instance.
    pub fn get_state(&self, id: &str) -> Option<InstanceState> {
        self.instances.get(id).map(|e| *e.state.lock())
    }

    /// Update the state of an instance (called by the runner when the VM
    /// transitions between lifecycle phases).
    pub fn set_state(&self, id: &str, state: InstanceState) {
        if let Some(entry) = self.instances.get(id) {
            *entry.state.lock() = state;
        }
    }

    /// Persist the instance registry to disk.
    pub fn save_instances(&self) -> Result<()> {
        let list: Vec<InstanceConfig> = self
            .instances
            .iter()
            .map(|e| e.config.clone())
            .collect();
        let raw = serde_json::to_string_pretty(&list)?;
        if let Some(parent) = self.registry_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&self.registry_path, raw)?;
        Ok(())
    }

    /// Persist the image registry to disk.
    fn save_images(&self) -> Result<()> {
        let images = self.images.read();
        let raw = serde_json::to_string_pretty(&*images)?;
        let path = paths::image_registry_file();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&path, raw)?;
        Ok(())
    }

    /// Load the instance registry from disk into memory.
    fn load_instances(&self) -> Result<()> {
        if !self.registry_path.exists() {
            return Ok(());
        }
        let raw = std::fs::read_to_string(&self.registry_path)?;
        let list: Vec<InstanceConfig> = serde_json::from_str(&raw)?;
        for cfg in list {
            self.instances.insert(
                cfg.id.clone(),
                ManagedInstance {
                    config: cfg,
                    state: parking_lot::Mutex::new(InstanceState::Stopped),
                },
            );
        }
        Ok(())
    }

    /// Create an empty overlay file if one doesn't exist. The file is just a
    /// sparse zero-byte placeholder until a real qcow2 layer is written.
    fn ensure_overlay(&self, path: &Path) -> Result<()> {
        if !path.exists() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(path, &[])?;
        }
        Ok(())
    }
}

impl Default for InstanceManager {
    fn default() -> Self {
        Self::new().expect("failed to initialise default instance manager")
    }
}

fn load_or_default<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> Result<T> {
    if !path.exists() {
        return Ok(T::default());
    }
    let raw = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

/// Wrapper used by the JSON registry on disk. Not strictly necessary — the
/// registry stores a top-level `Vec<InstanceConfig>` — but kept for forward
/// compatibility (e.g. adding a schema version later).
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct InstanceRegistry {
    schema: u32,
    instances: Vec<InstanceConfig>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn create_list_delete_instance() {
        let dir = tempdir().unwrap();
        let reg = dir.path().join("instances.json");
        let img_reg = dir.path().join("images.json");
        let mgr = InstanceManager::at(reg, img_reg).unwrap();

        // Create a fake system image.
        let img = SystemImage {
            name: "Test".into(),
            path: dir.path().join("test.img"),
            arch: nitroid_core::CpuArch::X86_64,
            fingerprint: "abc123".into(),
            size_bytes: 1_000_000_000,
            version: None,
            vendor: None,
        };
        let fp = mgr.register_image(img).unwrap();
        let id = mgr.create_instance("test-instance", &fp).unwrap();
        assert_eq!(mgr.list_instances().len(), 1);

        let cfg = mgr.get_config(&id).unwrap();
        assert_eq!(cfg.name, "test-instance");

        mgr.delete_instance(&id).unwrap();
        assert_eq!(mgr.list_instances().len(), 0);
    }

    #[test]
    fn persistence_round_trip() {
        let dir = tempdir().unwrap();
        let reg = dir.path().join("instances.json");
        let img_reg = dir.path().join("images.json");

        {
            let mgr = InstanceManager::at(reg.clone(), img_reg.clone()).unwrap();
            let img = SystemImage {
                name: "Test".into(),
                path: dir.path().join("test.img"),
                arch: nitroid_core::CpuArch::X86_64,
                fingerprint: "abc123".into(),
                size_bytes: 1_000_000_000,
                version: None,
                vendor: None,
            };
            let fp = mgr.register_image(img).unwrap();
            let _ = mgr.create_instance("persisted", &fp).unwrap();
        }

        // Reload — the instance should still be there.
        let mgr2 = InstanceManager::at(reg, img_reg).unwrap();
        assert_eq!(mgr2.list_instances().len(), 1);
    }
}
