// SPDX-License-Identifier: MIT OR Apache-2.0
//! The rest of Dired: marking by kind and regexp, links, chmod and
//! friends, shell commands on files, searching and replacing in the marked
//! files, and editing names in place (wdired).
use super::*;
use regex::Regex;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

/// Prompts these commands leave open.
pub enum Extra {
    ChangeMarks,
    MarkRegexp(char),
    MarkContaining,
    MarkExtension,
    Goto,
    Link(char, Vec<PathBuf>),
    RegexpOp(char, Vec<PathBuf>),
    Chmod(Vec<PathBuf>),
    Chown(bool, Vec<PathBuf>),
    Touch(Vec<PathBuf>),
    Shell(Vec<PathBuf>, bool),
    FindRegexp(Vec<PathBuf>),
    ReplaceFrom(Vec<PathBuf>),
    ReplaceApply(Vec<PathBuf>, String, String),
    Diff(PathBuf),
    CompressTo(Vec<PathBuf>),
}

fn dired_prompt(app: &mut App, prompt: &str, what: Extra) {
    app.prompt(prompt, Purpose::Dired(DiredAction::Extra(what)));
}

fn quote(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', "'\\''"))
}

fn name_of(p: &Path) -> String {
    p.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// Put `mark` on the first column of `line`.
fn put_mark(b: &mut Buffer, line: usize, mark: char) {
    b.replace_range(Pos::new(line, 0), Pos::new(line, 1), &mark.to_string());
    b.set_modified(false);
}

/// Mark every entry `pred` accepts; the number marked.
fn mark_where(app: &mut App, mark: char, pred: impl Fn(&Entry) -> bool) -> usize {
    let lines: Vec<usize> = entries(app.buf_ref())
        .into_iter()
        .filter(|(_, _, e)| pred(e))
        .map(|(l, _, _)| l)
        .collect();
    let b = app.buf();
    for &l in &lines {
        put_mark(b, l, mark);
    }
    decorate(b);
    lines.len()
}

fn report_marked(app: &mut App, n: usize, mark: char) {
    let what = if mark == 'D' { "flagged" } else { "marked" };
    app.msg(match n {
        0 => format!("(Nothing {what})"),
        1 => format!("1 file {what}"),
        n => format!("{n} files {what}"),
    });
}

/// Where a command's files come from: the marked ones, else this line's.
fn files(app: &App, here: &Option<Entry>) -> Option<Vec<PathBuf>> {
    targets(app, here.clone())
}

fn describe(paths: &[PathBuf]) -> String {
    match paths {
        [one] => name_of(one),
        many => format!("{} files", many.len()),
    }
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(windows)]
    {
        p.is_file()
            && p.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| {
                    matches!(
                        ext.to_ascii_lowercase().as_str(),
                        "bat" | "cmd" | "com" | "exe" | "sh"
                    )
                })
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

pub(super) fn command(
    app: &mut App,
    cmd: &str,
    dir: &Path,
    here: Option<Entry>,
    line: usize,
) -> bool {
    match cmd {
        // ----------------------------------------------------- marking
        "dired-mark-executables" => {
            let n = mark_where(app, '*', |e| is_executable(&e.path));
            report_marked(app, n, '*');
        }
        "dired-mark-directories" => {
            let n = mark_where(app, '*', |e| e.kind == 'd');
            report_marked(app, n, '*');
        }
        "dired-mark-symlinks" => {
            let n = mark_where(app, '*', |e| e.kind == 'l');
            report_marked(app, n, '*');
        }
        "dired-mark-subdir-files" => {
            let b = app.buf_ref();
            let Some(sec) = section_at(b, line) else {
                return true;
            };
            let in_sec: HashSet<usize> = entries(b)
                .into_iter()
                .filter(|(l, _, _)| section_at(b, *l).is_some_and(|s| s.header == sec.header))
                .map(|(l, _, _)| l)
                .collect();
            let b = app.buf();
            for &l in &in_sec {
                put_mark(b, l, '*');
            }
            decorate(b);
            report_marked(app, in_sec.len(), '*');
        }
        "dired-toggle-marks" => {
            let all = entries(app.buf_ref());
            let b = app.buf();
            for (l, m, _) in all {
                match m {
                    '*' => put_mark(b, l, ' '),
                    ' ' => put_mark(b, l, '*'),
                    _ => {}
                }
            }
            decorate(b);
        }
        "dired-change-marks" => dired_prompt(
            app,
            "Change marks (OLD NEW, e.g. \"* D\"): ",
            Extra::ChangeMarks,
        ),
        "dired-mark-files-regexp" => {
            dired_prompt(app, "Mark files (regexp): ", Extra::MarkRegexp('*'))
        }
        "dired-flag-files-regexp" => {
            dired_prompt(app, "Flag for deletion (regexp): ", Extra::MarkRegexp('D'))
        }
        "dired-mark-files-containing-regexp" => dired_prompt(
            app,
            "Mark files containing (regexp): ",
            Extra::MarkContaining,
        ),
        "dired-mark-extension" => {
            dired_prompt(app, "Mark files with extension: ", Extra::MarkExtension)
        }
        "dired-unmark-backward" => {
            let up = (0..line)
                .rev()
                .find(|&l| entry_on_line(app.buf_ref(), l).is_some());
            if let Some(l) = up {
                let b = app.buf();
                put_mark(b, l, ' ');
                decorate(b);
                let col = name_col_at(b, l);
                b.set_point(Pos::new(l, col));
            }
        }
        "dired-flag-backup-files" => {
            let n = mark_where(app, 'D', |e| e.name.ends_with('~'));
            report_marked(app, n, 'D');
        }
        "dired-flag-auto-save-files" => {
            let n = mark_where(app, 'D', |e| {
                e.name.len() > 2 && e.name.starts_with('#') && e.name.ends_with('#')
            });
            report_marked(app, n, 'D');
        }
        "dired-clean-directory" => {
            // Numbered backups `name.~N~`: keep the newest two of each file.
            let re = Regex::new(r"^(.*)\.~(\d+)~$").unwrap();
            let all = entries(app.buf_ref());
            let mut by_base: HashMap<String, Vec<(u64, usize)>> = HashMap::new();
            for (l, _, e) in &all {
                if let Some(c) = re.captures(&e.name) {
                    let n = c[2].parse().unwrap_or(0);
                    by_base.entry(c[1].to_string()).or_default().push((n, *l));
                }
            }
            let mut flag = Vec::new();
            for mut v in by_base.into_values() {
                v.sort();
                let keep = v.len().saturating_sub(2);
                flag.extend(v.into_iter().take(keep).map(|(_, l)| l));
            }
            let b = app.buf();
            for &l in &flag {
                put_mark(b, l, 'D');
            }
            decorate(b);
            report_marked(app, flag.len(), 'D');
        }
        "dired-next-marked-file" | "dired-prev-marked-file" => {
            let all = entries(app.buf_ref());
            let marked = |m: char| m == '*';
            let target = if cmd == "dired-next-marked-file" {
                all.iter().find(|(l, m, _)| *l > line && marked(*m))
            } else {
                all.iter().rev().find(|(l, m, _)| *l < line && marked(*m))
            };
            match target {
                Some((l, _, _)) => {
                    let b = app.buf();
                    let col = name_col_at(b, *l);
                    b.set_point(Pos::new(*l, col));
                }
                None => app.error("No more marked files"),
            }
        }
        "dired-next-dirline" | "dired-prev-dirline" => {
            let all = entries(app.buf_ref());
            let target = if cmd == "dired-next-dirline" {
                all.iter().find(|(l, _, e)| *l > line && e.kind == 'd')
            } else {
                all.iter()
                    .rev()
                    .find(|(l, _, e)| *l < line && e.kind == 'd')
            };
            match target {
                Some((l, _, _)) => {
                    let b = app.buf();
                    let col = name_col_at(b, *l);
                    b.set_point(Pos::new(*l, col));
                }
                None => app.error("No more subdirectories"),
            }
        }
        "dired-next-line" | "dired-previous-line" => {
            let n = app.count().max(1) as usize;
            let all = entries(app.buf_ref());
            let lines: Vec<usize> = all.iter().map(|(l, _, _)| *l).collect();
            let at = lines.iter().position(|&l| l >= line).unwrap_or(0);
            let to = if cmd == "dired-next-line" {
                (at + n).min(lines.len().saturating_sub(1))
            } else {
                at.saturating_sub(n)
            };
            if let Some(&l) = lines.get(to) {
                let b = app.buf();
                let col = name_col_at(b, l);
                b.set_point(Pos::new(l, col));
            }
        }
        "dired-goto-file" => dired_prompt(app, "Goto file: ", Extra::Goto),
        // ---------------------------------------------------- viewing
        "dired-view-file" => match here {
            Some(e) if e.kind != 'd' => {
                crate::minibuffer::visit_file(app, &e.path, false);
                crate::commands::execute(app, "read-only-mode");
            }
            Some(e) => open(app, &e.path),
            None => app.error("No file on this line"),
        },
        "dired-display-file" => match here {
            Some(e) => {
                crate::minibuffer::visit_file(app, &e.path, true);
                crate::commands::execute(app, "other-window");
            }
            None => app.error("No file on this line"),
        },
        "dired-find-alternate-file" => match here {
            Some(e) => {
                let old = app.active_id();
                if e.kind == 'd' {
                    open(app, &e.path);
                } else {
                    crate::minibuffer::visit_file(app, &e.path, false);
                }
                if app.active_id() != old {
                    let now = app.active_id();
                    app.editor.kill_buffer(old, Some(now));
                }
            }
            None => app.error("No file on this line"),
        },
        "dired-do-find-marked-files" => match files(app, &here) {
            Some(paths) => {
                for p in &paths {
                    if p.is_dir() {
                        open(app, p);
                    } else {
                        crate::minibuffer::visit_file(app, p, false);
                    }
                }
                app.msg(format!("Visited {}", describe(&paths)));
            }
            None => app.error("No file on this line"),
        },
        "dired-show-file-type" => match here {
            Some(e) => {
                let out = Command::new("file").arg("-b").arg(&e.path).output();
                match out {
                    Ok(o) => app.msg(format!(
                        "{}: {}",
                        e.name,
                        String::from_utf8_lossy(&o.stdout).trim()
                    )),
                    Err(err) => app.error(format!("file: {err}")),
                }
            }
            None => app.error("No file on this line"),
        },
        "dired-copy-filename-as-kill" => match files(app, &here) {
            Some(paths) => {
                let full = app.has_arg();
                let text = paths
                    .iter()
                    .map(|p| {
                        if full {
                            p.display().to_string()
                        } else {
                            name_of(p)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                app.kill_ring_save(text.clone());
                app.msg(text);
            }
            None => app.error("No file on this line"),
        },
        "dired-diff" => match here {
            Some(e) => {
                dired_prompt(app, &format!("Diff {} with: ", e.name), Extra::Diff(e.path));
                if let Some(mb) = app.minibuffer.as_mut() {
                    mb.input = format!("{}/", dir.display().to_string().trim_end_matches('/'));
                }
            }
            None => app.error("No file on this line"),
        },
        "dired-do-redisplay" => {
            revert(app, true);
        }
        "dired-do-kill-lines" => {
            if entry_on_line(app.buf_ref(), line).is_none()
                && section_at(app.buf_ref(), line).is_some_and(|s| s.header != 0)
                && paths_with_mark(app.buf_ref(), '*').is_empty()
            {
                kill_subdir(app, line);
                return true;
            }
            let marked: Vec<usize> = entries(app.buf_ref())
                .into_iter()
                .filter(|(_, m, _)| *m == '*')
                .map(|(l, _, _)| l)
                .collect();
            let lines = if marked.is_empty() {
                vec![line]
            } else {
                marked
            };
            let b = app.buf();
            for &l in lines.iter().rev() {
                if entry_on_line(b, l).is_some() {
                    let end = if l + 1 < b.line_count() {
                        Pos::new(l + 1, 0)
                    } else {
                        Pos::new(l, b.line(l).chars().count())
                    };
                    b.delete_range(Pos::new(l, 0), end);
                }
            }
            b.set_modified(false);
            decorate(b);
            app.msg(format!("Killed {} line(s)", lines.len()));
        }
        // ------------------------------------------------- operations
        "dired-do-symlink" | "dired-do-hardlink" | "dired-do-relsymlink" => {
            let kind = match cmd {
                "dired-do-symlink" => 'S',
                "dired-do-hardlink" => 'H',
                _ => 'Y',
            };
            let Some(paths) = files(app, &here) else {
                return true;
            };
            let verb = match kind {
                'S' => "Symlink",
                'H' => "Hardlink",
                _ => "Relative symlink",
            };
            dired_prompt(
                app,
                &format!("{verb} {} to: ", describe(&paths)),
                Extra::Link(kind, paths),
            );
            if let Some(mb) = app.minibuffer.as_mut() {
                mb.input = format!("{}/", dir.display().to_string().trim_end_matches('/'));
            }
        }
        "dired-do-rename-regexp"
        | "dired-do-copy-regexp"
        | "dired-do-hardlink-regexp"
        | "dired-do-symlink-regexp"
        | "dired-do-relsymlink-regexp" => {
            let (kind, verb) = match cmd {
                "dired-do-rename-regexp" => ('R', "Rename"),
                "dired-do-copy-regexp" => ('C', "Copy"),
                "dired-do-hardlink-regexp" => ('H', "Hardlink"),
                "dired-do-symlink-regexp" => ('S', "Symlink"),
                _ => ('Y', "Relative symlink"),
            };
            let Some(paths) = files(app, &here) else {
                return true;
            };
            dired_prompt(
                app,
                &format!(
                    "{verb} {} (REGEXP => NEWNAME, \\1 \\& allowed): ",
                    describe(&paths)
                ),
                Extra::RegexpOp(kind, paths),
            );
        }
        "dired-upcase" | "dired-downcase" => {
            let Some(paths) = files(app, &here) else {
                return true;
            };
            let up = cmd == "dired-upcase";
            let (mut done, mut failed) = (0, 0);
            for p in &paths {
                let n = name_of(p);
                let new = if up {
                    n.to_uppercase()
                } else {
                    n.to_lowercase()
                };
                if new != n
                    && !rfs::exists(&p.with_file_name(&new))
                    && rfs::rename(p, &p.with_file_name(&new)).is_ok()
                {
                    done += 1;
                } else if new != n {
                    failed += 1;
                }
            }
            revert(app, false);
            app.msg(format!(
                "Renamed {done} file(s){}",
                if failed > 0 {
                    format!(", {failed} failed")
                } else {
                    String::new()
                }
            ));
        }
        "dired-do-chmod" => {
            let Some(paths) = files(app, &here) else {
                return true;
            };
            dired_prompt(
                app,
                &format!(
                    "Change mode of {} to (octal or u+x,go-w): ",
                    describe(&paths)
                ),
                Extra::Chmod(paths),
            );
        }
        "dired-do-chgrp" | "dired-do-chown" => {
            let Some(paths) = files(app, &here) else {
                return true;
            };
            let group = cmd == "dired-do-chgrp";
            dired_prompt(
                app,
                &format!(
                    "Change {} of {} to: ",
                    if group { "group" } else { "owner" },
                    describe(&paths)
                ),
                Extra::Chown(group, paths),
            );
        }
        "dired-do-touch" => {
            let Some(paths) = files(app, &here) else {
                return true;
            };
            dired_prompt(
                app,
                &format!(
                    "Touch {} to (YYYY-MM-DD HH:MM, empty for now): ",
                    describe(&paths)
                ),
                Extra::Touch(paths),
            );
        }
        "dired-do-compress" => {
            let Some(paths) = files(app, &here) else {
                return true;
            };
            let (mut done, mut failed) = (0, Vec::new());
            for p in &paths {
                if p.is_dir() {
                    failed.push(format!("{}: is a directory", name_of(p)));
                    continue;
                }
                let tool = if name_of(p).ends_with(".gz") {
                    "gunzip"
                } else {
                    "gzip"
                };
                match Command::new(tool).arg(p).status() {
                    Ok(s) if s.success() => done += 1,
                    Ok(_) => failed.push(format!("{}: {tool} failed", name_of(p))),
                    Err(e) => failed.push(format!("{tool}: {e}")),
                }
            }
            revert(app, false);
            match failed.first() {
                None => app.msg(format!("Compressed or uncompressed {done} file(s)")),
                Some(f) => app.error(format!("{done} done, {} failed ({f})", failed.len())),
            }
        }
        "dired-do-compress-to" => {
            let Some(paths) = files(app, &here) else {
                return true;
            };
            dired_prompt(
                app,
                &format!(
                    "Compress {} to archive (.tar.gz .tgz .tar .zip): ",
                    describe(&paths)
                ),
                Extra::CompressTo(paths),
            );
            if let Some(mb) = app.minibuffer.as_mut() {
                mb.input = format!("{}/", dir.display().to_string().trim_end_matches('/'));
            }
        }
        "dired-do-print" => {
            let Some(paths) = files(app, &here) else {
                return true;
            };
            match Command::new("lpr").args(&paths).status() {
                Ok(s) if s.success() => app.msg(format!("Printed {}", describe(&paths))),
                Ok(_) => app.error("lpr failed"),
                Err(e) => app.error(format!("lpr: {e}")),
            }
        }
        "dired-do-shell-command" | "dired-do-async-shell-command" => {
            let Some(paths) = files(app, &here) else {
                return true;
            };
            let asynchronous = cmd.contains("async");
            dired_prompt(
                app,
                &format!(
                    "{} on {}: ",
                    if asynchronous { "& on" } else { "! on" },
                    describe(&paths)
                )
                .replace(" on on", " on"),
                Extra::Shell(paths, asynchronous),
            );
        }
        "dired-do-find-regexp" => {
            let Some(paths) = files(app, &here) else {
                return true;
            };
            dired_prompt(
                app,
                &format!("Search {} for (regexp): ", describe(&paths)),
                Extra::FindRegexp(paths),
            );
        }
        "dired-do-find-regexp-and-replace" => {
            let Some(paths) = files(app, &here) else {
                return true;
            };
            dired_prompt(
                app,
                &format!("Replace in {} (REGEXP => REPLACEMENT): ", describe(&paths)),
                Extra::ReplaceFrom(paths),
            );
        }
        "dired-hide-subdir" => {
            let Some(sec) = section_at(app.buf_ref(), line) else {
                return true;
            };
            set_hidden(app.buf(), sec.header, None);
            revert(app, true);
            let b = app.buf();
            b.set_point(Pos::new(sec.header, 0));
        }
        "dired-hide-all" => {
            let heads: Vec<usize> = sections(app.buf_ref()).iter().map(|s| s.header).collect();
            let any_shown = heads.iter().any(|&h| {
                !parse_header_line(&app.buf_ref().line(h)).is_some_and(|(_, v)| v.hidden)
            });
            let b = app.buf();
            for h in heads {
                set_hidden(b, h, Some(any_shown));
            }
            revert(app, true);
        }
        // ---------------------------------------------- editing names
        "dired-toggle-read-only" => wdired_start(app),
        _ => return false,
    }
    true
}

/// Hide or show (`None`: flip) the entries under the header on `line`.
fn set_hidden(b: &mut Buffer, header: usize, to: Option<bool>) {
    let text = b.line(header);
    let Some((dir, mut view)) = parse_header_line(&text) else {
        return;
    };
    view.hidden = to.unwrap_or(!view.hidden);
    let new = format!("  {}{}", dir.display(), view.suffix());
    b.replace_range(
        Pos::new(header, 0),
        Pos::new(header, text.chars().count()),
        &new,
    );
    b.set_modified(false);
}

// ----------------------------------------------------------- image-dired

pub const THUMBS_BUFFER: &str = "*image-dired*";
pub const THUMBS_MODE: &str = "image-dired-thumbnail-mode";

fn is_image(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "tif" | "tiff"
        )
    })
}

/// C-t d: the marked images (else every image listed), one picture each.
pub(super) fn image_dired(app: &mut App) {
    if app.buf_ref().major_mode() != MODE {
        return app.error("Not in a Dired buffer");
    }
    let marked = paths_with_mark(app.buf_ref(), '*');
    let mut images: Vec<PathBuf> = if marked.is_empty() {
        entries(app.buf_ref())
            .into_iter()
            .map(|(_, _, e)| e.path)
            .collect()
    } else {
        marked
    };
    images.retain(|p| is_image(p));
    if images.is_empty() {
        return app.error("No image files here");
    }
    let text: String = images
        .iter()
        .map(|p| format!("{}\n", p.display()))
        .collect();
    app.show_special_buffer(THUMBS_BUFFER, THUMBS_MODE, &text);
    let map: std::collections::BTreeMap<usize, PathBuf> =
        images.iter().cloned().enumerate().collect();
    let b = app.buf();
    b.set_read_only(true);
    b.set_inline_images(map);
    b.set_point(Pos::new(0, 0));
    app.msg(format!("{} image(s); RET opens one, q quits", images.len()));
}

/// RET in the thumbnails: visit the picture on this line.
pub fn image_dired_open(app: &mut App) {
    let line = app.buf_ref().point().line;
    let path = PathBuf::from(app.buf_ref().line(line).trim());
    if path.is_file() {
        crate::minibuffer::visit_file(app, &path, false);
    } else {
        app.error("No image on this line");
    }
}

// ---------------------------------------------------------------- wdired

/// C-x C-q: make the names editable. Nothing is renamed until C-c C-c.
pub(super) fn wdired_start(app: &mut App) {
    let id = app.active_id();
    let saved: Vec<(usize, PathBuf)> = entries(app.buf_ref())
        .into_iter()
        .map(|(l, _, e)| (l, e.path))
        .collect();
    app.wdired.insert(id, saved);
    let b = app.buf();
    b.set_read_only(false);
    b.set_major_mode(WDIRED_MODE);
    app.msg("Press C-c C-c to rename the files, C-c C-k to abort");
}

fn wdired_leave(app: &mut App) {
    let id = app.active_id();
    app.wdired.remove(&id);
    let b = app.buf();
    b.set_major_mode(MODE);
    b.set_read_only(true);
}

pub fn wdired_command(app: &mut App, cmd: &str) -> bool {
    match cmd {
        "wdired-abort-changes" => {
            if !app.wdired.contains_key(&app.active_id()) {
                app.error("Not editing a Dired buffer");
                return true;
            }
            wdired_leave(app);
            revert(app, true);
            app.msg("Changes aborted");
        }
        "wdired-finish-edit" => {
            let id = app.active_id();
            let Some(saved) = app.wdired.get(&id).cloned() else {
                app.error("Not editing a Dired buffer");
                return true;
            };
            let now: HashMap<usize, PathBuf> = entries(app.buf_ref())
                .into_iter()
                .map(|(l, _, e)| (l, e.path))
                .collect();
            if now.len() != saved.len() {
                app.error("The number of lines changed; fix that or abort with C-c C-k");
                return true;
            }
            let (mut done, mut failed) = (0, Vec::new());
            for (l, old) in &saved {
                let Some(new) = now.get(l) else { continue };
                if new == old {
                    continue;
                }
                if rfs::exists(new) {
                    failed.push(format!("{}: exists", name_of(new)));
                } else if let Err(e) = rfs::rename(old, new) {
                    failed.push(format!("{}: {e}", name_of(old)));
                } else {
                    done += 1;
                }
            }
            wdired_leave(app);
            revert(app, false);
            match failed.first() {
                None => app.msg(format!("{done} file(s) renamed")),
                Some(f) => app.error(format!("{done} renamed, {} failed ({f})", failed.len())),
            }
        }
        _ => return false,
    }
    true
}

// ------------------------------------------------------------- answers

/// `\1`-style groups and `\&` to the regex crate's `${1}`.
fn expand(replacement: &str) -> String {
    let mut out = String::new();
    let mut it = replacement.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\\' => match it.next() {
                Some('&') => out.push_str("${0}"),
                Some(d) if d.is_ascii_digit() => {
                    out.push_str("${");
                    out.push(d);
                    out.push('}');
                }
                Some('\\') => out.push('\\'),
                Some(o) => {
                    out.push('\\');
                    out.push(o);
                }
                None => out.push('\\'),
            },
            '$' => out.push_str("$$"),
            c => out.push(c),
        }
    }
    out
}

