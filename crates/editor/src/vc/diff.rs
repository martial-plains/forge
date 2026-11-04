// SPDX-License-Identifier: MIT OR Apache-2.0
//! Diffs: computing them (Myers' algorithm, for the cases with no VCS to
//! ask), and diff-mode, the buffer that shows one.
//!
//! In diff-mode, `n`/`p` move by hunk and `N`/`P` (or `}`/`{`) by file;
//! RET (or `o`, `C-c C-c`) visits the source line under point, `C-u RET`
//! the old version of it; `k` and `K` kill a hunk or a file from the diff;
//! `C-c C-a` applies the hunk to its file (`C-u C-c C-a` reverses it, and
//! applying a hunk that is already applied offers to undo it); `C-c C-s`
//! splits the hunk at point; `C-c C-r` reverses the whole diff; `C-c C-w`
//! regenerates it ignoring whitespace (again to stop); TAB folds the hunk
//! or file at point and S-TAB every file; `C-c C-v` shows the diff side by
//! side; `g` regenerates it. Changed words inside changed lines are
//! highlighted (forge.opt.diff_refine). `C-x C-q` makes the buffer
//! editable, and the hunk headers' line counts follow the edits.

use crate::app::App;
use forge_core::decoration::{DecoFace, DecoSpec, VcFace};
use forge_core::{Buffer, BufferId, Pos};
use std::path::{Path, PathBuf};

pub const MODE: &str = "diff-mode";
/// Diffs that aren't from version control (recover-this-file's `=`).
pub const DIFF_BUFFER: &str = "*Diff*";
/// vc-diff and vc-root-diff.
pub const VC_DIFF_BUFFER: &str = "*vc-diff*";
const DECO: &str = "diff";

// ------------------------------------------------------------------ Myers

/// One step of an edit script between two sequences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// `len` equal items at `a` and `b`.
    Equal { a: usize, b: usize, len: usize },
    /// Items `a..a+len` of the old sequence are gone.
    Delete { a: usize, len: usize },
    /// Items `b..b+len` of the new sequence are new.
    Insert { b: usize, len: usize },
}

/// The shortest edit script from `a` to `b` (Myers, "An O(ND) Difference
/// Algorithm"), common prefix and suffix trimmed first. Past `max_d`
/// differences it gives up and replaces the middle wholesale, which keeps
/// pathological inputs fast.
pub fn diff_seq<T: PartialEq>(a: &[T], b: &[T]) -> Vec<Op> {
    let max_d = 4000;
    let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..]
        .iter()
        .rev()
        .zip(b[pre..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (am, bm) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let mut ops = Vec::new();
    if pre > 0 {
        ops.push(Op::Equal {
            a: 0,
            b: 0,
            len: pre,
        });
    }
    let mut middle = myers(am, bm, max_d).unwrap_or_else(|| {
        let mut v = Vec::new();
        if !am.is_empty() {
            v.push(Op::Delete {
                a: 0,
                len: am.len(),
            });
        }
        if !bm.is_empty() {
            v.push(Op::Insert {
                b: 0,
                len: bm.len(),
            });
        }
        v
    });
    for op in &mut middle {
        match op {
            Op::Equal { a, b, .. } => {
                *a += pre;
                *b += pre;
            }
            Op::Delete { a, .. } => *a += pre,
            Op::Insert { b, .. } => *b += pre,
        }
    }
    ops.extend(middle);
    if suf > 0 {
        ops.push(Op::Equal {
            a: a.len() - suf,
            b: b.len() - suf,
            len: suf,
        });
    }
    coalesce(ops)
}

fn coalesce(ops: Vec<Op>) -> Vec<Op> {
    let mut out: Vec<Op> = Vec::new();
    for op in ops {
        match (out.last_mut(), op) {
            (Some(Op::Equal { len, .. }), Op::Equal { len: l2, .. }) => *len += l2,
            (Some(Op::Delete { len, .. }), Op::Delete { len: l2, .. }) => *len += l2,
            (Some(Op::Insert { len, .. }), Op::Insert { len: l2, .. }) => *len += l2,
            _ => out.push(op),
        }
    }
    out
}

fn myers<T: PartialEq>(a: &[T], b: &[T], max_d: usize) -> Option<Vec<Op>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    if n == 0 && m == 0 {
        return Some(Vec::new());
    }
    let max = (n + m) as usize;
    let limit = max.min(max_d);
    let offset = max as isize + 1;
    let mut v = vec![0isize; 2 * max + 3];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut found = None;
    'outer: for d in 0..=limit as isize {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let idx = (k + offset) as usize;
            let mut x = if k == -d || (k != d && v[idx - 1] < v[idx + 1]) {
                v[idx + 1]
            } else {
                v[idx - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[idx] = x;
            if x >= n && y >= m {
                found = Some(d);
                break 'outer;
            }
            k += 2;
        }
    }
    let d_final = found?;
    // Walk back through the trace.
    let mut ops_rev: Vec<Op> = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (1..=d_final).rev() {
        let v = &trace[d as usize];
        let k = x - y;
        let prev_k =
            if k == -d || (k != d && v[(k - 1 + offset) as usize] < v[(k + 1 + offset) as usize]) {
                k + 1
            } else {
                k - 1
            };
        let prev_x = v[(prev_k + offset) as usize];
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
            ops_rev.push(Op::Equal {
                a: x as usize,
                b: y as usize,
                len: 1,
            });
        }
        if x == prev_x {
            y -= 1;
            ops_rev.push(Op::Insert {
                b: y as usize,
                len: 1,
            });
        } else {
            x -= 1;
            ops_rev.push(Op::Delete {
                a: x as usize,
                len: 1,
            });
        }
    }
    while x > 0 && y > 0 {
        x -= 1;
        y -= 1;
        ops_rev.push(Op::Equal {
            a: x as usize,
            b: y as usize,
            len: 1,
        });
    }
    ops_rev.reverse();
    Some(coalesce(ops_rev))
}

