//! UI panels — each panel is a self-contained function that renders into an
//! egui central panel.

use eframe::egui;
use parking_lot::RwLock;
use std::sync::Arc;

use nitroid_core::{paths, EmulatorConfig, InstanceState, SystemImage};
use nitroid_downloader::{builtin_catalog, DownloadStage};
use nitroid_input::{builtin_profiles, load_keymap, save_keymap, Keymap};

use crate::app::{list_instances_for_display, NitroidApp, Panel};

pub struct SetupPanel;
pub struct InstancesPanel;
pub struct ImagesPanel;
pub struct DownloaderPanel;
pub struct SettingsPanel;

impl SetupPanel {
    pub fn render(app: &mut NitroidApp, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(32.0);
            ui.heading("Welcome to Nitroid");
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(
                    "Nitroid needs an Android system image to run. You can either download \
                     one automatically (recommended) or register an existing ISO.",
                )
                .color(app.theme.text_dim),
            );
            ui.add_space(16.0);

            let images = app.manager.list_images();
            if images.is_empty() {
                ui.label("No images registered yet.");
            } else {
                ui.label(format!("✓ {} image(s) registered", images.len()));
            }

            ui.add_space(16.0);
            ui.horizontal(|ui| {
                if ui.button("⬇ Open Downloader →").clicked() {
                    app.current_panel = Panel::Downloader;
                    app.needs_setup = false;
                }
                if ui.button("💿 Register existing ISO →").clicked() {
                    app.current_panel = Panel::Images;
                    app.needs_setup = false;
                }
            });
        });
    }
}

impl InstancesPanel {
    pub fn render(app: &mut NitroidApp, ctx: &egui::Context, _selected: Option<&str>) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Instances");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("＋ New Instance").clicked() {
                        // Open the create dialog — for now we just navigate to images.
                        app.current_panel = Panel::Images;
                    }
                });
            });
            ui.separator();
            ui.add_space(8.0);

            let instances = list_instances_for_display(&app.manager);
            if instances.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(80.0);
                    ui.label(egui::RichText::new("No instances yet").strong());
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("Click 'New Instance' to create your first one")
                            .color(app.theme.text_dim),
                    );
                });
                return;
            }

            egui::ScrollArea::vertical().show(ui, |ui| {
                for (cfg, state) in instances {
                    let row = egui::Frame::group(ui.style())
                        .fill(app.theme.surface)
                        .stroke(egui::Stroke::new(1.0_f32, app.theme.border))
                        .rounding(app.theme.rounding)
                        .inner_margin(app.theme.padding)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.add(egui::Label::new(
                                        egui::RichText::new(&cfg.name).strong().size(16.0),
                                    ));
                                    ui.add_space(2.0);
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{} × {} MB  •  {}×{}  •  {}",
                                            cfg.cpu_count,
                                            cfg.memory_mb,
                                            cfg.width,
                                            cfg.height,
                                            arch_label(cfg.arch),
                                        ))
                                        .color(app.theme.text_dim)
                                        .small(),
                                    );
                                });

                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        state_button(ui, state, &app.theme);
                                        ui.add_space(8.0);
                                        if ui.button("▶").on_hover_text("Start").clicked() {
                                            app.selected_instance = Some(cfg.id.clone());
                                        }
                                        if ui.button("⏸").on_hover_text("Pause").clicked() {
                                            app.selected_instance = Some(cfg.id.clone());
                                        }
                                        if ui.button("⏹").on_hover_text("Stop").clicked() {
                                            app.selected_instance = Some(cfg.id.clone());
                                        }
                                        if ui.button("⚙").on_hover_text("Settings").clicked() {
                                            app.selected_instance = Some(cfg.id.clone());
                                        }
                                    },
                                );
                            });
                        });
                    ui.add_space(4.0);
                    let _ = row;
                }
            });
        });
    }
}

impl ImagesPanel {
    pub fn render(app: &mut NitroidApp, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("System Images");
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(
                    "Register pre-built Android-x86 / Bliss OS images. \
                     Nitroid verifies integrity on every boot.",
                )
                .color(app.theme.text_dim),
            );
            ui.add_space(8.0);

            let images = app.manager.list_images();
            if images.is_empty() {
                ui.label("No images registered.");
            } else {
                egui::Grid::new("images_grid")
                    .num_columns(4)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label("Name");
                        ui.label("Arch");
                        ui.label("Size");
                        ui.label("Path");
                        ui.end_row();
                        for img in images {
                            ui.label(&img.name);
                            ui.label(arch_label(img.arch));
                            ui.label(format_size(img.size_bytes));
                            ui.label(img.path.to_string_lossy());
                            ui.end_row();
                        }
                    });
            }

            ui.add_space(16.0);
            ui.horizontal(|ui| {
                if ui.button("Register image…").clicked() {
                    if let Some(path) = rfd_pick_image() {
                        match SystemImage::register(&path, nitroid_core::CpuArch::X86_64) {
                            Ok(img) => {
                                let _ = app.manager.register_image(img);
                                if app.needs_setup {
                                    app.needs_setup = false;
                                    app.current_panel = Panel::Instances;
                                }
                            }
                            Err(e) => {
                                tracing::error!("failed to register image: {e}");
                            }
                        }
                    }
                }
                if ui.button("⬇ Open Downloader →").clicked() {
                    app.current_panel = Panel::Downloader;
                }
            });
        });
    }
}

