// SPDX-License-Identifier: MIT OR Apache-2.0
//! The commit message buffer.
//!
//! Type the message and `C-c C-c` commits; `C-c C-k` gives up (the text
//! is kept as a draft and offered next time). Below the message, a
//! read-only summary lists the files being committed. The first line is
//! the summary: it is highlighted, and a summary longer than 72
//! characters, or one not followed by a blank line, is questioned before
//! committing. Body lines wrap at 72 columns as you type (auto-fill-mode;
//! M-q fills a paragraph).
//!
//! `M-p`/`M-n` step through earlier messages and `M-r`/`M-s` search them
//! by regexp. Header lines at the top set commit options, as in Forge's
//! vc-git: `Author: Name <mail>`, `Date: ...`, `Amend: yes`,
//! `Sign-Off: yes`, `No-Verify: yes` (`C-c C-e`, `C-c C-s` and `C-c C-n`
//! toggle the last three). `C-c C-d` shows the diff being committed; it is
//! also shown when the buffer opens.

use super::backend::{CheckinOpts, FileState};
use super::{Fileset, runner};
use crate::app::App;
use forge_core::decoration::{DecoFace, DecoSpec, VcFace};
use forge_core::{Buffer, BufferId, WindowId};
use std::collections::HashMap;
use std::path::PathBuf;

pub const MODE: &str = "log-edit-mode";
pub const BUFFER: &str = "*vc-log*";
const DECO: &str = "log-edit";
/// Longest summary line before committing asks.
pub const SUMMARY_MAX: usize = 72;
pub const FILL_COLUMN: usize = 72;
const HISTORY_MAX: usize = 64;
const HEADERS: &[&str] = &[
    "Author",
    "Date",
    "Amend",
    "Sign-Off",
    "No-Verify",
    "Summary",
];

/// A commit in progress.
#[derive(Debug, Clone)]
pub struct Session {
    pub fileset: Fileset,
    /// Windows and what they showed before, to put back afterwards.
    windows: Vec<(WindowId, BufferId)>,
}

/// Earlier commit messages, and unfinished
/// ones by repository. Kept in the state directory between sessions.
#[derive(Debug, Default)]
pub struct History {
    loaded: bool,
    pub ring: Vec<String>,
    /// Where M-p / M-n are in the ring (None: the text being typed).
    pos: Option<usize>,
    /// What was typed before M-p.
    typed: Option<String>,
    pub drafts: HashMap<PathBuf, String>,
}

fn history_file() -> Option<PathBuf> {
    forge_core::config::state_dir().map(|d| d.join("vc-log-history"))
}

fn drafts_file() -> Option<PathBuf> {
    forge_core::config::state_dir().map(|d| d.join("vc-log-drafts"))
}

const RS: &str = "\u{1e}\n";
const FS: char = '\u{1f}';

impl History {
    fn load(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        if let Some(text) = history_file().and_then(|f| std::fs::read_to_string(f).ok()) {
            self.ring = text
                .split(RS)
                .filter(|m| !m.trim().is_empty())
                .map(str::to_string)
                .collect();
        }
        if let Some(text) = drafts_file().and_then(|f| std::fs::read_to_string(f).ok()) {
            for rec in text.split(RS) {
                if let Some((root, msg)) = rec.split_once(FS) {
                    self.drafts.insert(PathBuf::from(root), msg.to_string());
                }
            }
        }
    }

    fn save(&self) {
        if let Some(f) = history_file() {
            if let Some(d) = f.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(f, self.ring.join(RS));
        }
        if let Some(f) = drafts_file() {
            let text: Vec<String> = self
                .drafts
                .iter()
                .map(|(r, m)| format!("{}{FS}{m}", r.display()))
                .collect();
            let _ = std::fs::write(f, text.join(RS));
        }
    }

    pub fn push(&mut self, msg: &str) {
        self.load();
        let msg = msg.trim().to_string();
        if msg.is_empty() {
            return;
        }
        self.ring.retain(|m| m != &msg);
        self.ring.push(msg);
        if self.ring.len() > HISTORY_MAX {
            let extra = self.ring.len() - HISTORY_MAX;
            self.ring.drain(..extra);
        }
        self.save();
    }
}

