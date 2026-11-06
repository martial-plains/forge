// SPDX-License-Identifier: MIT OR Apache-2.0
//! Lock files and auto-saving them.
//!
//! **Locks.** A buffer locks its file (`.#name`, see forge_core::filelock)
//! when it is first modified. If another editor holds the
//! lock, Forge asks, as `ask-user-about-lock` does:
//!
//! ```text
//! notes.txt locked by alice@host (pid 4242): (s, q, p, ?)?
//! ```
//!
//! `s` steals the lock, `p` proceeds to edit without it, and `q` (or C-g)
//! gives up: the change that needed the lock is undone and "File is locked"
//! is reported. The lock is released when the buffer is saved, reverted,
//! undone back to the saved text, or killed, and when Forge exits.
//!
//! **Auto-saving.** Modified file buffers are written to `#name#`:
//! after `auto_save_interval` input events (300, like Forge's
//! `auto-save-interval`) and after `auto_save_timeout` seconds of idle
//! time (30, like `auto-save-timeout`). Saving deletes the auto-save
//! file; `M-x recover-this-file` brings it back after a crash; a buffer
//! that loses more than a quarter of a big text stops auto-saving until it
//! is saved. Forge also auto-saves everything when it is killed by a
//! signal (a closed terminal, `kill`) or crashes.

#![deny(clippy::unwrap_used, clippy::expect_used)]

use crate::app::App;
use crate::minibuffer::Purpose;
use forge_core::BufferId;
use std::path::Path;

/// `auto-save-interval`: input events between auto-saves.
pub const AUTO_SAVE_INTERVAL: u64 = 300;

/// A question answered with one key.
pub enum KeyChoice {
    /// `s`, `q`, `p` or `?` about a lock someone else holds.
    Lock {
        id: BufferId,
        file: String,
        owner: String,
    },
    /// recover-this-file: `y`, `n`, or `=` to see the differences first.
    Recover { id: BufferId, auto: String },
    /// revert-buffer when the buffer was auto-saved since it was last
    /// saved: revert from the auto-save file instead?
    RevertFromAutoSave { id: BufferId },
}

impl KeyChoice {
    pub fn help(&self) -> &'static str {
        match self {
            KeyChoice::Lock { .. } => {
                "s: steal the lock and edit;  p: proceed to edit without the lock;  q: don't make the change"
            }
            KeyChoice::Recover { .. } => {
                "y: recover the auto-saved text;  n: keep the file's;  =: show the differences first"
            }
            KeyChoice::RevertFromAutoSave { .. } => {
                "y: revert from the auto-save file;  n: revert from the file itself"
            }
        }
    }
}

/// Short file name for questions: `~/notes.txt`.
pub fn abbreviate(path: &Path) -> String {
    let s = path.display().to_string();
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && s.starts_with(&h) && s[h.len()..].starts_with('/') => {
            format!("~{}", &s[h.len()..])
        }
        _ => s,
    }
}

/// Ask about the first lock conflict that hasn't been asked about (the
/// selected buffer's first). Called after every command and when idle; a
/// question already up waits its turn.
pub fn ask_about_locks(app: &mut App) {
    if app.minibuffer.is_some() || app.query.is_some() {
        return;
    }
    let active = app.active_id();
    let mut ids = app.editor.buffer_ids();
    ids.sort_by_key(|id| *id != active);
    for id in ids {
        let Some(b) = app.editor.state.get_buffer_mut(id) else {
            continue;
        };
        let Some(owner) = b.take_lock_question() else {
            continue;
        };
        let file = b
            .path()
            .map(abbreviate)
            .unwrap_or_else(|| b.name().to_string());
        let owner = owner.describe();
        app.prompt(
            format!("{file} locked by {owner}: (s, q, p, ?)? "),
            Purpose::KeyChoice(KeyChoice::Lock { id, file, owner }),
        );
        return;
    }
}

