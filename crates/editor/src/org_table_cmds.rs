// SPDX-License-Identifier: MIT OR Apache-2.0
//! Org table commands (Phase 11, stage 3): the editor side of
//! `forge_core::org_table` and `org_calc`.

use crate::app::App;
use crate::org_cmds::{Ask, ask, ask_with, choose};
use forge_core::org_table::{Row, Separator, SortKind, field_index, is_table_line, is_tblfm};
use forge_core::{Buffer, Pos};
use forge_docs::CommandDoc;

pub const COMMANDS: &[CommandDoc] = &[
    CommandDoc::line(
        "org-table-create",
        "Org table: insert an empty table, asking for its size (COLUMNSxROWS).",
    ),
    CommandDoc::new(
        "org-table-create-or-convert-from-region",
        "Org table: turn the region into a table (tab, comma or space separated),",
    )
    .body("or insert a new table."),
    CommandDoc::line(
        "org-table-import",
        "Org table: insert a table from a delimited text file.",
    ),
    CommandDoc::line(
        "org-table-export",
        "Org table: write the table at point to a file (tsv, csv, orgtbl, html, latex).",
    ),
    CommandDoc::line(
        "org-table-header-line-mode",
        "Org table: toggle keeping a long table's header row on top of the window.",
    ),
    CommandDoc::line(
        "org-table-transpose-table-at-point",
        "Org table: swap the table's rows and columns.",
    ),
    CommandDoc::line("org-table-align", "Org table: realign the table at point."),
    CommandDoc::line(
        "org-table-goto-line",
        "Org table: move to the Nth data line of the table.",
    ),
    CommandDoc::line(
        "org-table-beginning-of-field",
        "Org table: move to the start of the field at point.",
    ),
    CommandDoc::line(
        "org-table-end-of-field",
        "Org table: move to the end of the field at point.",
    ),
    CommandDoc::line(
        "org-table-next-field",
        "Org table: realign and move to the next field, adding a row at the end.",
    ),
    CommandDoc::line(
        "org-table-previous-field",
        "Org table: realign and move to the previous field.",
    ),
    CommandDoc::new(
        "org-table-next-row",
        "Org table: realign and move to the same field in the next row,",
    )
    .body("adding a row at the end."),
    CommandDoc::line(
        "org-table-move-row-up",
        "Org table: move the current row up.",
    ),
    CommandDoc::line(
        "org-table-move-row-down",
        "Org table: move the current row down.",
    ),
    CommandDoc::line(
        "org-table-move-column-left",
        "Org table: move the current column left.",
    ),
    CommandDoc::line(
        "org-table-move-column-right",
        "Org table: move the current column right.",
    ),
    CommandDoc::line(
        "org-table-insert-row",
        "Org table: insert an empty row above (with a prefix argument, below).",
    ),
    CommandDoc::line(
        "org-table-kill-row",
        "Org table: delete the current row, putting it in the kill ring.",
    ),
    CommandDoc::line(
        "org-table-insert-column",
        "Org table: insert an empty column before the current one.",
    ),
    CommandDoc::line(
        "org-table-delete-column",
        "Org table: delete the current column.",
    ),
    CommandDoc::line(
        "org-table-insert-hline",
        "Org table: insert a horizontal rule above (with a prefix argument, below).",
    ),
    CommandDoc::line(
        "org-table-hline-and-move",
        "Org table: insert a rule below, and move to the row after it.",
    ),
    CommandDoc::line(
        "org-table-blank-field",
        "Org table: empty the field at point.",
    ),
    CommandDoc::new(
        "org-table-copy-down",
        "Org table: copy the field above into this one, adding one to numbers,",
    )
    .body("and go down."),
    CommandDoc::line(
        "org-table-kill-region",
        "Org table: cut the fields between mark and point.",
    ),
    CommandDoc::line(
        "org-table-copy-region",
        "Org table: copy the fields between mark and point.",
    ),
    CommandDoc::line(
        "org-table-paste-rectangle",
        "Org table: paste the copied fields with their top left at point.",
    ),
    CommandDoc::new(
        "org-table-wrap-region",
        "Org table: wrap the text of the field at point into several rows of a given",
    )
    .body("width."),
    CommandDoc::new(
        "org-table-shrink",
        "Org table: show columns with a <N> width cookie cut to N characters (point's",
    )
    .body("line stays in full)."),
    CommandDoc::line(
        "org-table-expand",
        "Org table: show shrunk columns in full again.",
    ),
    CommandDoc::line(
        "org-table-sort-lines",
        "Org table: sort the table's rows by the column at point.",
    ),
    CommandDoc::new(
        "org-table-recalculate",
        "Org table: apply the table's #+TBLFM: formulas (with a prefix argument,",
    )
    .body("repeat until nothing changes)."),
    CommandDoc::line(
        "org-table-recalculate-buffer-tables",
        "Org table: recalculate every table in the buffer that has a #+TBLFM: line.",
    ),
    CommandDoc::line(
        "org-table-iterate",
        "Org table: recalculate the table until nothing changes.",
    ),
    CommandDoc::line(
        "org-table-iterate-buffer-tables",
        "Org table: recalculate every table in the buffer until nothing changes.",
    ),
    CommandDoc::new(
        "org-table-eval-formula",
        "Org table: evaluate a formula for the field at point (with a prefix argument,",
    )
    .body("make it the column's formula)."),
    CommandDoc::line(
        "org-table-edit-formulas",
        "Org table: edit the table's #+TBLFM: formulas.",
    ),
    CommandDoc::line(
        "org-table-toggle-formula-debugger",
        "Org table: toggle reporting how each formula was evaluated.",
    ),
    CommandDoc::new(
        "org-table-toggle-coordinate-overlays",
        "Org table: toggle showing the @row$column of the field in the echo area as point",
    )
    .body("moves."),
    CommandDoc::line(
        "org-table-rotate-recalc-marks",
        "Org table: cycle the recalculation mark in the first column of this row.",
    ),
    CommandDoc::new(
        "org-table-sum",
        "Org table: sum the numbers above point in this column (with a prefix argument,",
    )
    .body("put it in the field)."),
    CommandDoc::line(
        "org-table-field-info",
        "Org table: show the coordinates and text of the field at point.",
    ),
];