/// A unified diff of two texts, with `context` lines around each change.
/// Empty when they are the same.
pub fn unified(old: &str, new: &str, old_label: &str, new_label: &str, context: usize) -> String {
    let a: Vec<&str> = split_lines(old);
    let b: Vec<&str> = split_lines(new);
    let ops = diff_seq(&a, &b);
    if ops.iter().all(|o| matches!(o, Op::Equal { .. })) {
        return String::new();
    }
    // Flatten to (tag, a index, b index) per line.
    let mut lines: Vec<(char, usize, usize)> = Vec::new();
    for op in &ops {
        match *op {
            Op::Equal { a: ai, b: bi, len } => {
                for i in 0..len {
                    lines.push((' ', ai + i, bi + i));
                }
            }
            Op::Delete { a: ai, len } => {
                for i in 0..len {
                    lines.push(('-', ai + i, usize::MAX));
                }
            }
            Op::Insert { b: bi, len } => {
                for i in 0..len {
                    lines.push(('+', usize::MAX, bi + i));
                }
            }
        }
    }
    let mut out = format!("--- {old_label}\n+++ {new_label}\n");
    let changed: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.0 != ' ')
        .map(|(i, _)| i)
        .collect();
    let mut i = 0;
    while i < changed.len() {
        let start = changed[i].saturating_sub(context);
        let mut end = changed[i];
        while i < changed.len() && changed[i] <= end + 2 * context {
            end = changed[i];
            i += 1;
        }
        let end = (end + context + 1).min(lines.len());
        let hunk = &lines[start..end];
        let a_start = hunk
            .iter()
            .find(|l| l.0 != '+')
            .map_or_else(|| a_before(&lines, start), |l| l.1 + 1);
        let b_start = hunk
            .iter()
            .find(|l| l.0 != '-')
            .map_or_else(|| b_before(&lines, start), |l| l.2 + 1);
        let a_len = hunk.iter().filter(|l| l.0 != '+').count();
        let b_len = hunk.iter().filter(|l| l.0 != '-').count();
        out.push_str(&format!(
            "@@ -{} +{} @@\n",
            range(a_start, a_len),
            range(b_start, b_len)
        ));
        for &(tag, ai, bi) in hunk {
            let text = if tag == '+' { b[bi] } else { a[ai] };
            out.push(tag);
            out.push_str(text.trim_end_matches('\n'));
            out.push('\n');
            if !text.ends_with('\n') {
                out.push_str("\\ No newline at end of file\n");
            }
        }
    }
    out
}

fn a_before(lines: &[(char, usize, usize)], start: usize) -> usize {
    lines[..start].iter().filter(|l| l.0 != '+').count()
}

fn b_before(lines: &[(char, usize, usize)], start: usize) -> usize {
    lines[..start].iter().filter(|l| l.0 != '-').count()
}

fn range(start: usize, len: usize) -> String {
    match len {
        1 => format!("{start}"),
        0 => format!("{},0", start.saturating_sub(1)),
        n => format!("{start},{n}"),
    }
}

/// Lines with their newlines (the last may have none).
fn split_lines(s: &str) -> Vec<&str> {
    if s.is_empty() {
        return Vec::new();
    }
    s.split_inclusive('\n').collect()
}

// ------------------------------------------------------------- parsing

/// A hunk: the line of its `@@` header and the lines after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// Line of the `@@` header.
    pub header: usize,
    /// One past its last line.
    pub end: usize,
    pub old_start: usize,
    pub new_start: usize,
}

/// One file's part of a diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// First line of its header (`diff --git`, or `---`).
    pub start: usize,
    pub end: usize,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub hunks: Vec<Hunk>,
}

/// `@@ -a,b +c,d @@`: (a, b, c, d).
pub fn parse_hunk_header(line: &str) -> Option<(usize, usize, usize, usize)> {
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, _) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let parse = |r: &str| -> Option<(usize, usize)> {
        match r.split_once(',') {
            Some((s, l)) => Some((s.parse().ok()?, l.parse().ok()?)),
            None => Some((r.parse().ok()?, 1)),
        }
    };
    let (a, b) = parse(old)?;
    let (c, d) = parse(new.split_whitespace().next()?)?;
    Some((a, b, c, d))
}

fn header_path(line: &str, prefix: &str) -> Option<String> {
    let p = line.strip_prefix(prefix)?;
    // "--- a/x\t2024-..." : drop a timestamp after a tab.
    let p = p.split('\t').next().unwrap_or(p).trim_end();
    if p == "/dev/null" {
        return None;
    }
    Some(p.to_string())
}

/// Split a diff into files and hunks.
pub fn parse(lines: &[String]) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut i = 0;
    let n = lines.len();
    while i < n {
        let l = &lines[i];
        let starts_file = l.starts_with("diff ")
            || (l.starts_with("--- ") && i + 1 < n && lines[i + 1].starts_with("+++ "));
        if starts_file {
            // A `diff` line and the header lines after it, or just ---/+++.
            let start = i;
            let mut f = FileDiff {
                start,
                end: start,
                old_path: None,
                new_path: None,
                hunks: Vec::new(),
            };
            if let Some(rest) = l.strip_prefix("diff --git ") {
                // Paths from the diff line, for diffs with no ---/+++
                // (a mode change, a binary file).
                if let Some((a, b)) = split_git_paths(rest) {
                    f.old_path = Some(a);
                    f.new_path = Some(b);
                }
            }
            if l.starts_with("diff ") {
                i += 1;
            }
            while i < n
                && !lines[i].starts_with("@@")
                && !(i > start && lines[i].starts_with("diff "))
            {
                if lines[i].starts_with("--- ") {
                    f.old_path = header_path(&lines[i], "--- ");
                } else if lines[i].starts_with("+++ ") {
                    f.new_path = header_path(&lines[i], "+++ ");
                    i += 1;
                    break;
                } else if let Some(p) = lines[i].strip_prefix("rename from ") {
                    f.old_path = Some(p.to_string());
                } else if let Some(p) = lines[i].strip_prefix("rename to ") {
                    f.new_path = Some(p.to_string());
                } else if lines[i].starts_with("new file") {
                    f.old_path = None;
                } else if lines[i].starts_with("deleted file") {
                    f.new_path = None;
                } else if !(lines[i].starts_with("index ")
                    || lines[i].starts_with("old mode")
                    || lines[i].starts_with("new mode")
                    || lines[i].starts_with("similarity")
                    || lines[i].starts_with("dissimilarity")
                    || lines[i].starts_with("copy ")
                    || lines[i].starts_with("Binary")
                    || lines[i].starts_with("GIT binary"))
                {
                    break;
                }
                i += 1;
            }
            let (hunks, next) = parse_hunks_from(lines, i);
            f.hunks = hunks;
            i = next;
            f.end = i;
            files.push(f);
            continue;
        }
        if l.starts_with("@@") {
            // Hunks with no file header: a file of unknown name.
            let (hunks, next) = parse_hunks_from(lines, i);
            files.push(FileDiff {
                start: i,
                end: next,
                old_path: None,
                new_path: None,
                hunks,
            });
            i = next;
            continue;
        }
        i += 1;
    }
    files
}

/// Is line `i` the start of a file's header?
fn file_header_at(lines: &[String], i: usize) -> bool {
    let t = &lines[i];
    t.starts_with("diff ")
        || (t.starts_with("--- ") && lines.get(i + 1).is_some_and(|l| l.starts_with("+++ ")))
}