/// The parts of the buffer: (headers, message), the protected summary
/// left out.
pub fn split_message(text: &str) -> (Vec<(String, String)>, String) {
    let mut headers = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        let Some((k, v)) = l.split_once(':') else {
            break;
        };
        if !HEADERS.iter().any(|h| h.eq_ignore_ascii_case(k.trim())) || k.contains(' ') {
            break;
        }
        headers.push((k.trim().to_string(), v.trim().to_string()));
        i += 1;
    }
    // Headers only count when a blank line (or the end) follows them.
    if !headers.is_empty() && i < lines.len() && !lines[i].trim().is_empty() {
        headers.clear();
        i = 0;
    }
    let body = lines[i..].join("\n");
    (headers, body.trim().to_string())
}

fn yes(v: &str) -> bool {
    matches!(
        v.to_ascii_lowercase().as_str(),
        "yes" | "y" | "t" | "true" | "1" | "on"
    )
}

/// The commit's options from its headers.
pub fn options(headers: &[(String, String)]) -> (CheckinOpts, Option<String>) {
    let mut o = CheckinOpts::default();
    let mut summary = None;
    for (k, v) in headers {
        match k.to_ascii_lowercase().as_str() {
            "author" if !v.is_empty() => o.author = Some(v.clone()),
            "date" if !v.is_empty() => o.date = Some(v.clone()),
            "amend" => o.amend = yes(v),
            "sign-off" => o.signoff = yes(v),
            "no-verify" => o.no_verify = yes(v),
            "summary" if !v.is_empty() => summary = Some(v.clone()),
            _ => {}
        }
    }
    (o, summary)
}

/// What's wrong with a message by the usual conventions.
pub fn check_format(msg: &str) -> Option<String> {
    let mut lines = msg.lines();
    let summary = lines.next().unwrap_or("");
    let n = summary.chars().count();
    if n > SUMMARY_MAX {
        return Some(format!(
            "The summary line is {n} characters (more than {SUMMARY_MAX})"
        ));
    }
    if let Some(second) = lines.next()
        && !second.trim().is_empty()
    {
        return Some("The summary line isn't followed by a blank line".into());
    }
    None
}

fn files_summary(fs: &Fileset, branch: Option<&str>) -> String {
    let mut s = String::from("\n");
    let on = branch.map(|b| format!(" to {b}")).unwrap_or_default();
    let n = fs.files.len();
    s.push_str(&format!(
        "# Committing {n} file{}{on} ({}, {}):\n",
        if n == 1 { "" } else { "s" },
        fs.kind.name(),
        crate::files::abbreviate(&fs.root)
    ));
    for (f, st) in &fs.files {
        s.push_str(&format!("#   {:<12} {f}\n", st.label()));
    }
    s.push_str(
        "# C-c C-c commits, C-c C-k cancels, C-c C-d shows the diff; this part isn't committed.\n",
    );
    s
}

/// Open the commit buffer for `fs`.
pub fn start(app: &mut App, fs: Fileset, amend: bool) {
    let fs = Fileset {
        files: fs
            .files
            .into_iter()
            .filter(|(_, s)| *s != FileState::Unregistered && *s != FileState::Ignored)
            .collect(),
        ..fs
    };
    if fs.files.is_empty() && !amend {
        return app.error("Nothing to commit: register the files first (C-x v i)");
    }
    app.vc.history.load();
    app.vc.history.pos = None;
    app.vc.history.typed = None;
    let draft = app.vc.history.drafts.remove(&fs.root);
    let branch = fs.backend().current_branch(&fs.root);
    let windows: Vec<(WindowId, BufferId)> = app
        .editor
        .windows
        .iter()
        .map(|w| (w.id, w.buffer))
        .collect();
    let id = match app.editor.find_buffer(BUFFER) {
        Some(id) => id,
        None => app.editor.add_buffer(Buffer::new(BUFFER)),
    };
    let mut head = String::new();
    let mut body = draft.clone().unwrap_or_default();
    if amend {
        head.push_str("Amend: yes\n\n");
        if body.is_empty() {
            body = fs.backend().last_message(&fs.root).unwrap_or_default();
        }
    }
    let message = format!("{head}{body}");
    let tail = files_summary(&fs, branch.as_deref());
    {
        let b = app.editor.state.get_buffer_mut(id).expect("made");
        b.set_locked(false);
        b.set_contents(&format!("{message}\n{tail}"));
        b.set_major_mode(MODE);
        b.fill_column = FILL_COLUMN;
        if !b.minor_mode_enabled("auto-fill-mode") {
            b.enable_minor_mode("auto-fill-mode");
        }
        let protected = message.chars().count() + 1;
        b.set_protected_tail(Some(protected));
        b.set_modified(false);
        // Point at the end of the message.
        let p = b.offset_to_pos(protected - 1);
        b.set_point(p);
        decorate(b);
    }
    app.vc.log_edits.insert(
        id,
        Session {
            fileset: fs.clone(),
            windows,
        },
    );
    if app.active_id() != id {
        crate::windows::pop_to_other_window(app);
        app.switch_to(id);
    }
    if draft.is_some() {
        app.msg("Restored your unfinished message (M-p for older ones)");
    }
    if !fs.files.is_empty() {
        super::show_diff(app, &fs, None, None, true);
    }
}

