// SPDX-License-Identifier: MIT OR Apache-2.0
//! Resolving conflicts. A file with conflict
//! markers turns smerge-mode on when it is visited, and its conflicts are
//! highlighted: upper (mine), base (when the markers have one) and lower
//! (theirs).
//!
//! `C-c ^ n`/`p` move between conflicts; `C-c ^ u` keeps the upper
//! version, `C-c ^ l` the lower, `C-c ^ b` the base, `C-c ^ a` all of
//! them and `C-c ^ RET` the one point is in (`m` and `o` are the old names
//! for `u` and `l`). `C-c ^ R` highlights the words that differ, and
//! `C-c ^ E` lays out the three versions above the file (a three-way
//! compare). Once no conflict is left, smerge-mode turns itself off; saving
//! the file then marks it resolved with version control.

use super::backend::FileState;
use crate::app::App;
use forge_core::decoration::{DecoFace, DecoSpec, VcFace};
use forge_core::{Buffer, BufferId, Pos};
use std::path::Path;

pub const MODE: &str = "smerge-mode";
const DECO: &str = "smerge";
const DECO_REFINE: &str = "smerge-refine";

/// One conflict's lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Conflict {
    /// `<<<<<<<`
    pub start: usize,
    /// `|||||||`, if the markers have a base.
    pub base: Option<usize>,
    /// `=======`
    pub sep: usize,
    /// `>>>>>>>`
    pub end: usize,
}

impl Conflict {
    pub fn upper(&self) -> std::ops::Range<usize> {
        self.start + 1..self.base.unwrap_or(self.sep)
    }

    pub fn base_lines(&self) -> Option<std::ops::Range<usize>> {
        self.base.map(|b| b + 1..self.sep)
    }

    pub fn lower(&self) -> std::ops::Range<usize> {
        self.sep + 1..self.end
    }
}