impl DownloaderPanel {
    pub fn render(app: &mut NitroidApp, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Image Downloader");
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(
                    "Download a stable Android-x86 image. The download runs in the \
                     background — you can keep using Nitroid while it completes.",
                )
                .color(app.theme.text_dim),
            );
            ui.add_space(16.0);

            let state = app.downloader.state();
            if state.active {
                ui.add_space(8.0);
                ui.label(format!("Stage: {:?}", state.stage));
                if state.total > 0 {
                    let pct = (state.downloaded as f32 / state.total as f32).clamp(0.0, 1.0);
                    ui.add(
                        egui::ProgressBar::new(pct)
                            .text(format!(
                                "{:.1}% — {}/{}",
                                pct * 100.0,
                                format_size(state.downloaded),
                                format_size(state.total)
                            ))
                            .desired_width(600.0),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!("{} / s", format_size(state.bytes_per_sec)))
                            .color(app.theme.text_dim)
                            .small(),
                    );
                } else {
                    ui.spinner();
                    ui.label(format!("Downloaded: {}", format_size(state.downloaded)));
                }
                ui.add_space(8.0);
                if let Some(candidate) = &state.candidate {
                    ui.label(format!("Image: {}", candidate.name));
                }
            } else if matches!(state.stage, DownloadStage::Done) {
                ui.label(
                    egui::RichText::new("✓ Download complete — image registered")
                        .color(app.theme.success)
                        .strong(),
                );
                ui.add_space(8.0);
                if ui.button("View in Images panel →").clicked() {
                    app.current_panel = Panel::Images;
                }
            } else if matches!(state.stage, DownloadStage::Failed) {
                ui.label(
                    egui::RichText::new(format!(
                        "✗ Download failed: {}",
                        state.error.as_deref().unwrap_or("unknown error")
                    ))
                    .color(app.theme.danger)
                    .strong(),
                );
            } else {
                ui.label("Select an image to download:");
                ui.add_space(8.0);
                let catalog = builtin_catalog();
                for candidate in catalog {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.add(egui::Label::new(
                                egui::RichText::new(&candidate.name).strong(),
                            ));
                            ui.label(
                                egui::RichText::new(format!(
                                    "v{} · {} · {}",
                                    candidate.version,
                                    arch_label(candidate.arch),
                                    format_size(candidate.size_hint)
                                ))
                                .color(app.theme.text_dim)
                                .small(),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Download").clicked() {
                                if let Err(e) = app.downloader.start(candidate.clone()) {
                                    tracing::error!("failed to start download: {e}");
                                }
                            }
                        });
                    });
                    ui.separator();
                }
            }
        });
    }
}

