// SPDX-License-Identifier: MIT OR Apache-2.0
use super::format::*;
use super::listing::*;
use super::render::*;
use super::*;

/// Read the directories again, keeping marks (unless `keep_marks` is
/// false), the way the buffer is laid out, the inline subdirectories and
/// point on the same file.
pub(super) fn revert(app: &mut App, keep_marks: bool) {
    reload(app, keep_marks, None, None, None);
}
/// [`revert`], with one part of the layout turned the other way and, if
/// asked, a different set of inline subdirectories (and which of them are
/// drawn as trees).
pub(super) fn reload(
    app: &mut App,
    keep_marks: bool,
    toggle: Option<&str>,
    subdirs: Option<Vec<PathBuf>>,
    trees: Option<Vec<PathBuf>>,
) {
    let Some(dir) = current_dir(app) else { return };
    let view = toggle.map_or_else(
        || current_view(app.buf_ref()),
        |what| current_view(app.buf_ref()).toggle(what),
    );
    let mut dirs = vec![dir.clone()];
    match subdirs {
        Some(list) => dirs.extend(list),
        None if view.tree => {}
        None => dirs.extend(sections(app.buf_ref()).into_iter().skip(1).map(|s| s.dir)),
    };
    let tree_set: HashSet<PathBuf> = match trees {
        Some(list) => list.into_iter().collect(),
        None if view.tree => HashSet::new(),
        None => tree_dirs_of(app.buf_ref()).into_iter().collect(),
    };
    let marks = if keep_marks {
        marks_of(app.buf_ref())
    } else {
        HashMap::new()
    };
    let point = app.buf_ref().point();
    let was = entry_on_line(app.buf_ref(), point.line).map(|e| e.path);
    let hidden: HashSet<PathBuf> = hidden_dirs_of(app.buf_ref()).into_iter().collect();
    match render_all_with(&dirs, &marks, view, &tree_set, &hidden) {
        Ok(text) => {
            let b = app.buf();
            b.set_contents(&text);
            decorate(b);
            let col = name_col(b).unwrap_or(0);
            b.set_point(Pos::new(point.line.min(last_entry_line(b)), col));
            if let Some(p) = was {
                goto_path(app, &p);
            }
        }
        Err(e) => app.error(format!("Cannot read {}: {e}", dir.display())),
    }
}
pub(super) fn dired_command(app: &mut App, cmd: &str) {
    if cmd == "dired-summary" {
        app.msg(
            "d flag  x delete  m mark  u unmark  RET visit  o other window  ^ up  C copy  R rename  + mkdir  s sort  g refresh  * % mark sets  ! shell  S H M Z links/mode/zip  C-x C-q edit names  C-c C- eza layout  (C-h m: all keys)",
        );
        return;
    }
    let Some(dir) = point_dir(app) else { return };
    let line = app.buf_ref().point().line;
    let here = entry_on_line(app.buf_ref(), line);
    match cmd {
        "dired-find-file" | "dired-find-file-other-window" => match here {
            Some(e) => {
                crate::minibuffer::visit_file(app, &e.path, cmd.ends_with("window"));
            }
            None => app.error("No file on this line"),
        },
        "dired-up-directory" => match rfs::parent(&dir) {
            Some(parent) => {
                open(app, &parent);
                goto_path(app, &dir);
            }
            None => app.error("Already at the top"),
        },
        "dired-maybe-insert-subdir" if current_view(app.buf_ref()).tree => {
            app.error("A tree already lists the subdirectories (T turns it off)")
        }
        "dired-kill-subdir" | "dired-next-subdir" | "dired-prev-subdir"
            if current_view(app.buf_ref()).tree =>
        {
            app.error("No inline subdirectories in a tree (T turns it off)")
        }
        "dired-maybe-insert-subdir" => insert_subdir(app, here),
        "dired-kill-subdir" => kill_subdir(app, line),
        "dired-next-subdir" | "dired-prev-subdir" => {
            let b = app.buf_ref();
            let secs = sections(b);
            let target = if cmd == "dired-next-subdir" {
                secs.iter().find(|s| s.header > line)
            } else {
                secs.iter().rev().find(|s| s.header + 2 < line)
            };
            match target {
                Some(s) => {
                    let (row, col) = (
                        (s.header + 2).min(b.line_count().saturating_sub(1)),
                        name_col_at(b, s.header),
                    );
                    app.buf().set_point(Pos::new(row, col));
                }
                None => app.error(if cmd == "dired-next-subdir" {
                    "No next subdirectory"
                } else {
                    "No previous subdirectory"
                }),
            }
        }
        "dired-mark" => set_mark(app, line, '*'),
        "dired-unmark" => set_mark(app, line, ' '),
        "dired-flag-file-deletion" => set_mark(app, line, 'D'),
        "dired-unmark-all-marks" => revert(app, false),
        // The layout toggles, like eza's own flags: s cycles --sort,
        // i/T --tree, and the rest switch their part on and off.
        "dired-toggle-sort"
        | "dired-toggle-hidden"
        | "dired-toggle-dirs-first"
        | "dired-toggle-tree"
        | "dired-toggle-inodes"
        | "dired-toggle-color-scale"
        | "dired-toggle-reverse"
        | "dired-toggle-git-ignore"
        | "dired-toggle-only-dirs"
        | "dired-toggle-only-files"
        | "dired-toggle-icons"
        | "dired-toggle-ls-style"
        | "dired-hide-details-mode"
        | "dired-omit-mode" => {
            let what = match cmd {
                "dired-toggle-sort" => "sort",
                "dired-toggle-hidden" => "all",
                "dired-toggle-dirs-first" => "dirs first",
                "dired-toggle-tree" => "tree",
                "dired-toggle-inodes" => "inode",
                "dired-toggle-reverse" => "reverse",
                "dired-toggle-git-ignore" => "git ignore",
                "dired-toggle-only-dirs" => "only dirs",
                "dired-toggle-only-files" => "only files",
                "dired-toggle-icons" => "icons",
                "dired-toggle-ls-style" => "ls",
                "dired-hide-details-mode" => "details",
                "dired-omit-mode" => "omit",
                _ => "scale",
            };
            let view = current_view(app.buf_ref()).toggle(what);
            reload(app, true, Some(what), None, None);
            app.msg(describe(&view, what));
        }
        "dired-tree-set-level" => {
            app.prompt("Tree level (1-8): ", Purpose::Dired(DiredAction::TreeLevel));
        }
        "dired-revert" => {
            revert(app, true);
            app.msg("Reverted");
        }
        "dired-do-flagged-delete" => {
            let flagged = paths_with_mark(app.buf_ref(), 'D');
            if flagged.is_empty() {
                return app.msg("(No deletions requested)");
            }
            ask_delete(app, flagged);
        }
        "dired-do-delete" => match targets(app, here) {
            Some(paths) => ask_delete(app, paths),
            None => app.error("No file on this line"),
        },
        "dired-do-copy" | "dired-do-rename" => {
            let copy = cmd == "dired-do-copy";
            let Some(paths) = targets(app, here) else {
                return app.error("No file on this line");
            };
            let what = match paths.as_slice() {
                [one] => one
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                many => format!("{} files", many.len()),
            };
            let (verb, action) = if copy {
                ("Copy", DiredAction::Copy(paths))
            } else {
                ("Rename", DiredAction::Rename(paths))
            };
            app.prompt(format!("{verb} {what} to: "), Purpose::Dired(action));
            if let Some(mb) = app.minibuffer.as_mut() {
                mb.input = format!("{}/", dir.display().to_string().trim_end_matches('/'));
            }
        }
        "dired-create-directory" => {
            app.prompt(
                "Create directory: ",
                Purpose::Dired(DiredAction::CreateDirectory(dir.clone())),
            );
            if let Some(mb) = app.minibuffer.as_mut() {
                mb.input = format!("{}/", dir.display().to_string().trim_end_matches('/'));
            }
        }
        _ if extra::command(app, cmd, &dir, here.clone(), line) => {}
        _ => app.error(format!("Unknown command: {cmd}")),
    }
}
/// `i`: list the directory on this line inline as an eza tree, below its
/// parent in its own section (or jump to its listing if it already has
/// one). `T` draws everything as a tree; `i` draws just this one.
pub(super) fn insert_subdir(app: &mut App, here: Option<Entry>) {
    let Some(entry) = here else {
        return app.error("No directory on this line");
    };
    let path = entry.path.clone();
    if entry.kind != 'd' && !rfs::is_dir(&path) {
        return app.error(format!("{} isn't a directory", entry.name));
    }
    let mut subdirs: Vec<PathBuf> = sections(app.buf_ref())
        .into_iter()
        .skip(1)
        .map(|s| s.dir)
        .collect();
    let mut trees = tree_dirs_of(app.buf_ref());
    let top = current_dir(app);
    if top.as_deref() != Some(path.as_path()) && !subdirs.contains(&path) {
        subdirs.push(path.clone());
        subdirs.sort();
        trees.push(path.clone());
        reload(app, true, None, Some(subdirs), Some(trees));
    } else if !trees.contains(&path) {
        // Already listed flat: draw this one as a tree instead.
        trees.push(path.clone());
        reload(app, true, None, Some(subdirs), Some(trees));
    }
    let b = app.buf();
    if let Some(sec) = sections(b).into_iter().find(|s| s.dir == path) {
        let row = (sec.header + 2).min(b.line_count().saturating_sub(1));
        let col = name_col_at(b, sec.header);
        b.set_point(Pos::new(row, col));
    }
}
/// `k`: remove the inline listing point is in (and any listings inside
/// it). The directory itself stays in its parent's listing.
pub(super) fn kill_subdir(app: &mut App, line: usize) {
    let Some(sec) = section_at(app.buf_ref(), line) else {
        return;
    };
    if sec.header == 0 {
        return app.error("Can't kill the top directory's listing (q buries the buffer)");
    }
    let subdirs: Vec<PathBuf> = sections(app.buf_ref())
        .into_iter()
        .skip(1)
        .map(|s| s.dir)
        .filter(|d| !d.starts_with(&sec.dir))
        .collect();
    let trees: Vec<PathBuf> = tree_dirs_of(app.buf_ref())
        .into_iter()
        .filter(|d| !d.starts_with(&sec.dir))
        .collect();
    reload(app, true, None, Some(subdirs), Some(trees));
    goto_path(app, &sec.dir);
}
/// Mark (or unmark) the file on `line`, then move to the next one.
pub(super) fn set_mark(app: &mut App, line: usize, mark: char) {
    let b = app.buf();
    if entry_on_line(b, line).is_none() {
        return;
    }
    b.replace_range(Pos::new(line, 0), Pos::new(line, 1), &mark.to_string());
    b.set_modified(false);
    decorate(b);
    // The next file, which may be in the next listing.
    let next = (line + 1..b.line_count())
        .find(|&l| entry_on_line(b, l).is_some())
        .unwrap_or(line);
    let col = name_col_at(b, next);
    b.set_point(Pos::new(next, col));
}
pub(super) fn paths_with_mark(b: &Buffer, mark: char) -> Vec<PathBuf> {
    entries(b)
        .into_iter()
        .filter(|(_, m, _)| *m == mark)
        .map(|(_, _, e)| e.path)
        .collect()
}
/// The `*`-marked files (in any listing), or else the one on this line.
pub(super) fn targets(app: &App, here: Option<Entry>) -> Option<Vec<PathBuf>> {
    let marked = paths_with_mark(app.buf_ref(), '*');
    if marked.is_empty() {
        here.map(|e| vec![e.path])
    } else {
        Some(marked)
    }
}

