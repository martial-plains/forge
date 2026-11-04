// SPDX-License-Identifier: MIT OR Apache-2.0
//! Annotations: each line of a file with the
//! revision, author and date that last changed it, colored by age.
//!
//! `p` and `n` annotate the file as of the previous or next revision; `j`
//! as of the revision of the line at point and `a` as of the one before
//! it; `l` shows that revision's log, `d` its diff for this file and `D`
//! its whole changeset; `f` visits the file as of it; `w` goes back to the
//! working revision; `v` hides or shows the annotations; RET visits the
//! file at the line.

use super::backend::{AnnotatedLine, Kind};
use super::{Fileset, runner};
use crate::app::App;
use forge_core::decoration::{ANNOTATE_AGES, DecoFace, DecoSpec, VcFace};
use forge_core::{Buffer, BufferId, Pos};
use std::path::{Path, PathBuf};

pub const MODE: &str = "vc-annotate-mode";
const DECO: &str = "vc-annotate";

#[derive(Debug, Clone)]
pub struct AnnotateView {
    pub kind: Kind,
    pub root: PathBuf,
    pub file: String,
    /// None: the working revision.
    pub rev: Option<String>,
    pub lines: Vec<AnnotatedLine>,
    pub visible: bool,
}

/// Annotate `file` as of `rev` (None: as it is now), with point on `line`.
pub fn annotate(
    app: &mut App,
    kind: Kind,
    root: &Path,
    file: &str,
    rev: Option<String>,
    line: Option<usize>,
) {
    let cmd = kind.backend().annotate(root, file, rev.as_deref());
    let view = AnnotateView {
        kind,
        root: root.to_path_buf(),
        file: file.to_string(),
        rev,
        lines: Vec::new(),
        visible: true,
    };
    let cmd_line = cmd.line();
    app.msg("Annotating...");
    runner::run(app, cmd, "Annotating", None, move |app, out| {
        if !out.success() {
            return runner::report_failure(app, &cmd_line, &out, true);
        }
        let mut view = view;
        view.lines = view.kind.backend().parse_annotate(&out.stdout);
        let id = show(app, view, line);
        if app.active_id() != id {
            crate::windows::pop_to_other_window(app);
            app.switch_to(id);
        }
        app.msg("Annotating...done");
    });
}

fn buffer_name(v: &AnnotateView) -> String {
    let base = Path::new(&v.file)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| v.file.clone());
    match &v.rev {
        Some(r) => format!("*Annotate {base} (rev {})*", short(r)),
        None => format!("*Annotate {base}*"),
    }
}

fn short(rev: &str) -> String {
    rev.chars().take(8).collect()
}

/// Which age face a time gets: newest 1, oldest ANNOTATE_AGES.
pub fn age_bucket(time: i64, now: i64) -> u8 {
    let days = ((now - time).max(0) / 86_400) as u64;
    const LIMITS: [u64; (ANNOTATE_AGES - 1) as usize] = [1, 7, 30, 90, 180, 365, 730, 1460, 2920];
    (LIMITS
        .iter()
        .position(|&l| days < l)
        .unwrap_or(LIMITS.len())
        + 1) as u8
}

fn uncommitted(rev: &str) -> bool {
    rev.bytes().all(|b| b == b'0') || rev.ends_with('+')
}

