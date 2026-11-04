// SPDX-License-Identifier: MIT OR Apache-2.0
//! Logs: one line per revision, with its hash,
//! date, author and subject; RET (or TAB) shows the whole message.
//!
//! `n`/`p` move between entries; `d` (or `=`) diffs the entry with its
//! parent, or the two marked entries (`m`), or the entries in the region;
//! `D` shows the whole changeset; `w` diffs the entry against the working
//! tree; `f` visits the file as of that revision; `a` annotates it; `+`
//! shows twice as many entries; `g` reads the log again.

use super::backend::{Cmd, Kind, LogEntry};
use super::{Fileset, runner};
use crate::app::App;
use forge_core::decoration::{DecoFace, DecoSpec, VcFace};
use forge_core::{Buffer, BufferId, Pos};
use std::collections::BTreeSet;
use std::path::PathBuf;

pub const MODE: &str = "log-view-mode";
const DECO: &str = "log-view";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Log,
    Incoming,
    Outgoing,
    /// History of lines `from..=to` of the one file.
    Region {
        from: usize,
        to: usize,
    },
}

#[derive(Debug, Clone)]
pub struct LogView {
    pub kind: Kind,
    pub root: PathBuf,
    pub files: Vec<String>,
    pub whole: bool,
    pub limit: usize,
    pub start: Option<String>,
    pub source: Source,
    pub entries: Vec<LogEntry>,
    pub expanded: BTreeSet<usize>,
    pub marked: BTreeSet<usize>,
    /// Line of each entry's first line.
    pub entry_lines: Vec<usize>,
}

impl LogView {
    /// The entry whose text point's line is in.
    fn entry_at(&self, line: usize) -> Option<usize> {
        self.entry_lines.iter().rposition(|&l| l <= line)
    }
}

fn buffer_name(source: Source) -> &'static str {
    match source {
        Source::Log => "*vc-change-log*",
        Source::Incoming => "*vc-incoming*",
        Source::Outgoing => "*vc-outgoing*",
        Source::Region { .. } => "*VC-history*",
    }
}

/// Run the log and show it.
pub fn print_log(
    app: &mut App,
    fs: &Fileset,
    whole: bool,
    limit: usize,
    start: Option<String>,
    source: Source,
) {
    let b = fs.backend();
    let files = if whole { Vec::new() } else { fs.names() };
    let cmd = match source {
        Source::Log => Ok(b.log(&fs.root, &files, limit, start.as_deref())),
        Source::Incoming => b.incoming(&fs.root, None),
        Source::Outgoing => b.outgoing(&fs.root, None),
        Source::Region { from, to } => match files.first() {
            Some(f) => b.region_history(&fs.root, f, from, to),
            None => Err("Region history needs a file".into()),
        },
    };
    let cmd = match cmd {
        Ok(c) => c,
        Err(e) => return app.error(e),
    };
    let view = LogView {
        kind: fs.kind,
        root: fs.root.clone(),
        files,
        whole,
        limit,
        start,
        source,
        entries: Vec::new(),
        expanded: BTreeSet::new(),
        marked: BTreeSet::new(),
        entry_lines: Vec::new(),
    };
    // Incoming changes need the remote's state first (Git); Mercurial's
    // incoming asks the remote itself.
    if source == Source::Incoming
        && fs.kind == Kind::Git
        && let Ok(fetch) = b.fetch(&fs.root)
    {
        let line = fetch.line();
        app.msg("Fetching...");
        return runner::run(app, fetch, "Fetching", None, move |app, out| {
            if !out.success() {
                return runner::report_failure(app, &line, &out, true);
            }
            run_log(app, cmd, view, None);
        });
    }
    run_log(app, cmd, view, None);
}

fn run_log(app: &mut App, cmd: Cmd, view: LogView, keep_line: Option<usize>) {
    let line = cmd.line();
    let source = view.source;
    runner::run(app, cmd, "Reading log", None, move |app, out| {
        // Mercurial's incoming/outgoing exit 1 for "nothing".
        let nothing = matches!(source, Source::Incoming | Source::Outgoing)
            && view.kind == Kind::Hg
            && out.code == Some(1);
        if !out.success() && !nothing {
            return runner::report_failure(app, &line, &out, true);
        }
        let mut view = view;
        view.entries = view.kind.backend().parse_log(&out.stdout);
        if view.entries.is_empty() {
            app.msg(match source {
                Source::Incoming => "No incoming changes".to_string(),
                Source::Outgoing => "No outgoing changes".to_string(),
                _ => "No log entries".to_string(),
            });
            if matches!(source, Source::Incoming | Source::Outgoing) {
                return;
            }
        }
        if let Source::Region { .. } = source {
            view.expanded = (0..view.entries.len()).collect();
        }
        show(app, view, keep_line);
    });
}