/// The message as typed: everything before the protected summary.
fn message_text(b: &Buffer) -> String {
    let end = b.protected_tail().unwrap_or(b.char_count());
    let all = b.get_contents();
    all.chars().take(end).collect()
}

fn decorate(b: &mut Buffer) {
    let text = message_text(b);
    let mut decos = Vec::new();
    let (headers, _) = split_message(&text);
    let mut offset = 0usize;
    let lines: Vec<&str> = text.split('\n').collect();
    let mut i = 0;
    for _ in &headers {
        let l = lines[i];
        let k = l
            .split_once(':')
            .map_or(l.len(), |(k, _)| k.chars().count() + 1);
        decos.push(DecoSpec::new(
            offset,
            offset + k,
            DecoFace::Vc(VcFace::LogEditHeader),
        ));
        offset += l.chars().count() + 1;
        i += 1;
    }
    // Skip the blank line after the headers, then the summary line.
    while i < lines.len() && lines[i].trim().is_empty() {
        offset += lines[i].chars().count() + 1;
        i += 1;
    }
    if let Some(summary) = lines.get(i) {
        let n = summary.chars().count();
        let ok = n.min(SUMMARY_MAX);
        if ok > 0 {
            decos.push(DecoSpec::new(
                offset,
                offset + ok,
                DecoFace::Vc(VcFace::LogEditSummary),
            ));
        }
        if n > SUMMARY_MAX {
            decos.push(
                DecoSpec::new(
                    offset + SUMMARY_MAX,
                    offset + n,
                    DecoFace::Vc(VcFace::LogEditOverlong),
                )
                .message(format!(
                    "The summary is {n} characters; keep it to {SUMMARY_MAX}"
                )),
            );
        }
        offset += n + 1;
        if let Some(second) = lines.get(i + 1)
            && !second.trim().is_empty()
        {
            let n2 = second.chars().count();
            decos.push(
                DecoSpec::new(offset, offset + n2, DecoFace::Vc(VcFace::LogEditOverlong))
                    .message("Leave a blank line after the summary"),
            );
        }
    }
    if let Some(p) = b.protected_tail() {
        decos.push(DecoSpec::new(
            p,
            b.char_count(),
            DecoFace::Vc(VcFace::LogEditFiles),
        ));
    }
    b.set_decorations(DECO, decos);
}

/// After each command in the commit buffer: faces, and auto-fill.
pub fn after_command(app: &mut App) {
    auto_fill(app);
    let b = app.buf();
    decorate(b);
}

/// auto-fill-mode in a commit message: the same breaking as everywhere
/// else, except the summary line (which is meant to be one line) is left
/// alone.
pub fn auto_fill(app: &mut App) {
    if app.last_command != "self-insert-command" {
        return;
    }
    if !app.buf_ref().minor_mode_enabled("auto-fill-mode") {
        return;
    }
    let p = app.buf_ref().point();
    let text = message_text(app.buf_ref());
    let (headers, _) = split_message(&text);
    let lines: Vec<&str> = text.split('\n').collect();
    let mut first = headers.len();
    while first < lines.len() && lines[first].trim().is_empty() {
        first += 1;
    }
    if p.line <= first {
        return;
    }
    let key = app.this_command_keys.last().cloned().unwrap_or_default();
    let space = key == "SPC" || key == " ";
    forge_core::auto_fill::maybe_break(app.buf(), space);
}