fn render(v: &AnnotateView) -> (String, Vec<DecoSpec>) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let colors = super::config().annotate_colors;
    let author_w = v
        .lines
        .iter()
        .map(|l| l.author.chars().count())
        .max()
        .unwrap_or(0)
        .min(14);
    let num_w = v.lines.len().to_string().len();
    let mut out = String::new();
    let mut decos = Vec::new();
    let mut offset = 0usize;
    for (i, l) in v.lines.iter().enumerate() {
        let author: String = l.author.chars().take(author_w).collect();
        let rev = if uncommitted(&l.rev) {
            "--------".to_string()
        } else {
            short(&l.rev)
        };
        let prefix = if v.visible {
            format!(
                "{rev:<8} ({author:<author_w$} {} {:>num_w$}) ",
                l.date,
                i + 1
            )
        } else {
            String::new()
        };
        let line = format!("{prefix}{}", l.text);
        let len = line.chars().count();
        if colors {
            let bucket = if uncommitted(&l.rev) {
                1
            } else {
                age_bucket(l.time, now)
            };
            let end = if v.visible {
                prefix.chars().count()
            } else {
                len
            };
            if end > 0 {
                decos.push(DecoSpec::new(
                    offset,
                    offset + end,
                    DecoFace::Vc(VcFace::AnnotateAge(bucket)),
                ));
            }
        }
        out.push_str(&line);
        out.push('\n');
        offset += len + 1;
    }
    (out, decos)
}

fn show(app: &mut App, v: AnnotateView, line: Option<usize>) -> BufferId {
    let name = buffer_name(&v);
    let id = match app.editor.find_buffer(&name) {
        Some(id) => id,
        None => app.editor.add_buffer(Buffer::new(&name)),
    };
    let (text, decos) = render(&v);
    let b = app.editor.state.get_buffer_mut(id).expect("made");
    let keep = b.point().line;
    b.set_locked(false);
    b.set_contents(&text);
    b.set_major_mode(MODE);
    b.set_decorations(DECO, decos);
    b.set_locked(true);
    let l = line.map(|l| l.saturating_sub(1)).unwrap_or(keep);
    b.set_point(Pos::new(l.min(b.line_count().saturating_sub(1)), 0));
    app.vc.annotations.insert(id, v);
    id
}

fn current(app: &App) -> Option<(AnnotateView, Option<AnnotatedLine>, usize)> {
    let v = app.vc.annotations.get(&app.active_id())?.clone();
    let line = app.buf_ref().point().line;
    let l = v.lines.get(line).cloned();
    Some((v, l, line))
}