/// Draw (or redraw) a log buffer.
fn show(app: &mut App, mut view: LogView, keep_line: Option<usize>) -> BufferId {
    let name = buffer_name(view.source);
    let id = match app.editor.find_buffer(name) {
        Some(id) => id,
        None => app.editor.add_buffer(Buffer::new(name)),
    };
    let (text, decos, lines) = render(&view);
    view.entry_lines = lines;
    let working = view.kind.backend().working_revision(&view.root);
    let b = app.editor.state.get_buffer_mut(id).expect("made");
    b.set_locked(false);
    b.set_contents(&text);
    b.set_major_mode(MODE);
    b.set_decorations(DECO, decos);
    b.set_locked(true);
    // Point on the working revision's entry when it's there, else the first.
    let line = keep_line.unwrap_or_else(|| {
        working
            .and_then(|w| view.entries.iter().position(|e| e.rev == w))
            .and_then(|i| view.entry_lines.get(i).copied())
            .unwrap_or(0)
    });
    let line = line.min(b.line_count().saturating_sub(1));
    b.set_point(Pos::new(line, 0));
    app.vc.logs.insert(id, view);
    if app.active_id() != id {
        crate::windows::pop_to_other_window(app);
        app.switch_to(id);
    }
    id
}

/// The buffer's text, its faces, and each entry's first line.
fn render(view: &LogView) -> (String, Vec<DecoSpec>, Vec<usize>) {
    let mut out = String::new();
    let mut decos: Vec<DecoSpec> = Vec::new();
    let mut lines = Vec::new();
    let mut offset = 0usize; // in chars
    let mut line_no = 0usize;
    let author_w = view
        .entries
        .iter()
        .map(|e| e.author.chars().count())
        .max()
        .unwrap_or(0)
        .min(20);
    let push = |out: &mut String, offset: &mut usize, line_no: &mut usize, s: &str| {
        out.push_str(s);
        out.push('\n');
        *offset += s.chars().count() + 1;
        *line_no += 1;
    };
    for (i, e) in view.entries.iter().enumerate() {
        lines.push(line_no);
        let mark = if view.marked.contains(&i) { "*" } else { " " };
        let author: String = e.author.chars().take(author_w).collect();
        let refs = if e.refs.is_empty() {
            String::new()
        } else {
            format!(" ({})", e.refs.join(", "))
        };
        let head = format!(
            "{mark} {}  {}  {author:<author_w$}  {}{refs}",
            e.short,
            e.date,
            e.subject()
        );
        // Faces by column.
        let start = offset;
        let mut col = 2;
        if view.marked.contains(&i) {
            decos.push(DecoSpec::new(
                start,
                start + 1,
                DecoFace::Vc(VcFace::VcDirMark),
            ));
        }
        for (len, f) in [
            (e.short.chars().count(), VcFace::LogViewRevision),
            (e.date.chars().count(), VcFace::LogViewDate),
            (author_w, VcFace::LogViewAuthor),
        ] {
            if len > 0 {
                decos.push(DecoSpec::new(
                    start + col,
                    start + col + len,
                    DecoFace::Vc(f),
                ));
            }
            col += len + 2;
        }
        let subj = e.subject().chars().count();
        if subj > 0 {
            decos.push(DecoSpec::new(
                start + col,
                start + col + subj,
                DecoFace::Vc(VcFace::LogViewMessage),
            ));
        }
        if !refs.is_empty() {
            let r = start + col + subj;
            decos.push(DecoSpec::new(
                r,
                r + refs.chars().count(),
                DecoFace::Vc(VcFace::LogViewRef),
            ));
        }
        push(&mut out, &mut offset, &mut line_no, &head);
        if view.expanded.contains(&i) {
            let detail = format!(
                "    Revision: {}\n    Author:   {} <{}>\n    Date:     {}{}",
                e.rev,
                e.author,
                e.email,
                e.date,
                if e.parents.is_empty() {
                    String::new()
                } else {
                    format!("\n    Parents:  {}", e.parents.join(" "))
                }
            );
            for l in detail.lines() {
                let s = offset;
                push(&mut out, &mut offset, &mut line_no, l);
                decos.push(DecoSpec::new(
                    s,
                    s + l.chars().count(),
                    DecoFace::Vc(VcFace::LogViewBody),
                ));
            }
            push(&mut out, &mut offset, &mut line_no, "");
            for l in e.message.lines() {
                let text = format!("    {l}");
                let s = offset;
                push(&mut out, &mut offset, &mut line_no, &text);
                decos.push(DecoSpec::new(
                    s,
                    s + text.chars().count(),
                    DecoFace::Vc(VcFace::LogViewBody),
                ));
            }
            if let Some(d) = &e.diff {
                push(&mut out, &mut offset, &mut line_no, "");
                for l in d.lines() {
                    let s = offset;
                    push(&mut out, &mut offset, &mut line_no, l);
                    let f = if l.starts_with("@@") {
                        Some(VcFace::DiffHunkHeader)
                    } else if l.starts_with("diff ")
                        || l.starts_with("--- ")
                        || l.starts_with("+++ ")
                    {
                        Some(VcFace::DiffFileHeader)
                    } else if l.starts_with('+') {
                        Some(VcFace::DiffAdded)
                    } else if l.starts_with('-') {
                        Some(VcFace::DiffRemoved)
                    } else {
                        None
                    };
                    if let Some(f) = f
                        && !l.is_empty()
                    {
                        decos.push(DecoSpec::new(s, s + l.chars().count(), DecoFace::Vc(f)));
                    }
                }
            }
            push(&mut out, &mut offset, &mut line_no, "");
        }
    }
    if view.limit > 0 && view.entries.len() >= view.limit && view.source == Source::Log {
        let more = format!(
            "-- Showing {} entries; + shows {} --",
            view.entries.len(),
            view.limit * 2
        );
        let s = offset;
        push(&mut out, &mut offset, &mut line_no, &more);
        decos.push(DecoSpec::new(
            s,
            s + more.chars().count(),
            DecoFace::Vc(VcFace::DiffNote),
        ));
    }
    (out, decos, lines)
}