/// The hunks starting at line `i`: each runs for as many lines as its
/// header counts, and on past that for lines that still look like a hunk's
/// (an edited diff whose counts are stale).
fn parse_hunks_from(lines: &[String], mut i: usize) -> (Vec<Hunk>, usize) {
    let n = lines.len();
    let mut hunks = Vec::new();
    while i < n && lines[i].starts_with("@@") {
        let (old_start, old_len, new_start, new_len) =
            parse_hunk_header(&lines[i]).unwrap_or((0, 0, 0, 0));
        let header = i;
        i += 1;
        let (mut old_left, mut new_left) = (old_len, new_len);
        while i < n {
            let t = &lines[i];
            let c = t.chars().next();
            if old_left == 0 && new_left == 0 {
                if t.starts_with("@@") || file_header_at(lines, i) {
                    break;
                }
                match c {
                    Some(' ' | '+' | '-' | '\\') => {
                        i += 1;
                        continue;
                    }
                    _ => break,
                }
            }
            match c {
                Some('\\') => {}
                Some(' ') | None => {
                    old_left = old_left.saturating_sub(1);
                    new_left = new_left.saturating_sub(1);
                }
                Some('-') => old_left = old_left.saturating_sub(1),
                Some('+') => new_left = new_left.saturating_sub(1),
                _ => break,
            }
            i += 1;
        }
        hunks.push(Hunk {
            header,
            end: i,
            old_start,
            new_start,
        });
    }
    (hunks, i)
}

/// `a/x b/x` from a `diff --git` line (names may contain spaces when the
/// two halves are the same).
fn split_git_paths(rest: &str) -> Option<(String, String)> {
    let words: Vec<&str> = rest.split(' ').collect();
    let n = words.len();
    if n.is_multiple_of(2) {
        let a = words[..n / 2].join(" ");
        let b = words[n / 2..].join(" ");
        let strip = |s: &str| {
            s.strip_prefix("a/")
                .or_else(|| s.strip_prefix("b/"))
                .unwrap_or(s)
                .to_string()
        };
        if strip(&a) == strip(&b) {
            return Some((a, b));
        }
    }
    let (a, b) = rest.split_once(' ')?;
    Some((a.to_string(), b.to_string()))
}

// --------------------------------------------------------- decoration

/// Faces for a diff's lines, and refined words inside changed lines.
pub fn decorate(b: &mut Buffer, refine: bool) {
    let n = b.line_count();
    let lines: Vec<String> = (0..n).map(|l| b.line(l)).collect();
    let mut decos: Vec<DecoSpec> = Vec::new();
    let files = parse(&lines);
    let mut in_header = vec![false; n];
    for f in &files {
        let first_hunk = f.hunks.first().map_or(f.end, |h| h.header);
        for x in in_header.iter_mut().take(first_hunk).skip(f.start) {
            *x = true;
        }
    }
    let starts: Vec<usize> = (0..n).map(|l| b.pos_to_offset(Pos::new(l, 0))).collect();
    for (l, text) in lines.iter().enumerate() {
        let len = text.chars().count();
        if len == 0 {
            continue;
        }
        let face = if in_header[l] {
            if text.starts_with("--- ") || text.starts_with("+++ ") {
                VcFace::DiffFileHeader
            } else {
                VcFace::DiffHeader
            }
        } else if text.starts_with("@@") {
            VcFace::DiffHunkHeader
        } else if text.starts_with('+') {
            VcFace::DiffAdded
        } else if text.starts_with('-') {
            VcFace::DiffRemoved
        } else if text.starts_with('\\') {
            VcFace::DiffNote
        } else {
            continue;
        };
        decos.push(DecoSpec::new(
            starts[l],
            starts[l] + len,
            DecoFace::Vc(face),
        ));
    }
    if refine {
        for f in &files {
            for h in &f.hunks {
                refine_hunk(&lines, &starts, h, &mut decos);
            }
        }
    }
    b.set_decorations(DECO, decos);
}

/// Words that changed between each run of removed lines and the added
/// lines right after it.
fn refine_hunk(lines: &[String], starts: &[usize], h: &Hunk, decos: &mut Vec<DecoSpec>) {
    if h.end - h.header > 2000 {
        return;
    }
    let mut i = h.header + 1;
    while i < h.end {
        if !lines[i].starts_with('-') {
            i += 1;
            continue;
        }
        let del_start = i;
        while i < h.end && lines[i].starts_with('-') {
            i += 1;
        }
        let add_start = i;
        while i < h.end && lines[i].starts_with('+') {
            i += 1;
        }
        if add_start == i {
            continue;
        }
        refine_pair(
            lines,
            starts,
            del_start..add_start,
            add_start..i,
            (VcFace::DiffRefineRemoved, VcFace::DiffRefineAdded),
            1,
            decos,
        );
    }
}

/// Mark the changed words between two runs of lines (each line skipping
/// `skip` leading characters: the `+`/`-`).
pub fn refine_pair(
    lines: &[String],
    starts: &[usize],
    old: std::ops::Range<usize>,
    new: std::ops::Range<usize>,
    faces: (VcFace, VcFace),
    skip: usize,
    decos: &mut Vec<DecoSpec>,
) {
    // Tokens with their absolute char offsets.
    let tokens = |r: std::ops::Range<usize>| -> Vec<(String, usize, usize)> {
        let mut out = Vec::new();
        for l in r {
            let chars: Vec<char> = lines[l].chars().skip(skip).collect();
            let base = starts[l] + skip;
            let mut j = 0;
            while j < chars.len() {
                let c = chars[j];
                let k = if c.is_alphanumeric() || c == '_' {
                    let mut k = j;
                    while k < chars.len() && (chars[k].is_alphanumeric() || chars[k] == '_') {
                        k += 1;
                    }
                    k
                } else if c.is_whitespace() {
                    let mut k = j;
                    while k < chars.len() && chars[k].is_whitespace() {
                        k += 1;
                    }
                    k
                } else {
                    j + 1
                };
                out.push((chars[j..k].iter().collect(), base + j, base + k));
                j = k;
            }
            out.push(("\n".into(), base + chars.len(), base + chars.len()));
        }
        out
    };
    let a = tokens(old);
    let b = tokens(new);
    if a.len() + b.len() > 20_000 {
        return;
    }
    let ta: Vec<&str> = a.iter().map(|t| t.0.as_str()).collect();
    let tb: Vec<&str> = b.iter().map(|t| t.0.as_str()).collect();
    let ops = diff_seq(&ta, &tb);
    // Nothing in common: every word changed, and marking them all says
    // nothing the line colors don't.
    let common: usize = ops
        .iter()
        .map(|o| match o {
            Op::Equal { len, a, .. } => ta[*a..*a + *len]
                .iter()
                .filter(|t| !t.trim().is_empty())
                .count(),
            _ => 0,
        })
        .sum();
    if common == 0 {
        return;
    }
    for op in ops {
        match op {
            Op::Delete { a: ai, len } => {
                for t in &a[ai..ai + len] {
                    if t.2 > t.1 {
                        decos.push(DecoSpec::new(t.1, t.2, DecoFace::Vc(faces.0)));
                    }
                }
            }
            Op::Insert { b: bi, len } => {
                for t in &b[bi..bi + len] {
                    if t.2 > t.1 {
                        decos.push(DecoSpec::new(t.1, t.2, DecoFace::Vc(faces.1)));
                    }
                }
            }
            Op::Equal { .. } => {}
        }
    }
}

