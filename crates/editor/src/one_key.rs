// SPDX-License-Identifier: MIT OR Apache-2.0
//! A menu in a window that waits for one key: the Org agenda dispatcher
//! (`org-agenda`), where Forge shows `*Agenda Commands*`, reads a single
//! key, then deletes the window and kills the buffer. The same shape
//! serves any command that offers a one-key menu.

use forge_core::{BufferId, Direction, WindowId};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::App;

/// A menu waiting for its key.
pub struct OneKey {
    buffer: BufferId,
    /// The window made for the menu; None if the frame could not be split.
    window: Option<WindowId>,
    /// The window that was selected before, selected again afterwards.
    origin: WindowId,
    prompt: String,
    /// The keys that answer it. Another key is refused and the menu stays.
    valid: Vec<char>,
    on_key: fn(&mut App, char),
}

/// Show `text` in a new window under `name` and wait for one of `valid`.
pub fn open(
    app: &mut App,
    name: &str,
    text: &str,
    prompt: &str,
    valid: Vec<char>,
    on_key: fn(&mut App, char),
) {
    close_existing(app);
    let origin = app.editor.windows.selected_id();
    let id = match app.editor.find_buffer(name) {
        Some(id) => id,
        None => app.editor.add_buffer(forge_core::Buffer::new(name)),
    };
    if let Some(b) = app.editor.state.get_buffer_mut(id) {
        b.set_contents(text);
        b.set_read_only(true);
    }
    let window = app.editor.split_window(Direction::Rows).ok();
    if let Some(w) = window {
        app.select_window(w);
    }
    app.switch_to(id);
    app.win_mut().scroll_top = 0;
    app.msg(prompt);
    app.one_key = Some(OneKey { buffer: id, window, origin, prompt: prompt.to_string(), valid, on_key });
}

fn close_existing(app: &mut App) {
    if app.one_key.is_some() {
        close(app);
    }
}

/// Delete the menu's window and kill its buffer.
fn close(app: &mut App) -> Option<OneKey> {
    let m = app.one_key.take()?;
    if let Some(w) = m.window
        && app.editor.windows.get(w).is_some()
        && app.editor.windows.count() > 1
    {
        let _ = app.editor.delete_window(w);
    }
    if app.editor.windows.get(m.origin).is_some() {
        app.select_window(m.origin);
    }
    if app.editor.state.buffers.contains_key(&m.buffer) {
        app.kill_buffer(m.buffer);
    }
    Some(m)
}

/// A key while a menu is waiting. Always true when one is.
pub fn handle_key(app: &mut App, key: KeyEvent) -> bool {
    let Some(m) = app.one_key.as_ref() else { return false };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let quit = matches!(key.code, KeyCode::Esc) || (ctrl && key.code == KeyCode::Char('g'));
    let chosen = match key.code {
        KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => Some(c),
        _ => None,
    };
    if quit {
        close(app);
        app.msg("Quit");
        return true;
    }
    match chosen.filter(|c| m.valid.contains(c)) {
        Some(c) => {
            let on_key = m.on_key;
            close(app);
            app.message = None;
            on_key(app, c);
        }
        None => {
            // Forge reads again after a key that is no command.
            let prompt = m.prompt.clone();
            app.msg(format!("Invalid key; {prompt}"));
        }
    }
    true
}
