//! Nitroid core — shared types, configuration, and instance definitions.
//!
//! This crate contains the cross-platform building blocks used by every other
//! Nitroid crate: error types, the global configuration model, the Android
//! system image registry, and the in-memory representation of a running
//! (or stopped) emulator instance.

pub mod config;
pub mod error;
pub mod image;
pub mod instance;
pub mod paths;

pub use config::{AccelBackend, CpuArch, EmulatorConfig, GraphicsBackend};
pub use error::{CoreError, Result};
pub use image::SystemImage;
pub use instance::{InstanceConfig, InstanceId, InstanceState};

/// Project name used in directory paths and window titles.
pub const APP_NAME: &str = "Nitroid";
/// Organisation name used in directory paths.
pub const ORG_NAME: &str = "nitroid";
/// Semantic version of the core library.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
