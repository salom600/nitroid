//! Keymap data model — what each host input should map to on the guest.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// A scan code as reported by the host OS. We use the **Linux evdev**
/// keycode space (https://www.kernel.org/doc/html/latest/input/event-codes.html)
/// as the canonical representation — the UI translates from the host's native
/// space (Windows virtual keys, Wayland keysyms) before looking up the
/// keymap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScanCode(pub u16);

/// Host mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Side,
    Extra,
}

/// A rectangle on the guest display that an input should target. Coordinates
/// are in guest display pixels (0..width, 0..height).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TouchTarget {
    pub x: u32,
    pub y: u32,
    /// Optional radius for pressure-sensitive gestures.
    pub radius: Option<u16>,
}

/// A region of the host screen that, when the mouse is moved inside it,
/// controls a virtual joystick anchored at `joystick_origin`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MouseRegion {
    /// Top-left corner (host display pixels).
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    /// Where the virtual joystick is anchored (guest display pixels).
    pub joystick_origin: TouchTarget,
    /// Maximum pixel distance from origin that maps to full joystick deflection.
    pub sensitivity: f32,
}

/// What to do when a key is pressed or released.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum KeyAction {
    /// Tap a single touch target on press, release on release.
    Tap { target: TouchTarget },
    /// Hold the target down while the key is held.
    Hold { target: TouchTarget },
    /// Toggle between pressed/released on each keypress.
    Toggle { target: TouchTarget, held: bool },
    /// On press, simulate a swipe from `from` to `to` over `duration_ms`.
    Swipe {
        from: TouchTarget,
        to: TouchTarget,
        duration_ms: u16,
    },
    /// Trigger a macro — a sequence of taps with delays.
    Macro { steps: Vec<MacroStep> },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MacroStep {
    pub target: TouchTarget,
    pub delay_ms: u16,
}

/// A complete keymap — the union of key bindings, mouse bindings, and
/// mouse-region (joystick) definitions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Keymap {
    pub name: String,
    pub description: String,
    pub guest_width: u32,
    pub guest_height: u32,
    /// Keyboard bindings. Key = scan code, value = action.
    pub keys: HashMap<ScanCode, KeyAction>,
    /// Mouse button bindings.
    pub mouse_buttons: HashMap<MouseButton, KeyAction>,
    /// Mouse regions that act as virtual joysticks.
    pub mouse_regions: Vec<MouseRegion>,
    /// Mouse sensitivity as a free-look camera (right-click drag to look).
    pub look_sensitivity: f32,
    /// Whether to capture the host cursor while the emulator has focus.
    pub capture_cursor: bool,
}

impl Keymap {
    /// Create an empty keymap sized for the given guest display.
    pub fn for_resolution(name: impl Into<String>, w: u32, h: u32) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            guest_width: w,
            guest_height: h,
            keys: HashMap::new(),
            mouse_buttons: HashMap::new(),
            mouse_regions: Vec::new(),
            look_sensitivity: 1.0,
            capture_cursor: false,
        }
    }

    /// Bind a keyboard scan code to an action.
    pub fn bind_key(&mut self, sc: ScanCode, action: KeyAction) -> &mut Self {
        self.keys.insert(sc, action);
        self
    }

    /// Bind a mouse button to an action.
    pub fn bind_mouse(&mut self, btn: MouseButton, action: KeyAction) -> &mut Self {
        self.mouse_buttons.insert(btn, action);
        self
    }

    /// Add a mouse region (virtual joystick).
    pub fn add_region(&mut self, region: MouseRegion) -> &mut Self {
        self.mouse_regions.push(region);
        self
    }
}

/// Convenience constants for common Linux evdev scan codes. These are the
/// same codes the Linux kernel reports for physical keys, so they work
/// identically across Wayland and X11. Windows keys are translated to this
/// space by the UI layer.
pub mod scancodes {
    use super::ScanCode;
    pub const KEY_W: ScanCode = ScanCode(0x11);
    pub const KEY_A: ScanCode = ScanCode(0x1E);
    pub const KEY_S: ScanCode = ScanCode(0x1F);
    pub const KEY_D: ScanCode = ScanCode(0x20);
    pub const KEY_SPACE: ScanCode = ScanCode(0x39);
    pub const KEY_LEFTSHIFT: ScanCode = ScanCode(0x2A);
    pub const KEY_LEFTCTRL: ScanCode = ScanCode(0x1D);
    pub const KEY_R: ScanCode = ScanCode(0x13);
    pub const KEY_F: ScanCode = ScanCode(0x21);
    pub const KEY_G: ScanCode = ScanCode(0x22);
    pub const KEY_1: ScanCode = ScanCode(0x02);
    pub const KEY_2: ScanCode = ScanCode(0x03);
    pub const KEY_3: ScanCode = ScanCode(0x04);
    pub const KEY_4: ScanCode = ScanCode(0x05);
    pub const KEY_5: ScanCode = ScanCode(0x06);
    pub const KEY_TAB: ScanCode = ScanCode(0x0F);
    pub const KEY_ENTER: ScanCode = ScanCode(0x1C);
    pub const KEY_ESC: ScanCode = ScanCode(0x01);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_keymap() {
        let km = Keymap::for_resolution("test", 1280, 720);
        assert_eq!(km.guest_width, 1280);
        assert!(km.keys.is_empty());
    }

    #[test]
    fn bind_and_serialize() {
        let mut km = Keymap::for_resolution("test", 1280, 720);
        km.bind_key(
            scancodes::KEY_SPACE,
            KeyAction::Tap {
                target: TouchTarget {
                    x: 640,
                    y: 360,
                    radius: None,
                },
            },
        );
        let s = serde_json::to_string(&km).unwrap();
        let back: Keymap = serde_json::from_str(&s).unwrap();
        assert_eq!(back.keys.len(), 1);
    }
}