/// Point, if it is in a table.
fn here(app: &mut App) -> Option<Pos> {
    let p = app.buf_ref().point();
    if is_table_line(&app.buf_ref().line(p.line)) {
        Some(p)
    } else {
        app.error("Not in a table");
        None
    }
}

/// `@row$col` of point, rows counted without rules.
pub fn coordinates(b: &Buffer, p: Pos) -> String {
    let Some((first, rows, _)) = b.org_table_rows(p.line) else {
        return String::new();
    };
    let row = rows[..=(p.line - first)]
        .iter()
        .filter(|r| !r.is_rule())
        .count();
    format!("@{row}${}", field_index(&b.line(p.line), p.col) + 1)
}

fn moved(app: &mut App, to: Option<Pos>) {
    if let Some(q) = to {
        app.buf().set_point(q);
        if app.org.coordinate_overlays {
            let c = coordinates(app.buf_ref(), q);
            app.msg(c);
        }
    }
}

pub fn next_row(app: &mut App) {
    let p = app.buf_ref().point();
    let to = app.buf().org_table_next_row(p);
    moved(app, to);
    recalc_if_marked(app);
}

pub fn next_field(app: &mut App) {
    let p = app.buf_ref().point();
    let to = app.buf().org_table_next_field(p);
    moved(app, to);
}

pub fn previous_field(app: &mut App) {
    let p = app.buf_ref().point();
    let to = app.buf().org_table_previous_field(p);
    moved(app, to);
}

/// Rows marked `#` are recalculated when you leave them; do it quietly.
fn recalc_if_marked(_app: &mut App) {}

/// The region as two points, whether or not it is active.
fn region(app: &App) -> Option<(Pos, Pos)> {
    let b = app.buf_ref();
    let m = b.mark()?;
    Some((m, b.point()))
}

fn recalc(app: &mut App, line: usize, iterate: bool) {
    match app.buf().org_table_recalculate(line, iterate, None) {
        Ok(n) => app.msg(format!("Recalculated: {n} field(s) changed")),
        Err(e) => app.error(e),
    }
}

fn recalc_all(app: &mut App, iterate: bool) {
    let (n, errors) = app.buf().org_table_recalculate_all(iterate);
    if errors.is_empty() {
        app.msg(format!("Recalculated {n} table(s)"));
    } else {
        app.error(errors.join("; "));
    }
}

