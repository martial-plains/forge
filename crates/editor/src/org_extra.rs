// SPDX-License-Identifier: MIT OR Apache-2.0
//! Column view (`org-columns`) and attachments (`org-attach`).

use crate::app::App;
use crate::org_cmds::{Ask, ask, ask_with, choose, now};
use forge_core::org_columns::{self as oc, Col, Row};
use forge_core::org_work::PriorityAction;
use forge_core::{Buffer, BufferId, Pos};
use std::path::{Path, PathBuf};

pub const COLUMNS_BUFFER: &str = "*Org Columns*";
pub const COLUMNS_MODE: &str = "org-columns-mode";

pub const COMMANDS: &[forge_docs::CommandDoc] = &[
    forge_docs::CommandDoc::new(
        "org-link-preview",
        "Org: preview the links in the active region, at point, or in the current",
    )
    .body(
        "section (C-c C-x C-v). A prefix argument clears previews at point or in the \
         current section, two prefix arguments preview all links in the buffer, three \
         hide them all. A numeric 1 previews described links too (toggling the \
         region/point/section); a numeric 11 toggles them in the whole buffer.",
    ),
    forge_docs::CommandDoc::new(
        "org-link-preview-refresh",
        "Org: preview the links in the whole buffer and look at the linked files again",
    )
    .body("(C-c C-x C-M-v)."),
    forge_docs::CommandDoc::new(
        "org-link-preview-region",
        "Org: preview the links in the active region, or the whole buffer. A prefix",
    )
    .body("argument previews links that have a description too."),
    forge_docs::CommandDoc::line(
        "org-link-preview-clear",
        "Org: clear link previews in the active region, or the whole buffer.",
    ),
    forge_docs::CommandDoc::line(
        "org-global-cycle",
        "Org: cycle the whole buffer: overview, contents, everything.",
    ),
    forge_docs::CommandDoc::new(
        "org-columns",
        "Org: show the column view of this subtree (or the whole file before the first",
    )
    .body("heading)."),
    forge_docs::CommandDoc::line(
        "org-columns-edit-value",
        "Org columns: edit the value in the cell at point.",
    ),
    forge_docs::CommandDoc::line(
        "org-columns-edit-allowed",
        "Org columns: set the allowed values (NAME_ALL) of the column at point.",
    ),
    forge_docs::CommandDoc::line("org-columns-quit", "Org columns: leave the column view."),
    forge_docs::CommandDoc::line(
        "org-columns-next-allowed-value",
        "Org columns: switch the cell to the next allowed value.",
    ),
    forge_docs::CommandDoc::line(
        "org-columns-previous-allowed-value",
        "Org columns: switch the cell to the previous allowed value.",
    ),
    forge_docs::CommandDoc::line(
        "org-columns-show-value",
        "Org columns: show the full value of the cell at point.",
    ),
    forge_docs::CommandDoc::line(
        "org-columns-widen",
        "Org columns: make the column at point wider (with a prefix argument, narrower).",
    ),
    forge_docs::CommandDoc::line("org-columns-redo", "Org columns: rebuild the column view."),
    forge_docs::CommandDoc::line(
        "org-columns-new",
        "Org columns: add a column, given as %width NAME (title){summary}.",
    ),
    forge_docs::CommandDoc::line(
        "org-columns-delete",
        "Org columns: remove the column at point.",
    ),
    forge_docs::CommandDoc::line(
        "org-columns-capture-view",
        "Org columns: insert the view as an Org table in the source buffer.",
    ),
    forge_docs::CommandDoc::line(
        "org-attach",
        "Org attach: choose an attachment command for this entry.",
    ),
    forge_docs::CommandDoc::new(
        "org-attach-attach",
        "Org attach: copy a file into the entry's attachment directory (with a prefix",
    )
    .body("argument, move it)."),
    forge_docs::CommandDoc::line(
        "org-attach-buffer",
        "Org attach: attach the contents of a buffer as a file.",
    ),
    forge_docs::CommandDoc::line(
        "org-attach-new",
        "Org attach: create a new empty attachment and visit it.",
    ),
    forge_docs::CommandDoc::line(
        "org-attach-open",
        "Org attach: visit an attachment of this entry.",
    ),
    forge_docs::CommandDoc::line(
        "org-attach-open-in-forge",
        "Org attach: visit an attachment of this entry in the editor.",
    ),
    forge_docs::CommandDoc::line(
        "org-attach-reveal",
        "Org attach: list the entry's attachment directory.",
    ),
    forge_docs::CommandDoc::line(
        "org-attach-reveal-in-forge",
        "Org attach: list the entry's attachment directory in the editor.",
    ),
    forge_docs::CommandDoc::line(
        "org-attach-sync",
        "Org attach: set the ATTACH tag from what the attachment directory holds.",
    ),
    forge_docs::CommandDoc::new(
        "org-attach-dired-to-subtree",
        "Org attach: attach the marked directory-listing files to the heading in the last",
    )
    .body("Org buffer."),
];

