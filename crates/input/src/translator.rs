//! The translator — turns host input events into guest output actions.
//!
//! This is the brain of the input engine. It takes a [`HostEvent`], consults
//! the active [`Keymap`], and produces zero or more [`OutputAction`]s that
//! the virtualization layer injects into the guest via virtio-input.

use parking_lot::Mutex;

use crate::keymap::{KeyAction, Keymap, MouseButton, ScanCode};
use nitroid_core::Result;

/// What the host tells us happened. Time is in microseconds since process
/// start — using `Instant` directly is preferred but events may be replayed
/// from a recording.
#[derive(Debug, Clone, Copy)]
pub enum HostEvent {
    KeyDown {
        code: ScanCode,
        time_us: u64,
    },
    KeyUp {
        code: ScanCode,
        time_us: u64,
    },
    MouseDown {
        button: MouseButton,
        time_us: u64,
    },
    MouseUp {
        button: MouseButton,
        time_us: u64,
    },
    MouseMove {
        x: i32,
        y: i32,
        time_us: u64,
    },
    Wheel {
        delta: i32,
        time_us: u64,
    },
    /// Gamepad axes — `axis_id` matches the Linux gamepad convention.
    GamepadAxis {
        axis_id: u8,
        value: f32,
        time_us: u64,
    },
    /// Gamepad button (using evdev BTN_GAMEPAD/BTN_SOUTH/etc. numbering).
    GamepadButton {
        code: u16,
        pressed: bool,
        time_us: u64,
    },
}

/// What we want to do on the guest. Coordinates are in guest display pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutputAction {
    /// Touch down at (x, y) with `pressure` (0..=255).
    TouchDown {
        slot: u32,
        x: u32,
        y: u32,
        pressure: u16,
    },
    /// Move an existing touch.
    TouchMove { slot: u32, x: u32, y: u32 },
    /// Release a touch.
    TouchUp { slot: u32 },
    /// Synthesise a key press (Linux evdev keycode).
    KeyDown { code: u16 },
    /// Synthesise a key release.
    KeyUp { code: u16 },
}

/// The translator itself. Owns the active keymap and tracks per-slot touch
/// state so we can correctly emit move/up events.
pub struct InputTranslator {
    keymap: Mutex<Keymap>,
    /// Slot tracking: each host binding gets a stable slot ID.
    slots: Mutex<SlotTracker>,
    /// Toggle state — persists between events for `KeyAction::Toggle`.
    toggles: Mutex<std::collections::HashMap<String, bool>>,
}

#[derive(Default)]
struct SlotTracker {
    next_slot: u32,
    /// Map from a stable key (binding identifier) to the slot it was assigned.
    assigned: std::collections::HashMap<String, u32>,
}

impl SlotTracker {
    fn slot_for(&mut self, key: &str) -> u32 {
        if let Some(&s) = self.assigned.get(key) {
            return s;
        }
        let s = self.next_slot;
        self.next_slot += 1;
        self.assigned.insert(key.to_string(), s);
        s
    }
}

impl InputTranslator {
    pub fn new(keymap: Keymap) -> Self {
        Self {
            keymap: Mutex::new(keymap),
            slots: Mutex::new(SlotTracker::default()),
            toggles: Mutex::new(Default::default()),
        }
    }

    /// Replace the active keymap atomically.
    pub fn set_keymap(&self, keymap: Keymap) {
        *self.keymap.lock() = keymap;
        // Don't reset slot assignments — in-flight touches would be orphaned.
    }

    /// Process a single host event. Returns the list of guest actions to
    /// inject.
    pub fn translate(&self, event: HostEvent) -> Result<Vec<OutputAction>> {
        let keymap = self.keymap.lock();
        let mut out = Vec::new();

        match event {
            HostEvent::KeyDown { code, .. } => {
                if let Some(action) = keymap.keys.get(&code) {
                    self.apply_press(action, &format!("k{:?}", code.0), &mut out);
                }
            }
            HostEvent::KeyUp { code, .. } => {
                if let Some(action) = keymap.keys.get(&code) {
                    self.apply_release(action, &format!("k{:?}", code.0), &mut out);
                }
            }
            HostEvent::MouseDown { button, .. } => {
                if let Some(action) = keymap.mouse_buttons.get(&button) {
                    self.apply_press(action, &format!("m{:?}", button), &mut out);
                }
            }
            HostEvent::MouseUp { button, .. } => {
                if let Some(action) = keymap.mouse_buttons.get(&button) {
                    self.apply_release(action, &format!("m{:?}", button), &mut out);
                }
            }
            HostEvent::MouseMove { x, y, .. } => {
                // Find any mouse region the cursor is inside.
                for region in &keymap.mouse_regions {
                    if (x as u32) >= region.x
                        && (x as u32) < region.x + region.width
                        && (y as u32) >= region.y
                        && (y as u32) < region.y + region.height
                    {
                        let dx = (x - region.x as i32) as f32 / region.width as f32;
                        let dy = (y - region.y as i32) as f32 / region.height as f32;
                        let jx = region.joystick_origin.x as f32 + dx * region.sensitivity;
                        let jy = region.joystick_origin.y as f32 + dy * region.sensitivity;
                        let slot = self.slots.lock().slot_for(&format!("r{}", region.x));
                        out.push(OutputAction::TouchMove {
                            slot,
                            x: jx.round() as u32,
                            y: jy.round() as u32,
                        });
                    }
                }
            }
            HostEvent::Wheel { .. } => {
                // Scroll events are not directly mapped — surface them as
                // vertical drag gestures in a future iteration.
            }
            HostEvent::GamepadAxis { .. } | HostEvent::GamepadButton { .. } => {
                // Gamepad support is handled by a separate translator that
                // knows the canonical Android gamepad layout.
            }
        }

        Ok(out)
    }