fn split_arrow(input: &str) -> Option<(&str, &str)> {
    input.split_once("=>").map(|(a, b)| (a.trim(), b.trim()))
}

/// A path as seen from `from_dir`, for relative symlinks.
fn relative(target: &Path, from_dir: &Path) -> PathBuf {
    let t: Vec<_> = target.components().collect();
    let f: Vec<_> = from_dir.components().collect();
    let common = t.iter().zip(&f).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..f.len() {
        out.push("..");
    }
    for c in &t[common..] {
        out.push(c);
    }
    out
}

fn make_link(kind: char, src: &Path, dest: &Path) -> io::Result<()> {
    let is_dir = src.is_dir();
    match kind {
        'H' => fs::hard_link(src, dest),
        'Y' => {
            let from = dest.parent().unwrap_or(Path::new("."));
            make_symlink(&relative(src, from), dest, is_dir)
        }
        _ => make_symlink(src, dest, is_dir),
    }
}

#[cfg(unix)]
fn make_symlink(src: &Path, dest: &Path, _is_dir: bool) -> io::Result<()> {
    std::os::unix::fs::symlink(src, dest)
}

#[cfg(windows)]
fn make_symlink(src: &Path, dest: &Path, is_dir: bool) -> io::Result<()> {
    if is_dir {
        std::os::windows::fs::symlink_dir(src, dest)
    } else {
        std::os::windows::fs::symlink_file(src, dest)
    }
}