pub const COLUMNS_BINDINGS: &[(&str, &str)] = &[
    ("e", "org-columns-edit-value"),
    ("C-c C-c", "org-columns-edit-value"),
    ("E", "org-columns-edit-allowed"),
    ("q", "org-columns-quit"),
    ("n", "org-columns-next-allowed-value"),
    ("p", "org-columns-previous-allowed-value"),
    ("v", "org-columns-show-value"),
    ("<", "org-columns-widen"),
    (">", "org-columns-widen"),
    ("g", "org-columns-redo"),
    ("r", "org-columns-redo"),
    ("a", "org-columns-edit-allowed"),
    ("N", "org-columns-new"),
    ("D", "org-columns-delete"),
    ("c", "org-columns-capture-view"),
];

pub struct ColumnsState {
    pub source: BufferId,
    pub range: (usize, usize),
    pub cols: Vec<Col>,
    pub rows: Vec<Row>,
    pub buffer: Option<BufferId>,
    pub edit: Option<(usize, usize)>,
}

fn err(app: &mut App, m: impl Into<String>) {
    app.error(m.into());
}

// --------------------------------------------------------------- columns

fn show_columns(app: &mut App, mut st: ColumnsState) {
    let text = {
        let Some(b) = app.editor.state.get_buffer(st.source) else {
            return;
        };
        st.rows = b.org_columns_rows(&st.cols, st.range.0, st.range.1);
        let mut lines = oc::render(&st.cols, &st.rows);
        lines.push(String::new());
        lines.push(format!("Columns: {}", oc::format_to_string(&st.cols)));
        lines.join("\n")
    };
    let id = match app.editor.find_buffer(COLUMNS_BUFFER) {
        Some(id) => id,
        None => app.editor.add_buffer(Buffer::new(COLUMNS_BUFFER)),
    };
    let keep = if app.active_id() == id {
        Some(app.buf_ref().point())
    } else {
        None
    };
    if let Some(b) = app.editor.state.get_buffer_mut(id) {
        b.set_locked(false);
        b.set_contents(&text);
        b.set_major_mode(COLUMNS_MODE);
        b.set_locked(true);
    }
    if app.active_id() != id {
        app.switch_to(id);
    }
    let p = keep.unwrap_or(Pos::new(2.min(st.rows.len() + 1), 0));
    let last = app.buf_ref().line_count().saturating_sub(1);
    app.buf().set_point(Pos::new(p.line.min(last), p.col));
    st.buffer = Some(id);
    app.org.columns = Some(st);
}

fn rebuild_columns(app: &mut App) {
    if let Some(st) = app.org.columns.take() {
        show_columns(app, st);
    }
}

/// The (row, column) under point in the columns buffer.
fn cell(app: &App) -> Option<(usize, usize)> {
    let st = app.org.columns.as_ref()?;
    if app.buf_ref().major_mode() != COLUMNS_MODE {
        return None;
    }
    let p = app.buf_ref().point();
    let row = p.line.checked_sub(2).filter(|r| *r < st.rows.len())?;
    Some((row, oc::column_at(&st.cols, p.col)))
}

fn apply_cell(app: &mut App, row: usize, col: usize, value: &str) -> Result<(), String> {
    let (src, h, prop) = {
        let st = app.org.columns.as_ref().ok_or("No column view")?;
        (st.source, st.rows[row].line, st.cols[col].prop.clone())
    };
    let b = app
        .editor
        .state
        .get_buffer_mut(src)
        .ok_or("The source buffer is gone")?;
    match prop.as_str() {
        "ITEM" => return Err("Edit the heading text in the Org buffer".into()),
        "TODO" => crate::org_agenda_ui::set_todo_keyword(b, h, value),
        "TAGS" => {
            let tags: Vec<String> = value
                .split([':', ',', ' '])
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .collect();
            b.org_set_tags(h, &tags);
        }
        "PRIORITY" => {
            let act = match value.chars().next() {
                None | Some(' ') => PriorityAction::Remove,
                Some(c) => PriorityAction::Set(c.to_ascii_uppercase()),
            };
            b.org_set_priority(h, act)?;
        }
        p => {
            if value.is_empty() {
                b.org_delete_property(h, p);
            } else {
                b.org_set_property(h, p, value);
            }
        }
    }
    Ok(())
}