fn session(app: &App, id: BufferId) -> Option<Session> {
    app.vc.log_edits.get(&id).cloned()
}

/// Put the windows back the way they were before the commit buffer.
fn restore_windows(app: &mut App, s: &Session, log: BufferId) {
    let before: Vec<WindowId> = s.windows.iter().map(|(w, _)| *w).collect();
    let extra: Vec<WindowId> = app
        .editor
        .windows
        .iter()
        .map(|w| w.id)
        .filter(|w| !before.contains(w))
        .collect();
    for w in extra {
        if app.editor.windows.count() > 1 {
            let _ = app.editor.delete_window(w);
        }
    }
    let selected = app.editor.windows.selected_id();
    for (w, buf) in &s.windows {
        if app.editor.windows.iter().any(|x| x.id == *w)
            && app.editor.state.get_buffer(*buf).is_some()
            && *buf != log
        {
            app.select_window(*w);
            app.switch_to(*buf);
        }
    }
    if app.editor.windows.iter().any(|x| x.id == selected) {
        app.select_window(selected);
    }
}

/// C-c C-c (`force` after the conventions question).
pub fn finish(app: &mut App, id: BufferId, force: bool) {
    let Some(s) = session(app, id) else {
        return app.error("Not in a commit buffer");
    };
    let Some(b) = app.editor.state.get_buffer(id) else {
        return;
    };
    let text = message_text(b);
    let (headers, body) = split_message(&text);
    let (opts, summary) = options(&headers);
    let msg = match summary {
        Some(sum) if !body.is_empty() => format!("{sum}\n\n{body}"),
        Some(sum) => sum,
        None => body,
    };
    if msg.trim().is_empty() {
        return app.error("Empty commit message: type one, or C-c C-k to give up");
    }
    if !force && let Some(problem) = check_format(&msg) {
        return app.prompt(
            format!("{problem}; commit anyway? (y or n) "),
            crate::minibuffer::Purpose::Confirm(crate::minibuffer::ConfirmAction::Vc(
                super::Confirm::CommitAnyway(id),
            )),
        );
    }
    let fs = s.fileset.clone();
    let names = fs.names();
    let cmd = fs.backend().checkin(&fs.root, &names, &msg, &opts);
    let line = cmd.line();
    let summary_line = msg.lines().next().unwrap_or("").to_string();
    let amend = opts.amend;
    app.msg("Committing...");
    runner::run(app, cmd, "Committing", None, move |app, out| {
        if !out.success() {
            // A hook said no, or nothing to commit: keep the message.
            app.error(format!(
                "Commit failed: {} (see *vc*)",
                out.error_text(&line)
            ));
            let vc = runner::output_buffer(app);
            crate::compile::display_buffer(app, vc);
            return;
        }
        app.vc.history.push(&msg);
        app.vc.history.drafts.remove(&fs.root);
        app.vc.history.save();
        super::dir::note_committed(app, &fs.root, &names);
        let s = app.vc.log_edits.remove(&id);
        if let Some(s) = &s {
            restore_windows(app, s, id);
        }
        if app.editor.state.get_buffer(id).is_some() {
            if let Some(b) = app.editor.state.get_buffer_mut(id) {
                b.set_modified(false);
            }
            app.kill_buffer(id);
        }
        if let Some(d) = app.editor.find_buffer(super::diff::VC_DIFF_BUFFER)
            && app.editor.windows.iter().any(|w| w.buffer == d)
            && app.editor.windows.count() > 1
        {
            let ws: Vec<WindowId> = app
                .editor
                .windows
                .iter()
                .filter(|w| w.buffer == d)
                .map(|w| w.id)
                .collect();
            for w in ws {
                if app.editor.windows.count() > 1 {
                    let _ = app.editor.delete_window(w);
                }
            }
        }
        let root = fs.root.clone();
        super::refresh_root(app, &root);
        app.msg(format!(
            "{}: {summary_line}",
            if amend { "Amended" } else { "Committed" }
        ));
    });
}

/// Keep what was typed for next time (C-c C-k, or the buffer killed).
pub fn save_draft(app: &mut App, id: BufferId) {
    let Some(s) = app.vc.log_edits.get(&id).cloned() else {
        return;
    };
    let Some(b) = app.editor.state.get_buffer(id) else {
        return;
    };
    let text = message_text(b);
    let (_, body) = split_message(&text);
    app.vc.history.load();
    if body.trim().is_empty() {
        app.vc.history.drafts.remove(&s.fileset.root);
    } else {
        app.vc.history.drafts.insert(s.fileset.root.clone(), body);
    }
    app.vc.history.save();
}

