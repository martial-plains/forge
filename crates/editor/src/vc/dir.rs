// SPDX-License-Identifier: MIT OR Apache-2.0
//! The status buffer: the repository's branch
//! and upstream at the top, then its files grouped by state.
//!
//! `n`/`p` move between files and TAB/S-TAB between groups; `m`/`u` mark
//! and unmark (the region's files with an active region), `M` marks every
//! file in the same state (`C-u M`: every file), `U` unmarks all and DEL
//! unmarks upward. Commands act on the marked files, or the file at point
//! (on a group's heading, every file in the group): `v` next action
//! (commit), `=` diff, `l` log, `i` register, `a`/`A` stage/unstage,
//! `d` deletes unregistered files, `G` ignores. `D`, `L`, `I`, `O`, `+`,
//! `P` work on the whole repository. `x` hides the files with nothing to
//! commit, `!` shows ignored files, `g` reads the status again, `?` lists
//! the keys.

use super::backend::{FileState, GROUP_ORDER, Kind, RepoInfo, StatusEntry};
use super::runner;
use crate::app::App;
use forge_core::decoration::{DecoFace, DecoSpec, VcFace};
use forge_core::{Buffer, BufferId, Pos};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub const MODE: &str = "vc-dir-mode";
const DECO: &str = "vc-dir";
pub const HELP_MAP: &str = "vc-dir-help";