fn columns_command(app: &mut App, cmd: &str) -> bool {
    if cmd == "org-columns" {
        let src = app.active_id();
        if app.buf_ref().major_mode() != forge_core::org::MODE {
            err(app, "Not in an Org buffer");
            return true;
        }
        let line = app.buf_ref().point().line;
        let h = app.buf_ref().org_heading_at(line);
        let range = match h {
            Some(h) => (h, app.buf_ref().org_subtree_end(h)),
            None => (0, app.buf_ref().line_count().saturating_sub(1)),
        };
        let fmt = app.buf_ref().org_columns_format(h);
        match oc::parse_format(&fmt) {
            Ok(cols) => show_columns(
                app,
                ColumnsState {
                    source: src,
                    range,
                    cols,
                    rows: Vec::new(),
                    buffer: None,
                    edit: None,
                },
            ),
            Err(e) => err(app, e),
        }
        return true;
    }
    if app.org.columns.is_none() || app.buf_ref().major_mode() != COLUMNS_MODE {
        err(app, "Not in an Org column view; open one with org-columns");
        return true;
    }
    let back = app.has_arg();
    match cmd {
        "org-columns-redo" => rebuild_columns(app),
        "org-columns-quit" => {
            let (src, id) = {
                let st = app.org.columns.take().expect("checked");
                (st.source, app.active_id())
            };
            app.kill_buffer(id);
            if app.editor.state.buffers.contains_key(&src) {
                app.switch_to(src);
            }
        }
        "org-columns-edit-value" => {
            let Some((r, c)) = cell(app) else {
                return err_ret(app, "No cell at point");
            };
            let st = app.org.columns.as_mut().expect("checked");
            st.edit = Some((r, c));
            let cur = st.rows[r].cells[c].clone();
            let name = st.cols[c].prop.clone();
            let cur = if name == "TAGS" {
                cur.trim_matches(':').to_string()
            } else {
                cur
            };
            ask_with(app, &format!("Edit {name}: "), Ask::ColumnsEdit, &cur);
        }
        "org-columns-next-allowed-value" | "org-columns-previous-allowed-value" => {
            let Some((r, c)) = cell(app) else {
                return err_ret(app, "No cell at point");
            };
            let (src, h, prop, cur) = {
                let st = app.org.columns.as_ref().expect("checked");
                (
                    st.source,
                    st.rows[r].line,
                    st.cols[c].prop.clone(),
                    st.rows[r].cells[c].clone(),
                )
            };
            let allowed = app
                .editor
                .state
                .get_buffer(src)
                .map(|b| b.org_allowed_values(h, &prop))
                .unwrap_or_default();
            if allowed.is_empty() {
                err(app, format!("No allowed values for {prop}"));
                return true;
            }
            let fwd = cmd.contains("next") != back;
            let i = allowed.iter().position(|a| *a == cur);
            let n = allowed.len();
            let next = match (i, fwd) {
                (Some(i), true) => (i + 1) % n,
                (Some(i), false) => (i + n - 1) % n,
                (None, true) => 0,
                (None, false) => n - 1,
            };
            match apply_cell(app, r, c, &allowed[next]) {
                Ok(()) => rebuild_columns(app),
                Err(e) => err(app, e),
            }
        }
        "org-columns-edit-allowed" => {
            let Some((r, c)) = cell(app) else {
                return err_ret(app, "No cell at point");
            };
            let st = app.org.columns.as_mut().expect("checked");
            st.edit = Some((r, c));
            let (src, h, prop) = (st.source, st.rows[r].line, st.cols[c].prop.clone());
            let cur = app
                .editor
                .state
                .get_buffer(src)
                .and_then(|b| b.org_get_property(h, &format!("{prop}_ALL"), true))
                .unwrap_or_default();
            ask_with(
                app,
                &format!("Allowed values for {prop} (space separated): "),
                Ask::ColumnsAllowed,
                &cur,
            );
        }
        "org-columns-show-value" => {
            let Some((r, c)) = cell(app) else {
                return err_ret(app, "No cell at point");
            };
            let st = app.org.columns.as_ref().expect("checked");
            let m = format!("{}: {}", st.cols[c].prop, st.rows[r].cells[c]);
            app.msg(m);
        }
        "org-columns-widen" => {
            let col = {
                let p = app.buf_ref().point();
                let st = app.org.columns.as_ref().expect("checked");
                oc::column_at(&st.cols, p.col)
            };
            let st = app.org.columns.as_mut().expect("checked");
            let w = &mut st.cols[col].width;
            *w = if back {
                w.saturating_sub(1).max(2)
            } else {
                *w + 1
            };
            rebuild_columns(app);
        }
        "org-columns-new" => ask(
            app,
            "New column (%width NAME(title){summary}): ",
            Ask::ColumnsNew,
        ),
        "org-columns-delete" => {
            let p = app.buf_ref().point();
            let st = app.org.columns.as_mut().expect("checked");
            let c = oc::column_at(&st.cols, p.col);
            if st.cols.len() <= 1 {
                err(app, "Cannot delete the last column");
                return true;
            }
            st.cols.remove(c);
            rebuild_columns(app);
        }
        "org-columns-capture-view" => {
            let st = app.org.columns.as_ref().expect("checked");
            let mut out = String::new();
            out.push_str(&format!(
                "| {} |\n|-\n",
                st.cols
                    .iter()
                    .map(|c| c.title.clone())
                    .collect::<Vec<_>>()
                    .join(" | ")
            ));
            for r in &st.rows {
                out.push_str(&format!("| {} |\n", r.cells.join(" | ")));
            }
            let src = st.source;
            let at = app
                .editor
                .state
                .get_buffer(src)
                .map_or(0, |b| b.line_count());
            if let Some(b) = app.editor.state.get_buffer_mut(src) {
                b.org_insert_line_after(at.saturating_sub(1), out.trim_end());
            }
            app.msg("Column view inserted at the end of the source buffer".to_string());
        }
        _ => return false,
    }
    true
}

