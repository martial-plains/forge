// SPDX-License-Identifier: MIT OR Apache-2.0
//! Keyboard macros: C-x ( ... C-x ) records keys, C-x e plays them back.
//!
//! The recorded state lives in [`forge_core::Kmacro`] (core owns it so the
//! plugin runtime can read it); this module holds the commands and the
//! event <-> description conversion. Macros record key presses, not edits:
//! replaying "search for foo, then change the next word" has to search
//! again from wherever point is now, which only works if the keys are
//! re-run.

use crate::app::App;
use crate::keymap::{describe_key, key_event};
use ratatui::crossterm::event::KeyEvent;

pub use forge_core::Kmacro;

/// Longest a C-u 0 C-x e ("repeat until something fails") may run.
const REPEAT_LIMIT: usize = forge_core::kmacro::REPEAT_LIMIT;

/// The description `handle_key` records for a key press, if it has one.
pub fn record_key(key: KeyEvent) -> Option<String> {
    describe_key(&key, false)
}

pub fn execute(app: &mut App, cmd: &str) -> bool {
    match cmd {
        "start-kbd-macro" => start(app),
        "end-kbd-macro" => {
            if end(app) && app.count() > 1 {
                call(app, app.count() - 1);
            }
        }
        "call-last-kbd-macro" => call(app, app.count()),
        // C-x e: end the definition if one is going, then run it.
        "kmacro-end-and-call-macro" => {
            if app.editor.kmacro.recording.is_some() {
                end(app);
            }
            call(app, app.count());
            if !app.bell && app.editor.kmacro.last.is_some() {
                app.msg("(Type e to repeat macro)");
            }
        }
        "kmacro-end-or-call-macro" => {
            if app.editor.kmacro.recording.is_some() {
                end(app);
            } else {
                call(app, app.count());
            }
        }
        _ => return false,
    }
    true
}

fn start(app: &mut App) {
    if app.editor.kmacro.executing {
        return;
    }
    if app.editor.kmacro.recording.is_some() {
        return app.error("Already defining keyboard macro");
    }
    app.editor.kmacro.recording = Some(Vec::new());
    app.editor.kmacro.seq_start = 0;
    app.msg("Defining kbd macro...");
}

/// Stop recording. Returns false if no macro was being defined.
fn end(app: &mut App) -> bool {
    if app.editor.kmacro.executing {
        return false;
    }
    let Some(mut keys) = app.editor.kmacro.recording.take() else {
        app.error("Not defining kbd macro");
        return false;
    };
    keys.truncate(app.editor.kmacro.seq_start);
    app.editor.kmacro.finish(keys);
    app.msg("Keyboard macro defined");
    true
}

/// Run the last macro `times` times (0: until something fails).
fn call(app: &mut App, times: i64) {
    if app.editor.kmacro.executing {
        return;
    }
    let Some(keys) = app.editor.kmacro.last.clone() else {
        return app.error("No kbd macro has been defined");
    };
    if times < 0 {
        return app.error("Can't run a keyboard macro a negative number of times");
    }
    let n = if times == 0 {
        REPEAT_LIMIT
    } else {
        times as usize
    };
    app.editor.kmacro.executing = true;
    app.bell = false;
    'outer: for _ in 0..n {
        for k in &keys {
            if let Some(ev) = key_event(k) {
                app.handle_key(ev);
            }
            if app.bell || app.quit {
                break 'outer;
            }
        }
        if keys.is_empty() {
            break;
        }
    }
    app.editor.kmacro.executing = false;
    // Repeating until failure is the expected way for C-u 0 to stop.
    if times == 0 {
        app.bell = false;
    }
}
