//! Modern, lightweight control panel built on egui.
//!
//! The UI is intentionally minimal — Nitroid prioritises resource efficiency
//! over visual chrome. Everything renders through egui's `glow` backend so
//! we get a single, self-contained binary that doesn't pull in a system webview.
//!
//! ## Screens
//!
//! - **Library** — list of instances, "create" / "clone" / "delete" buttons.
//! - **Instance settings** — CPU/memory/resolution/keymap editor.
//! - **Image manager** — register / verify / delete Android system images.
//! - **Global settings** — backend, graphics API, telemetry.
//! - **Performance overlay** — toggled at runtime, shows CPU/GPU/RAM usage.

pub mod app;
pub mod panels;
pub mod theme;

pub use app::NitroidApp;
pub use theme::Theme;