fn err_ret(app: &mut App, m: &str) -> bool {
    app.error(m.to_string());
    true
}

// ------------------------------------------------------------ attachments

fn new_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut x = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64)
        ^ ((std::process::id() as u64) << 32);
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let (a, b) = (next(), next());
    let h = format!("{a:016x}{b:016x}");
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// The attachment directory of the entry at `h` in `id`, creating the ID
/// when `create`. None for a buffer without a file.
fn attach_dir(app: &mut App, id: BufferId, h: usize, create: bool) -> Result<PathBuf, String> {
    let b = app.editor.state.get_buffer_mut(id).ok_or("No buffer")?;
    let base = b
        .path()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .ok_or("This buffer has no file; save it first")?;
    if let Some(d) = b.org_get_property(h, "DIR", true) {
        let p = crate::vc::expand_home(&d);
        return Ok(if p.is_absolute() { p } else { base.join(p) });
    }
    let mut idv = b.org_get_property(h, "ID", false);
    if idv.is_none() {
        if !create {
            return Err("This entry has no attachments".into());
        }
        let n = new_id();
        b.org_set_property(h, "ID", &n);
        idv = Some(n);
    }
    let idv = idv.expect("set above");
    let compact: String = idv.chars().filter(|c| *c != '-').collect();
    let (a, rest) = compact.split_at(2.min(compact.len()));
    Ok(base.join("data").join(a).join(rest))
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().is_file())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn tag_attach(app: &mut App, id: BufferId, h: usize) {
    if let Some(b) = app.editor.state.get_buffer_mut(id)
        && !b.org_heading_tags(h).iter().any(|t| t == "ATTACH")
    {
        b.org_toggle_tag(h, "ATTACH");
    }
}

fn here(app: &mut App) -> Option<(BufferId, usize)> {
    let b = app.buf_ref();
    if b.major_mode() != forge_core::org::MODE {
        app.error("Not in an Org buffer");
        return None;
    }
    let h = b.org_heading_at(b.point().line);
    match h {
        Some(h) => Some((app.active_id(), h)),
        None => {
            app.error("Not in an outline entry");
            None
        }
    }
}

/// Attach the file at `src` to entry `h` of `id` (copy, or move when `mv`).
fn attach_file(
    app: &mut App,
    id: BufferId,
    h: usize,
    src: &Path,
    mv: bool,
) -> Result<String, String> {
    if !src.is_file() {
        return Err(format!("Not a file: {}", src.display()));
    }
    let dir = attach_dir(app, id, h, true)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
    let name = src.file_name().ok_or("No file name")?;
    let dest = dir.join(name);
    if dest.exists() {
        return Err(format!("{} is already attached", name.to_string_lossy()));
    }
    std::fs::copy(src, &dest).map_err(|e| format!("Cannot copy: {e}"))?;
    if mv {
        std::fs::remove_file(src)
            .map_err(|e| format!("Copied, but cannot remove the original: {e}"))?;
    }
    tag_attach(app, id, h);
    Ok(name.to_string_lossy().to_string())
}

