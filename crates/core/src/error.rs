//! Error types shared across the entire Nitroid workspace.
//!
//! Every crate converts its private errors into [`CoreError`] at the workspace
//! boundary so the top-level binary has a single, predictable error surface.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, CoreError>;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("I/O error: {source}")]
    Io {
        #[from]
        source: std::io::Error,
    },

    #[error("serialization error: {source}")]
    Serde {
        #[from]
        source: serde_json::Error,
    },

    #[error("toml parse error: {source}")]
    Toml {
        #[from]
        source: toml::de::Error,
    },

    #[error("virtualization backend unavailable: {0}")]
    VirtualizationUnavailable(String),

    #[error("graphics backend error: {0}")]
    Graphics(String),

    #[error("instance not found: {0}")]
    InstanceNotFound(String),

    #[error("instance already exists: {0}")]
    InstanceExists(String),

    #[error("system image not found: {0}")]
    ImageNotFound(String),

    #[error("invalid system image: {0}")]
    InvalidImage(String),

    #[error("input mapping error: {0}")]
    InputMapping(String),

    #[error("translation error: {0}")]
    Translation(String),

    #[error("ISO 9660 parse error: {0}")]
    Iso9660(String),

    #[error("backend error: {0}")]
    Backend(String),

    #[error("misc: {0}")]
    Other(String),
}

impl From<anyhow::Error> for CoreError {
    fn from(err: anyhow::Error) -> Self {
        CoreError::Other(err.to_string())
    }
}