/// Replace the message (keeping the headers) with `text`.
fn set_body(app: &mut App, id: BufferId, text: &str) {
    let Some(b) = app.editor.state.get_buffer_mut(id) else {
        return;
    };
    let msg = message_text(b);
    let (headers, _) = split_message(&msg);
    let head: String = headers.iter().map(|(k, v)| format!("{k}: {v}\n")).collect();
    let head = if head.is_empty() {
        head
    } else {
        format!("{head}\n")
    };
    let new = format!("{head}{text}\n");
    let end = b.protected_tail().unwrap_or(b.char_count());
    b.as_owner(|b| b.replace_offsets(0, end, &new));
    b.set_protected_tail(Some(new.chars().count()));
    b.set_modified(true);
    let p = b.offset_to_pos(new.chars().count() - 1);
    b.set_point(p);
    decorate(b);
}

fn current_body(app: &App, id: BufferId) -> String {
    app.editor
        .state
        .get_buffer(id)
        .map(|b| split_message(&message_text(b)).1)
        .unwrap_or_default()
}

/// M-p / M-n.
fn step_history(app: &mut App, id: BufferId, back: bool) {
    app.vc.history.load();
    let n = app.vc.history.ring.len();
    if n == 0 {
        return app.error("No earlier messages");
    }
    let h = &mut app.vc.history;
    let next = match (h.pos, back) {
        (None, true) => Some(n - 1),
        (None, false) => return app.error("End of the message history"),
        (Some(0), true) => return app.error("Beginning of the message history"),
        (Some(p), true) => Some(p - 1),
        (Some(p), false) if p + 1 < n => Some(p + 1),
        (Some(_), false) => None,
    };
    if h.pos.is_none() {
        let typed = current_body(app, id);
        app.vc.history.typed = Some(typed);
    }
    app.vc.history.pos = next;
    let text = match next {
        Some(i) => app.vc.history.ring[i].clone(),
        None => app.vc.history.typed.take().unwrap_or_default(),
    };
    set_body(app, id, &text);
    if let Some(i) = next {
        app.msg(format!("Message {} of {n}", i + 1));
    }
}

/// M-r / M-s with the regexp typed.
pub fn search(app: &mut App, id: BufferId, backward: bool, regexp: &str) {
    let re = match regex::Regex::new(regexp) {
        Ok(r) => r,
        Err(e) => return app.error(format!("Bad regexp: {e}")),
    };
    app.vc.history.load();
    let n = app.vc.history.ring.len();
    let start = app.vc.history.pos;
    let order: Vec<usize> = if backward {
        let from = start.unwrap_or(n);
        (0..from).rev().collect()
    } else {
        let from = start.map_or(n, |p| p + 1);
        (from..n).collect()
    };
    match order
        .into_iter()
        .find(|&i| re.is_match(&app.vc.history.ring[i]))
    {
        Some(i) => {
            if app.vc.history.pos.is_none() {
                let typed = current_body(app, id);
                app.vc.history.typed = Some(typed);
            }
            app.vc.history.pos = Some(i);
            let text = app.vc.history.ring[i].clone();
            set_body(app, id, &text);
            app.msg(format!("Message {} of {n}", i + 1));
        }
        None => app.error(format!("No earlier message matches {regexp}")),
    }
}

/// Turn a `Name: yes` header on or off.
fn toggle_header(app: &mut App, id: BufferId, name: &str) -> bool {
    let Some(b) = app.editor.state.get_buffer_mut(id) else {
        return false;
    };
    let msg = message_text(b);
    let (mut headers, body) = split_message(&msg);
    let on = match headers
        .iter()
        .position(|(k, _)| k.eq_ignore_ascii_case(name))
    {
        Some(i) => {
            let was = yes(&headers[i].1);
            if was {
                headers.remove(i);
            } else {
                headers[i].1 = "yes".into();
            }
            !was
        }
        None => {
            headers.push((name.to_string(), "yes".into()));
            true
        }
    };
    let head: String = headers.iter().map(|(k, v)| format!("{k}: {v}\n")).collect();
    let head = if head.is_empty() {
        head
    } else {
        format!("{head}\n")
    };
    let new = format!("{head}{body}\n");
    let end = b.protected_tail().unwrap_or(b.char_count());
    b.as_owner(|b| b.replace_offsets(0, end, &new));
    b.set_protected_tail(Some(new.chars().count()));
    let p = b.offset_to_pos(new.chars().count() - 1);
    b.set_point(p);
    decorate(b);
    on
}