fn attach_command(app: &mut App, cmd: &str) -> bool {
    match cmd {
        "org-attach" => {
            choose(
                app,
                "Attachment command: ",
                Ask::AttachDispatch,
                [
                    "a: attach a file",
                    "b: attach a buffer",
                    "n: new attachment",
                    "o: open an attachment",
                    "f: list the directory",
                    "s: sync the ATTACH tag",
                ]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            );
        }
        "org-attach-attach" => {
            if here(app).is_some() {
                ask(
                    app,
                    "File to attach: ",
                    Ask::AttachFile { mv: app.has_arg() },
                );
            }
        }
        "org-attach-buffer" => {
            if here(app).is_some() {
                let names: Vec<String> = app
                    .editor
                    .state
                    .buffers
                    .values()
                    .map(|b| b.name().to_string())
                    .filter(|n| !n.starts_with('*'))
                    .collect();
                choose(app, "Attach buffer: ", Ask::AttachBuffer, names);
            }
        }
        "org-attach-new" => {
            if here(app).is_some() {
                ask(app, "New attachment name: ", Ask::AttachNew);
            }
        }
        "org-attach-open" | "org-attach-open-in-forge" => {
            let Some((id, h)) = here(app) else {
                return true;
            };
            match attach_dir(app, id, h, false) {
                Ok(d) => {
                    let f = files_in(&d);
                    if f.is_empty() {
                        err(app, "No attachments");
                    } else {
                        choose(app, "Open attachment: ", Ask::AttachOpen, f);
                    }
                }
                Err(e) => err(app, e),
            }
        }
        "org-attach-reveal" | "org-attach-reveal-in-forge" => {
            let Some((id, h)) = here(app) else {
                return true;
            };
            match attach_dir(app, id, h, false) {
                Ok(d) if d.is_dir() => crate::dired::open(app, &d),
                Ok(d) => err(app, format!("{} does not exist yet", d.display())),
                Err(e) => err(app, e),
            }
        }
        "org-attach-sync" => {
            let Some((id, h)) = here(app) else {
                return true;
            };
            let has = attach_dir(app, id, h, false)
                .map(|d| !files_in(&d).is_empty())
                .unwrap_or(false);
            if let Some(b) = app.editor.state.get_buffer_mut(id) {
                let tagged = b.org_heading_tags(h).iter().any(|t| t == "ATTACH");
                if has != tagged {
                    b.org_toggle_tag(h, "ATTACH");
                }
            }
            app.msg(
                if has {
                    "Entry has attachments; ATTACH tag set"
                } else {
                    "No attachments; ATTACH tag removed"
                }
                .to_string(),
            );
        }
        "org-attach-dired-to-subtree" => {
            let Some(files) = crate::dired::selected_paths(app) else {
                err(app, "Not in a directory listing");
                return true;
            };
            let target = app
                .editor
                .state
                .buffers
                .iter()
                .filter(|(_, b)| b.major_mode() == forge_core::org::MODE)
                .map(|(id, _)| *id)
                .min();
            let Some(id) = target else {
                err(app, "No Org buffer to attach to");
                return true;
            };
            let h = app
                .editor
                .state
                .get_buffer(id)
                .and_then(|b| b.org_heading_at(b.point().line));
            let Some(h) = h else {
                err(app, "Point is not in an entry of the Org buffer");
                return true;
            };
            let (mut ok, mut bad) = (0, Vec::new());
            for f in files {
                match attach_file(app, id, h, &f, false) {
                    Ok(_) => ok += 1,
                    Err(e) => bad.push(e),
                }
            }
            app.msg(if bad.is_empty() {
                format!("Attached {ok} file(s)")
            } else {
                format!("Attached {ok}; {}", bad.join("; "))
            });
        }
        _ => return false,
    }
    true
}

// ------------------------------------------------------- link previews

fn link_at_point(app: &App) -> Option<usize> {
    let b = app.buf_ref();
    let p = b.point();
    let links = forge_core::org_ext::links_in(&b.line(p.line));
    links
        .iter()
        .any(|l| p.col >= l.start && p.col < l.end)
        .then_some(p.line)
}

/// The lines an `org-link-preview` acts on: the active region, else the
/// link at point, else the current section (Elisp's
/// `org-link-preview--get-link` fallbacks).
fn preview_target(app: &App) -> (usize, usize) {
    if let Some((a, b)) = app.buf_ref().active_region()
        && a != b
    {
        return (a.line.min(b.line), a.line.max(b.line));
    }
    if let Some(l) = link_at_point(app) {
        return (l, l);
    }
    current_section(app)
}

fn current_section(app: &App) -> (usize, usize) {
    let b = app.buf_ref();
    match b.org_heading_at(b.point().line) {
        Some(h) => (h, b.org_subtree_end(h)),
        None => (0, b.line_count().saturating_sub(1)),
    }
}

/// Turn previews on or off for the inclusive line range `(a, b)`. Turning
/// them off while the whole buffer is previewed falls back to ranges for
/// the rest of the buffer.
fn set_previews(app: &mut App, a: usize, b: usize, described: Option<bool>, on: bool) {
    let lines = app.buf_ref().line_count().saturating_sub(1);
    let d = &mut app.buf().org_display;
    if let Some(x) = described {
        d.link_preview_described = x;
    }
    if on {
        d.link_preview_ranges.push((a.min(lines), b.min(lines)));
        d.link_preview_ranges.sort_unstable();
        return;
    }
    if d.link_preview {
        d.link_preview = false;
        if a > 0 {
            d.link_preview_ranges.push((0, a - 1));
        }
        if b < lines {
            d.link_preview_ranges.push((b + 1, lines));
        }
        d.link_preview_ranges.sort_unstable();
        return;
    }
    let mut kept = Vec::new();
    for &(x, y) in &d.link_preview_ranges {
        if y < a || x > b {
            kept.push((x, y));
        } else {
            if x < a {
                kept.push((x, a - 1));
            }
            if y > b {
                kept.push((b + 1, y));
            }
        }
    }
    d.link_preview_ranges = kept;
}