    fn apply_press(&self, action: &KeyAction, binding_key: &str, out: &mut Vec<OutputAction>) {
        let slot = self.slots.lock().slot_for(binding_key);
        match action {
            KeyAction::Tap { target } | KeyAction::Hold { target } => {
                out.push(OutputAction::TouchDown {
                    slot,
                    x: target.x,
                    y: target.y,
                    pressure: 255,
                });
            }
            KeyAction::Toggle { target, .. } => {
                let mut tg = self.toggles.lock();
                let key = format!("{binding_key}:toggle");
                let state = tg.entry(key).or_insert(false);
                *state = !*state;
                if *state {
                    out.push(OutputAction::TouchDown {
                        slot,
                        x: target.x,
                        y: target.y,
                        pressure: 255,
                    });
                } else {
                    out.push(OutputAction::TouchUp { slot });
                }
            }
            KeyAction::Swipe {
                from,
                to,
                duration_ms,
            } => {
                // Simplified swipe: touch down, then a sequence of moves to
                // approximate the gesture over `duration_ms`.
                out.push(OutputAction::TouchDown {
                    slot,
                    x: from.x,
                    y: from.y,
                    pressure: 255,
                });
                let steps = (*duration_ms as usize / 16).max(1);
                for i in 1..=steps {
                    let t = i as f32 / steps as f32;
                    let x = (from.x as f32 + (to.x as f32 - from.x as f32) * t).round() as u32;
                    let y = (from.y as f32 + (to.y as f32 - from.y as f32) * t).round() as u32;
                    out.push(OutputAction::TouchMove { slot, x, y });
                }
                out.push(OutputAction::TouchUp { slot });
            }
            KeyAction::Macro { steps } => {
                // Macros are emitted as a sequence of immediate taps — the
                // delays between them are handled by the caller (e.g. the
                // event-loop scheduler).
                for (i, step) in steps.iter().enumerate() {
                    let macro_slot = slot + 1 + i as u32;
                    out.push(OutputAction::TouchDown {
                        slot: macro_slot,
                        x: step.target.x,
                        y: step.target.y,
                        pressure: 255,
                    });
                    out.push(OutputAction::TouchUp { slot: macro_slot });
                }
            }
        }
    }

    fn apply_release(&self, action: &KeyAction, binding_key: &str, out: &mut Vec<OutputAction>) {
        let slot = self.slots.lock().slot_for(binding_key);
        match action {
            KeyAction::Tap { .. } | KeyAction::Hold { .. } => {
                out.push(OutputAction::TouchUp { slot });
            }
            // Toggle and Swipe handle their own release state on press.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::{scancodes, TouchTarget};

    fn make_keymap() -> Keymap {
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
        km.bind_key(
            scancodes::KEY_W,
            KeyAction::Hold {
                target: TouchTarget {
                    x: 100,
                    y: 100,
                    radius: None,
                },
            },
        );
        km
    }

    #[test]
    fn key_down_produces_touch_down() {
        let t = InputTranslator::new(make_keymap());
        let out = t
            .translate(HostEvent::KeyDown {
                code: scancodes::KEY_SPACE,
                time_us: 0,
            })
            .unwrap();
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0],
            OutputAction::TouchDown { x: 640, y: 360, .. }
        ));
    }

    #[test]
    fn key_up_produces_touch_up() {
        let t = InputTranslator::new(make_keymap());
        t.translate(HostEvent::KeyDown {
            code: scancodes::KEY_SPACE,
            time_us: 0,
        })
        .unwrap();
        let out = t
            .translate(HostEvent::KeyUp {
                code: scancodes::KEY_SPACE,
                time_us: 1,
            })
            .unwrap();
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], OutputAction::TouchUp { .. }));
    }

    #[test]
    fn toggle_persists_state() {
        let mut km = Keymap::for_resolution("test", 1280, 720);
        km.bind_key(
            scancodes::KEY_1,
            KeyAction::Toggle {
                target: TouchTarget {
                    x: 10,
                    y: 10,
                    radius: None,
                },
                held: false,
            },
        );
        let t = InputTranslator::new(km);
        let out1 = t
            .translate(HostEvent::KeyDown {
                code: scancodes::KEY_1,
                time_us: 0,
            })
            .unwrap();
        assert!(matches!(out1[0], OutputAction::TouchDown { .. }));
        let out2 = t
            .translate(HostEvent::KeyDown {
                code: scancodes::KEY_1,
                time_us: 1,
            })
            .unwrap();
        assert!(matches!(out2[0], OutputAction::TouchUp { .. }));
    }

    #[test]
    fn swipe_produces_move_sequence() {
        let mut km = Keymap::for_resolution("test", 1280, 720);
        km.bind_key(
            scancodes::KEY_2,
            KeyAction::Swipe {
                from: TouchTarget {
                    x: 0,
                    y: 0,
                    radius: None,
                },
                to: TouchTarget {
                    x: 100,
                    y: 0,
                    radius: None,
                },
                duration_ms: 64,
            },
        );
        let t = InputTranslator::new(km);
        let out = t
            .translate(HostEvent::KeyDown {
                code: scancodes::KEY_2,
                time_us: 0,
            })
            .unwrap();
        // TouchDown + 4 moves + TouchUp
        assert!(out.len() >= 3);
        assert!(matches!(out[0], OutputAction::TouchDown { .. }));
        assert!(matches!(*out.last().unwrap(), OutputAction::TouchUp { .. }));
    }
}