#[cfg(not(any(unix, windows)))]
fn make_symlink(_src: &Path, _dest: &Path, _is_dir: bool) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "symbolic links are not supported on this platform",
    ))
}

fn link_all(app: &mut App, kind: char, sources: Vec<PathBuf>, target: &Path) {
    let into_dir = target.is_dir();
    if sources.len() > 1 && !into_dir {
        return app.error(format!("{} isn't a directory", target.display()));
    }
    let (mut done, mut failed) = (0, Vec::new());
    for src in &sources {
        let dest = if into_dir {
            target.join(src.file_name().unwrap_or_default())
        } else {
            target.to_path_buf()
        };
        match make_link(kind, src, &dest) {
            Ok(()) => done += 1,
            Err(e) => failed.push(format!("{}: {e}", name_of(src))),
        }
    }
    revert(app, false);
    match failed.first() {
        None => app.msg(format!("Linked {done} file(s)")),
        Some(f) => app.error(format!("Linked {done}, {} failed ({f})", failed.len())),
    }
}

/// Apply one chmod clause list to a mode.
#[cfg(unix)]
fn chmod_mode(spec: &str, old: u32, is_dir: bool) -> Option<u32> {
    if !spec.is_empty() && spec.chars().all(|c| c.is_digit(8)) {
        return u32::from_str_radix(spec, 8).ok();
    }
    let mut mode = old & 0o7777;
    for clause in spec.split(',') {
        let split = clause.find(['+', '-', '='])?;
        let who = &clause[..split];
        let who: Vec<char> = if who.is_empty() || who == "a" {
            vec!['u', 'g', 'o']
        } else {
            who.chars().collect()
        };
        let mut rest = &clause[split..];
        while let Some(op) = rest.chars().next() {
            let end = rest[1..]
                .find(['+', '-', '='])
                .map_or(rest.len(), |i| i + 1);
            let perms = &rest[1..end];
            let mut bits = 0u32;
            for p in perms.chars() {
                bits |= match p {
                    'r' => 4,
                    'w' => 2,
                    'x' => 1,
                    'X' if is_dir || old & 0o111 != 0 => 1,
                    'X' => 0,
                    's' | 't' => 0,
                    _ => return None,
                };
            }
            for w in &who {
                let shift = match w {
                    'u' => 6,
                    'g' => 3,
                    'o' => 0,
                    _ => return None,
                };
                let mask = 7 << shift;
                match op {
                    '+' => mode |= bits << shift,
                    '-' => mode &= !(bits << shift),
                    _ => mode = (mode & !mask) | (bits << shift),
                }
            }
            rest = &rest[end..];
        }
    }
    Some(mode)
}