fn toggle_previews(app: &mut App, a: usize, b: usize, described: bool) {
    let d = &app.buf_ref().org_display;
    let showing = d.previews_any() && (a..=b).all(|li| d.previews_line(li));
    set_previews(app, a, b, Some(described), !showing);
}

fn preview_command(app: &mut App, cmd: &str) -> bool {
    if app.buf_ref().major_mode() != forge_core::org::MODE {
        err(app, "Not in an Org buffer");
        return true;
    }
    match cmd {
        "org-link-preview-refresh" => {
            app.buf_ref().org_display.clear_cache();
            app.buf().org_display.link_preview = true;
            app.buf().org_display.link_preview_ranges.clear();
            app.msg("Link previews refreshed for the whole buffer".to_string());
        }
        "org-link-preview-region" => {
            let whole = app.buf_ref().active_region().is_none_or(|(a, b)| a == b);
            let described = app.has_arg();
            if whole {
                app.buf().org_display.link_preview = true;
                app.buf().org_display.link_preview_ranges.clear();
            } else {
                let (a, b) = preview_target(app);
                set_previews(app, a, b, Some(described), true);
            }
            app.buf().org_display.link_preview_described = described;
            app.msg(if described {
                "Link previews on (described links too)".to_string()
            } else {
                "Link previews on".to_string()
            });
        }
        "org-link-preview-clear" => {
            let whole = app.buf_ref().active_region().is_none_or(|(a, b)| a == b);
            if whole {
                app.buf().org_display.link_preview = false;
                app.buf().org_display.link_preview_ranges.clear();
            } else {
                let (a, b) = preview_target(app);
                set_previews(app, a, b, None, false);
            }
            app.msg("Link previews cleared".to_string());
        }
        "org-link-preview" => preview_at_point_or_all(app),
        _ => return false,
    }
    true
}

fn preview_at_point_or_all(app: &mut App) {
    use crate::app::PrefixArg;
    match app.prefix_arg {
        // C-u C-u C-u (or more): hide every preview.
        Some(PrefixArg::Raw(n)) if n >= 64 => {
            app.buf().org_display.link_preview = false;
            app.buf().org_display.link_preview_ranges.clear();
            app.buf().org_display.link_preview_described = false;
            app.msg("Link previews hidden for the whole buffer".to_string());
        }
        // C-u C-u: preview every link in the buffer.
        Some(PrefixArg::Raw(16)) => {
            app.buf().org_display.link_preview = true;
            app.buf().org_display.link_preview_ranges.clear();
            app.msg("Link previews on for the whole buffer".to_string());
        }
        // C-u: clear previews at point or in the current section.
        Some(PrefixArg::Raw(_)) => {
            let (a, b) = match link_at_point(app) {
                Some(l) => (l, l),
                None => current_section(app),
            };
            set_previews(app, a, b, None, false);
            app.msg("Link previews cleared".to_string());
        }
        // C-u 11: toggle described previews in the whole buffer.
        Some(PrefixArg::Number(11)) => {
            let on = !app.buf_ref().org_display.link_preview;
            app.buf().org_display.link_preview_described = true;
            app.buf().org_display.link_preview = on;
            app.buf().org_display.link_preview_ranges.clear();
            app.msg(if on {
                "Link previews on for the whole buffer (described links too)".to_string()
            } else {
                "Link previews off".to_string()
            });
        }
        // C-u 1: toggle described previews in the region/point/section.
        Some(PrefixArg::Number(1)) => {
            let (a, b) = preview_target(app);
            toggle_previews(app, a, b, true);
            app.msg("Link previews toggled (described links too)".to_string());
        }
        _ => {
            let (a, b) = preview_target(app);
            set_previews(app, a, b, None, true);
            app.msg("Link previews on".to_string());
        }
    }
}

pub fn command(app: &mut App, cmd: &str) -> bool {
    if cmd.starts_with("org-link-preview") {
        return preview_command(app, cmd);
    }
    if cmd == "org-global-cycle" {
        return crate::commands::execute(app, "org-shifttab");
    }
    if cmd.starts_with("org-columns") {
        return columns_command(app, cmd);
    }
    if cmd.starts_with("org-attach") {
        return attach_command(app, cmd);
    }
    false
}

