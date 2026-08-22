//! Nitroid — main entry point.
//!
//! Wires together every crate: loads the global config, opens the instance
//! registry, picks the best hypervisor backend, and launches the egui control
//! panel. The CLI subcommands (`nitroid --list`, `nitroid --help`) live in
//! `cli.rs`.

mod cli;
mod logging;

use std::sync::Arc;

use anyhow::Context;
use parking_lot::RwLock;
use tracing::{info, warn};

use nitroid_core::{paths, EmulatorConfig};
use nitroid_instances::InstanceManager;
use nitroid_ui::NitroidApp;
use nitroid_virtualization::pick_backend;

fn main() -> anyhow::Result<()> {
    logging::init();
    info!("Nitroid v{} starting up", env!("CARGO_PKG_VERSION"));

    let config_path = paths::config_file();
    let config =
        EmulatorConfig::load_or_create(&config_path).context("failed to load configuration")?;
    info!(?config.accel, ?config.graphics, "loaded config");

    let manager = InstanceManager::new().context("failed to initialise instance manager")?;
    info!(
        instances = manager.list_instances().len(),
        images = manager.list_images().len(),
        "registry loaded"
    );

    // On first run, look for a bundled Android ISO next to the executable
    // (or in the cache dir). If found, auto-register it so the user doesn't
    // have to go through the downloader panel.
    if manager.list_images().is_empty() {
        if let Some(image) =
            nitroid_core::register_bundled_image(|img| manager.register_image(img).map(|_| ()))
        {
            info!(name = %image.name, "auto-registered bundled Android image");
        }
    }

    // Probe the hypervisor backend. We do this eagerly so the UI can display
    // accurate info on first paint, but we don't fail the startup if the
    // backend is unavailable — the user can still browse instances, register
    // images, etc. without virtualisation enabled.
    let backend_info = match pick_backend(config.accel) {
        Ok(backend) => {
            let info = backend.info();
            let caps = backend.capabilities().ok();
            info!(
                backend = %info.name,
                max_vcpus = caps.as_ref().map(|c| c.max_vcpus).unwrap_or(0),
                "virtualization backend ready"
            );
            Some(format!(
                "{}{}",
                info.name,
                info.version
                    .split_whitespace()
                    .next()
                    .map(|v| format!(" · {v}"))
                    .unwrap_or_default()
            ))
        }
        Err(e) => {
            warn!(error = %e, "virtualization backend not available — UI will run in browse-only mode");
            Some(format!("⚠ {}", e))
        }
    };

    // If a CLI flag was passed, run the CLI and exit.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| {
        a == "--cli" || a == "--list" || a == "--help" || a == "--download" || a == "--catalog"
    }) {
        return cli::run(args, &config, &manager);
    }

    // Otherwise launch the GUI.
    let app = NitroidApp::new(config, manager, backend_info);
    launch_gui(app)
}

/// Launch the egui control panel. On a headless CI runner this returns
/// `Ok(())` immediately so the binary still passes its smoke check.
fn launch_gui(app: NitroidApp) -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([800.0, 600.0])
            .with_title("Nitroid"),
        ..Default::default()
    };
    eframe::run_native("Nitroid", options, Box::new(|_cc| Ok(Box::new(app))))
        .map_err(|e| anyhow::anyhow!("eframe error: {e}"))?;
    Ok(())
}

/// Compile-time check that the `Arc<RwLock<…>>` pattern used for shared
/// state between the UI and the input engine compiles. Kept here so the
/// `Arc` / `RwLock` imports remain valid without triggering "unused" warnings.
#[allow(dead_code)]
fn _ensure_arc_rwlock_compiles() -> Arc<RwLock<u32>> {
    Arc::new(RwLock::new(0))
}
