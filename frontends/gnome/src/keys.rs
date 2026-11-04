// SPDX-License-Identifier: MIT OR Apache-2.0
//! GTK keys as the editor's keys.

use forge_ffi::{Key, KeyInput};
use gtk4::gdk::{self, ModifierType};
use gtk4::glib::translate::IntoGlib;

/// None when the key is plain text: the input method commits it instead.
pub fn translate(keyval: gdk::Key, mods: ModifierType) -> Option<KeyInput> {
    let control = mods.contains(ModifierType::CONTROL_MASK);
    let meta = mods.contains(ModifierType::ALT_MASK) || mods.contains(ModifierType::META_MASK);
    let shift = mods.contains(ModifierType::SHIFT_MASK);
    let named = match keyval {
        gdk::Key::Return | gdk::Key::KP_Enter => Some(Key::Enter),
        gdk::Key::Tab => Some(Key::Tab),
        gdk::Key::ISO_Left_Tab => Some(Key::BackTab),
        gdk::Key::BackSpace => Some(Key::Backspace),
        gdk::Key::Delete | gdk::Key::KP_Delete => Some(Key::Delete),
        gdk::Key::Escape => Some(Key::Escape),
        gdk::Key::Left | gdk::Key::KP_Left => Some(Key::Left),
        gdk::Key::Right | gdk::Key::KP_Right => Some(Key::Right),
        gdk::Key::Up | gdk::Key::KP_Up => Some(Key::Up),
        gdk::Key::Down | gdk::Key::KP_Down => Some(Key::Down),
        gdk::Key::Home | gdk::Key::KP_Home => Some(Key::Home),
        gdk::Key::End | gdk::Key::KP_End => Some(Key::End),
        gdk::Key::Page_Up | gdk::Key::KP_Page_Up => Some(Key::PageUp),
        gdk::Key::Page_Down | gdk::Key::KP_Page_Down => Some(Key::PageDown),
        gdk::Key::Insert | gdk::Key::KP_Insert => Some(Key::Insert),
        k => function_key(k).map(|n| Key::F { n }),
    };
    if let Some(key) = named {
        return Some(KeyInput { key, control, meta, shift });
    }
    // Control and Alt chords are keys; anything else is text.
    if control || meta {
        let c = keyval.to_unicode().filter(|c| !c.is_control())?;
        return Some(KeyInput { key: Key::Char { c: c.to_string() }, control, meta, shift });
    }
    None
}

fn function_key(k: gdk::Key) -> Option<u8> {
    let n = k.into_glib().checked_sub(gdk::Key::F1.into_glib())?;
    (n < 12).then_some(n as u8 + 1)
}