#[derive(Debug, Clone)]
pub struct DirView {
    pub kind: Kind,
    pub root: PathBuf,
    /// The directory shown, relative to the root ("" for all of it).
    pub dir: String,
    pub entries: Vec<StatusEntry>,
    pub info: RepoInfo,
    pub marked: BTreeSet<String>,
    /// Files committed since the last `g`: shown as up to date until `x`.
    pub up_to_date: BTreeSet<String>,
    pub hide_unregistered: bool,
    pub show_ignored: bool,
    pub loading: bool,
    /// For each line, the entry on it.
    lines: Vec<Line>,
    /// The entries as shown, in order (what `Line::File` indexes).
    flat_cache: Vec<StatusEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Line {
    Header,
    Group(String),
    File(usize),
}

/// Open the status buffer for the repository `dir` is in.
pub fn open_dir(app: &mut App, dir: &Path) {
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let Some((kind, root)) = super::responsible(&dir) else {
        return app.error(format!(
            "{} is not under version control (M-x vc-create-repo makes a repository)",
            crate::files::abbreviate(&dir)
        ));
    };
    let rel = super::backend::relative(&root, &dir).unwrap_or_default();
    let existing = app
        .vc
        .dirs
        .iter()
        .find(|(_, v)| v.root == root && v.dir == rel)
        .map(|(id, _)| *id)
        .filter(|id| app.editor.state.get_buffer(*id).is_some());
    let id = match existing {
        Some(id) => id,
        None => {
            let name = app.editor.unique_name("*vc-dir*");
            let mut b = Buffer::new(&name);
            b.set_major_mode(MODE);
            b.set_locked(true);
            let id = app.editor.add_buffer(b);
            app.vc.dirs.insert(
                id,
                DirView {
                    kind,
                    root: root.clone(),
                    dir: rel,
                    entries: Vec::new(),
                    info: RepoInfo::default(),
                    marked: BTreeSet::new(),
                    up_to_date: BTreeSet::new(),
                    hide_unregistered: false,
                    show_ignored: false,
                    loading: true,
                    lines: Vec::new(),
                    flat_cache: Vec::new(),
                },
            );
            id
        }
    };
    app.switch_to(id);
    redraw(app, id);
    refresh(app, id);
}

/// Read the status again (asynchronously; the buffer says "Updating").
pub fn refresh(app: &mut App, id: BufferId) {
    let Some(v) = app.vc.dirs.get_mut(&id) else {
        return;
    };
    v.loading = true;
    let (kind, root, dir, ignored) = (v.kind, v.root.clone(), v.dir.clone(), v.show_ignored);
    redraw(app, id);
    let cmd = kind.backend().dir_status_cmd(&root, &dir, ignored);
    let line = cmd.line();
    runner::run(app, cmd, "Status", None, move |app, out| {
        let Some(v) = app.vc.dirs.get_mut(&id) else {
            return;
        };
        v.loading = false;
        if !out.success() {
            redraw(app, id);
            return runner::report_failure(app, &line, &out, true);
        }
        let (entries, info) = kind.backend().parse_dir_status(&root, &out.stdout);
        // A file committed since: up to date, unless it changed again.
        let changed: BTreeSet<String> = entries.iter().map(|e| e.path.clone()).collect();
        v.up_to_date.retain(|f| !changed.contains(f));
        v.marked
            .retain(|f| changed.contains(f) || v.up_to_date.contains(f));
        v.entries = entries;
        v.info = info;
        redraw(app, id);
    });
}

/// Refresh every status buffer showing `root`.
pub fn refresh_for_root(app: &mut App, root: &Path) {
    let ids: Vec<BufferId> = app
        .vc
        .dirs
        .iter()
        .filter(|(_, v)| v.root == root)
        .map(|(id, _)| *id)
        .collect();
    for id in ids {
        if app.editor.state.get_buffer(id).is_some() {
            refresh(app, id);
        } else {
            app.vc.dirs.remove(&id);
        }
    }
}

/// Files just committed show as up to date until the next `g` or `x`.
pub fn note_committed(app: &mut App, root: &Path, files: &[String]) {
    for v in app.vc.dirs.values_mut().filter(|v| v.root == root) {
        for f in files {
            v.up_to_date.insert(f.clone());
            v.marked.remove(f);
        }
    }
}

/// The entries in display order: group by group.
fn visible(v: &DirView) -> Vec<(String, Vec<StatusEntry>)> {
    let mut all: Vec<StatusEntry> = v.entries.clone();
    for f in &v.up_to_date {
        if !all.iter().any(|e| &e.path == f) {
            all.push(StatusEntry {
                path: f.clone(),
                state: FileState::UpToDate,
                extra: None,
            });
        }
    }
    let mut out = Vec::new();
    for g in GROUP_ORDER {
        let mut items: Vec<StatusEntry> = all
            .iter()
            .filter(|e| e.state.group() == *g)
            .filter(|e| !(v.hide_unregistered && e.state == FileState::Unregistered))
            .cloned()
            .collect();
        if items.is_empty() {
            continue;
        }
        items.sort_by(|a, b| a.path.cmp(&b.path));
        out.push((g.to_string(), items));
    }
    out
}

fn redraw(app: &mut App, id: BufferId) {
    let Some(v) = app.vc.dirs.get(&id).cloned() else {
        return;
    };
    let mut text = String::new();
    let mut decos: Vec<DecoSpec> = Vec::new();
    let mut lines: Vec<Line> = Vec::new();
    let mut offset = 0usize;
    let add = |text: &mut String, offset: &mut usize, lines: &mut Vec<Line>, s: &str, l: Line| {
        text.push_str(s);
        text.push('\n');
        *offset += s.chars().count() + 1;
        lines.push(l);
    };
    let header = |key: &str, val: &str| format!("{key:<12}: {val}");
    let mut head: Vec<(String, String)> = vec![
        ("VC backend".into(), v.kind.name().into()),
        (
            "Working dir".into(),
            format!(
                "{}/{}",
                crate::files::abbreviate(&v.root).trim_end_matches('/'),
                if v.dir.is_empty() {
                    String::new()
                } else {
                    format!("{}/", v.dir)
                }
            ),
        ),
    ];
    let branch = match (&v.info.branch, &v.info.head) {
        (Some(b), _) => b.clone(),
        (None, Some(h)) => format!("(detached at {h})"),
        (None, None) => "(no commits yet)".into(),
    };
    head.push(("Branch".into(), branch));
    if let Some(up) = &v.info.upstream {
        let ab = match (v.info.ahead, v.info.behind) {
            (Some(0), Some(0)) => " (up to date)".to_string(),
            (Some(a), Some(b)) => {
                let mut parts = Vec::new();
                if a > 0 {
                    parts.push(format!("{a} ahead"));
                }
                if b > 0 {
                    parts.push(format!("{b} behind"));
                }
                format!(" ({})", parts.join(", "))
            }
            _ => String::new(),
        };
        head.push(("Tracking".into(), format!("{up}{ab}")));
    } else if v.kind == Kind::Git && v.info.branch.is_some() {
        head.push((
            "Tracking".into(),
            "(no upstream; P asks where to push)".into(),
        ));
    }
    if let Some(url) = &v.info.remote_url {
        head.push(("Remote".into(), url.clone()));
    }
    if v.info.stashes > 0 {
        head.push((
            "Stash".into(),
            format!(
                "{} entr{} (z l lists, z p pops)",
                v.info.stashes,
                if v.info.stashes == 1 { "y" } else { "ies" }
            ),
        ));
    }
    if let Some(op) = &v.info.operation {
        head.push(("In progress".into(), format!("{op} (resolve, then commit)")));
    }
    if v.loading {
        head.push(("Status".into(), "Updating...".into()));
    }
    for (k, val) in &head {
        let s = header(k, val);
        let start = offset;
        decos.push(DecoSpec::new(
            start,
            start + 12,
            DecoFace::Vc(VcFace::VcDirHeader),
        ));
        decos.push(DecoSpec::new(
            start + 14,
            start + s.chars().count(),
            DecoFace::Vc(VcFace::VcDirHeaderValue),
        ));
        add(&mut text, &mut offset, &mut lines, &s, Line::Header);
    }
    add(&mut text, &mut offset, &mut lines, "", Line::Header);
    let groups = visible(&v);
    if groups.is_empty() && !v.loading {
        let s = "  Nothing to commit: the working tree is clean";
        let start = offset;
        decos.push(DecoSpec::new(
            start,
            start + s.chars().count(),
            DecoFace::Vc(VcFace::VcDirUpToDate),
        ));
        add(&mut text, &mut offset, &mut lines, s, Line::Header);
    }
    let mut flat: Vec<StatusEntry> = Vec::new();
    for (g, items) in &groups {
        let s = format!("{g} ({})", items.len());
        let start = offset;
        decos.push(DecoSpec::new(
            start,
            start + s.chars().count(),
            DecoFace::Vc(VcFace::VcDirDirectory),
        ));
        add(
            &mut text,
            &mut offset,
            &mut lines,
            &s,
            Line::Group(g.clone()),
        );
        for e in items {
            let idx = flat.len();
            flat.push(e.clone());
            let marked = v.marked.contains(&e.path);
            let mark = if marked { '*' } else { ' ' };
            let label = e.state.label();
            let extra = e
                .extra
                .as_ref()
                .map(|x| format!("  ({x})"))
                .unwrap_or_default();
            let s = format!("  {mark} {label:<13} {}{extra}", e.path);
            let start = offset;
            if marked {
                decos.push(DecoSpec::new(
                    start + 2,
                    start + 3,
                    DecoFace::Vc(VcFace::VcDirMark),
                ));
            }
            let face = match e.state {
                FileState::Edited
                | FileState::NeedsMerge
                | FileState::NeedsUpdate
                | FileState::LockedBy(_) => VcFace::VcDirEdited,
                FileState::Added => VcFace::VcDirAdded,
                FileState::Removed | FileState::Missing | FileState::Conflict => {
                    VcFace::VcDirWarning
                }
                FileState::UpToDate => VcFace::VcDirUpToDate,
                FileState::Ignored => VcFace::VcDirIgnored,
                FileState::Unregistered => VcFace::VcDirUnregistered,
            };
            decos.push(DecoSpec::new(
                start + 4,
                start + 4 + label.chars().count(),
                DecoFace::Vc(face),
            ));
            let p = start + 18;
            decos.push(DecoSpec::new(
                p,
                p + e.path.chars().count(),
                DecoFace::Vc(VcFace::VcDirFile),
            ));
            add(&mut text, &mut offset, &mut lines, &s, Line::File(idx));
        }
    }
    // Point stays on the same file (or line).
    let (old_line, old_file) = match app.editor.state.get_buffer(id) {
        Some(b) => {
            let l = b.point().line;
            let f = app.vc.dirs.get(&id).and_then(|old| match old.lines.get(l) {
                Some(Line::File(i)) => old_flat(old).get(*i).map(|e| e.path.clone()),
                _ => None,
            });
            (l, f)
        }
        None => (0, None),
    };
    let target = old_file
        .and_then(|f| {
            lines
                .iter()
                .position(|l| matches!(l, Line::File(i) if flat[*i].path == f))
        })
        .or_else(|| {
            // The first file, on a fresh buffer.
            (old_line == 0)
                .then(|| lines.iter().position(|l| matches!(l, Line::File(_))))
                .flatten()
        })
        .unwrap_or(old_line);
    if let Some(dv) = app.vc.dirs.get_mut(&id) {
        dv.lines = lines;
        dv.flat_cache = flat;
    }
    if let Some(b) = app.editor.state.get_buffer_mut(id) {
        b.set_locked(false);
        b.set_contents(&text);
        b.set_decorations(DECO, decos);
        b.set_locked(true);
        let t = target.min(b.line_count().saturating_sub(1));
        b.set_point(Pos::new(t, 4));
    }
}

fn old_flat(v: &DirView) -> &[StatusEntry] {
    &v.flat_cache
}

impl DirView {
    fn entry_on(&self, line: usize) -> Option<&StatusEntry> {
        match self.lines.get(line)? {
            Line::File(i) => self.flat_cache.get(*i),
            _ => None,
        }
    }