/// The log-edit commands. False if `cmd` isn't one.
pub fn execute(app: &mut App, cmd: &str) -> bool {
    if !cmd.starts_with("log-edit-") {
        return false;
    }
    let id = app.active_id();
    let Some(s) = session(app, id) else {
        app.error("Not in a commit buffer (C-x v v starts one)");
        return true;
    };
    match cmd {
        "log-edit-done" => finish(app, id, false),
        "log-edit-kill" => {
            save_draft(app, id);
            app.vc.log_edits.remove(&id);
            restore_windows(app, &s, id);
            if let Some(b) = app.editor.state.get_buffer_mut(id) {
                b.set_modified(false);
            }
            app.kill_buffer(id);
            app.msg("Commit canceled; the message is kept for next time");
        }
        "log-edit-show-diff" => super::show_diff(app, &s.fileset, None, None, false),
        "log-edit-show-files" => {
            let list: Vec<String> = s
                .fileset
                .files
                .iter()
                .map(|(f, st)| format!("{f} ({})", st.label()))
                .collect();
            app.msg(format!("Committing: {}", list.join(", ")));
        }
        "log-edit-previous-comment" => step_history(app, id, true),
        "log-edit-next-comment" => step_history(app, id, false),
        "log-edit-comment-search-backward" | "log-edit-comment-search-forward" => {
            let backward = cmd.ends_with("backward");
            super::ask(
                app,
                if backward {
                    "Comment search backward (regexp): "
                } else {
                    "Comment search forward (regexp): "
                },
                super::Ask::LogEditSearch(id, backward),
                Vec::new(),
                "",
            );
        }
        "log-edit-toggle-amend" => {
            if !s.fileset.backend().caps().amend {
                app.error(format!("{} can't amend", s.fileset.kind.name()));
                return true;
            }
            let on = toggle_header(app, id, "Amend");
            if on
                && current_body(app, id).is_empty()
                && let Some(last) = s.fileset.backend().last_message(&s.fileset.root)
            {
                set_body(app, id, &last);
            }
            app.msg(if on {
                "Amending the last commit"
            } else {
                "Making a new commit"
            });
        }
        "log-edit-toggle-signoff" => {
            let on = toggle_header(app, id, "Sign-Off");
            app.msg(if on {
                "Adding a Signed-off-by line"
            } else {
                "No Signed-off-by line"
            });
        }
        "log-edit-toggle-no-verify" => {
            let on = toggle_header(app, id, "No-Verify");
            app.msg(if on {
                "Skipping the commit hooks"
            } else {
                "Running the commit hooks"
            });
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_are_split_off_only_when_known_and_followed_by_a_blank() {
        let (h, b) = split_message("Author: A <a@x>\nAmend: yes\n\nSummary\n\nBody\n");
        assert_eq!(
            h,
            vec![
                ("Author".into(), "A <a@x>".into()),
                ("Amend".into(), "yes".into())
            ]
        );
        assert_eq!(b, "Summary\n\nBody");
        let (o, _) = options(&h);
        assert!(o.amend);
        assert_eq!(o.author.as_deref(), Some("A <a@x>"));
        // "Fix: x" is a message, not a header.
        let (h, b) = split_message("Fix: the parser\n\nDetails");
        assert!(h.is_empty());
        assert_eq!(b, "Fix: the parser\n\nDetails");
        // A known name with text right after it is the message too.
        let (h, b) = split_message("Date: today\nmore text");
        assert!(h.is_empty());
        assert_eq!(b, "Date: today\nmore text");
    }

    #[test]
    fn conventions() {
        assert_eq!(check_format("Short\n\nBody"), None);
        assert!(
            check_format(&"x".repeat(80))
                .unwrap()
                .contains("80 characters")
        );
        assert!(
            check_format("Summary\nno blank")
                .unwrap()
                .contains("blank line")
        );
    }
}