fn current(app: &App) -> Option<(BufferId, LogView, Option<usize>)> {
    let id = app.active_id();
    let v = app.vc.logs.get(&id)?.clone();
    let line = app.buf_ref().point().line;
    let e = v.entry_at(line);
    Some((id, v, e))
}

fn redraw(app: &mut App, id: BufferId, view: LogView, entry: Option<usize>) {
    let (text, decos, lines) = render(&view);
    let mut view = view;
    view.entry_lines = lines;
    let line = entry
        .and_then(|i| view.entry_lines.get(i).copied())
        .unwrap_or(0);
    if let Some(b) = app.editor.state.get_buffer_mut(id) {
        b.set_locked(false);
        b.set_contents(&text);
        b.set_decorations(DECO, decos);
        b.set_locked(true);
        b.set_point(Pos::new(line.min(b.line_count().saturating_sub(1)), 0));
    }
    app.vc.logs.insert(id, view);
}

fn rerun(app: &mut App, view: LogView, keep: Option<usize>) {
    let b = view.kind.backend();
    let cmd = match view.source {
        Source::Log => Ok(b.log(&view.root, &view.files, view.limit, view.start.as_deref())),
        Source::Incoming => b.incoming(&view.root, None),
        Source::Outgoing => b.outgoing(&view.root, None),
        Source::Region { from, to } => b.region_history(&view.root, &view.files[0], from, to),
    };
    match cmd {
        Ok(c) => run_log(app, c, view, keep),
        Err(e) => app.error(e),
    }
}

/// The revision before entry `e` (its first parent).
fn parent_of(view: &LogView, e: &LogEntry) -> String {
    if let Some(p) = e.parents.first() {
        return p.clone();
    }
    match view.kind {
        // The root commit: against the empty tree.
        Kind::Git => "4b825dc642cb6eb9a060e54bf8d69288fbee4904".into(),
        Kind::Hg => "null".into(),
    }
}

/// C-x v h: the history of the lines in the region (or point's line).
pub fn region_history(app: &mut App) {
    let fs = match super::deduce_fileset(app, false) {
        Ok(f) => f,
        Err(e) => return app.error(e),
    };
    if fs.files.len() != 1 || !app.vc.files.contains_key(&app.active_id()) {
        return app.error("Region history works in a file's buffer");
    }
    let b = app.buf_ref();
    let (from, to) = match b.active_region() {
        Some((a, z)) => {
            // A region ending at the start of a line doesn't take that line.
            let last = if z.col == 0 && z.line > a.line {
                z.line - 1
            } else {
                z.line
            };
            (a.line + 1, last + 1)
        }
        None => (b.point().line + 1, b.point().line + 1),
    };
    print_log(app, &fs, false, 0, None, Source::Region { from, to });
}