/// The `#+TBLFM:` lines under the table at `line`, as one formula text.
fn formulas_text(b: &Buffer, line: usize) -> String {
    let Some((_, last)) = b.org_table_bounds(line) else {
        return String::new();
    };
    b.org_table_tblfm_lines(last)
        .into_iter()
        .map(|l| b.line(l).trim_start()[8..].trim().to_string())
        .collect::<Vec<_>>()
        .join(" :: ")
}

/// Replace the table's TBLFM lines by one made from `text`.
fn set_formulas(b: &mut Buffer, line: usize, text: &str) -> bool {
    let Some((_, last)) = b.org_table_bounds(line) else {
        return false;
    };
    let old = b.org_table_tblfm_lines(last);
    let t = text.trim();
    match (old.first().copied(), t.is_empty()) {
        (Some(first), true) => {
            for l in old.iter().rev() {
                b.org_delete_line(*l);
            }
            let _ = first;
        }
        (Some(first), false) => {
            for l in old.iter().skip(1).rev() {
                b.org_delete_line(*l);
            }
            b.org_set_line(first, &format!("#+TBLFM: {t}"));
        }
        (None, false) => b.org_insert_line_after(last, &format!("#+TBLFM: {t}")),
        (None, true) => {}
    }
    true
}

pub fn command(app: &mut App, cmd: &str) -> bool {
    if !cmd.starts_with("org-table-") {
        return false;
    }
    let back = app.has_arg();
    let line = app.buf_ref().point().line;
    match cmd {
        "org-table-create" => ask_with(app, "Table size (COLUMNSxROWS): ", Ask::TableCreate, "5x2"),
        "org-table-create-or-convert-from-region" => match app.buf_ref().active_region() {
            Some((a, b)) => {
                let last = if b.col == 0 && b.line > a.line {
                    b.line - 1
                } else {
                    b.line
                };
                let sep = match app.raw_arg() {
                    true if app.count() == 4 => Some(Separator::Tab),
                    true if app.count() == 16 => Some(Separator::Comma),
                    true => Some(Separator::Spaces(1)),
                    false if app.has_arg() => Some(Separator::Spaces(app.count().max(1) as usize)),
                    false => None,
                };
                let n = app.buf().org_table_convert_lines(a.line, last, sep);
                app.buf().deactivate_mark();
                app.msg(format!("Converted {n} line(s) to a table"));
            }
            None => ask_with(app, "Table size (COLUMNSxROWS): ", Ask::TableCreate, "5x2"),
        },
        "org-table-import" => ask(app, "Import table from file: ", Ask::TableImportFile),
        "org-table-export" => {
            if here(app).is_some() {
                choose(
                    app,
                    "Export format: ",
                    Ask::TableExportFormat,
                    ["tsv", "csv", "orgtbl", "html", "latex"]
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                );
            }
        }
        "org-table-header-line-mode" => {
            let on = !app.buf_ref().org_display.table_header;
            app.buf().org_display.table_header = on;
            app.org.header_line = on;
            app.msg(format!(
                "Org table header line {}",
                if on { "on" } else { "off" }
            ));
        }
        "org-table-transpose-table-at-point" => {
            if here(app).is_some() && !app.buf().org_table_transpose(line) {
                app.error("Not in a table");
            }
        }
        "org-table-align" => {
            if let Some(p) = here(app) {
                let to = app.buf().org_table_align(p.line, p.col);
                moved(app, to);
            }
        }
        "org-table-goto-line" => {
            if here(app).is_some() {
                ask(app, "Table line: ", Ask::TableGotoLine);
            }
        }
        "org-table-beginning-of-field" | "org-table-end-of-field" => {
            if let Some(p) = here(app) {
                let to = app.buf_ref().org_table_field_edge(p, cmd.contains("end"));
                moved(app, to);
            }
        }
        "org-table-next-field" => {
            if here(app).is_some() {
                next_field(app);
            }
        }
        "org-table-previous-field" => {
            if here(app).is_some() {
                previous_field(app);
            }
        }
        "org-table-next-row" => {
            if here(app).is_some() {
                next_row(app);
            }
        }
        "org-table-move-row-up" | "org-table-move-row-down" => {
            if let Some(p) = here(app)
                && !app.buf().org_table_move_row(p, cmd.ends_with("down"))
            {
                app.error("Cannot move the row further");
            }
        }
        "org-table-move-column-left" | "org-table-move-column-right" => {
            if let Some(p) = here(app)
                && !app.buf().org_table_move_column(p, cmd.ends_with("right"))
            {
                app.error("Cannot move the column further");
            }
        }
        "org-table-insert-row" => {
            if let Some(p) = here(app) {
                let to = app.buf().org_table_insert_row(p, back);
                moved(app, to);
            }
        }
        "org-table-insert-hline" => {
            if let Some(p) = here(app) {
                let to = app.buf().org_table_insert_hline(p, back);
                moved(app, to);
            }
        }
        "org-table-hline-and-move" => {
            if let Some(p) = here(app) {
                let to = app.buf().org_table_hline_and_move(p);
                moved(app, to);
            }
        }
        "org-table-kill-row" => {
            if let Some(p) = here(app)
                && let Some(text) = app.buf().org_table_kill_row(p)
            {
                app.kill(text, false);
            }
        }
        "org-table-insert-column" => {
            if let Some(p) = here(app) {
                let to = app.buf().org_table_insert_column(p);
                moved(app, to);
            }
        }
        "org-table-delete-column" => {
            if let Some(p) = here(app) {
                let to = app.buf().org_table_delete_column(p);
                moved(app, to);
            }
        }
        "org-table-blank-field" => {
            if let Some(p) = here(app) {
                app.buf().org_table_set_field(p, "");
            }
        }
        "org-table-copy-down" => {
            if let Some(p) = here(app) {
                let to = app.buf().org_table_copy_down(p);
                match to {
                    Some(_) => moved(app, to),
                    None => app.error("No field above"),
                }
            }
        }
        "org-table-kill-region" | "org-table-copy-region" => {
            if here(app).is_some() {
                match region(app) {
                    Some((a, b)) => {
                        let cut = cmd.contains("kill");
                        match app.buf().org_table_copy_region(a, b, cut) {
                            Some(text) => {
                                app.org.table_clip = Some(text.clone());
                                app.kill(text, false);
                                app.buf().deactivate_mark();
                                app.msg(
                                    if cut { "Fields cut" } else { "Fields copied" }.to_string(),
                                );
                            }
                            None => app.error("Region is not in a table"),
                        }
                    }
                    None => app.error("No region"),
                }
            }
        }
        "org-table-paste-rectangle" => {
            if let Some(p) = here(app) {
                match app.org.table_clip.clone() {
                    Some(t) => {
                        let n = app.buf().org_table_paste_rectangle(p, &t).unwrap_or(0);
                        app.msg(format!("Pasted {n} field(s)"));
                    }
                    None => app.error("Nothing copied with org-table-copy-region"),
                }
            }
        }
        "org-table-wrap-region" => {
            if here(app).is_some() {
                ask_with(app, "Wrap to width: ", Ask::TableWrapWidth, "30");
            }
        }
        "org-table-shrink" | "org-table-expand" => {
            let on = cmd == "org-table-shrink";
            if on {
                let has_cookie = here(app).is_some_and(|p| {
                    let b = app.buf_ref();
                    b.org_table_bounds(p.line).is_some_and(|(a, z)| {
                        (a..=z).any(|l| {
                            b.line(l).contains("<")
                                && b.line(l).split('|').any(|c| {
                                    let c = c.trim();
                                    c.starts_with('<')
                                        && c.ends_with('>')
                                        && c.len() > 2
                                        && c[1..c.len() - 1]
                                            .trim_start_matches(['l', 'r', 'c'])
                                            .chars()
                                            .all(|d| d.is_ascii_digit())
                                })
                        })
                    })
                });
                if !has_cookie {
                    app.error("No <N> width cookie in this table; put one such as <10> in a row of the column to shrink".to_string());
                    return true;
                }
            }
            app.buf().org_display.table_shrink = on;
            app.msg(
                if on {
                    "Columns with width cookies shrunk (point's line shows in full)"
                } else {
                    "Columns expanded"
                }
                .to_string(),
            );
        }
        "org-table-sort-lines" => {
            if here(app).is_some() {
                choose(
                    app,
                    "Sort column: ",
                    Ask::TableSort,
                    [
                        "a: alphabetic",
                        "A: alphabetic, reversed",
                        "n: numeric",
                        "N: numeric, reversed",
                        "t: time",
                        "T: time, reversed",
                        "x: alphabetic, case-sensitive",
                        "X: alphabetic, case-sensitive, reversed",
                    ]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                );
            }
        }
        "org-table-recalculate" | "org-table-iterate" => {
            if let Some(p) = here(app) {
                recalc(app, p.line, cmd == "org-table-iterate" || back);
            }
        }
        "org-table-recalculate-buffer-tables" => recalc_all(app, false),
        "org-table-iterate-buffer-tables" => recalc_all(app, true),
        "org-table-eval-formula" => {
            if here(app).is_some() {
                ask(
                    app,
                    if back {
                        "Column formula: "
                    } else {
                        "Field formula: "
                    },
                    Ask::TableFormula { column: back },
                );
            }
        }
        "org-table-edit-formulas" => {
            if here(app).is_some() {
                let text = formulas_text(app.buf_ref(), line);
                ask_with(
                    app,
                    "Formulas (separated by ::): ",
                    Ask::TableEditFormulas,
                    &text,
                );
            }
        }
        "org-table-toggle-formula-debugger" => {
            app.org.formula_debug = !app.org.formula_debug;
            app.msg(format!(
                "Formula debugger {}",
                if app.org.formula_debug { "on" } else { "off" }
            ));
        }
        "org-table-toggle-coordinate-overlays" => {
            app.org.coordinate_overlays = !app.org.coordinate_overlays;
            app.msg(format!(
                "Field coordinates {} (shown in the echo area as point moves)",
                if app.org.coordinate_overlays {
                    "on"
                } else {
                    "off"
                }
            ));
        }
        "org-table-rotate-recalc-marks" => {
            if let Some(p) = here(app) {
                match app.buf().org_table_rotate_mark(p) {
                    Some(m) => app.msg(format!("Mark: {}", if m.is_empty() { "none" } else { &m })),
                    None => app.error("Not on a data row"),
                }
            }
        }
        "org-table-sum" => {
            if let Some(p) = here(app) {
                match app.buf().org_table_column_sum(p) {
                    Some(v) => {
                        let text = forge_core::org_calc::format_number(v);
                        if back {
                            app.buf().org_table_set_field(p, &text);
                        }
                        app.kill(text.clone(), false);
                        app.msg(format!("Sum: {text}"));
                    }
                    None => app.error("Not in a table"),
                }
            }
        }
        "org-table-field-info" => {
            if let Some(p) = here(app) {
                let c = coordinates(app.buf_ref(), p);
                let text = app.buf_ref().org_table_field_text(p).unwrap_or_default();
                app.msg(format!("Field {c}: {text}"));
            }
        }
        _ => return false,
    }
    true
}