/// The annotate commands. False if `cmd` isn't one.
pub fn execute(app: &mut App, cmd: &str) -> bool {
    if !cmd.starts_with("vc-annotate-") {
        return false;
    }
    let Some((v, at, line)) = current(app) else {
        app.error("Not in an annotation buffer");
        return true;
    };
    let b = v.kind.backend();
    let committed = |app: &mut App| -> Option<AnnotatedLine> {
        match &at {
            Some(l) if !uncommitted(&l.rev) => Some(l.clone()),
            Some(_) => {
                app.error("This line isn't committed yet");
                None
            }
            None => None,
        }
    };
    match cmd {
        "vc-annotate-prev-revision" => {
            let base = v.rev.clone().or_else(|| b.working_revision(&v.root));
            let prev = match &v.rev {
                // From the working tree, the first step back is the
                // working revision itself.
                None => base,
                Some(r) => b.previous_revision(&v.root, Some(&v.file), r),
            };
            match prev {
                Some(p) => annotate(app, v.kind, &v.root, &v.file, Some(p), Some(line + 1)),
                None => app.error("No previous revision"),
            }
        }
        "vc-annotate-next-revision" => match &v.rev {
            None => app.error("Already at the working revision"),
            Some(r) => match b.next_revision(&v.root, Some(&v.file), r) {
                Some(n) => annotate(app, v.kind, &v.root, &v.file, Some(n), Some(line + 1)),
                None => annotate(app, v.kind, &v.root, &v.file, None, Some(line + 1)),
            },
        },
        "vc-annotate-working-revision" => {
            annotate(app, v.kind, &v.root, &v.file, None, Some(line + 1))
        }
        "vc-annotate-revision-at-line" => {
            if let Some(l) = committed(app) {
                annotate(
                    app,
                    v.kind,
                    &v.root,
                    &l.file.clone(),
                    Some(l.rev.clone()),
                    Some(l.orig_line),
                );
            }
        }
        "vc-annotate-revision-previous-to-line" => {
            if let Some(l) = committed(app) {
                let prev = l.previous.clone().or_else(|| {
                    b.previous_revision(&v.root, Some(&l.file), &l.rev)
                        .map(|p| (p, l.file.clone()))
                });
                match prev {
                    Some((rev, file)) => {
                        annotate(app, v.kind, &v.root, &file, Some(rev), Some(l.orig_line))
                    }
                    None => app.error("That revision added the line: there's nothing before it"),
                }
            }
        }
        "vc-annotate-show-log-revision-at-line" => {
            if let Some(l) = committed(app) {
                let fs = Fileset {
                    kind: v.kind,
                    root: v.root.clone(),
                    files: vec![(l.file.clone(), super::backend::FileState::UpToDate)],
                };
                super::log::print_log(
                    app,
                    &fs,
                    false,
                    super::config().log_limit,
                    Some(l.rev.clone()),
                    super::log::Source::Log,
                );
            }
        }
        "vc-annotate-show-diff-revision-at-line"
        | "vc-annotate-show-changeset-diff-revision-at-line" => {
            if let Some(l) = committed(app) {
                let whole = cmd.contains("changeset");
                let parent = l
                    .previous
                    .as_ref()
                    .map(|(r, _)| r.clone())
                    .or_else(|| b.previous_revision(&v.root, None, &l.rev));
                let parent = parent.unwrap_or_else(|| match v.kind {
                    Kind::Git => "4b825dc642cb6eb9a060e54bf8d69288fbee4904".into(),
                    Kind::Hg => "null".into(),
                });
                let mut files = Vec::new();
                if !whole {
                    files.push((l.file.clone(), super::backend::FileState::UpToDate));
                    if let Some((_, old)) = &l.previous
                        && old != &l.file
                    {
                        files.push((old.clone(), super::backend::FileState::UpToDate));
                    }
                }
                let fs = Fileset {
                    kind: v.kind,
                    root: v.root.clone(),
                    files,
                };
                super::show_diff(app, &fs, Some(parent), Some(l.rev.clone()), false);
            }
        }
        "vc-annotate-find-revision-at-line" => {
            if let Some(l) = committed(app) {
                super::show_revision(
                    app,
                    v.kind,
                    &v.root,
                    &l.file,
                    &l.rev,
                    true,
                    Some(l.orig_line),
                );
            }
        }
        "vc-annotate-toggle-annotation-visibility" => {
            let mut v = v;
            v.visible = !v.visible;
            let id = app.active_id();
            let (text, decos) = render(&v);
            let b = app.buf();
            b.set_locked(false);
            b.set_contents(&text);
            b.set_decorations(DECO, decos);
            b.set_locked(true);
            b.set_point(Pos::new(line.min(b.line_count().saturating_sub(1)), 0));
            app.vc.annotations.insert(id, v);
        }
        "vc-annotate-goto-line" => {
            let path = v.root.join(&v.file);
            let target = match (&v.rev, &at) {
                (None, _) => line + 1,
                (Some(_), Some(l)) => l.orig_line.max(1),
                _ => line + 1,
            };
            match app.editor.open_file(&path) {
                Ok((id, _)) => {
                    crate::windows::pop_to_other_window(app);
                    app.switch_to(id);
                    let b = app.buf();
                    let l = (target - 1).min(b.line_count().saturating_sub(1));
                    b.set_point(Pos::new(l, 0));
                }
                Err(e) => app.error(format!("Cannot open {}: {e}", path.display())),
            }
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_go_from_new_to_old() {
        let now = 1_000_000_000;
        assert_eq!(age_bucket(now, now), 1);
        assert_eq!(age_bucket(now - 3 * 86_400, now), 2);
        assert_eq!(age_bucket(now - 400 * 86_400, now), 7);
        assert_eq!(age_bucket(0, now), ANNOTATE_AGES);
    }
}