// ------------------------------------------------------------- the view

/// How a diff buffer was made, so `g` and the whitespace toggle can make
/// it again, and RET can find the files.
#[derive(Clone, Debug, Default)]
pub struct DiffView {
    /// Paths in the diff are relative to this.
    pub root: PathBuf,
    pub kind: Option<super::backend::Kind>,
    pub files: Vec<String>,
    pub rev1: Option<String>,
    pub rev2: Option<String>,
    pub ignore_ws: bool,
    /// Can be regenerated (it came from a VCS diff).
    pub regenerable: bool,
}

/// Show `text` in buffer `name` in diff-mode. Returns its id.
pub fn show(app: &mut App, name: &str, text: &str, view: Option<DiffView>) -> BufferId {
    let id = match app.editor.find_buffer(name) {
        Some(id) => id,
        None => app.editor.add_buffer(Buffer::new(name)),
    };
    let refine = super::config().diff_refine;
    {
        let b = app.editor.state.get_buffer_mut(id).expect("just made");
        b.set_locked(false);
        b.set_contents(text);
        b.set_major_mode(MODE);
        b.unfold_all();
        decorate(b, refine);
        b.set_locked(true);
        let first = (0..b.line_count()).find(|&l| b.line(l).starts_with("@@"));
        b.set_point(Pos::new(first.unwrap_or(0), 0));
    }
    match view {
        Some(v) => {
            app.vc.diffs.insert(id, v);
        }
        None => {
            app.vc.diffs.remove(&id);
        }
    }
    id
}

/// Re-run the diff a buffer came from (`g`), keeping point's line.
pub fn regenerate(app: &mut App, id: BufferId) {
    let Some(view) = app.vc.diffs.get(&id).cloned() else {
        return app.error("This diff can't be regenerated");
    };
    let (Some(kind), true) = (view.kind, view.regenerable) else {
        return app.error("This diff can't be regenerated");
    };
    let line = app
        .editor
        .state
        .get_buffer(id)
        .map_or(0, |b| b.point().line);
    let cmd = kind.backend().diff(
        &view.root,
        &view.files,
        view.rev1.as_deref(),
        view.rev2.as_deref(),
        view.ignore_ws,
    );
    let name = app
        .editor
        .state
        .get_buffer(id)
        .map(|b| b.name().to_string())
        .unwrap_or_else(|| VC_DIFF_BUFFER.into());
    super::runner::run(app, cmd.clone(), "Diffing", None, move |app, out| {
        if !out.success() && out.stdout.is_empty() {
            return super::runner::report_failure(app, &cmd.line(), &out, true);
        }
        let text = if out.stdout.is_empty() {
            "No differences.\n".to_string()
        } else {
            out.stdout
        };
        let id = show(app, &name, &text, Some(view));
        if let Some(b) = app.editor.state.get_buffer_mut(id) {
            let l = line.min(b.line_count().saturating_sub(1));
            b.set_point(Pos::new(l, 0));
        }
    });
}

fn lines_of(b: &Buffer) -> Vec<String> {
    (0..b.line_count()).map(|l| b.line(l)).collect()
}

/// The file and hunk point is in.
fn at_point(b: &Buffer) -> Option<(FileDiff, Option<Hunk>)> {
    let lines = lines_of(b);
    let line = b.point().line;
    let files = parse(&lines);
    let f = files
        .into_iter()
        .find(|f| f.start <= line && line < f.end.max(f.start + 1))?;
    let h = f
        .hunks
        .iter()
        .find(|h| h.header <= line && line < h.end)
        .cloned();
    Some((f, h))
}

/// Resolve a path from a diff header against `root`: as written, or with
/// git's `a/` / `b/` taken off.
fn resolve(root: &Path, p: &str) -> PathBuf {
    let direct = root.join(p);
    if direct.exists() {
        return direct;
    }
    for prefix in ["a/", "b/"] {
        if let Some(s) = p.strip_prefix(prefix) {
            let q = root.join(s);
            if q.exists() || !direct.parent().is_some_and(Path::exists) {
                return q;
            }
        }
    }
    direct
}

/// Where in the old and new files a line of a hunk is: (old line, new
/// line), 1-based, and how far into the line point is.
fn source_position(b: &Buffer, h: &Hunk) -> (usize, usize, usize) {
    let p = b.point();
    let (mut old, mut new) = (h.old_start, h.new_start);
    let target = p.line.max(h.header + 1);
    for l in h.header + 1..target.min(h.end) {
        match b.line(l).chars().next() {
            Some('-') => old += 1,
            Some('+') => new += 1,
            Some('\\') => {}
            _ => {
                old += 1;
                new += 1;
            }
        }
    }
    let col = if p.line > h.header {
        p.col.saturating_sub(1)
    } else {
        0
    };
    (old.max(1), new.max(1), col)
}

/// RET: visit the line under point in its file (or with `old`, in the
/// old version).
pub fn goto_source(app: &mut App, old: bool) {
    let id = app.active_id();
    let b = app.buf_ref();
    let Some((f, h)) = at_point(b) else {
        return app.error("Not in a diff");
    };
    let view = app.vc.diffs.get(&id).cloned().unwrap_or_else(|| DiffView {
        root: crate::compile::default_directory(app),
        ..Default::default()
    });
    let (old_line, new_line, col) = match &h {
        Some(h) => source_position(b, h),
        None => (1, 1, 0),
    };
    let on_removed = h.is_some() && b.line(b.point().line).starts_with('-');
    let use_old = old || (on_removed && f.new_path.is_none());
    if use_old {
        let Some(path) = f.old_path.clone() else {
            return app.error("The file is new: it has no old version");
        };
        let rel = path.strip_prefix("a/").unwrap_or(&path).to_string();
        match (
            view.kind,
            view.rev1
                .clone()
                .or_else(|| view.kind.and(Some("HEAD".to_string()))),
        ) {
            (Some(kind), Some(rev)) => {
                let rev = if kind == super::backend::Kind::Hg && rev == "HEAD" {
                    ".".to_string()
                } else {
                    rev
                };
                super::show_revision(app, kind, &view.root, &rel, &rev, true, Some(old_line));
            }
            _ => {
                let p = resolve(&view.root, &path);
                visit_line(app, &p, old_line, col);
            }
        }
        return;
    }
    let Some(path) = f.new_path.clone().or(f.old_path.clone()) else {
        return app.error("No file name in this diff");
    };
    // A diff against an older revision: the new side is a revision too.
    if let (Some(kind), Some(rev2)) = (view.kind, view.rev2.clone()) {
        let rel = path.strip_prefix("b/").unwrap_or(&path).to_string();
        return super::show_revision(app, kind, &view.root, &rel, &rev2, true, Some(new_line));
    }
    let p = resolve(&view.root, &path);
    visit_line(app, &p, new_line, col);
}