/// The conflicts in `lines`, in order.
pub fn find_conflicts(lines: &[String]) -> Vec<Conflict> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].starts_with("<<<<<<<") {
            let start = i;
            let mut base = None;
            let mut sep = None;
            let mut j = i + 1;
            while j < lines.len() {
                let l = &lines[j];
                if l.starts_with("|||||||") && sep.is_none() && base.is_none() {
                    base = Some(j);
                } else if l.starts_with("=======") && sep.is_none() {
                    sep = Some(j);
                } else if l.starts_with(">>>>>>>")
                    && let Some(sep) = sep
                {
                    out.push(Conflict {
                        start,
                        base,
                        sep,
                        end: j,
                    });
                    break;
                } else if l.starts_with("<<<<<<<") {
                    // An unterminated one: start again here.
                    j -= 1;
                    break;
                }
                j += 1;
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}

fn lines_of(b: &Buffer) -> Vec<String> {
    (0..b.line_count()).map(|l| b.line(l)).collect()
}

pub fn file_has_markers(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .map(|t| {
            let lines: Vec<String> = t.lines().map(str::to_string).collect();
            !find_conflicts(&lines).is_empty()
        })
        .unwrap_or(false)
}

/// Turn smerge-mode on in a buffer that has conflict markers (`announce`:
/// say so in the echo area).
pub fn maybe_enable(app: &mut App, id: BufferId, announce: bool) {
    let Some(b) = app.editor.state.get_buffer_mut(id) else {
        return;
    };
    let lines = lines_of(b);
    let conflicts = find_conflicts(&lines);
    if conflicts.is_empty() {
        return;
    }
    if !b.minor_mode_enabled(MODE) {
        b.enable_minor_mode(MODE);
    }
    decorate(b);
    if announce || app.active_id() == id {
        let n = conflicts.len();
        app.msg(format!(
            "{n} conflict{} here: C-c ^ n goes to the next, C-c ^ u/l keeps upper/lower, C-c ^ E compares",
            if n == 1 { "" } else { "s" }
        ));
    }
}

/// Faces on every conflict.
pub fn decorate(b: &mut Buffer) {
    let lines = lines_of(b);
    let mut decos = Vec::new();
    let start_of = |l: usize| b.pos_to_offset(Pos::new(l, 0));
    let span = |r: std::ops::Range<usize>, f: VcFace, decos: &mut Vec<DecoSpec>| {
        for l in r {
            let s = start_of(l);
            let len = lines[l].chars().count();
            if len > 0 {
                decos.push(DecoSpec::new(s, s + len, DecoFace::Vc(f)));
            }
        }
    };
    for c in find_conflicts(&lines) {
        let mut markers = vec![c.start, c.sep, c.end];
        markers.extend(c.base);
        for m in markers {
            span(m..m + 1, VcFace::SmergeMarkers, &mut decos);
        }
        span(c.upper(), VcFace::SmergeUpper, &mut decos);
        if let Some(r) = c.base_lines() {
            span(r, VcFace::SmergeBase, &mut decos);
        }
        span(c.lower(), VcFace::SmergeLower, &mut decos);
    }
    b.set_decorations(DECO, decos);
}

/// After each command in an smerge buffer: faces follow the edits, and the
/// mode goes when the last conflict does.
pub fn after_command(app: &mut App) {
    let b = app.buf();
    let lines = lines_of(b);
    if find_conflicts(&lines).is_empty() {
        b.disable_minor_mode(MODE);
        b.clear_decorations(DECO);
        b.clear_decorations(DECO_REFINE);
        let resolved_here = app
            .vc
            .files
            .get(&app.active_id())
            .is_some_and(|i| i.state == FileState::Conflict);
        if app.message.is_none() || resolved_here {
            app.msg(if resolved_here {
                "No conflicts left: save, and the file is marked resolved"
            } else {
                "No conflicts left"
            });
        }
        return;
    }
    decorate(b);
}

/// When a conflicted file is saved with no markers left, mark it resolved
///.
pub fn after_save(app: &mut App, id: BufferId) {
    let Some(info) = app.vc.files.get(&id).cloned() else {
        return;
    };
    if info.state != FileState::Conflict {
        return;
    }
    let Some(b) = app.editor.state.get_buffer(id) else {
        return;
    };
    if !find_conflicts(&lines_of(b)).is_empty() {
        return;
    }
    let cmd = info
        .kind
        .backend()
        .mark_resolved(&info.root, std::slice::from_ref(&info.rel));
    super::run_then_refresh(
        app,
        cmd,
        "Resolving",
        &info.root,
        format!("Marked {} as resolved", info.rel),
    );
}

fn conflict_at(b: &Buffer) -> Option<Conflict> {
    let line = b.point().line;
    find_conflicts(&lines_of(b))
        .into_iter()
        .find(|c| c.start <= line && line <= c.end)
}

/// Move to the next (or previous) conflict. False if there is none.
pub fn next_conflict(app: &mut App, forward: bool, from_top: bool) -> bool {
    let b = app.buf_ref();
    let line = if from_top { 0 } else { b.point().line };
    let cs = find_conflicts(&lines_of(b));
    let target = if forward {
        cs.iter().find(|c| {
            if from_top {
                c.start >= line
            } else {
                c.start > line
            }
        })
    } else {
        cs.iter().rev().find(|c| c.start < line)
    };
    match target {
        Some(c) => {
            let l = c.start;
            app.buf().set_point(Pos::new(l, 0));
            true
        }
        None => {
            app.error(if forward {
                "No next conflict"
            } else {
                "No previous conflict"
            });
            false
        }
    }
}

/// Replace the conflict at point with the lines `pick` chooses.
fn keep(app: &mut App, pick: impl Fn(&Conflict) -> Vec<std::ops::Range<usize>>, what: &str) {
    let b = app.buf_ref();
    let Some(c) = conflict_at(b) else {
        return app.error("No conflict here (C-c ^ n moves to the next)");
    };
    let lines = lines_of(b);
    let ranges = pick(&c);
    if ranges.is_empty() {
        return app.error(format!("This conflict has no {what} version"));
    }
    let mut keep: Vec<String> = Vec::new();
    for r in ranges {
        keep.extend(lines[r].iter().cloned());
    }
    let a = b.pos_to_offset(Pos::new(c.start, 0));
    let z = if c.end + 1 >= b.line_count() {
        b.char_count()
    } else {
        b.pos_to_offset(Pos::new(c.end + 1, 0))
    };
    let ends_nl = c.end + 1 < b.line_count();
    let mut text = keep.join("\n");
    if !keep.is_empty() && ends_nl {
        text.push('\n');
    }
    let b = app.buf();
    b.replace_offsets(a, z, &text);
    b.set_point(Pos::new(c.start, 0));
    b.clear_decorations(DECO_REFINE);
    // On to the next one, as smerge-auto-advance... does by hand.
    let left = find_conflicts(&lines_of(app.buf_ref())).len();
    if left > 0 {
        app.msg(format!(
            "Kept the {what} version; {left} conflict{} left",
            if left == 1 { "" } else { "s" }
        ));
    }
}

/// C-c ^ R: the words that differ between the sides.
fn refine(app: &mut App) {
    let b = app.buf_ref();
    let Some(c) = conflict_at(b) else {
        return app.error("No conflict here");
    };
    let lines = lines_of(b);
    let starts: Vec<usize> = (0..lines.len())
        .map(|l| b.pos_to_offset(Pos::new(l, 0)))
        .collect();
    let mut decos = Vec::new();
    super::diff::refine_pair(
        &lines,
        &starts,
        c.upper(),
        c.lower(),
        (VcFace::SmergeRefinedRemoved, VcFace::SmergeRefinedAdded),
        0,
        &mut decos,
    );
    app.buf().set_decorations(DECO_REFINE, decos);
}

/// A three-way compare in progress.
#[derive(Debug, Clone)]
pub struct MergeSession {
    pub result: BufferId,
    pub sides: Vec<BufferId>,
}

/// The text with every conflict resolved to one side (0 upper, 1 base,
/// 2 lower), for files version control can't give the sides of.
fn side_from_markers(lines: &[String], side: u8) -> String {
    let cs = find_conflicts(lines);
    let mut out = Vec::new();
    let mut i = 0;
    for c in cs {
        out.extend(lines[i..c.start].iter().cloned());
        let r = match side {
            0 => Some(c.upper()),
            1 => c.base_lines(),
            _ => Some(c.lower()),
        };
        if let Some(r) = r {
            out.extend(lines[r].iter().cloned());
        }
        i = c.end + 1;
    }
    out.extend(lines[i..].iter().cloned());
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// C-c ^ E: the upper, base and lower versions side by side above the
/// file, which stays where the conflicts are resolved.
fn ediff(app: &mut App) {
    let result = app.active_id();
    let b = app.buf_ref();
    let lines = lines_of(b);
    let base_name = b.name().to_string();
    let mode = b.major_mode().to_string();
    let versions = app.vc.files.get(&result).and_then(|i| {
        i.kind
            .backend()
            .conflict_versions(&i.root, &i.rel)
            .ok()
            .map(|v| (v.mine, v.base, v.theirs))
    });
    let (upper, base, lower) = match versions {
        Some(v) => v,
        None => {
            if find_conflicts(&lines).is_empty() {
                return app.error("No conflicts in this buffer");
            }
            let has_base = find_conflicts(&lines).iter().any(|c| c.base.is_some());
            (
                side_from_markers(&lines, 0),
                has_base.then(|| side_from_markers(&lines, 1)),
                side_from_markers(&lines, 2),
            )
        }
    };
    let make = |app: &mut App, label: &str, text: &str| -> BufferId {
        let name = format!("*{label}: {base_name}*");
        let id = match app.editor.find_buffer(&name) {
            Some(id) => id,
            None => app.editor.add_buffer(Buffer::new(&name)),
        };
        let b = app.editor.state.get_buffer_mut(id).expect("made");
        b.set_locked(false);
        b.set_contents(text);
        b.set_major_mode(mode.clone());
        b.set_locked(true);
        app.editor.sync_language(id);
        id
    };
    let up = make(app, "upper (mine)", &upper);
    let low = make(app, "lower (theirs)", &lower);
    let bs = base.as_deref().map(|t| make(app, "base", t));
    // Highlight what each side changed against the base (or the other).
    let reference = base.clone().unwrap_or_else(|| lower.clone());
    mark_changes(app, up, &reference, &upper, VcFace::SmergeUpper);
    mark_changes(
        app,
        low,
        &base.clone().unwrap_or(upper.clone()),
        &lower,
        VcFace::SmergeLower,
    );
    // Layout: the three on top, the result below.
    app.editor.delete_other_windows();
    let result_win = app.editor.windows.selected_id();
    if let Err(e) = app.editor.split_window(forge_core::window::Direction::Rows) {
        return app.error(e);
    }
    let top = result_win;
    app.select_window(top);
    app.switch_to(up);
    let mut tops = vec![up];
    if let Some(b) = bs {
        tops.push(b);
    }
    tops.push(low);
    for buf in tops.iter().skip(1) {
        if app
            .editor
            .split_window(forge_core::window::Direction::Columns)
            .is_err()
        {
            break;
        }
        let next = app.editor.windows.nth_from_selected(1);
        app.select_window(next);
        app.switch_to(*buf);
    }
    // The result: the window left below.
    let bottom = app
        .editor
        .windows
        .iter()
        .find(|w| !tops.contains(&w.buffer))
        .map(|w| w.id);
    if let Some(w) = bottom {
        app.select_window(w);
    }
    app.switch_to(result);
    let _ = app.editor.windows.selected_id();
    app.vc.merge = Some(MergeSession {
        result,
        sides: tops,
    });
    let _ = next_conflict(app, true, true);
    app.msg("Upper, base and lower above; resolve below (C-c ^ u/l/b/a), then save");
}

/// Color the lines of `text` that differ from `reference`.
fn mark_changes(app: &mut App, id: BufferId, reference: &str, text: &str, face: VcFace) {
    let a: Vec<&str> = reference.lines().collect();
    let t: Vec<&str> = text.lines().collect();
    let ops = super::diff::diff_seq(&a, &t);
    let Some(b) = app.editor.state.get_buffer_mut(id) else {
        return;
    };
    let mut decos = Vec::new();
    for op in ops {
        if let super::diff::Op::Insert { b: bi, len } = op {
            for l in bi..bi + len {
                let s = b.pos_to_offset(Pos::new(l, 0));
                let n = b.line_len(l);
                if n > 0 {
                    decos.push(DecoSpec::new(s, s + n, DecoFace::Vc(face)));
                }
            }
        }
    }
    b.set_decorations(DECO, decos);
}

/// The smerge commands. False if `cmd` isn't one.
pub fn execute(app: &mut App, cmd: &str) -> bool {
    if !cmd.starts_with("smerge-") {
        return false;
    }
    match cmd {
        "smerge-mode" => {
            let id = app.active_id();
            let on = app.buf_ref().minor_mode_enabled(MODE);
            if on {
                let b = app.buf();
                b.disable_minor_mode(MODE);
                b.clear_decorations(DECO);
                b.clear_decorations(DECO_REFINE);
                app.msg("smerge-mode disabled");
            } else {
                maybe_enable(app, id, true);
                if !app.buf_ref().minor_mode_enabled(MODE) {
                    app.msg("No conflict markers in this buffer");
                }
            }
        }
        "smerge-next" => {
            let _ = next_conflict(app, true, false);
        }
        "smerge-prev" => {
            let _ = next_conflict(app, false, false);
        }
        "smerge-keep-upper" => keep(app, |c| vec![c.upper()], "upper"),
        "smerge-keep-lower" => keep(app, |c| vec![c.lower()], "lower"),
        "smerge-keep-base" => keep(app, |c| c.base_lines().into_iter().collect(), "base"),
        "smerge-keep-all" => keep(
            app,
            |c| {
                let mut v = vec![c.upper()];
                v.extend(c.base_lines());
                v.push(c.lower());
                v
            },
            "every",
        ),
        "smerge-keep-current" => {
            let line = app.buf_ref().point().line;
            let Some(c) = conflict_at(app.buf_ref()) else {
                app.error("No conflict here");
                return true;
            };
            if c.upper().contains(&line) || line == c.start {
                keep(app, |c| vec![c.upper()], "upper");
            } else if c
                .base_lines()
                .is_some_and(|r| r.contains(&line) || Some(line) == c.base)
            {
                keep(app, |c| c.base_lines().into_iter().collect(), "base");
            } else if c.lower().contains(&line) || line == c.end {
                keep(app, |c| vec![c.lower()], "lower");
            } else {
                app.error("Point is on the separator: move into the version to keep");
            }
        }
        "smerge-refine" => refine(app),
        "smerge-ediff" => ediff(app),
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ls(s: &str) -> Vec<String> {
        s.lines().map(str::to_string).collect()
    }

    #[test]
    fn conflicts_with_and_without_a_base() {
        let text = "a\n<<<<<<< HEAD\nmine\n=======\ntheirs\n>>>>>>> other\nb\n<<<<<<< ours\nm2\n||||||| base\nb2\n=======\nt2\n>>>>>>> theirs\n";
        let cs = find_conflicts(&ls(text));
        assert_eq!(cs.len(), 2);
        assert_eq!(
            cs[0],
            Conflict {
                start: 1,
                base: None,
                sep: 3,
                end: 5
            }
        );
        assert_eq!(cs[1].base, Some(9));
        assert_eq!(cs[1].upper(), 8..9);
        assert_eq!(cs[1].base_lines(), Some(10..11));
        assert_eq!(cs[1].lower(), 12..13);
        assert_eq!(side_from_markers(&ls(text), 0), "a\nmine\nb\nm2\n");
        assert_eq!(side_from_markers(&ls(text), 2), "a\ntheirs\nb\nt2\n");
        assert!(find_conflicts(&ls("<<<<<<< x\nno end\n")).is_empty());
    }

    #[test]
    fn keeping_a_side() {
        let mut app = App::new(forge_core::Editor::new());
        let id = app.editor.add_buffer(Buffer::new("c"));
        app.editor.show_buffer(id);
        app.buf()
            .set_contents("a\n<<<<<<< HEAD\nmine\n=======\ntheirs\n>>>>>>> o\nz\n");
        maybe_enable(&mut app, id, false);
        assert!(app.buf_ref().minor_mode_enabled(MODE));
        app.buf().set_point(Pos::new(2, 0));
        execute(&mut app, "smerge-keep-lower");
        assert_eq!(app.buf_ref().get_contents(), "a\ntheirs\nz\n");
        after_command(&mut app);
        assert!(!app.buf_ref().minor_mode_enabled(MODE), "no conflicts left");
    }
}