/// Run `cmd` through the shell in `dir` as `shell-command` would.
fn shell(app: &mut App, dir: &Path, cmd: &str) {
    let id = app.active_id();
    app.jobs.dirs.insert(id, dir.to_path_buf());
    crate::compile::answered(app, &Purpose::ShellCommand { insert: false }, cmd);
}

/// An answered prompt. Returns the action back when it isn't ours.
pub(super) fn answered(app: &mut App, extra: Extra, input: &str) {
    let t = input.trim();
    let dir = current_dir(app).unwrap_or_else(|| start_dir(app));
    match extra {
        Extra::ChangeMarks => {
            let mut c = t.chars().filter(|c| !c.is_whitespace());
            let (Some(old), Some(new)) = (c.next(), c.next()) else {
                return app.error("Give the old and the new mark, like \"* D\"");
            };
            let all = entries(app.buf_ref());
            let b = app.buf();
            let mut n = 0;
            for (l, m, _) in all {
                if m == old {
                    put_mark(b, l, new);
                    n += 1;
                }
            }
            decorate(b);
            app.msg(format!("{n} mark(s) changed"));
        }
        Extra::MarkRegexp(mark) => match Regex::new(t) {
            Ok(re) => {
                let n = mark_where(app, mark, |e| re.is_match(&e.name));
                report_marked(app, n, mark);
            }
            Err(e) => app.error(format!("Bad regexp: {e}")),
        },
        Extra::MarkContaining => match Regex::new(t) {
            Ok(re) => {
                let n = mark_where(app, '*', |e| {
                    e.kind != 'd' && fs::read_to_string(&e.path).is_ok_and(|s| re.is_match(&s))
                });
                report_marked(app, n, '*');
            }
            Err(e) => app.error(format!("Bad regexp: {e}")),
        },
        Extra::MarkExtension => {
            let ext = t.trim_start_matches('.');
            if ext.is_empty() {
                return;
            }
            let suffix = format!(".{ext}");
            let n = mark_where(app, '*', |e| e.name.ends_with(&suffix));
            report_marked(app, n, '*');
        }
        Extra::Goto => {
            let typed = crate::minibuffer::expand_path(t);
            let path = if typed.is_absolute() {
                typed
            } else {
                dir.join(typed)
            };
            if !goto_path(app, &path) {
                // A bare name anywhere in the listings.
                let hit = entries(app.buf_ref())
                    .into_iter()
                    .find(|(_, _, e)| e.name == t);
                match hit {
                    Some((l, _, _)) => {
                        let b = app.buf();
                        let col = name_col_at(b, l);
                        b.set_point(Pos::new(l, col));
                    }
                    None => app.error(format!("No file {t} in this listing")),
                }
            }
        }
        Extra::Link(kind, sources) => {
            if t.is_empty() {
                return;
            }
            let typed = crate::minibuffer::expand_path(t);
            let target = if typed.is_absolute() {
                typed
            } else {
                dir.join(typed)
            };
            link_all(app, kind, sources, &target);
        }
        Extra::RegexpOp(kind, sources) => {
            let Some((from, to)) = split_arrow(t) else {
                return app.error("Type REGEXP => NEWNAME");
            };
            let re = match Regex::new(from) {
                Ok(r) => r,
                Err(e) => return app.error(format!("Bad regexp: {e}")),
            };
            let rep = expand(to);
            let (mut done, mut failed) = (0, Vec::new());
            for src in &sources {
                let name = name_of(src);
                if !re.is_match(&name) {
                    continue;
                }
                let new = re.replace(&name, rep.as_str()).into_owned();
                let dest = src.parent().unwrap_or(Path::new(".")).join(&new);
                let result = if rfs::exists(&dest) {
                    Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "file already exists",
                    ))
                } else {
                    match kind {
                        'R' => rfs::rename(src, &dest),
                        'C' => rfs::copy(src, &dest),
                        k => make_link(k, src, &dest),
                    }
                };
                match result {
                    Ok(()) => done += 1,
                    Err(e) => failed.push(format!("{name}: {e}")),
                }
            }
            revert(app, false);
            match failed.first() {
                None => app.msg(format!("{done} file(s) done")),
                Some(f) => app.error(format!("{done} done, {} failed ({f})", failed.len())),
            }
        }
        Extra::Chmod(_paths) => {
            #[cfg(unix)]
            {
                if t.is_empty() {
                    return;
                }
                let (mut done, mut failed) = (0, Vec::new());
                for p in &_paths {
                    if rfs::is_remote(p) {
                        match rfs::chmod(p, t) {
                            Ok(()) => done += 1,
                            Err(e) => failed.push(format!("{}: {e}", name_of(p))),
                        }
                        continue;
                    }
                    let Ok(meta) = fs::metadata(p) else {
                        failed.push(format!("{}: cannot stat", name_of(p)));
                        continue;
                    };
                    let Some(mode) = chmod_mode(t, meta.permissions().mode(), meta.is_dir()) else {
                        return app.error(format!("Bad mode: {t}"));
                    };
                    match fs::set_permissions(p, fs::Permissions::from_mode(mode)) {
                        Ok(()) => done += 1,
                        Err(e) => failed.push(format!("{}: {e}", name_of(p))),
                    }
                }
                revert(app, true);
                match failed.first() {
                    None => app.msg(format!("Changed the mode of {done} file(s)")),
                    Some(f) => app.error(format!("{done} done, {} failed ({f})", failed.len())),
                }
            }
            #[cfg(not(unix))]
            app.error("Changing file modes is not supported on this platform");
        }
        Extra::Chown(group, paths) => {
            if t.is_empty() {
                return;
            }
            let tool = if group { "chgrp" } else { "chown" };
            match Command::new(tool).arg(t).args(&paths).output() {
                Ok(o) if o.status.success() => {
                    revert(app, true);
                    app.msg(format!("{tool} {t}: {}", describe(&paths)));
                }
                Ok(o) => app.error(String::from_utf8_lossy(&o.stderr).trim().to_string()),
                Err(e) => app.error(format!("{tool}: {e}")),
            }
        }
        Extra::Touch(paths) => {
            let mut c = Command::new("touch");
            if !t.is_empty() {
                c.arg("-d").arg(t);
            }
            match c.args(&paths).output() {
                Ok(o) if o.status.success() => {
                    revert(app, true);
                    app.msg(format!("Touched {}", describe(&paths)));
                }
                Ok(o) => app.error(String::from_utf8_lossy(&o.stderr).trim().to_string()),
                Err(e) => app.error(format!("touch: {e}")),
            }
        }
        Extra::Shell(paths, asynchronous) => {
            if t.is_empty() {
                return;
            }
            let names: Vec<String> = paths
                .iter()
                .map(|p| quote(Path::new(&name_of(p))))
                .collect();
            let mut cmd = if t.contains('?') && !t.contains('*') {
                // `?` runs the command once for each file.
                names
                    .iter()
                    .map(|n| t.replace('?', n))
                    .collect::<Vec<_>>()
                    .join("; ")
            } else if t.contains('*') {
                t.replace('*', &names.join(" "))
            } else {
                format!("{t} {}", names.join(" "))
            };
            if asynchronous {
                cmd.push_str(" &");
            }
            shell(app, &dir, &cmd);
        }
        Extra::FindRegexp(paths) => {
            if t.is_empty() {
                return;
            }
            let list: Vec<String> = paths.iter().map(|p| quote(p)).collect();
            let pat = t.replace('\'', "'\\''");
            shell(
                app,
                &dir,
                &format!("grep -nH -E -e '{pat}' -- {}", list.join(" ")),
            );
        }
        Extra::ReplaceFrom(paths) => {
            let Some((from, to)) = split_arrow(t) else {
                return app.error("Type REGEXP => REPLACEMENT");
            };
            let re = match Regex::new(from) {
                Ok(r) => r,
                Err(e) => return app.error(format!("Bad regexp: {e}")),
            };
            let (mut matches, mut in_files) = (0, 0);
            for p in paths.iter().filter(|p| p.is_file()) {
                if let Ok(s) = fs::read_to_string(p) {
                    let n = re.find_iter(&s).count();
                    if n > 0 {
                        matches += n;
                        in_files += 1;
                    }
                }
            }
            if matches == 0 {
                return app.msg("No matches in the marked files".to_string());
            }
            app.prompt(
                format!("Replace {matches} match(es) in {in_files} file(s)? (y or n) "),
                Purpose::Dired(DiredAction::Extra(Extra::ReplaceApply(
                    paths,
                    from.to_string(),
                    to.to_string(),
                ))),
            );
        }
        Extra::ReplaceApply(paths, from, to) => {
            if !t.eq_ignore_ascii_case("y") && !t.eq_ignore_ascii_case("yes") {
                return app.msg("Replacement cancelled".to_string());
            }
            let Ok(re) = Regex::new(&from) else { return };
            let rep = expand(&to);
            let (mut files, mut failed) = (0, Vec::new());
            for p in paths.iter().filter(|p| p.is_file()) {
                let Ok(s) = fs::read_to_string(p) else {
                    continue;
                };
                if !re.is_match(&s) {
                    continue;
                }
                let new = re.replace_all(&s, rep.as_str()).into_owned();
                match fs::write(p, new) {
                    Ok(()) => files += 1,
                    Err(e) => failed.push(format!("{}: {e}", name_of(p))),
                }
            }
            match failed.first() {
                None => app.msg(format!(
                    "Replaced in {files} file(s); revert open buffers to see it"
                )),
                Some(f) => app.error(format!("{files} done, {} failed ({f})", failed.len())),
            }
        }
        Extra::Diff(a) => {
            if t.is_empty() {
                return;
            }
            let typed = crate::minibuffer::expand_path(t);
            let b = if typed.is_absolute() {
                typed
            } else {
                dir.join(typed)
            };
            shell(app, &dir, &format!("diff -u {} {}", quote(&a), quote(&b)));
        }
        Extra::CompressTo(paths) => {
            if t.is_empty() {
                return;
            }
            let typed = crate::minibuffer::expand_path(t);
            let archive = if typed.is_absolute() {
                typed
            } else {
                dir.join(typed)
            };
            let names: Vec<String> = paths
                .iter()
                .map(|p| quote(Path::new(&name_of(p))))
                .collect();
            let a = quote(&archive);
            let lower = archive.display().to_string().to_lowercase();
            let cmd = if lower.ends_with(".zip") {
                format!("zip -r {a} {}", names.join(" "))
            } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
                format!("tar czf {a} {}", names.join(" "))
            } else if lower.ends_with(".tar") {
                format!("tar cf {a} {}", names.join(" "))
            } else {
                return app.error("Use .tar.gz, .tgz, .tar or .zip");
            };
            match Command::new("sh")
                .arg("-c")
                .arg(&cmd)
                .current_dir(&dir)
                .output()
            {
                Ok(o) if o.status.success() => {
                    revert(app, true);
                    app.msg(format!("Wrote {}", archive.display()));
                }
                Ok(o) => app.error(String::from_utf8_lossy(&o.stderr).trim().to_string()),
                Err(e) => app.error(format!("{e}")),
            }
        }
    }
}
