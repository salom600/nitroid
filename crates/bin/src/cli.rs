//! Minimal CLI — `nitroid --list`, `nitroid --cli run`, etc.
//!
//! This is intentionally lightweight; the GUI is the primary interface. The
//! CLI exists for headless setups (e.g. running an instance inside a Linux
//! server without a display) and for CI smoke tests.

use std::sync::Arc;

use nitroid_core::{EmulatorConfig, InstanceState};
use nitroid_instances::InstanceManager;

pub fn run(
    args: Vec<String>,
    _config: &EmulatorConfig,
    manager: &InstanceManager,
) -> anyhow::Result<()> {
    let prog = args.first().map(|s| s.as_str()).unwrap_or("nitroid");
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help(prog);
        return Ok(());
    }
    if args.iter().any(|a| a == "--list") {
        print_instances(manager);
        return Ok(());
    }
    print_help(prog);
    Ok(())
}

fn print_instances(manager: &InstanceManager) {
    let instances = manager.list_instances();
    if instances.is_empty() {
        println!("No instances.");
        return;
    }
    println!("{:<24} {:<8} {:<12} {:<14} {}", "ID", "STATE", "CPU×MEM", "RESOLUTION", "NAME");
    println!("{}", "-".repeat(80));
    for (cfg, state) in instances {
        println!(
            "{:<24} {:<8} {:<12} {:<14} {}",
            cfg.id,
            state_label(state),
            format!("{}×{}MB", cfg.cpu_count, cfg.memory_mb),
            format!("{}×{}", cfg.width, cfg.height),
            cfg.name,
        );
    }
    let images = manager.list_images();
    println!();
    println!("Images: {}", images.len());
    for img in images {
        println!("  - {} ({})", img.name, img.fingerprint);
    }
}

fn state_label(s: InstanceState) -> &'static str {
    match s {
        InstanceState::Stopped => "stopped",
        InstanceState::Booting => "booting",
        InstanceState::Running => "running",
        InstanceState::Paused => "paused",
        InstanceState::Crashed => "crashed",
        InstanceState::Saving => "saving",
    }
}

fn print_help(prog: &str) {
    println!("Nitroid — ultra-light Android emulator");
    println!();
    println!("USAGE: {prog} [OPTIONS]");
    println!();
    println!("OPTIONS:");
    println!("  (no args)     Launch the GUI control panel");
    println!("  --list        Print registered instances and images");
    println!("  --cli         Run in CLI mode (useful for headless servers)");
    println!("  --help, -h    Print this message");
    println!();
    println!("EXAMPLES:");
    println!("  {prog}                  # open the control panel");
    println!("  {prog} --list           # show all instances + images");
}

#[allow(dead_code)]
fn _ensure_arc_compiles() -> Arc<()> {
    Arc::new(())
}