/// A key typed at a [`KeyChoice`] question. False keeps the question up.
pub fn answer(app: &mut App, choice: &KeyChoice, c: char) -> bool {
    match (choice, c.to_ascii_lowercase()) {
        (_, '?') => {
            app.msg(choice.help());
            false
        }
        (KeyChoice::Lock { id, .. }, 's') => {
            if let Some(b) = app.editor.state.get_buffer_mut(*id) {
                match b.steal_file_lock() {
                    Ok(()) => app.message = None,
                    Err(e) => app.error(format!("Couldn't take the lock: {e}")),
                }
            }
            true
        }
        (KeyChoice::Lock { id, .. }, 'p') => {
            if let Some(b) = app.editor.state.get_buffer_mut(*id) {
                b.ignore_file_lock();
            }
            app.message = None;
            true
        }
        (KeyChoice::Lock { .. }, 'q') => {
            give_up_change(app, choice);
            true
        }
        (KeyChoice::Recover { id, .. }, 'y') => {
            recover(app, *id);
            true
        }
        (KeyChoice::Recover { id, .. }, '=') => {
            show_auto_save_diff(app, *id);
            false
        }
        (KeyChoice::Recover { .. }, 'n') => {
            app.msg("Recover-file canceled");
            true
        }
        (KeyChoice::RevertFromAutoSave { id }, 'y') => {
            recover(app, *id);
            true
        }
        (KeyChoice::RevertFromAutoSave { id }, 'n') => {
            ask_revert_from_file(app, *id);
            true
        }
        _ => {
            app.msg(format!(
                "Please answer one of the keys shown ({})",
                choice.help()
            ));
            false
        }
    }
}

/// C-g at a [`KeyChoice`] question.
pub fn cancel(app: &mut App, choice: &KeyChoice) {
    if let KeyChoice::Lock { .. } = choice {
        give_up_change(app, choice);
    }
}

/// `q`: Forge signals file-locked before the change happens; here it has
/// happened, so it is undone.
fn give_up_change(app: &mut App, choice: &KeyChoice) {
    let KeyChoice::Lock { id, file, owner } = choice else {
        return;
    };
    if let Some(b) = app.editor.state.get_buffer_mut(*id) {
        b.undo_to_saved();
        // Back to unmodified, the next change asks again. If the saved
        // text can't be reached, stop asking about this lock.
        if b.is_modified() {
            b.ignore_file_lock();
        }
    }
    app.error(format!("File is locked: {file}, {owner}"));
}

/// Replace the buffer's text with the auto-save file's.
fn recover(app: &mut App, id: BufferId) {
    let Some(b) = app.editor.state.get_buffer_mut(id) else {
        return;
    };
    let name = b.name().to_string();
    match b.recover_auto_save() {
        Ok(()) => app.msg(format!(
            "Recovered {name} from its auto-save file; C-x C-s to keep it"
        )),
        Err(e) => app.error(format!("{name}: {e}")),
    }
}

fn ask_revert_from_file(app: &mut App, id: BufferId) {
    let Some(p) = app.editor.state.get_buffer(id).and_then(|b| b.path()) else {
        return;
    };
    let msg = format!("Revert buffer from file {}? (y or n) ", abbreviate(p));
    app.prompt(
        msg,
        Purpose::Confirm(crate::minibuffer::ConfirmAction::Revert(id)),
    );
}

/// `=` at the recover question: the differences between the file and its
/// auto-save, in *Diff*.
fn show_auto_save_diff(app: &mut App, id: BufferId) {
    let Some(b) = app.editor.state.get_buffer(id) else {
        return;
    };
    let (Some(file), Some(auto)) = (b.path().map(Path::to_path_buf), b.auto_save_path()) else {
        return;
    };
    let old = std::fs::read_to_string(&file).unwrap_or_default();
    let new = std::fs::read_to_string(&auto).unwrap_or_default();
    let text = crate::vc::diff::unified(&old, &new, &abbreviate(&file), &abbreviate(&auto), 3);
    let text = if text.is_empty() {
        "No differences.\n".to_string()
    } else {
        text
    };
    crate::vc::diff::show(app, crate::vc::diff::DIFF_BUFFER, &text, None);
    if let Some(id) = app.editor.find_buffer(crate::vc::diff::DIFF_BUFFER) {
        crate::compile::display_buffer(app, id);
    }
}