fn visit_line(app: &mut App, path: &Path, line: usize, col: usize) {
    match app.editor.open_file(path) {
        Ok((id, _)) => {
            crate::windows::pop_to_other_window(app);
            app.switch_to(id);
            let b = app.buf();
            let l = line.saturating_sub(1).min(b.line_count().saturating_sub(1));
            let c = col.min(b.line_len(l));
            b.set_point(Pos::new(l, c));
        }
        Err(e) => app.error(format!("Cannot open {}: {e}", path.display())),
    }
}

/// n / p: the next or previous hunk header.
pub fn hunk_move(app: &mut App, forward: bool, count: usize) {
    move_to(app, forward, count, |l| l.starts_with("@@"), "hunk");
}

/// N / P: the next or previous file.
pub fn file_move(app: &mut App, forward: bool, count: usize) {
    let lines = lines_of(app.buf_ref());
    let starts: Vec<usize> = parse(&lines).iter().map(|f| f.start).collect();
    let here = app.buf_ref().point().line;
    let mut target = None;
    let mut left = count.max(1);
    let mut from = here;
    while left > 0 {
        let next = if forward {
            starts.iter().copied().find(|&s| s > from)
        } else {
            starts.iter().rev().copied().find(|&s| s < from)
        };
        match next {
            Some(s) => {
                target = Some(s);
                from = s;
            }
            None => break,
        }
        left -= 1;
    }
    match target {
        Some(l) => app.buf().set_point(Pos::new(l, 0)),
        None => app.error(if forward {
            "No next file"
        } else {
            "No previous file"
        }),
    }
}

fn move_to(app: &mut App, forward: bool, count: usize, is: impl Fn(&str) -> bool, what: &str) {
    let b = app.buf_ref();
    let n = b.line_count();
    let mut l = b.point().line;
    let mut found = None;
    for _ in 0..count.max(1) {
        let next = if forward {
            (l + 1..n).find(|&x| is(&b.line(x)))
        } else {
            (0..l).rev().find(|&x| is(&b.line(x)))
        };
        match next {
            Some(x) => {
                l = x;
                found = Some(x);
            }
            None => break,
        }
    }
    match found {
        Some(x) => {
            // Point onto the hunk's first change.
            app.buf().set_point(Pos::new(x, 0));
        }
        None => app.error(if forward {
            format!("No next {what}")
        } else {
            format!("No previous {what}")
        }),
    }
}

/// Edit the diff buffer as its owner (it is read-only to commands).
fn edit_diff(app: &mut App, f: impl FnOnce(&mut Buffer)) {
    let refine = super::config().diff_refine;
    let b = app.buf();
    b.as_owner(|b| {
        f(b);
    });
    b.break_undo_group();
    decorate(b, refine);
}

fn delete_lines(b: &mut Buffer, first: usize, end: usize) {
    let a = b.pos_to_offset(Pos::new(first, 0));
    let z = if end >= b.line_count() {
        b.char_count()
    } else {
        b.pos_to_offset(Pos::new(end, 0))
    };
    b.replace_offsets(a, z, "");
}

/// k: remove the hunk at point from the diff (and the file's header when
/// it was the file's last hunk).
pub fn kill_hunk(app: &mut App) {
    let Some((f, h)) = at_point(app.buf_ref()) else {
        return app.error("Not in a diff");
    };
    let Some(h) = h else {
        return kill_file(app);
    };
    let (first, end) = if f.hunks.len() == 1 {
        (f.start, f.end)
    } else {
        (h.header, h.end)
    };
    edit_diff(app, |b| delete_lines(b, first, end));
    let b = app.buf();
    let l = first.min(b.line_count().saturating_sub(1));
    b.set_point(Pos::new(l, 0));
}

/// K: remove the whole file at point from the diff.
pub fn kill_file(app: &mut App) {
    let Some((f, _)) = at_point(app.buf_ref()) else {
        return app.error("Not in a diff");
    };
    edit_diff(app, |b| delete_lines(b, f.start, f.end));
    let b = app.buf();
    let l = f.start.min(b.line_count().saturating_sub(1));
    b.set_point(Pos::new(l, 0));
}

/// C-c C-s: split the hunk at point into two, at point's line.
pub fn split_hunk(app: &mut App) {
    let b = app.buf_ref();
    let Some((_, Some(h))) = at_point(b) else {
        return app.error("Not in a hunk");
    };
    let line = b.point().line;
    if line <= h.header + 1 || line >= h.end {
        return app.error("Can't split a hunk at its first line");
    }
    let (old, new, _) = source_position(b, &h);
    let header = format!("@@ -{old} +{new} @@\n");
    edit_diff(app, |b| {
        let at = b.pos_to_offset(Pos::new(line, 0));
        b.replace_offsets(at, at, &header);
        fix_hunk_headers(b);
    });
    app.buf().set_point(Pos::new(line, 0));
}

/// Recompute every hunk header's line counts from its lines (after edits).
pub fn fix_hunk_headers(b: &mut Buffer) {
    let lines = lines_of(b);
    let files = parse(&lines);
    // From the bottom, so earlier offsets stay right.
    let mut fixes: Vec<(usize, String)> = Vec::new();
    for f in &files {
        for h in &f.hunks {
            let mut old = 0;
            let mut new = 0;
            for l in &lines[h.header + 1..h.end] {
                match l.chars().next() {
                    Some('-') => old += 1,
                    Some('+') => new += 1,
                    Some('\\') => {}
                    _ => {
                        old += 1;
                        new += 1;
                    }
                }
            }
            let Some((a, _, c, _)) = parse_hunk_header(&lines[h.header]) else {
                continue;
            };
            let tail = lines[h.header]
                .split_once(" @@")
                .map(|x| x.1)
                .unwrap_or("")
                .to_string();
            let fixed = format!("@@ -{} +{} @@{tail}", range_hdr(a, old), range_hdr(c, new));
            if fixed != lines[h.header] {
                fixes.push((h.header, fixed));
            }
        }
    }
    for (l, text) in fixes.into_iter().rev() {
        let a = b.pos_to_offset(Pos::new(l, 0));
        let z = a + b.line_len(l);
        b.replace_offsets(a, z, &text);
    }
}

fn range_hdr(start: usize, len: usize) -> String {
    if len == 1 {
        format!("{start}")
    } else {
        format!("{start},{len}")
    }
}