    fn group_of_line(&self, line: usize) -> Option<&str> {
        match self.lines.get(line)? {
            Line::Group(g) => Some(g),
            _ => None,
        }
    }
}

/// The files a command acts on: the marked ones, else the one at point
/// (every file of the group, on its heading).
pub fn selected(app: &App) -> Vec<(String, FileState)> {
    let id = app.active_id();
    let Some(v) = app.vc.dirs.get(&id) else {
        return Vec::new();
    };
    let all: Vec<&StatusEntry> = v.flat_cache.iter().collect();
    if !v.marked.is_empty() {
        return all
            .iter()
            .filter(|e| v.marked.contains(&e.path))
            .map(|e| (e.path.clone(), e.state.clone()))
            .collect();
    }
    let line = app.buf_ref().point().line;
    if let Some(e) = v.entry_on(line) {
        return vec![(e.path.clone(), e.state.clone())];
    }
    if let Some(g) = v.group_of_line(line) {
        return all
            .iter()
            .filter(|e| e.state.group() == g)
            .map(|e| (e.path.clone(), e.state.clone()))
            .collect();
    }
    Vec::new()
}

fn file_lines(v: &DirView) -> Vec<usize> {
    v.lines
        .iter()
        .enumerate()
        .filter(|(_, l)| matches!(l, Line::File(_)))
        .map(|(i, _)| i)
        .collect()
}

fn move_lines(app: &mut App, forward: bool, n: usize, groups: bool) {
    let id = app.active_id();
    let Some(v) = app.vc.dirs.get(&id) else {
        return;
    };
    let targets: Vec<usize> = if groups {
        v.lines
            .iter()
            .enumerate()
            .filter(|(_, l)| matches!(l, Line::Group(_)))
            .map(|(i, _)| i)
            .collect()
    } else {
        file_lines(v)
    };
    let here = app.buf_ref().point().line;
    let mut at = here;
    let mut moved = false;
    for _ in 0..n.max(1) {
        let next = if forward {
            targets.iter().copied().find(|&t| t > at)
        } else {
            targets.iter().rev().copied().find(|&t| t < at)
        };
        match next {
            Some(t) => {
                at = t;
                moved = true;
            }
            None => break,
        }
    }
    if moved {
        app.buf()
            .set_point(Pos::new(at, if groups { 0 } else { 4 }));
    } else {
        app.error(if forward {
            "No next line"
        } else {
            "No previous line"
        });
    }
}

fn mark(app: &mut App, on: bool) {
    let id = app.active_id();
    let region = app.buf_ref().active_region();
    let line = app.buf_ref().point().line;
    let Some(v) = app.vc.dirs.get_mut(&id) else {
        return;
    };
    let lines: Vec<usize> = match region {
        Some((a, z)) => (a.line..=z.line).collect(),
        None => vec![line],
    };
    let mut files: Vec<String> = Vec::new();
    for l in &lines {
        if let Some(e) = v.entry_on(*l) {
            files.push(e.path.clone());
        } else if let Some(g) = v.group_of_line(*l).map(str::to_string) {
            files.extend(
                v.flat_cache
                    .iter()
                    .filter(|e| e.state.group() == g)
                    .map(|e| e.path.clone()),
            );
        }
    }
    for f in files {
        if on {
            v.marked.insert(f);
        } else {
            v.marked.remove(&f);
        }
    }
    redraw(app, id);
    if region.is_some() {
        app.buf().deactivate_mark();
    } else {
        move_lines_quiet(app, line);
    }
}

fn move_lines_quiet(app: &mut App, from: usize) {
    let id = app.active_id();
    let Some(v) = app.vc.dirs.get(&id) else {
        return;
    };
    if let Some(t) = file_lines(v).into_iter().find(|&t| t > from) {
        app.buf().set_point(Pos::new(t, 4));
    }
}

/// The vc-dir commands. False if `cmd` isn't one.
pub fn execute(app: &mut App, cmd: &str) -> bool {
    if !cmd.starts_with("vc-dir-") {
        return false;
    }
    let id = app.active_id();
    if !app.vc.dirs.contains_key(&id) {
        app.error("Not in a vc-dir buffer (C-x v d)");
        return true;
    }
    let n = app.count().unsigned_abs() as usize;
    match cmd {
        "vc-dir-next-line" => move_lines(app, true, n, false),
        "vc-dir-previous-line" => move_lines(app, false, n, false),
        "vc-dir-next-group" => move_lines(app, true, n, true),
        "vc-dir-previous-group" => move_lines(app, false, n, true),
        "vc-dir-copy-file-names" => {
            let names: Vec<String> = selected(app).into_iter().map(|(f, _)| f).collect();
            if names.is_empty() {
                app.error("No files to copy");
            } else {
                let n = names.len();
                app.kill_ring_save(names.join("\n"));
                app.msg(format!("Copied {n} file name(s)"));
            }
        }
        "vc-dir-mark" => mark(app, true),
        "vc-dir-unmark" => mark(app, false),
        "vc-dir-unmark-file-up" => {
            let line = app.buf_ref().point().line;
            let Some(v) = app.vc.dirs.get_mut(&id) else {
                return true;
            };
            let prev = file_lines(v).into_iter().rev().find(|&t| t < line);
            if let Some(p) = prev {
                if let Some(f) = v.entry_on(p).map(|e| e.path.clone()) {
                    v.marked.remove(&f);
                }
                redraw(app, id);
                app.buf().set_point(Pos::new(p, 4));
            }
        }
        "vc-dir-mark-all-files" => {
            let all = app.has_arg();
            let line = app.buf_ref().point().line;
            let Some(v) = app.vc.dirs.get_mut(&id) else {
                return true;
            };
            let state = v.entry_on(line).map(|e| e.state.clone());
            let files: Vec<String> = v
                .flat_cache
                .iter()
                .filter(|e| all || Some(&e.state) == state.as_ref())
                .map(|e| e.path.clone())
                .collect();
            if files.is_empty() {
                app.error("Move to a file first (or C-u M marks every file)");
                return true;
            }
            v.marked.extend(files);
            redraw(app, id);
        }
        "vc-dir-unmark-all-files" => {
            if let Some(v) = app.vc.dirs.get_mut(&id) {
                v.marked.clear();
            }
            redraw(app, id);
        }
        "vc-dir-refresh" => {
            if let Some(v) = app.vc.dirs.get_mut(&id) {
                v.up_to_date.clear();
                v.hide_unregistered = false;
            }
            refresh(app, id);
        }
        "vc-dir-hide-up-to-date" => {
            let arg = app.has_arg();
            if let Some(v) = app.vc.dirs.get_mut(&id) {
                v.up_to_date.clear();
                if arg {
                    v.hide_unregistered = true;
                }
            }
            redraw(app, id);
        }
        "vc-dir-toggle-ignored" => {
            let on = match app.vc.dirs.get_mut(&id) {
                Some(v) => {
                    v.show_ignored = !v.show_ignored;
                    v.show_ignored
                }
                None => return true,
            };
            app.msg(if on {
                "Showing ignored files"
            } else {
                "Hiding ignored files"
            });
            refresh(app, id);
        }
        "vc-dir-find-file" | "vc-dir-find-file-other-window" => {
            let line = app.buf_ref().point().line;
            let Some(v) = app.vc.dirs.get(&id) else {
                return true;
            };
            let Some(e) = v.entry_on(line) else {
                app.error("No file on this line");
                return true;
            };
            let path = v.root.join(&e.path);
            if path.is_dir() {
                crate::dired::open(app, &path);
                return true;
            }
            match app.editor.open_file(&path) {
                Ok((fid, _)) => {
                    if cmd.ends_with("other-window") {
                        crate::windows::pop_to_other_window(app);
                    }
                    app.switch_to(fid);
                }
                Err(err) => app.error(format!("Cannot open {}: {err}", path.display())),
            }
        }
        "vc-dir-clean-files" => {
            let files = selected(app);
            if files.is_empty() {
                app.error("No files selected");
                return true;
            }
            if files.iter().any(|(_, s)| s.registered()) {
                app.error(
                    "Only unregistered files are deleted here (C-x v x deletes a tracked file)",
                );
                return true;
            }
            let Some(v) = app.vc.dirs.get(&id) else {
                return true;
            };
            let fs = super::Fileset {
                kind: v.kind,
                root: v.root.clone(),
                files: files.clone(),
            };
            let p = match files.as_slice() {
                [(one, _)] => format!("Delete {one} from disk? (y or n) "),
                many => format!(
                    "Delete {} unregistered files from disk? (y or n) ",
                    many.len()
                ),
            };
            app.prompt(
                p,
                crate::minibuffer::Purpose::Confirm(crate::minibuffer::ConfirmAction::Vc(
                    super::Confirm::CleanFiles(fs),
                )),
            );
        }
        "vc-dir-stage" | "vc-dir-unstage" => {
            let files: Vec<String> = selected(app).into_iter().map(|(f, _)| f).collect();
            if files.is_empty() {
                app.error("No files selected");
                return true;
            }
            let Some(v) = app.vc.dirs.get(&id) else {
                return true;
            };
            let (kind, root) = (v.kind, v.root.clone());
            let stage = cmd == "vc-dir-stage";
            let r = if stage {
                kind.backend().stage(&root, &files)
            } else {
                kind.backend().unstage(&root, &files)
            };
            match r {
                Ok(c) => {
                    let what = if stage { "Staged" } else { "Unstaged" };
                    let n = files.len();
                    super::run_then_refresh(
                        app,
                        c,
                        what,
                        &root,
                        format!("{what} {n} file{}", if n == 1 { "" } else { "s" }),
                    );
                }
                Err(e) => app.error(e),
            }
        }
        "vc-dir-help" => {
            use forge_core::keymap::{OnUnbound, TransientOpts};
            app.editor.push_transient(
                HELP_MAP,
                TransientOpts::default()
                    .on_unbound(OnUnbound::Swallow)
                    .one_shot(true),
            );
            app.msg("vc-dir keys: press one to run it, C-g or q to close");
        }
        _ => return false,
    }
    true
}

/// The help overlay's map: vc-dir's keys, and the ways out.
pub fn install_help_map(keymaps: &mut forge_core::Keymaps) {
    use forge_core::keymap::{Origin, Scope};
    let scope = Scope::Transient(HELP_MAP.into());
    for (k, c) in super::DIR_BINDINGS
        .iter()
        .filter(|(_, c)| *c != "vc-dir-help")
    {
        keymaps.bind(&scope, k, c, Origin::Package).expect("valid");
    }
    for k in ["C-g", "q", "?"] {
        keymaps
            .bind(&scope, k, "keyboard-quit", Origin::Package)
            .expect("valid");
    }
}
