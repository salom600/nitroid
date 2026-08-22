//! The top-level egui app — wires together the instance manager, keymap
//! store, and the side navigation between panels.

use std::sync::Arc;

use eframe::egui;
use parking_lot::RwLock;

use nitroid_core::{EmulatorConfig, InstanceConfig, InstanceState};
use nitroid_instances::InstanceManager;
use nitroid_input::{builtin_profiles, Keymap};

use crate::panels::{
    ImagesPanel, InstancesPanel, SettingsPanel, SetupPanel,
};
use crate::theme::Theme;

/// Which panel is currently visible in the main content area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Setup,
    Instances,
    Images,
    Settings,
}

pub struct NitroidApp {
    pub theme: Theme,
    pub config: Arc<RwLock<EmulatorConfig>>,
    pub manager: Arc<InstanceManager>,
    pub active_keymap: Arc<RwLock<Keymap>>,
    pub current_panel: Panel,
    pub show_perf_overlay: bool,
    pub selected_instance: Option<String>,
    /// True if the user has not yet registered any system image — drives the
    /// "first-run setup" panel.
    pub needs_setup: bool,
    pub backend_info: Option<String>,
}

impl NitroidApp {
    pub fn new(
        config: EmulatorConfig,
        manager: InstanceManager,
        backend_info: Option<String>,
    ) -> Self {
        let manager = Arc::new(manager);
        let needs_setup = manager.list_images().is_empty();

        // Pick a default keymap from the built-in profiles — PUBG Mobile is
        // a sane default since most users install Nitroid to play games like
        // it. The user can switch via the Settings panel.
        let default_keymap = builtin_profiles()
            .into_iter()
            .find(|(id, _, _)| *id == "pubg")
            .map(|(_, _, km)| km)
            .unwrap_or_else(Keymap::default);

        Self {
            theme: Theme::dark(),
            config: Arc::new(RwLock::new(config)),
            manager,
            active_keymap: Arc::new(RwLock::new(default_keymap)),
            current_panel: if needs_setup { Panel::Setup } else { Panel::Instances },
            show_perf_overlay: false,
            selected_instance: None,
            needs_setup,
            backend_info,
        }
    }

    fn render_sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("sidebar")
            .exact_width(220.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.add_space(16.0);
                ui.heading("Nitroid");
                ui.label(
                    egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                        .color(self.theme.text_dim)
                        .small(),
                );
                ui.add_space(self.theme.spacing);

                let buttons = [
                    (Panel::Setup, "⚙  Setup", self.needs_setup),
                    (Panel::Instances, "▢  Instances", !self.needs_setup),
                    (Panel::Images, "💿  System Images", true),
                    (Panel::Settings, "✦  Settings", true),
                ];

                for (panel, label, enabled) in buttons {
                    ui.add_enabled_ui(enabled, |ui| {
                        let active = self.current_panel == panel;
                        let btn = egui::SelectableLabel::new(active, label);
                        if ui.add(btn).clicked() {
                            self.current_panel = panel;
                        }
                    });
                }

                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.add_space(self.theme.spacing);
                    ui.checkbox(&mut self.show_perf_overlay, "Performance overlay");
                    if let Some(info) = &self.backend_info {
                        ui.label(
                            egui::RichText::new(info)
                                .color(self.theme.text_dim)
                                .small(),
                        );
                    }
                });
            });
    }

    fn render_perf_overlay(&self, ctx: &egui::Context) {
        if !self.show_perf_overlay {
            return;
        }
        egui::Window::new("Performance")
            .title_bar(false)
            .anchor(egui::Align2::RIGHT_TOP, [-8.0, 8.0])
            .resizable(false)
            .show(ctx, |ui| {
                let instances = self.manager.list_instances();
                let running = instances
                    .iter()
                    .filter(|(_, s)| *s == InstanceState::Running)
                    .count();
                ui.label(format!("Running instances: {running}"));
                ui.label(format!("Total instances: {}", instances.len()));
                ui.label(format!("Images: {}", self.manager.list_images().len()));
            });
    }
}

impl eframe::App for NitroidApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.theme.install(ctx);

        self.render_sidebar(ctx);

        let clipboard = self.selected_instance.clone();
        let panel = self.current_panel;
        match panel {
            Panel::Setup => SetupPanel::render(self, ctx),
            Panel::Instances => InstancesPanel::render(self, ctx, clipboard.as_deref()),
            Panel::Images => ImagesPanel::render(self, ctx),
            Panel::Settings => SettingsPanel::render(self, ctx),
        }

        self.render_perf_overlay(ctx);
    }
}

/// Helper used by the panels to enumerate instances for display.
pub fn list_instances_for_display(
    manager: &InstanceManager,
) -> Vec<(InstanceConfig, InstanceState)> {
    manager.list_instances()
}