/// TAB on a table line, S-TAB, and the other editing keys with table
/// meanings. True if the key was a table's.
pub fn key(app: &mut App, name: &str) -> bool {
    let p = app.buf_ref().point();
    if !is_table_line(&app.buf_ref().line(p.line)) {
        return false;
    }
    match name {
        "tab" => next_field(app),
        "backtab" => previous_field(app),
        "metaup" => {
            app.buf().org_table_move_row(p, false);
        }
        "metadown" => {
            app.buf().org_table_move_row(p, true);
        }
        "metaleft" => {
            app.buf().org_table_move_column(p, false);
        }
        "metaright" => {
            app.buf().org_table_move_column(p, true);
        }
        "shiftmetaup" => {
            if let Some(t) = app.buf().org_table_kill_row(p) {
                app.kill(t, false);
            }
        }
        "shiftmetadown" => {
            let to = app.buf().org_table_insert_row(p, false);
            moved(app, to);
        }
        "shiftmetaleft" => {
            let to = app.buf().org_table_delete_column(p);
            moved(app, to);
        }
        "shiftmetaright" => {
            let to = app.buf().org_table_insert_column(p);
            moved(app, to);
        }
        "ctrl-c-star" => recalc(app, p.line, false),
        "ctrl-c-ctrl-c" => {
            let to = app.buf().org_table_align(p.line, p.col);
            moved(app, to);
            if let Some((_, last)) = app.buf_ref().org_table_bounds(p.line)
                && !app.buf_ref().org_table_tblfm_lines(last).is_empty()
            {
                recalc(app, p.line, false);
            }
        }
        _ => return false,
    }
    true
}

