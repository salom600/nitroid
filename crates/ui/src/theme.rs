//! Theme — the colour palette and typography used across the control panel.
//!
//! The default look is "dark editorial" — near-black background, single
//! accent colour (a vivid cyan that reads well on both OLED and IPS displays),
//! and a body font that emphasises legibility over decoration.

use egui::{Color32, Rounding, Vec2};

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub bg: Color32,
    pub surface: Color32,
    pub surface_alt: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub accent: Color32,
    pub accent_dim: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub danger: Color32,
    pub border: Color32,
    pub rounding: Rounding,
    pub padding: Vec2,
    pub spacing: f32,
}

impl Theme {
    pub const fn dark() -> Self {
        Theme {
            bg: Color32::from_rgb(0x10, 0x12, 0x16),
            surface: Color32::from_rgb(0x18, 0x1B, 0x22),
            surface_alt: Color32::from_rgb(0x22, 0x26, 0x2E),
            text: Color32::from_rgb(0xE6, 0xE8, 0xEC),
            text_dim: Color32::from_rgb(0x8A, 0x90, 0x9C),
            accent: Color32::from_rgb(0x38, 0xD9, 0xFF),
            accent_dim: Color32::from_rgb(0x1E, 0x6E, 0x82),
            success: Color32::from_rgb(0x4E, 0xC9, 0xB0),
            warning: Color32::from_rgb(0xFF, 0xB8, 0x66),
            danger: Color32::from_rgb(0xF4, 0x6B, 0x7C),
            border: Color32::from_rgb(0x2C, 0x30, 0x38),
            rounding: Rounding::same(6.0),
            padding: Vec2::new(16.0, 10.0),
            spacing: 12.0,
        }
    }

    pub fn light() -> Self {
        let dark = Self::dark();
        Theme {
            bg: Color32::from_rgb(0xFA, 0xFB, 0xFC),
            surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            surface_alt: Color32::from_rgb(0xF0, 0xF2, 0xF5),
            text: Color32::from_rgb(0x1A, 0x1C, 0x22),
            text_dim: Color32::from_rgb(0x55, 0x5C, 0x68),
            border: Color32::from_rgb(0xE0, 0xE4, 0xEA),
            ..dark
        }
    }

    /// Apply the theme to an egui context. Call once at startup (and again
    /// if the theme changes at runtime).
    pub fn install(self, ctx: &egui::Context) {
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = self.bg;
        visuals.window_fill = self.surface;
        visuals.extreme_bg_color = self.bg;
        visuals.faint_bg_color = self.surface_alt;
        visuals.window_rounding = self.rounding;
        ctx.set_visuals(visuals);
    }
}