/// C-c C-r: turn the diff around (removed lines become added and back).
pub fn reverse_direction(app: &mut App) {
    let lines = lines_of(app.buf_ref());
    let files = parse(&lines);
    let mut out: Vec<String> = lines.clone();
    for f in &files {
        let first_hunk = f.hunks.first().map_or(f.end, |h| h.header);
        for l in f.start..first_hunk {
            if let Some(p) = lines[l].strip_prefix("--- ") {
                if l + 1 < first_hunk
                    && let Some(q) = lines[l + 1].strip_prefix("+++ ")
                {
                    out[l] = format!("--- {q}");
                    out[l + 1] = format!("+++ {p}");
                }
            }
        }
        for h in &f.hunks {
            if let Some((a, b2, c, d)) = parse_hunk_header(&lines[h.header]) {
                let tail = lines[h.header].split_once(" @@").map(|x| x.1).unwrap_or("");
                out[h.header] = format!("@@ -{} +{} @@{tail}", range_hdr(c, d), range_hdr(a, b2));
            }
            // Within each change, the + lines go first after reversal.
            let mut i = h.header + 1;
            while i < h.end {
                if lines[i].starts_with('-') || lines[i].starts_with('+') {
                    let s = i;
                    while i < h.end && (lines[i].starts_with('-') || lines[i].starts_with('+')) {
                        i += 1;
                    }
                    let block = &lines[s..i];
                    let adds: Vec<String> = block
                        .iter()
                        .filter(|l| l.starts_with('+'))
                        .map(|l| format!("-{}", &l[1..]))
                        .collect();
                    let dels: Vec<String> = block
                        .iter()
                        .filter(|l| l.starts_with('-'))
                        .map(|l| format!("+{}", &l[1..]))
                        .collect();
                    for (k, l) in adds.into_iter().chain(dels).enumerate() {
                        out[s + k] = l;
                    }
                } else {
                    i += 1;
                }
            }
        }
    }
    let text = out.join("\n");
    let point = app.buf_ref().point();
    edit_diff(app, |b| {
        let end = b.char_count();
        b.replace_offsets(0, end, &text);
    });
    app.buf().set_point(point);
    let id = app.active_id();
    if let Some(v) = app.vc.diffs.get_mut(&id) {
        std::mem::swap(&mut v.rev1, &mut v.rev2);
        v.regenerable = false;
    }
}

/// The text a hunk expects (`old`) and the text it makes (`new`), each
/// as lines.
fn hunk_sides(lines: &[String], h: &Hunk) -> (Vec<String>, Vec<String>) {
    let mut old = Vec::new();
    let mut new = Vec::new();
    for l in &lines[h.header + 1..h.end] {
        match l.chars().next() {
            Some('-') => old.push(l[1..].to_string()),
            Some('+') => new.push(l[1..].to_string()),
            Some('\\') => {}
            Some(_) => {
                old.push(l[1..].to_string());
                new.push(l[1..].to_string());
            }
            None => {
                old.push(String::new());
                new.push(String::new());
            }
        }
    }
    (old, new)
}

/// Where `needle` (whole lines) occurs in `b`, nearest to line `near`.
fn find_lines(b: &Buffer, needle: &[String], near: usize) -> Option<usize> {
    let n = b.line_count();
    if needle.is_empty() {
        return Some(near.min(n));
    }
    let matches_at = |s: usize| -> bool {
        s + needle.len() <= n && needle.iter().enumerate().all(|(i, l)| b.line(s + i) == *l)
    };
    let max = n.max(near) + 1;
    for d in 0..max {
        if near >= d && matches_at(near - d) {
            return Some(near - d);
        }
        if d > 0 && near + d < n && matches_at(near + d) {
            return Some(near + d);
        }
        if near < d && near + d >= n {
            break;
        }
    }
    None
}

/// The last hunk applied, so it can be undone.
#[derive(Clone, Debug)]
pub struct Applied {
    pub buffer: BufferId,
    /// The buffer's version right after applying.
    pub version: u64,
    pub reverse: bool,
}

/// C-c C-a: apply the hunk at point to its file's buffer (reversed with
/// `reverse`). A hunk that is already applied is offered for undoing.
pub fn apply_hunk(app: &mut App, reverse: bool) {
    let diff_id = app.active_id();
    let b = app.buf_ref();
    let Some((f, Some(h))) = at_point(b) else {
        return app.error("Not in a hunk");
    };
    let lines = lines_of(b);
    let (old, new) = hunk_sides(&lines, &h);
    let (from, to, near) = if reverse {
        (new, old, h.new_start.saturating_sub(1))
    } else {
        (old, new, h.old_start.saturating_sub(1))
    };
    let view = app
        .vc
        .diffs
        .get(&diff_id)
        .cloned()
        .unwrap_or_else(|| DiffView {
            root: crate::compile::default_directory(app),
            ..Default::default()
        });
    let Some(path) = f.new_path.clone().or(f.old_path.clone()) else {
        return app.error("No file name in this diff");
    };
    let path = resolve(&view.root, &path);
    let target = match app.editor.open_file(&path) {
        Ok((id, _)) => id,
        Err(e) => return app.error(format!("Cannot open {}: {e}", path.display())),
    };
    let tb = app.editor.state.get_buffer(target).expect("just opened");
    let at = find_lines(tb, &from, near);
    match at {
        Some(line) => {
            let tb = app.editor.state.get_buffer_mut(target).expect("open");
            let a = tb.pos_to_offset(Pos::new(line, 0));
            let end_line = line + from.len();
            let z = if end_line >= tb.line_count() {
                tb.char_count()
            } else {
                tb.pos_to_offset(Pos::new(end_line, 0))
            };
            let mut text = to.join("\n");
            if !to.is_empty() && (end_line < tb.line_count() || tb.get_contents().ends_with('\n')) {
                text.push('\n');
            }
            if from.is_empty()
                && end_line >= tb.line_count()
                && tb.char_count() > 0
                && !tb.get_contents().ends_with('\n')
            {
                text.insert(0, '\n');
            }
            tb.break_undo_group();
            tb.replace_offsets(a, z, &text);
            tb.break_undo_group();
            tb.set_point(Pos::new(line, 0));
            let version = tb.version();
            app.vc.last_apply = Some(Applied {
                buffer: target,
                version,
                reverse,
            });
            crate::compile::display_buffer(app, target);
            for w in app
                .editor
                .windows
                .iter()
                .filter(|w| w.buffer == target)
                .map(|w| w.id)
                .collect::<Vec<_>>()
            {
                let off = app
                    .editor
                    .state
                    .get_buffer(target)
                    .map_or(0, |b| b.pos_to_offset(Pos::new(line, 0)));
                app.editor.set_window_point(w, off);
            }
            let moved = line as isize - near as isize;
            let off = if moved == 0 {
                String::new()
            } else {
                format!(" (offset {moved})")
            };
            app.msg(format!(
                "Hunk {}{off}",
                if reverse { "reversed" } else { "applied" }
            ));
            // Move to the next hunk.
            let _ = diff_id;
            if app.active_id() == diff_id {
                hunk_move_quiet(app);
            }
        }
        None => {
            // Already applied? The other side is there.
            if find_lines(tb, &to, near).is_some() {
                app.prompt(
                    "Hunk already applied; undo it? (y or n) ",
                    crate::minibuffer::Purpose::Confirm(crate::minibuffer::ConfirmAction::Vc(
                        super::Confirm::UndoHunk { reverse: !reverse },
                    )),
                );
            } else {
                app.error("Can't find the text to patch");
            }
        }
    }
}

