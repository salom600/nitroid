//! Keymapping engine — translates host input (keyboard, mouse, gamepad) into
//! the Android guest's input coordinates and injects them via virtio-input.
//!
//! ## Design
//!
//! The engine is **stateless across frames**: each host input event is
//! processed independently through the active [`Keymap`], producing zero or
//! more [`OutputAction`]s. This makes the engine trivially testable and
//! keeps latency predictable — there are no per-frame accumulators.
//!
//! ## Latency
//!
//! Target end-to-end input latency (host event → guest touch handler) is
//! ≤4 ms on a 60 Hz guest. The engine itself adds <100 µs; the rest of the
//! budget is consumed by the virtio-input dispatch and the guest kernel's
//! input subsystem.

pub mod keymap;
pub mod profiles;
pub mod translator;

pub use keymap::{KeyAction, Keymap, MouseButton as HostMouseButton, MouseRegion, ScanCode, TouchTarget};
pub use profiles::{builtin_profiles, profile_by_id, ProfileId};
pub use translator::{InputTranslator, OutputAction};

use nitroid_core::Result;

/// Load a keymap from a JSON file. Returns the default keymap if the file
/// doesn't exist.
pub fn load_keymap(path: &std::path::Path) -> Result<Keymap> {
    if path.exists() {
        let raw = std::fs::read_to_string(path)?;
        let map: Keymap = serde_json::from_str(&raw)?;
        Ok(map)
    } else {
        Ok(Keymap::default())
    }
}

/// Persist a keymap to disk as JSON.
pub fn save_keymap(map: &Keymap, path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let s = serde_json::to_string_pretty(map)?;
    std::fs::write(path, s)?;
    Ok(())
}
