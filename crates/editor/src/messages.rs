// SPDX-License-Identifier: MIT OR Apache-2.0
//! The *Messages* buffer: messages-buffer, and the log every echoed
//! message goes to.

use crate::app::App;
use forge_core::plugin::runtime::Level;
use forge_core::{Buffer, BufferId, MessageLogMax};

/// The last line written to *Messages*, so a run of related messages can
/// replace it instead of piling up.
#[derive(Clone)]
pub(crate) struct LogEntry {
    /// The whole line, without its newline, as written.
    pub line: String,
    /// The message text alone (no `[source]` or level tag).
    pub text: String,
    /// The source tag, so messages from different sources don't combine.
    pub source: String,
}

/// Like messages-buffer: the *Messages* buffer, made on first
/// use. Plugins never see events from it (they log there).
pub fn messages_buffer(app: &mut App) -> BufferId {
    let name = app.messages_buffer_name.clone();
    match app.editor.find_buffer(&name) {
        Some(id) => id,
        None => {
            let mut b = Buffer::new(&name);
            b.set_major_mode("messages-buffer-mode");
            let id = app.editor.add_buffer(b);
            if let Some(h) = app.plugin_host.as_mut() {
                h.ignore_buffer(id);
            }
            id
        }
    }
}

/// Append to *Messages*, like `message` logging to the
/// messages-buffer. Respects `message-log-max` (off, a line limit, or no
/// limit) and collapses a message that extends the previous one.
pub fn log(app: &mut App, source: &str, level: Level, text: &str) {
    if app.message_log_max == MessageLogMax::Off {
        return;
    }
    let tag = match level {
        Level::Debug => "debug ",
        Level::Info => "",
        Level::Warn => "warning ",
        Level::Error => "error ",
    };
    let line = format!("[{source}] {tag}{text}");

    // The buffer can outlive the plugin host (it is made at startup, the
    // host later), so keep it ignored on every append.
    let id = messages_buffer(app);
    if let Some(h) = app.plugin_host.as_mut() {
        h.ignore_buffer(id);
    }

    if let Some(prev) = app.last_messages_log.clone() {
        // The same message again: Forge keeps the single copy already there.
        if prev.source == source && prev.text == text {
            return;
        }
        if prev.source == source
            && combines(&prev.text, text)
            && replace_last(app, &prev.line, &line)
        {
            app.last_messages_log = Some(LogEntry {
                line,
                text: text.to_string(),
                source: source.to_string(),
            });
            trim(app);
            return;
        }
    }

    append(app, &line);
    app.last_messages_log = Some(LogEntry {
        line,
        text: text.to_string(),
        source: source.to_string(),
    });
    trim(app);
}

/// True when `new` extends `prev` in combines messages: `new`
/// keeps going where `prev` stopped (a progress message that ended in "..."
/// and grew into a longer one), or both share a prefix that ends in "...".
fn combines(prev: &str, new: &str) -> bool {
    if prev.is_empty() || new == prev {
        return false;
    }
    if new.starts_with(prev) {
        return true;
    }
    let common = common_prefix(prev, new);
    common.ends_with("...")
        && common.chars().count() < prev.chars().count()
        && common.chars().count() < new.chars().count()
}

fn common_prefix(a: &str, b: &str) -> String {
    a.chars()
        .zip(b.chars())
        .take_while(|(x, y)| x == y)
        .map(|(x, _)| x)
        .collect()
}

/// Replace the previous message's line with a new one, if it is still the
/// buffer's last line (the user may have edited *Messages* by hand). Says
/// whether it did.
fn replace_last(app: &mut App, old_line: &str, new_line: &str) -> bool {
    let id = messages_buffer(app);
    let b = app.editor.state.get_buffer_mut(id).expect("just made");
    let contents = b.get_contents();
    let suffix = format!("{old_line}\n");
    if !contents.ends_with(&suffix) {
        return false;
    }
    let start = contents.chars().count() - suffix.chars().count();
    let p = b.point();
    b.replace_offsets(
        start,
        start + suffix.chars().count(),
        &format!("{new_line}\n"),
    );
    b.set_point(p);
    b.set_modified(false);
    b.break_undo_group();
    true
}

fn append(app: &mut App, line: &str) {
    let id = messages_buffer(app);
    let b = app.editor.state.get_buffer_mut(id).expect("just made");
    // Appending shouldn't count as an edit anyone can undo.
    let end = b.end();
    let p = b.point();
    b.replace_range(end, end, &format!("{line}\n"));
    b.set_point(p);
    b.set_modified(false);
    b.break_undo_group();
}

/// Drop the oldest lines once *Messages* is over `message-log-max`.
fn trim(app: &mut App) {
    let MessageLogMax::Lines(keep) = app.message_log_max else {
        return;
    };
    let id = messages_buffer(app);
    let b = app.editor.state.get_buffer_mut(id).expect("just made");
    let contents = b.get_contents();
    let count = contents.matches('\n').count();
    if count <= keep {
        return;
    }
    // The byte just past the `count - keep`-th newline is where the kept
    // lines start.
    let drop = count - keep;
    let mut bytes = 0usize;
    for _ in 0..drop {
        match contents[bytes..].find('\n') {
            Some(i) => bytes += i + 1,
            None => break,
        }
    }
    let chars = contents[..bytes].chars().count();
    let p = b.point();
    b.replace_offsets(0, chars, "");
    b.set_point(p);
    b.set_modified(false);
    b.break_undo_group();
}