/// C-c C-c on a `#+TBLFM:` line.
pub fn on_tblfm_line(app: &mut App) -> bool {
    let p = app.buf_ref().point();
    if !is_tblfm(&app.buf_ref().line(p.line)) {
        return false;
    }
    // The table is the one just above the formulas.
    let mut l = p.line;
    while l > 0 && is_tblfm(&app.buf_ref().line(l)) {
        l -= 1;
    }
    if is_table_line(&app.buf_ref().line(l)) {
        recalc(app, l, false);
        true
    } else {
        false
    }
}

pub fn answer(app: &mut App, a: &Ask, input: &str) -> bool {
    let t = input.trim();
    let line = app.buf_ref().point().line;
    match a {
        Ask::TableCreate => {
            let (c, r) = t
                .to_lowercase()
                .split_once('x')
                .map_or((None, None), |(c, r)| {
                    (
                        c.trim().parse::<usize>().ok(),
                        r.trim().parse::<usize>().ok(),
                    )
                });
            match (c, r) {
                (Some(c), Some(r)) if c > 0 && r > 0 => {
                    app.buf().org_table_insert_new(line, c, r);
                }
                _ => app.error(format!("Bad table size: {t} (want COLUMNSxROWS)")),
            }
        }
        Ask::TableGotoLine => match t.parse::<usize>() {
            Ok(n) => {
                let to = app.buf_ref().org_table_goto_row(line, n);
                match to {
                    Some(_) => moved(app, to),
                    None => app.error(format!("No line {n} in this table")),
                }
            }
            Err(_) => app.error("Not a number"),
        },
        Ask::TableSort => {
            let c = t.chars().next().unwrap_or(' ');
            match SortKind::from_char(c) {
                Some((kind, rev)) => {
                    let p = app.buf_ref().point();
                    match app.buf().org_table_sort(p, kind, rev) {
                        Some(n) => app.msg(format!("Sorted {n} row(s)")),
                        None => app.error("Not in a table"),
                    }
                }
                None => app.error(format!("Unknown sort type {c}")),
            }
        }
        Ask::TableWrapWidth => match t.parse::<usize>() {
            Ok(w) if w > 0 => {
                let p = app.buf_ref().point();
                let n = app.buf().org_table_wrap_region(p, w).unwrap_or(0);
                app.msg(format!("Wrapped into {n} row(s)"));
            }
            _ => app.error("Bad width"),
        },
        Ask::TableExportFormat => {
            let format = if t.is_empty() {
                "tsv".to_string()
            } else {
                t.to_string()
            };
            match app.buf_ref().org_table_export(line, &format) {
                Ok(text) => ask(
                    app,
                    "Export to file: ",
                    Ask::TableExportFile { format, text },
                ),
                Err(e) => app.error(e),
            }
        }
        Ask::TableExportFile { format, text } => {
            if !t.is_empty() {
                let path = crate::vc::expand_home(t);
                match std::fs::write(&path, format!("{text}\n")) {
                    Ok(()) => app.msg(format!("Table exported to {} as {format}", path.display())),
                    Err(e) => app.error(format!("Cannot write {}: {e}", path.display())),
                }
            }
        }
        Ask::TableImportFile => {
            if !t.is_empty() {
                let path = crate::vc::expand_home(t);
                match std::fs::read_to_string(&path) {
                    Ok(text) => {
                        let n = app.buf().org_table_import(line, &text);
                        app.msg(format!("Imported {n} line(s)"));
                    }
                    Err(e) => app.error(format!("Cannot read {}: {e}", path.display())),
                }
            }
        }
        Ask::TableFormula { column } => {
            if t.is_empty() {
                return true;
            }
            let p = app.buf_ref().point();
            let Some((first, rows, _)) = app.buf_ref().org_table_rows(p.line) else {
                app.error("Not in a table");
                return true;
            };
            let col = field_index(&app.buf_ref().line(p.line), p.col) + 1;
            if *column {
                let existing = formulas_text(app.buf_ref(), p.line);
                let mine = format!("${col}={t}");
                let joined = if existing.is_empty() {
                    mine
                } else {
                    format!("{existing} :: {mine}")
                };
                set_formulas(app.buf(), p.line, &joined);
                recalc(app, p.line, false);
            } else {
                match forge_core::org_calc::eval_for_field(&rows, p.line - first, col, t) {
                    Ok(text) => {
                        app.buf().org_table_set_field(p, &text);
                        if app.org.formula_debug {
                            app.msg(format!("{t} => {text}"));
                        }
                    }
                    Err(e) => app.error(e),
                }
            }
        }
        Ask::TableEditFormulas => {
            let p = app.buf_ref().point();
            if set_formulas(app.buf(), p.line, t) && !t.is_empty() {
                recalc(app, p.line, false);
            }
        }
        _ => return false,
    }
    true
}

#[allow(dead_code)]
fn unused(_: Row) {}