impl SettingsPanel {
    pub fn render(app: &mut NitroidApp, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Settings");
            ui.add_space(8.0);

            let mut cfg = app.config.write().clone();

            ui.collapsing("Backend", |ui| {
                ui.horizontal(|ui| {
                    ui.label("Acceleration:");
                    let opts = ["Auto", "KVM", "WHPX", "TCG"];
                    let cur = match cfg.accel {
                        nitroid_core::AccelBackend::Auto => 0,
                        nitroid_core::AccelBackend::Kvm => 1,
                        nitroid_core::AccelBackend::Whpx => 2,
                        nitroid_core::AccelBackend::Tcg => 3,
                    };
                    egui::ComboBox::from_id_source("accel_combo")
                        .selected_text(opts[cur])
                        .show_ui(ui, |ui| {
                            for (i, label) in opts.iter().enumerate() {
                                ui.selectable_value(
                                    &mut cfg.accel,
                                    match i {
                                        0 => nitroid_core::AccelBackend::Auto,
                                        1 => nitroid_core::AccelBackend::Kvm,
                                        2 => nitroid_core::AccelBackend::Whpx,
                                        _ => nitroid_core::AccelBackend::Tcg,
                                    },
                                    *label,
                                );
                            }
                        });
                });

                ui.horizontal(|ui| {
                    ui.label("Graphics API:");
                    let opts = ["Auto", "Vulkan", "DX12", "Metal", "OpenGL"];
                    let cur = match cfg.graphics {
                        nitroid_core::GraphicsBackend::Auto => 0,
                        nitroid_core::GraphicsBackend::Vulkan => 1,
                        nitroid_core::GraphicsBackend::Dx12 => 2,
                        nitroid_core::GraphicsBackend::Metal => 3,
                        nitroid_core::GraphicsBackend::OpenGl => 4,
                    };
                    egui::ComboBox::from_id_source("gfx_combo")
                        .selected_text(opts[cur])
                        .show_ui(ui, |ui| {
                            for (i, label) in opts.iter().enumerate() {
                                ui.selectable_value(
                                    &mut cfg.graphics,
                                    match i {
                                        0 => nitroid_core::GraphicsBackend::Auto,
                                        1 => nitroid_core::GraphicsBackend::Vulkan,
                                        2 => nitroid_core::GraphicsBackend::Dx12,
                                        3 => nitroid_core::GraphicsBackend::Metal,
                                        _ => nitroid_core::GraphicsBackend::OpenGl,
                                    },
                                    *label,
                                );
                            }
                        });
                });
            });

            ui.collapsing("Defaults", |ui| {
                ui.horizontal(|ui| {
                    ui.label("Memory (MB):");
                    ui.add(
                        egui::Slider::new(&mut cfg.default_memory_mb, 1024..=32768).step_by(256.0),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("CPU count:");
                    ui.add(egui::Slider::new(&mut cfg.default_cpu_count, 1..=16));
                });
                ui.horizontal(|ui| {
                    ui.label("Refresh rate (Hz):");
                    ui.add(
                        egui::Slider::new(&mut cfg.default_refresh_rate, 30..=240).step_by(15.0),
                    );
                });
            });

            ui.collapsing("Keymap", |ui| {
                let current = app.active_keymap.read().name.clone();
                ui.label(format!("Active: {current}"));
                ui.add_space(8.0);
                ui.label("Built-in profiles:");
                for (id, name, _) in builtin_profiles() {
                    if ui.button(name).clicked() {
                        if let Some(km) = nitroid_input::profile_by_id(id) {
                            *app.active_keymap.write() = km;
                        }
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Export active keymap…").clicked() {
                        let path = paths::keymap_file();
                        if let Err(e) = save_keymap(&app.active_keymap.read(), &path) {
                            tracing::error!("failed to save keymap: {e}");
                        }
                    }
                    if ui.button("Import keymap…").clicked() {
                        let path = paths::keymap_file();
                        if let Ok(km) = load_keymap(&path) {
                            *app.active_keymap.write() = km;
                        }
                    }
                });
            });

            ui.collapsing("Telemetry", |ui| {
                ui.checkbox(&mut cfg.telemetry, "Send anonymous usage stats");
            });

            ui.add_space(16.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    let path = paths::config_file();
                    if let Err(e) = cfg.save(&path) {
                        tracing::error!("failed to save config: {e}");
                    } else {
                        *app.config.write() = cfg.clone();
                    }
                }
                if ui.button("Reset to defaults").clicked() {
                    cfg = EmulatorConfig::default();
                }
            });

            let _ = cfg;
        });
    }
}

fn state_button(ui: &mut egui::Ui, state: InstanceState, theme: &crate::theme::Theme) {
    let (label, color) = match state {
        InstanceState::Stopped => ("● stopped", theme.text_dim),
        InstanceState::Booting => ("● booting", theme.warning),
        InstanceState::Running => ("● running", theme.success),
        InstanceState::Paused => ("● paused", theme.text_dim),
        InstanceState::Crashed => ("● crashed", theme.danger),
        InstanceState::Saving => ("● saving", theme.warning),
    };
    ui.label(egui::RichText::new(label).color(color).strong());
}

fn arch_label(arch: nitroid_core::CpuArch) -> &'static str {
    match arch {
        nitroid_core::CpuArch::X86_64 => "x86_64",
        nitroid_core::CpuArch::Aarch64 => "arm64",
        nitroid_core::CpuArch::Armv7 => "armv7",
    }
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let kb = bytes as f64 / 1024.0;
    if kb < 1024.0 {
        return format!("{:.0} KB", kb);
    }
    let mb = kb / 1024.0;
    if mb < 1024.0 {
        return format!("{:.0} MB", mb);
    }
    format!("{:.2} GB", mb / 1024.0)
}

/// Stand-in for a real file picker. `rfd` would normally be used here; we
/// keep it as a stub so the workspace compiles on CI without an X server.
fn rfd_pick_image() -> Option<std::path::PathBuf> {
    // In a real build, this calls `rfd::FileDialog::new().pick_file()`.
    // For the scaffold we return the first .img in the cache dir, if any.
    let cache = paths::cache_dir();
    if let Ok(entries) = std::fs::read_dir(&cache) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) == Some("img") {
                return Some(p);
            }
        }
    }
    None
}

/// Compile-time check that the `Arc<RwLock<Keymap>>` is usable from the UI
/// thread. This is here to keep the import alive when no panel uses it.
#[allow(dead_code)]
fn _ensure_arc_keymap_compiles() -> Arc<RwLock<Keymap>> {
    Arc::new(RwLock::new(Keymap::default()))
}