fn hunk_move_quiet(app: &mut App) {
    let b = app.buf_ref();
    let n = b.line_count();
    let l = b.point().line;
    if let Some(x) = (l + 1..n).find(|&x| b.line(x).starts_with("@@")) {
        app.buf().set_point(Pos::new(x, 0));
    }
}

/// Undo the last hunk application, if its buffer hasn't changed since.
pub fn undo_apply(app: &mut App) {
    let Some(a) = app.vc.last_apply.take() else {
        return app.error("No hunk has been applied");
    };
    let Some(b) = app.editor.state.get_buffer_mut(a.buffer) else {
        return app.error("The patched buffer is gone");
    };
    if b.version() != a.version {
        return app.error("The patched buffer has changed since; use undo there");
    }
    b.undo();
    let name = b.name().to_string();
    app.msg(format!("Undid the hunk in {name}"));
}

/// TAB: fold or unfold the hunk (on its header) or file (elsewhere in
/// its header) at point.
pub fn toggle_fold(app: &mut App) {
    let b = app.buf_ref();
    let Some((f, h)) = at_point(b) else {
        return;
    };
    let line = b.point().line;
    let (first, last) = match &h {
        Some(h) if line == h.header => (h.header + 1, h.end.saturating_sub(1)),
        _ => {
            // The file: keep its first header line.
            (f.start + 1, f.end.saturating_sub(1))
        }
    };
    let anchor = first.saturating_sub(1);
    let b = app.buf();
    if b.line_folded(anchor) {
        b.unfold_line(anchor);
    } else if last >= first {
        b.fold_lines(first, last);
    }
    b.set_point(Pos::new(anchor, 0));
}

/// S-TAB: fold every file down to its first line, or unfold them all.
pub fn toggle_fold_all(app: &mut App) {
    let lines = lines_of(app.buf_ref());
    let files = parse(&lines);
    let b = app.buf();
    if !b.folds().is_empty() {
        b.unfold_all();
        return;
    }
    for f in &files {
        if f.end > f.start + 1 {
            b.fold_lines(f.start + 1, f.end - 1);
        }
    }
    let first = files.first().map_or(0, |f| f.start);
    b.set_point(Pos::new(first, 0));
}

/// C-c C-w: show the diff again ignoring whitespace changes (or not).
pub fn toggle_whitespace(app: &mut App) {
    let id = app.active_id();
    let Some(v) = app.vc.diffs.get_mut(&id) else {
        return app.error("This diff can't be regenerated");
    };
    if !v.regenerable {
        return app.error("This diff can't be regenerated");
    }
    v.ignore_ws = !v.ignore_ws;
    let on = v.ignore_ws;
    regenerate(app, id);
    app.msg(if on {
        "Ignoring whitespace changes"
    } else {
        "Showing whitespace changes"
    });
}

/// C-c C-b: (re)highlight the changed words in the hunk at point, even
/// with forge.opt.diff_refine off.
pub fn refine_at_point(app: &mut App) {
    let b = app.buf_ref();
    let Some((_, Some(h))) = at_point(b) else {
        return app.error("Not in a hunk");
    };
    let lines = lines_of(b);
    let starts: Vec<usize> = (0..lines.len())
        .map(|l| b.pos_to_offset(Pos::new(l, 0)))
        .collect();
    let mut decos = Vec::new();
    refine_hunk(&lines, &starts, &h, &mut decos);
    let b = app.buf();
    b.set_decorations("diff-refine", decos);
}

/// After a command edits an editable diff: fix the hunk headers and the
/// faces.
pub fn after_edit(app: &mut App) {
    let refine = super::config().diff_refine;
    let b = app.buf();
    if b.major_mode() != MODE || b.locked() {
        return;
    }
    let version = b.version();
    let id = app.active_id();
    if app.vc.diff_versions.get(&id) == Some(&version) {
        return;
    }
    let b = app.buf();
    let point = b.point_offset();
    fix_hunk_headers(b);
    b.set_offset(point.min(b.char_count()));
    decorate(b, refine);
    let v = b.version();
    app.vc.diff_versions.insert(id, v);
}

// --------------------------------------------------------- side by side