// ------------------------------------------------------------- deleting
pub(super) fn ask_delete(app: &mut App, paths: Vec<PathBuf>) {
    let prompt = match paths.as_slice() {
        [one] => format!(
            "Delete {}? (y or n) ",
            one.file_name().unwrap_or_default().to_string_lossy()
        ),
        many => format!("Delete {} files? (y or n) ", many.len()),
    };
    app.prompt(prompt, Purpose::Confirm(ConfirmAction::DiredDelete(paths)));
}
pub(super) fn transfer(app: &mut App, sources: Vec<PathBuf>, target: &Path, copy: bool) {
    let into_dir = rfs::is_dir(target);
    if sources.len() > 1 && !into_dir {
        return app.error(format!("{} isn't a directory", target.display()));
    }
    let (mut done, mut failed) = (0, Vec::new());
    for src in &sources {
        let name = src.file_name().unwrap_or_default();
        let dest = if into_dir {
            target.join(name)
        } else {
            target.to_path_buf()
        };
        let result = if rfs::exists(&dest) {
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "file already exists",
            ))
        } else if copy {
            rfs::copy(src, &dest)
        } else {
            rfs::rename(src, &dest)
        };
        match result {
            Ok(()) => done += 1,
            Err(e) => failed.push(format!("{}: {e}", name.to_string_lossy())),
        }
    }
    if app.buf_ref().major_mode() == MODE {
        revert(app, false);
    }
    let verb = if copy { "Copied" } else { "Renamed" };
    match failed.first() {
        None => app.msg(format!("{verb} {done} file(s)")),
        Some(first) => app.error(format!("{verb} {done}, {} failed ({first})", failed.len())),
    }
}
pub(super) fn copy_recursive(src: &Path, dest: &Path) -> io::Result<()> {
    if fs::symlink_metadata(src)?.is_dir() {
        if dest.starts_with(src) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "can't copy a directory into itself",
            ));
        }
        fs::create_dir(dest)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &dest.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(src, dest).map(|_| ())
    }
}