/// The log-view commands. False if `cmd` isn't one.
pub fn execute(app: &mut App, cmd: &str) -> bool {
    if !cmd.starts_with("log-view-") {
        return false;
    }
    let Some((id, view, entry)) = current(app) else {
        app.error("Not in a log buffer");
        return true;
    };
    let n = app.count().unsigned_abs().max(1) as usize;
    match cmd {
        "log-view-next-entry" | "log-view-previous-entry" => {
            let fwd = cmd == "log-view-next-entry";
            let line = app.buf_ref().point().line;
            let here = view.entry_at(line);
            let target = match (fwd, here) {
                (true, None) => Some(n - 1),
                (true, Some(i)) => Some(i + n),
                (false, Some(i)) if view.entry_lines.get(i) == Some(&line) => i.checked_sub(n),
                (false, Some(i)) => Some(i + 1 - n.min(i + 1)),
                (false, None) => None,
            };
            match target.and_then(|t| view.entry_lines.get(t).copied()) {
                Some(l) => app.buf().set_point(Pos::new(l, 0)),
                None => app.error(if fwd {
                    "No next log entry"
                } else {
                    "No previous log entry"
                }),
            }
        }
        "log-view-toggle-entry-display" => {
            let Some(i) = entry else { return true };
            let mut v = view;
            if !v.expanded.remove(&i) {
                v.expanded.insert(i);
            }
            redraw(app, id, v, Some(i));
        }
        "log-view-toggle-mark-entry" => {
            let Some(i) = entry else { return true };
            let mut v = view;
            if !v.marked.remove(&i) {
                v.marked.insert(i);
            }
            redraw(
                app,
                id,
                v,
                Some(
                    (i + 1).min(
                        app.vc
                            .logs
                            .get(&id)
                            .map_or(i, |v| v.entries.len().saturating_sub(1)),
                    ),
                ),
            );
            // Move on, as marking does.
            let next = (i + 1).min(
                app.vc
                    .logs
                    .get(&id)
                    .map_or(0, |v| v.entries.len().saturating_sub(1)),
            );
            if let Some(l) = app
                .vc
                .logs
                .get(&id)
                .and_then(|v| v.entry_lines.get(next).copied())
            {
                app.buf().set_point(Pos::new(l, 0));
            }
        }
        "log-view-show-more" => {
            let mut v = view;
            if v.limit == 0 {
                app.msg("Showing every entry already");
                return true;
            }
            v.limit *= 2;
            let keep = app.buf_ref().point().line;
            rerun(app, v, Some(keep));
        }
        "log-view-refresh" => {
            let keep = app.buf_ref().point().line;
            rerun(app, view, Some(keep));
        }
        "log-view-diff" | "log-view-diff-changeset" | "log-view-diff-working" => {
            let fs = Fileset {
                kind: view.kind,
                root: view.root.clone(),
                files: if cmd == "log-view-diff-changeset" {
                    Vec::new()
                } else {
                    view.files
                        .iter()
                        .map(|f| (f.clone(), super::backend::FileState::UpToDate))
                        .collect()
                },
            };
            // Two marked entries, or a region over several: between them.
            let mut pair: Vec<usize> = view.marked.iter().copied().collect();
            if pair.len() != 2
                && let Some((a, z)) = app.buf_ref().active_region()
                && let (Some(x), Some(y)) = (view.entry_at(a.line), view.entry_at(z.line))
                && x != y
            {
                pair = vec![x, y];
            }
            if cmd == "log-view-diff-working" {
                let Some(i) = entry else { return true };
                let rev = view.entries[i].rev.clone();
                super::show_diff(app, &fs, Some(rev), None, false);
            } else if pair.len() == 2 && cmd == "log-view-diff" {
                // The later one in the log is the older revision.
                let (newer, older) = (pair[0].min(pair[1]), pair[0].max(pair[1]));
                let r1 = view.entries[older].rev.clone();
                let r2 = view.entries[newer].rev.clone();
                super::show_diff(app, &fs, Some(r1), Some(r2), false);
            } else {
                let Some(i) = entry else { return true };
                let e = &view.entries[i];
                let r1 = parent_of(&view, e);
                super::show_diff(app, &fs, Some(r1), Some(e.rev.clone()), false);
            }
        }
        "log-view-find-revision" | "log-view-annotate-version" => {
            let Some(i) = entry else { return true };
            let file = match view.files.as_slice() {
                [one] if !view.root.join(one).is_dir() => one.clone(),
                _ => {
                    app.error(
                        "This log isn't of one file: visit a revision from a file's log (C-x v l)",
                    );
                    return true;
                }
            };
            let rev = view.entries[i].rev.clone();
            if cmd == "log-view-find-revision" {
                super::show_revision(app, view.kind, &view.root, &file, &rev, true, None);
            } else {
                super::annotate::annotate(app, view.kind, &view.root, &file, Some(rev), None);
            }
        }
        _ => return false,
    }
    true
}
