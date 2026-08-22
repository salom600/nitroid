//! Logging initialisation — uses `tracing` with a console subscriber.
//!
//! Honours the `RUST_LOG` env var if set, otherwise defaults to `info` for
//! the workspace crates and `warn` for everything else.

use tracing_subscriber::{fmt, prelude::*, EnvFilter};

pub fn init() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("nitroid=info,nitroid_core=info,nitroid_input=info,nitroid_instances=info,wgpu=warn,warn")
    });

    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(false).with_ansi(true));

    tracing::subscriber::set_global_default(subscriber)
        .ok();
}