/// recover-this-file: offer the auto-save file's text.
pub fn recover_this_file(app: &mut App) {
    let b = app.buf_ref();
    let Some(a) = b.auto_save_path() else {
        return app.error("This buffer is not visiting a file");
    };
    if !a.exists() {
        return app.error(format!("Auto-save file {} not current", abbreviate(&a)));
    }
    let auto = abbreviate(&a);
    let id = app.active_id();
    app.prompt(
        format!("Recover auto save file {auto}? (y, n, = or ?) "),
        Purpose::KeyChoice(KeyChoice::Recover { id, auto }),
    );
}

/// revert-buffer: from the auto-save file if the buffer was auto-saved
/// since it was saved, else from the file.
pub fn revert_buffer(app: &mut App) {
    let id = app.active_id();
    let b = app.buf_ref();
    if b.path().is_none() {
        return app.error("This buffer is not visiting a file");
    }
    if b.recently_auto_saved() {
        app.prompt(
            "An auto-save file is newer than this buffer. Revert to it? (y or n) ",
            Purpose::KeyChoice(KeyChoice::RevertFromAutoSave { id }),
        );
    } else {
        ask_revert_from_file(app, id);
    }
}

/// After the selected buffer was saved: offer to delete excess numbered
/// backups, and let version
/// control catch up.
pub fn after_save(app: &mut App) {
    let id = app.active_id();
    crate::init_review::record_saved(app);
    crate::vc::after_save(app, id);
    let excess = app.buf().take_excess_backups();
    if !excess.is_empty() && app.minibuffer.is_none() {
        let name = app.buf_ref().name().to_string();
        app.prompt(
            format!("Delete excess backup versions of {name}? (y or n) "),
            Purpose::Confirm(crate::minibuffer::ConfirmAction::DeleteBackups(excess)),
        );
    }
}

// ------------------------------------------------------------- auto-save

/// Auto-save every buffer that needs it. Says
/// "Auto-saving...done" unless `quiet` or something else is in the echo
/// area, and turns auto-saving off in a buffer that shrank a lot.
pub fn do_auto_save(app: &mut App, quiet: bool) {
    let mut failed = Vec::new();
    let mut shrunk = Vec::new();
    let mut saved = 0;
    for b in app.editor.state.buffers.values_mut() {
        if !b.needs_auto_save() {
            continue;
        }
        if b.shrunk_a_lot() {
            b.disable_auto_save();
            shrunk.push(b.name().to_string());
            continue;
        }
        match b.auto_save() {
            Ok(()) => saved += 1,
            Err(e) => failed.push(format!("{}: {e}", b.name())),
        }
    }
    app.input_since_auto_save = 0;
    if !failed.is_empty() {
        app.error(format!("Auto-saving...error! {}", failed.join("; ")));
    } else if let Some(name) = shrunk.first() {
        app.msg(format!(
            "Buffer {name} has shrunk a lot; auto save disabled in that buffer until next real save"
        ));
    } else if saved > 0 && !quiet && app.message.is_none() && app.minibuffer.is_none() {
        app.msg("Auto-saving...done");
    }
}

/// One more input event: auto-save once there have been
/// `auto_save_interval` of them.
pub fn count_input(app: &mut App) {
    if app.auto_save_interval == 0 || !app.auto_save_default {
        return;
    }
    app.input_since_auto_save += 1;
    if app.input_since_auto_save >= app.auto_save_interval {
        do_auto_save(app, false);
    }
}

/// When idle: auto-save once there has been no input for
/// `auto_save_timeout`.
pub fn idle_auto_save(app: &mut App) {
    if app.auto_save_default
        && !app.auto_save_idle.is_zero()
        && app.last_input.elapsed() >= app.auto_save_idle
    {
        do_auto_save(app, false);
    }
}

/// Visiting a file that has auto-save data newer than itself.
pub fn warn_about_auto_save_data(app: &mut App) {
    let b = app.buf_ref();
    if b.path().is_some() && b.has_newer_auto_save() {
        let name = b.name().to_string();
        app.msg(format!(
            "{name} has auto save data; consider M-x recover-this-file"
        ));
    }
}