pub fn answer(app: &mut App, a: &Ask, input: &str) -> bool {
    let t = input.trim();
    match a {
        Ask::ColumnsEdit => {
            let edit = app.org.columns.as_ref().and_then(|s| s.edit);
            if let Some((r, c)) = edit {
                match apply_cell(app, r, c, t) {
                    Ok(()) => rebuild_columns(app),
                    Err(e) => err(app, e),
                }
            }
        }
        Ask::ColumnsAllowed => {
            let info = app.org.columns.as_ref().and_then(|s| {
                s.edit
                    .map(|(r, c)| (s.source, s.rows[r].line, s.cols[c].prop.clone()))
            });
            if let Some((src, h, prop)) = info {
                if let Some(b) = app.editor.state.get_buffer_mut(src) {
                    if t.is_empty() {
                        b.org_delete_property(h, &format!("{prop}_ALL"));
                    } else {
                        b.org_set_property(h, &format!("{prop}_ALL"), t);
                    }
                }
                rebuild_columns(app);
            }
        }
        Ask::ColumnsNew => match oc::parse_format(t) {
            Ok(mut cols) => {
                if let Some(st) = app.org.columns.as_mut() {
                    st.cols.append(&mut cols);
                }
                rebuild_columns(app);
            }
            Err(e) => err(app, e),
        },
        Ask::AttachDispatch => {
            let cmd = match t.chars().next() {
                Some('a') => "org-attach-attach",
                Some('b') => "org-attach-buffer",
                Some('n') => "org-attach-new",
                Some('o') => "org-attach-open",
                Some('f') => "org-attach-reveal",
                Some('s') => "org-attach-sync",
                _ => return err_ret(app, "No such attachment command"),
            };
            attach_command(app, cmd);
        }
        Ask::AttachFile { mv } => {
            let Some((id, h)) = here(app) else {
                return true;
            };
            let src = crate::vc::expand_home(t);
            match attach_file(app, id, h, &src, *mv) {
                Ok(n) => app.msg(format!("Attached {n}")),
                Err(e) => err(app, e),
            }
        }
        Ask::AttachBuffer => {
            let Some((id, h)) = here(app) else {
                return true;
            };
            let found = app
                .editor
                .state
                .buffers
                .values()
                .find(|b| b.name() == t)
                .map(|b| (b.name().to_string(), b.get_contents()));
            match found {
                Some((name, text)) => match attach_dir(app, id, h, true) {
                    Ok(d) => {
                        let r = std::fs::create_dir_all(&d)
                            .and_then(|_| std::fs::write(d.join(&name), text));
                        match r {
                            Ok(()) => {
                                tag_attach(app, id, h);
                                app.msg(format!("Attached buffer {name}"));
                            }
                            Err(e) => err(app, format!("Cannot write attachment: {e}")),
                        }
                    }
                    Err(e) => err(app, e),
                },
                None => err(app, format!("No buffer {t}")),
            }
        }
        Ask::AttachNew => {
            let Some((id, h)) = here(app) else {
                return true;
            };
            if t.is_empty() || t.contains('/') {
                return err_ret(app, "Give a plain file name");
            }
            match attach_dir(app, id, h, true) {
                Ok(d) => {
                    let p = d.join(t);
                    let r = std::fs::create_dir_all(&d).and_then(|_| {
                        if p.exists() {
                            Ok(())
                        } else {
                            std::fs::write(&p, "")
                        }
                    });
                    match r {
                        Ok(()) => {
                            tag_attach(app, id, h);
                            crate::minibuffer::visit_file(app, &p, false);
                        }
                        Err(e) => err(app, format!("Cannot create attachment: {e}")),
                    }
                }
                Err(e) => err(app, e),
            }
        }
        Ask::AttachOpen => {
            let Some((id, h)) = here(app) else {
                return true;
            };
            match attach_dir(app, id, h, false) {
                Ok(d) => {
                    let p = d.join(t);
                    if p.is_file() {
                        crate::minibuffer::visit_file(app, &p, false);
                    } else {
                        err(app, format!("No attachment {t}"));
                    }
                }
                Err(e) => err(app, e),
            }
        }
        _ => return false,
    }
    let _ = now;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_uuid_shaped_and_differ() {
        let (a, b) = (new_id(), new_id());
        assert_eq!(a.len(), 36);
        assert_eq!(a.matches('-').count(), 4);
        assert_ne!(a, b);
    }

    fn app_with(text: &str, path: &Path) -> App {
        let mut app = App::new(forge_core::Editor::default());
        let mut b = Buffer::new("t.org");
        b.set_contents(text);
        b.set_major_mode(forge_core::org::MODE);
        b.set_visited_file(path.to_path_buf());
        let id = app.editor.add_buffer(b);
        app.editor.show_buffer(id);
        app
    }

    #[test]
    fn attach_copies_a_file_and_tags_the_entry() {
        let dir = std::env::temp_dir().join(format!("forge-attach-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("note.txt");
        std::fs::write(&src, "hi").unwrap();
        let mut app = app_with("* Entry\n", &dir.join("t.org"));
        assert!(answer(
            &mut app,
            &Ask::AttachFile { mv: false },
            src.to_str().unwrap()
        ));
        let text = app.buf_ref().get_contents();
        assert!(text.contains(":ID:") && text.contains(":ATTACH:"), "{text}");
        let id = app
            .buf_ref()
            .org_get_property(0, "ID", false)
            .unwrap()
            .replace('-', "");
        assert!(
            dir.join("data")
                .join(&id[..2])
                .join(&id[2..])
                .join("note.txt")
                .is_file()
        );
        // Attaching it again is refused.
        answer(
            &mut app,
            &Ask::AttachFile { mv: false },
            src.to_str().unwrap(),
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn display_commands_toggle_the_buffers_org_display() {
        let mut app = app_with(
            "| <4> | |\n|-----+-|\n| abcdefgh | x |\n",
            Path::new("/tmp/x.org"),
        );
        app.buf().set_point(Pos::new(2, 2));
        assert!(!app.buf_ref().org_display.table_shrink);
        assert!(crate::commands::execute(&mut app, "org-table-shrink"));
        assert!(app.buf_ref().org_display.table_shrink);
        assert!(crate::commands::execute(&mut app, "org-table-expand"));
        assert!(!app.buf_ref().org_display.table_shrink);
        assert!(crate::commands::execute(
            &mut app,
            "org-table-header-line-mode"
        ));
        assert!(app.buf_ref().org_display.table_header);
        assert!(crate::commands::execute(
            &mut app,
            "org-link-preview-region"
        ));
        assert!(app.buf_ref().org_display.link_preview);
        assert!(crate::commands::execute(&mut app, "org-link-preview-clear"));
        assert!(!app.buf_ref().org_display.link_preview);
        // No cookie, no shrinking.
        let mut app = app_with("| abcdefgh | x |\n", Path::new("/tmp/y.org"));
        crate::commands::execute(&mut app, "org-table-shrink");
        assert!(!app.buf_ref().org_display.table_shrink);
    }

    #[test]
    fn link_preview_previews_the_section_and_prefix_args_toggle_it() {
        let mut app = app_with(
            "one [[file:a.png]]\n* Section\n[[file:b.png]]\nmore\n* Other\n[[file:c.png]]\n",
            Path::new("/tmp/x.org"),
        );
        // Point in the second section: only that subtree previews.
        app.buf().set_point(Pos::new(4, 0));
        assert!(command(&mut app, "org-link-preview"));
        let d = &app.buf_ref().org_display;
        assert!(d.previews_line(4) && d.previews_line(5));
        assert!(!d.previews_line(0) && !d.previews_line(2));
        // C-u C-u previews the whole buffer.
        app.prefix_arg = Some(crate::app::PrefixArg::Raw(16));
        assert!(command(&mut app, "org-link-preview"));
        assert!(app.buf_ref().org_display.link_preview);
        // C-u C-u C-u hides everything.
        app.prefix_arg = Some(crate::app::PrefixArg::Raw(64));
        assert!(command(&mut app, "org-link-preview"));
        let d = &app.buf_ref().org_display;
        assert!(!d.link_preview && d.link_preview_ranges.is_empty());
    }

    #[test]
    fn link_preview_at_point_previews_only_that_line() {
        let mut app = app_with("a\n[[file:b.png]]\nc\n", Path::new("/tmp/x.org"));
        app.buf().set_point(Pos::new(1, 3));
        assert!(command(&mut app, "org-link-preview"));
        let d = &app.buf_ref().org_display;
        assert!(d.previews_line(1));
        assert!(!d.previews_line(0) && !d.previews_line(2));
        // With no active region and no link at point, preview the section.
        app.buf().set_point(Pos::new(0, 0));
        assert!(command(&mut app, "org-link-preview"));
        assert!(app.buf_ref().org_display.previews_line(0));
    }

    #[test]
    fn column_view_edits_and_cycles_values() {
        let mut app = app_with(
            "#+COLUMNS: %15ITEM %Effort\n* Task\n:PROPERTIES:\n:Effort_ALL: 1:00 2:00\n:END:\n",
            Path::new("/tmp/x.org"),
        );
        app.buf().set_point(Pos::new(1, 0));
        assert!(command(&mut app, "org-columns"));
        assert_eq!(app.buf_ref().major_mode(), COLUMNS_MODE);
        // Cursor on the Effort cell of the first row.
        app.buf().set_point(Pos::new(2, 20));
        assert!(command(&mut app, "org-columns-next-allowed-value"));
        assert!(app.org.columns.as_ref().unwrap().rows[0].cells[1] == "1:00");
        assert!(command(&mut app, "org-columns-quit"));
        assert!(app.buf_ref().get_contents().contains(":Effort: 1:00"));
    }
}
