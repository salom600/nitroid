//! Built-in keymap profiles for popular Android games.
//!
//! These are *starter* profiles — every player will tune them to their own
//! preferences, but having a sensible default per game means new users get
//! a playable experience within minutes.

use crate::keymap::{KeyAction, Keymap, MouseRegion, MouseButton, TouchTarget, scancodes};

pub type ProfileId = &'static str;

/// Return all built-in profiles as `(id, name, keymap)` triples.
pub fn builtin_profiles() -> Vec<(ProfileId, &'static str, Keymap)> {
    vec![
        ("pubg", "PUBG Mobile", pubg_profile()),
        ("free_fire", "Free Fire", free_fire_profile()),
        ("cod_mobile", "Call of Duty Mobile", cod_profile()),
        ("generic_fps", "Generic FPS", generic_fps_profile()),
        ("mmorpg", "MMORPG", mmorpg_profile()),
    ]
}

/// Look up a profile by ID. Returns `None` if not found.
pub fn profile_by_id(id: ProfileId) -> Option<Keymap> {
    builtin_profiles()
        .into_iter()
        .find(|(pid, _, _)| *pid == id)
        .map(|(_, _, km)| km)
}

/// A sensible PUBG Mobile layout: WASD movement, mouse-look, click-to-shoot,
/// 1/2/3 weapon switch, R reload, Space jump.
fn pubg_profile() -> Keymap {
    let mut km = Keymap::for_resolution("PUBG Mobile", 1280, 720);
    km.description = "PUBG Mobile default layout".into();
    km.capture_cursor = true;
    km.look_sensitivity = 1.5;

    // Movement joystick anchored at the lower-left.
    km.add_region(MouseRegion {
        x: 0,
        y: 360,
        width: 400,
        height: 360,
        joystick_origin: TouchTarget { x: 200, y: 600, radius: Some(80) },
        sensitivity: 80.0,
    });

    km.bind_key(scancodes::KEY_SPACE, KeyAction::Tap {
        target: TouchTarget { x: 800, y: 600, radius: None },
    });
    km.bind_key(scancodes::KEY_LEFTSHIFT, KeyAction::Hold {
        target: TouchTarget { x: 700, y: 500, radius: None },
    });
    km.bind_key(scancodes::KEY_R, KeyAction::Tap {
        target: TouchTarget { x: 900, y: 600, radius: None },
    });
    km.bind_key(scancodes::KEY_1, KeyAction::Tap {
        target: TouchTarget { x: 1100, y: 100, radius: None },
    });
    km.bind_key(scancodes::KEY_2, KeyAction::Tap {
        target: TouchTarget { x: 1140, y: 140, radius: None },
    });
    km.bind_key(scancodes::KEY_3, KeyAction::Tap {
        target: TouchTarget { x: 1180, y: 180, radius: None },
    });

    km.bind_mouse(MouseButton::Left, KeyAction::Tap {
        target: TouchTarget { x: 640, y: 360, radius: None },
    });
    km.bind_mouse(MouseButton::Right, KeyAction::Hold {
        target: TouchTarget { x: 640, y: 360, radius: Some(40) },
    });
    km
}

/// Free Fire has fewer action buttons than PUBG — simpler layout.
fn free_fire_profile() -> Keymap {
    let mut km = Keymap::for_resolution("Free Fire", 1280, 720);
    km.description = "Free Fire default layout".into();
    km.capture_cursor = true;

    km.add_region(MouseRegion {
        x: 0,
        y: 360,
        width: 400,
        height: 360,
        joystick_origin: TouchTarget { x: 200, y: 600, radius: Some(80) },
        sensitivity: 80.0,
    });

    km.bind_key(scancodes::KEY_SPACE, KeyAction::Tap {
        target: TouchTarget { x: 800, y: 600, radius: None },
    });
    km.bind_key(scancodes::KEY_R, KeyAction::Tap {
        target: TouchTarget { x: 900, y: 600, radius: None },
    });
    km.bind_mouse(MouseButton::Left, KeyAction::Tap {
        target: TouchTarget { x: 640, y: 360, radius: None },
    });
    km
}

/// Call of Duty Mobile — added grenade button (G) and slide (Ctrl).
fn cod_profile() -> Keymap {
    let mut km = pubg_profile();
    km.name = "Call of Duty Mobile".into();
    km.description = "CoD Mobile default layout".into();
    km.bind_key(scancodes::KEY_G, KeyAction::Tap {
        target: TouchTarget { x: 1000, y: 600, radius: None },
    });
    km.bind_key(scancodes::KEY_LEFTCTRL, KeyAction::Tap {
        target: TouchTarget { x: 600, y: 600, radius: None },
    });
    km
}

/// A generic FPS profile that works for most shooters without bespoke tuning.
fn generic_fps_profile() -> Keymap {
    let mut km = Keymap::for_resolution("Generic FPS", 1280, 720);
    km.description = "Generic FPS layout — works for most shooters".into();
    km.capture_cursor = true;
    km.look_sensitivity = 1.0;

    km.add_region(MouseRegion {
        x: 0,
        y: 360,
        width: 400,
        height: 360,
        joystick_origin: TouchTarget { x: 200, y: 600, radius: Some(80) },
        sensitivity: 80.0,
    });
    km.bind_mouse(MouseButton::Left, KeyAction::Tap {
        target: TouchTarget { x: 640, y: 360, radius: None },
    });
    km.bind_mouse(MouseButton::Right, KeyAction::Hold {
        target: TouchTarget { x: 640, y: 360, radius: Some(40) },
    });
    km.bind_key(scancodes::KEY_SPACE, KeyAction::Tap {
        target: TouchTarget { x: 900, y: 600, radius: None },
    });
    km.bind_key(scancodes::KEY_R, KeyAction::Tap {
        target: TouchTarget { x: 1000, y: 600, radius: None },
    });
    km
}

/// An MMORPG profile — hotkeys 1-5 mapped to ability buttons in a row.
fn mmorpg_profile() -> Keymap {
    let mut km = Keymap::for_resolution("MMORPG", 1280, 720);
    km.description = "MMORPG layout — 5 hotkeys".into();

    for (i, sc) in [
        scancodes::KEY_1,
        scancodes::KEY_2,
        scancodes::KEY_3,
        scancodes::KEY_4,
        scancodes::KEY_5,
    ]
    .iter()
    .enumerate()
    {
        km.bind_key(*sc, KeyAction::Tap {
            target: TouchTarget {
                x: 400 + (i as u32) * 100,
                y: 600,
                radius: None,
            },
        });
    }
    km
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_profiles_have_keys() {
        for (id, name, km) in builtin_profiles() {
            assert!(!km.keys.is_empty() || !km.mouse_regions.is_empty(),
                "profile {id:?} ({name}) has no bindings");
            assert!(km.guest_width > 0 && km.guest_height > 0, "profile {id:?} has bad resolution");
        }
    }

    #[test]
    fn profile_lookup_works() {
        assert!(profile_by_id("pubg").is_some());
        assert!(profile_by_id("nonexistent").is_none());
    }

    #[test]
    fn profiles_are_serializable() {
        for (_, _, km) in builtin_profiles() {
            let s = serde_json::to_string(&km).unwrap();
            let back: Keymap = serde_json::from_str(&s).unwrap();
            assert_eq!(km.keys.len(), back.keys.len());
        }
    }
}