/// C-c C-v: the diff as two columns, old on the left and new on the right.
pub fn side_by_side(app: &mut App) {
    let lines = lines_of(app.buf_ref());
    let files = parse(&lines);
    if files.is_empty() {
        return app.error("Nothing to show");
    }
    let width = app.win().width.max(40);
    let col = (width - 3) / 2;
    let num_w = 5;
    let text_w = col.saturating_sub(num_w + 1).max(10);
    let cell = |n: Option<usize>, tag: char, text: &str| -> String {
        let num = n.map_or(" ".repeat(num_w), |n| format!("{n:>num_w$}"));
        let t: String = text.chars().take(text_w).collect();
        let pad = text_w.saturating_sub(unicode_width::UnicodeWidthStr::width(t.as_str()));
        format!("{num}{tag}{t}{}", " ".repeat(pad))
    };
    let mut out = String::new();
    let mut faces: Vec<(usize, usize, char)> = Vec::new(); // (line, side, tag)
    let mut row = 0usize;
    for f in &files {
        let name = f
            .new_path
            .clone()
            .or(f.old_path.clone())
            .unwrap_or_default();
        out.push_str(&format!("{name}\n"));
        faces.push((row, 2, 'F'));
        row += 1;
        for h in &f.hunks {
            out.push_str(&format!("{}\n", lines[h.header]));
            faces.push((row, 2, '@'));
            row += 1;
            let (mut old, mut new) = (h.old_start, h.new_start);
            let mut i = h.header + 1;
            while i < h.end {
                let l = &lines[i];
                match l.chars().next() {
                    Some('-') | Some('+') => {
                        let s = i;
                        while i < h.end && lines[i].starts_with('-') {
                            i += 1;
                        }
                        let dels: Vec<&String> = lines[s..i].iter().collect();
                        let s2 = i;
                        while i < h.end && lines[i].starts_with('+') {
                            i += 1;
                        }
                        let adds: Vec<&String> = lines[s2..i].iter().collect();
                        for k in 0..dels.len().max(adds.len()) {
                            let left = dels.get(k).map(|d| {
                                old += 1;
                                cell(Some(old - 1), '-', &d[1..])
                            });
                            let right = adds.get(k).map(|a| {
                                new += 1;
                                cell(Some(new - 1), '+', &a[1..])
                            });
                            let blank = " ".repeat(num_w + 1 + text_w);
                            out.push_str(&format!(
                                "{} | {}\n",
                                left.clone().unwrap_or_else(|| blank.clone()),
                                right.clone().unwrap_or(blank)
                            ));
                            if left.is_some() {
                                faces.push((row, 0, '-'));
                            }
                            if right.is_some() {
                                faces.push((row, 1, '+'));
                            }
                            row += 1;
                        }
                        if s == i {
                            i += 1;
                        }
                    }
                    Some('\\') => i += 1,
                    _ => {
                        let t = l.get(1..).unwrap_or("");
                        out.push_str(&format!(
                            "{} | {}\n",
                            cell(Some(old), ' ', t),
                            cell(Some(new), ' ', t)
                        ));
                        old += 1;
                        new += 1;
                        row += 1;
                        i += 1;
                    }
                }
            }
        }
    }
    let name = "*vc-diff side by side*";
    let id = match app.editor.find_buffer(name) {
        Some(id) => id,
        None => app.editor.add_buffer(Buffer::new(name)),
    };
    let b = app.editor.state.get_buffer_mut(id).expect("made");
    b.set_locked(false);
    b.set_contents(&out);
    b.set_major_mode("diff-side-by-side-mode");
    let mut decos = Vec::new();
    for (l, side, tag) in faces {
        let start = b.pos_to_offset(Pos::new(l, 0));
        let len = b.line_len(l);
        let half = num_w + 1 + text_w;
        let (a, z, face) = match (side, tag) {
            (2, 'F') => (0, len, VcFace::DiffFileHeader),
            (2, _) => (0, len, VcFace::DiffHunkHeader),
            (0, _) => (0, half.min(len), VcFace::DiffRemoved),
            _ => ((half + 3).min(len), len, VcFace::DiffAdded),
        };
        if z > a {
            decos.push(DecoSpec::new(start + a, start + z, DecoFace::Vc(face)));
        }
    }
    b.set_decorations(DECO, decos);
    b.set_locked(true);
    b.set_point(Pos::new(0, 0));
    app.switch_to(id);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply_ops(a: &[&str], b: &[&str], ops: &[Op]) -> Vec<String> {
        let mut out = Vec::new();
        for op in ops {
            match *op {
                Op::Equal { a: ai, len, .. } => {
                    out.extend(a[ai..ai + len].iter().map(|s| s.to_string()))
                }
                Op::Insert { b: bi, len } => {
                    out.extend(b[bi..bi + len].iter().map(|s| s.to_string()))
                }
                Op::Delete { .. } => {}
            }
        }
        out
    }

    #[test]
    fn myers_finds_a_shortest_script() {
        let a = ["a", "b", "c", "a", "b", "b", "a"];
        let b = ["c", "b", "a", "b", "a", "c"];
        let ops = diff_seq(&a, &b);
        assert_eq!(apply_ops(&a, &b, &ops), b.to_vec());
        let edits: usize = ops
            .iter()
            .map(|o| match o {
                Op::Equal { .. } => 0,
                Op::Delete { len, .. } | Op::Insert { len, .. } => *len,
            })
            .sum();
        assert_eq!(edits, 5, "the classic example takes 5 edits");
        assert_eq!(diff_seq::<&str>(&[], &[]), vec![]);
        assert_eq!(
            diff_seq(&["x"], &["x"]),
            vec![Op::Equal { a: 0, b: 0, len: 1 }]
        );
    }

    #[test]
    fn unified_diff_round_trips_through_the_parser() {
        let old = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n";
        let new = "one\nTWO\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\neleven\n";
        let d = unified(old, new, "a/f", "b/f", 2);
        assert_eq!(
            d,
            "--- a/f\n+++ b/f\n@@ -1,4 +1,4 @@\n one\n-two\n+TWO\n three\n four\n@@ -9,2 +9,3 @@\n nine\n ten\n+eleven\n"
        );
        let lines: Vec<String> = d.lines().map(str::to_string).collect();
        let files = parse(&lines);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].old_path.as_deref(), Some("a/f"));
        assert_eq!(files[0].hunks.len(), 2);
        assert_eq!(files[0].hunks[1].old_start, 9);
        assert_eq!(unified("same\n", "same\n", "a", "b", 3), "");
        let nl = unified("a", "b", "x", "y", 3);
        assert!(nl.contains("\\ No newline at end of file"));
    }

    #[test]
    fn git_diffs_parse_into_files_and_hunks() {
        let d = "diff --git src/a.rs src/a.rs\nindex 1..2 100644\n--- src/a.rs\n+++ src/a.rs\n@@ -1,2 +1,2 @@\n-x\n+y\n z\ndiff --git new.txt new.txt\nnew file mode 100644\nindex 0..1\n--- /dev/null\n+++ new.txt\n@@ -0,0 +1 @@\n+hello\ndiff --git bin bin\nBinary files differ\n";
        let lines: Vec<String> = d.lines().map(str::to_string).collect();
        let f = parse(&lines);
        assert_eq!(f.len(), 3);
        assert_eq!(f[0].new_path.as_deref(), Some("src/a.rs"));
        assert_eq!(f[0].hunks[0].end, 8);
        assert_eq!(f[1].old_path, None);
        assert_eq!(f[1].new_path.as_deref(), Some("new.txt"));
        assert_eq!(f[2].hunks.len(), 0);
        assert_eq!(f[2].start, 15);
    }

    #[test]
    fn hunk_headers() {
        assert_eq!(
            parse_hunk_header("@@ -1,3 +1,4 @@ fn x()"),
            Some((1, 3, 1, 4))
        );
        assert_eq!(parse_hunk_header("@@ -5 +5 @@"), Some((5, 1, 5, 1)));
        assert_eq!(parse_hunk_header("@@ -0,0 +1 @@"), Some((0, 0, 1, 1)));
        assert_eq!(parse_hunk_header("nope"), None);
    }

    #[test]
    fn headers_follow_edits() {
        let mut b = Buffer::new("d");
        b.set_contents("--- a\n+++ b\n@@ -1,2 +1,2 @@\n x\n-y\n+z\n+extra\n");
        fix_hunk_headers(&mut b);
        assert_eq!(b.line(2), "@@ -1,2 +1,3 @@");
    }

    #[test]
    fn refinement_marks_only_the_changed_words() {
        let mut b = Buffer::new("d");
        b.set_contents("@@ -1 +1 @@\n-let x = old_name(1);\n+let x = new_name(1);\n");
        decorate(&mut b, true);
        let refined: Vec<String> = b
            .decorations()
            .iter()
            .filter(|d| {
                matches!(
                    d.face,
                    DecoFace::Vc(VcFace::DiffRefineRemoved | VcFace::DiffRefineAdded)
                )
            })
            .map(|d| {
                b.get_contents()
                    .chars()
                    .skip(d.start)
                    .take(d.end - d.start)
                    .collect()
            })
            .collect();
        assert_eq!(refined, vec!["old_name", "new_name"]);
    }
}
