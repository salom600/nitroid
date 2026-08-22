//! Minimal CLI — `nitroid --list`, `nitroid --cli run`, `nitroid download`, etc.
//!
//! This is intentionally lightweight; the GUI is the primary interface. The
//! CLI exists for headless setups (e.g. running an instance inside a Linux
//! server without a display) and for CI smoke tests.

use nitroid_core::{EmulatorConfig, InstanceState};
use nitroid_downloader::{builtin_catalog, download_default_image};
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
    if args.iter().any(|a| a == "--download") {
        return run_download();
    }
    if args.iter().any(|a| a == "--catalog") {
        return print_catalog();
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
    println!(
        "{:<24} {:<8} {:<12} {:<14} NAME",
        "ID", "STATE", "CPU×MEM", "RESOLUTION"
    );
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

fn run_download() -> anyhow::Result<()> {
    println!("Downloading default Android image (Android-x86 9.0)...");
    let path = download_default_image()?;
    println!("✓ Downloaded to: {}", path.display());
    Ok(())
}

fn print_catalog() -> anyhow::Result<()> {
    let catalog = builtin_catalog();
    println!("{:<28} {:<10} {:<14} SIZE", "NAME", "VERSION", "ARCH");
    println!("{}", "-".repeat(80));
    for c in catalog {
        println!(
            "{:<28} {:<10} {:<14} ~{} MB",
            c.name,
            c.version,
            match c.arch {
                nitroid_core::CpuArch::X86_64 => "x86_64",
                nitroid_core::CpuArch::Aarch64 => "aarch64",
                nitroid_core::CpuArch::Armv7 => "armv7",
            },
            c.size_hint / 1_000_000
        );
    }
    Ok(())
}

fn print_help(prog: &str) {
    println!(
        "Nitroid v{} — ultra-light Android emulator",
        env!("CARGO_PKG_VERSION")
    );
    println!();
    println!("USAGE: {prog} [OPTIONS]");
    println!();
    println!("OPTIONS:");
    println!("  (no args)     Launch the GUI control panel");
    println!("  --list        Print registered instances and images");
    println!("  --download    Download the default Android-x86 image");
    println!("  --catalog     Print the downloadable image catalog");
    println!("  --cli         Run in CLI mode (useful for headless servers)");
    println!("  --help, -h    Print this message");
    println!();
    println!("EXAMPLES:");
    println!("  {prog}                  # open the control panel");
    println!("  {prog} --list           # show all instances + images");
    println!("  {prog} --download       # fetch Android-x86 9.0");
    println!("  {prog} --catalog        # list available images");
}
