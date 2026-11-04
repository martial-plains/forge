// SPDX-License-Identifier: MIT OR Apache-2.0
//! Commands for the features Forge has built in (Phase 9): line numbers,
//! wrapping, hl-line, the fill column indicator, show-paren and
//! electric-pair, electric-quote, commenting, whitespace, dabbrev,
//! counting, recent files, imenu and occur. The work is in forge-core;
//! these are the keys and prompts.

use crate::app::App;
use crate::minibuffer::{Choice, Purpose, ThingAction};
use forge_core::decoration::{LineNumbers, Wrap};
use forge_core::editing;
use forge_core::treesit::{self, Tactic};
use forge_core::{Buffer, BufferId, Pos, fill};

/// The *Occur* buffer's lines, and which buffer they come from.
pub struct Occur {
    pub buffer: BufferId,
    pub source: BufferId,
    /// Source line for each line of *Occur* (None for the header).
    pub lines: Vec<Option<usize>>,
}

/// M-/ state: repeating it tries the next candidate.
pub struct Dabbrev {
    start: usize,
    /// What was typed, to put back when the candidates run out.
    prefix: String,
    /// End of what was inserted so far.
    end: usize,
    candidates: Vec<String>,
    next: usize,
}

fn on_off(on: bool) -> &'static str {
    if on { "enabled" } else { "disabled" }
}

/// A buffer boolean option, falling back to the global default.
fn toggle_option(app: &mut App, option: &str, default: bool, what: &str) {
    let now = app
        .buf_ref()
        .option(option)
        .map_or(default, |v| v == "true");
    let _ = app
        .buf()
        .set_option(option, if now { "false" } else { "true" });
    app.msg(format!("{what} {} in current buffer", on_off(!now)));
}

/// Lines of the region (or point's line): first..=last.
fn region_lines(app: &App) -> (usize, usize) {
    match app.buf_ref().active_region() {
        Some((a, b)) => (
            a.line,
            if b.col == 0 && b.line > a.line {
                b.line - 1
            } else {
                b.line
            },
        ),
        None => {
            let l = app.buf_ref().point().line;
            (l, l)
        }
    }
}

/// Run a built-in command from this module. False if it isn't one.
pub fn execute(app: &mut App, cmd: &str) -> bool {
    match cmd {
        // ------------------------------------------------------ display
        "display-line-numbers-mode" => {
            let now = app
                .buf_ref()
                .options
                .line_numbers
                .unwrap_or(app.line_numbers);
            let next = if app.has_arg() {
                LineNumbers::Relative
            } else if now == LineNumbers::Off {
                LineNumbers::Absolute
            } else {
                LineNumbers::Off
            };
            let _ = app.buf().set_option("line-numbers", next.name());
            app.msg(format!(
                "Display-Line-Numbers mode {} in current buffer",
                on_off(next != LineNumbers::Off)
            ));
        }
        "global-display-line-numbers-mode" => {
            app.line_numbers = match (app.line_numbers, app.has_arg()) {
                (_, true) => LineNumbers::Relative,
                (LineNumbers::Off, _) => LineNumbers::Absolute,
                _ => LineNumbers::Off,
            };
            app.msg(format!(
                "Global Display-Line-Numbers mode {}",
                on_off(app.line_numbers != LineNumbers::Off)
            ));
        }
        "visual-line-mode" => {
            let now = app.buf_ref().options.wrap.unwrap_or(app.wrap);
            let next = if now == Wrap::Word {
                Wrap::Off
            } else {
                Wrap::Word
            };
            let _ = app.buf().set_option("wrap", next.name());
            app.msg(format!(
                "Visual-Line mode {} in current buffer",
                on_off(next == Wrap::Word)
            ));
        }
        "toggle-truncate-lines" => {
            let now = app.buf_ref().options.wrap.unwrap_or(app.wrap);
            let next = if now == Wrap::Off {
                Wrap::Char
            } else {
                Wrap::Off
            };
            let _ = app.buf().set_option("wrap", next.name());
            app.msg(format!("Truncate long lines {}", on_off(next == Wrap::Off)));
        }
        "hl-line-mode" => toggle_option(app, "hl-line", app.hl_line, "Hl-Line mode"),
        "global-hl-line-mode" => {
            app.hl_line = !app.hl_line;
            app.msg(format!("Global Hl-Line mode {}", on_off(app.hl_line)));
        }
        "display-fill-column-indicator-mode" => toggle_option(
            app,
            "display-fill-column-indicator",
            app.fill_column_indicator,
            "Display-Fill-Column-Indicator mode",
        ),
        "global-display-fill-column-indicator-mode" => {
            app.fill_column_indicator = !app.fill_column_indicator;
            app.msg(format!(
                "Global Display-Fill-Column-Indicator mode {}",
                on_off(app.fill_column_indicator)
            ));
        }
        "whitespace-mode" => toggle_option(app, "whitespace", false, "Whitespace mode"),
        "show-paren-mode" => {
            app.show_paren = !app.show_paren;
            app.msg(format!("Show-Paren mode {}", on_off(app.show_paren)));
        }
        "scroll-bar-mode" => {
            let on = if app.has_arg() { true } else { !app.scroll_bar };
            app.scroll_bar = on;
            app.msg(format!("Scroll Bar mode {}", on_off(on)));
        }
        "horizontal-scroll-bar-mode" => {
            let on = if app.has_arg() {
                true
            } else {
                !app.horizontal_scroll_bar
            };
            app.horizontal_scroll_bar = on;
            app.msg(format!("Horizontal Scroll Bar mode {}", on_off(on)));
        }
        "menu-bar-mode" => {
            let on = if app.has_arg() {
                // With a prefix argument, on (unless it already is).
                true
            } else {
                !crate::menu::mode_on(app)
            };
            crate::menu::set_mode(app, on);
            app.msg(format!("Menu Bar mode {}", on_off(on)));
        }
        c if c.starts_with(forge_core::menu::SELECT_BUFFER) => {
            let name = &c[forge_core::menu::SELECT_BUFFER.len()..];
            match app.editor.find_buffer(name) {
                Some(id) => app.switch_to(id),
                None => app.error(format!("No such buffer: {name}")),
            }
        }
        "kill-this-buffer" => {
            let id = app.active_id();
            let b = app.editor.state.get_buffer(id).expect("active");
            if b.is_modified() && b.path().is_some() {
                let name = b.name().to_string();
                app.prompt(
                    format!("Buffer {name} modified; kill anyway? (y or n) "),
                    Purpose::Confirm(crate::minibuffer::ConfirmAction::KillBuffer(id)),
                );
            } else {
                app.kill_buffer(id);
            }
        }
        "next-buffer" | "previous-buffer" => {
            let ids: Vec<_> = app
                .editor
                .buffer_ids()
                .into_iter()
                .filter(|id| {
                    app.editor
                        .state
                        .get_buffer(*id)
                        .is_some_and(|b| !b.name().starts_with(' '))
                })
                .collect();
            let here = app.active_id();
            if let Some(i) = ids.iter().position(|id| *id == here) {
                let n = ids.len();
                let j = if cmd == "next-buffer" {
                    (i + 1) % n
                } else {
                    (i + n - 1) % n
                };
                app.editor.show_buffer(ids[j]);
            }
        }
        "context-menu-mode" => {
            let on = if app.has_arg() {
                true
            } else {
                !crate::menu::context_mode_on(app)
            };
            crate::menu::set_context_mode(app, on);
            app.msg(format!("Context-Menu mode {}", on_off(on)));
        }
        "context-menu-open" => crate::menu::cmd_context_open(app),
        "mouse-yank-at-click" => crate::menu::cmd_yank_at_click(app),
        "delete-active-region" => crate::menu::cmd_delete_active_region(app),
        "context-menu-select-all" => crate::menu::cmd_select(app, "all"),
        "context-menu-select-line" => crate::menu::cmd_select(app, "line"),
        "context-menu-select-string" => crate::menu::cmd_select(app, "string"),
        "context-menu-select-none" => crate::menu::cmd_select(app, "none"),
        c if c.starts_with(forge_core::context_menu::YANK_PREFIX) => {
            match c[forge_core::context_menu::YANK_PREFIX.len()..].parse::<usize>() {
                Ok(n) => crate::menu::cmd_yank_entry(app, n),
                Err(_) => return false,
            }
        }
        "electric-pair-mode" => {
            app.electric_pair = !app.electric_pair;
            app.msg(format!("Electric-Pair mode {}", on_off(app.electric_pair)));
        }
        "electric-quote-mode" => {
            let on = if app.has_arg() {
                true
            } else {
                !app.electric_quote
            };
            app.electric_quote = on;
            app.msg(format!("Electric-Quote mode {}", on_off(on)));
        }
        "electric-quote-local-mode" => {
            let now = app
                .buf_ref()
                .options
                .electric_quote
                .unwrap_or(app.electric_quote);
            let on = if app.has_arg() { true } else { !now };
            let _ = app
                .buf()
                .set_option("electric-quote", if on { "true" } else { "false" });
            app.msg(format!(
                "Electric-Quote mode {} in current buffer",
                on_off(on)
            ));
        }

        // ---------------------------------------------------- comments
        "comment-line" => comment_line(app),
        c if c.starts_with("org-") || c == "outline-up-heading" => return org_command(app, c),
        "comment-dwim" => comment_dwim(app),

        // -------------------------------------------------- whitespace
        "delete-trailing-whitespace" => {
            let (a, b) = match app.buf_ref().active_region() {
                Some(_) => region_lines(app),
                None => (0, app.buf_ref().line_count().saturating_sub(1)),
            };
            let n = app.buf().delete_trailing_whitespace(a, b);
            app.msg(if n == 0 {
                "No trailing whitespace".to_string()
            } else {
                format!("Deleted trailing whitespace on {n} line(s)")
            });
        }

        // ------------------------------------------------------ others
        "dabbrev-expand" => dabbrev_expand(app),
        "count-words" | "count-words-region" => {
            let b = app.buf_ref();
            let (what, text) = match b.active_region() {
                Some((a, z)) => ("Region", b.text_range(a, z)),
                None if cmd == "count-words-region" => {
                    app.error("The mark is not set now, so there is no region");
                    return true;
                }
                None => ("Buffer", b.get_contents()),
            };
            let (l, w, c) = editing::count(&text);
            let s = |n: usize, one: &str| format!("{n} {one}{}", if n == 1 { "" } else { "s" });
            app.msg(format!(
                "{what} has {}, {}, and {}",
                s(l, "line"),
                s(w, "word"),
                s(c, "character")
            ));
        }
        "recentf-open" => {
            let files = app.editor.session.recent_files();
            if files.is_empty() {
                app.error("No recent files yet");
            } else {
                app.prompt(
                    "Open recent file: ",
                    Purpose::Choose {
                        what: Choice::RecentFile,
                        candidates: files,
                    },
                );
            }
        }
        "imenu" => {
            let id = app.active_id();
            app.editor.sync_language(id);
            let defs = app.buf().definitions();
            if defs.is_empty() {
                app.error(format!("No definitions found in {}", app.buf_ref().name()));
            } else {
                let labels = defs
                    .iter()
                    .map(|(n, k, p)| format!("{n} ({k}, line {})", p.line + 1))
                    .collect();
                let places = defs.iter().map(|d| d.2).collect();
                app.prompt(
                    "Go to definition: ",
                    Purpose::Choose {
                        what: Choice::Imenu(places),
                        candidates: labels,
                    },
                );
            }
        }
        "occur" => app.prompt("List lines matching regexp: ", Purpose::Occur),
        "occur-mode-goto-occurrence" => occur_goto(app),
        _ => return tree_command(app, cmd),
    }
    true
}

// -------------------------------------------------------------- comments

fn no_comment_syntax(app: &mut App) {
    let mode = app.buf_ref().major_mode().to_string();
    app.error(format!("{mode} has no comment syntax"));
}

/// C-x C-;: comment or uncomment the region's lines, or this line and
/// move down (with an argument, that many lines).
fn comment_line(app: &mut App) {
    let id = app.active_id();
    app.editor.sync_language(id);
    let region = app.buf_ref().active_region().is_some();
    let (a, b) = if region {
        region_lines(app)
    } else {
        let l = app.buf_ref().point().line;
        (l, l + (app.count().max(1) as usize) - 1)
    };
    match app.buf().toggle_comment_lines(a, b) {
        None => no_comment_syntax(app),
        Some(_) if !region => {
            let next = (b + 1).min(app.buf_ref().line_count().saturating_sub(1));
            app.buf().set_point(Pos::new(next, 0));
        }
        Some(_) => app.buf().deactivate_mark(),
    }
}

/// M-;: with a region, comment or uncomment it; on a line with a
/// comment, go to it; on a blank line, start one; else add one at the
/// end of the line.
fn comment_dwim(app: &mut App) {
    let id = app.active_id();
    app.editor.sync_language(id);
    if app.buf_ref().active_region().is_some() {
        let (a, b) = region_lines(app);
        if app.buf().toggle_comment_lines(a, b).is_none() {
            no_comment_syntax(app);
        }
        app.buf().deactivate_mark();
        return;
    }
    let Some(marker) = app.buf_ref().comment_marker() else {
        return no_comment_syntax(app);
    };
    let line = app.buf_ref().point().line;
    let text = app.buf_ref().line(line);
    if let Some(col) = app.buf_ref().comment_column(line) {
        let after = text
            .chars()
            .skip(col + marker.chars().count())
            .take_while(|c| *c == ' ')
            .count();
        app.buf()
            .set_point(Pos::new(line, col + marker.chars().count() + after));
    } else if text.trim().is_empty() {
        app.editor.indent_line(line);
        app.buf().move_line_end();
        app.buf().insert_str(&format!("{marker} "));
    } else {
        let code = text.trim_end().chars().count();
        let len = text.chars().count();
        let start = app.buf_ref().pos_to_offset(Pos::new(line, code));
        app.buf()
            .replace_offsets(start, start + (len - code), &format!("  {marker} "));
    }
}

// --------------------------------------------------------------- dabbrev

/// M-/: complete the word before point from words in the buffers,
/// nearest first. Again tries the next one.
fn dabbrev_expand(app: &mut App) {
    let point = app.buf_ref().point_offset();
    let again = app.last_command == "dabbrev-expand"
        && app.dabbrev.as_ref().is_some_and(|d| d.end == point);
    if !again {
        let id = app.active_id();
        match editing::dabbrev(&app.editor, id, point, 200) {
            Some((start, candidates)) if !candidates.is_empty() => {
                let prefix = app.buf_ref().text_offsets(start, point);
                app.dabbrev = Some(Dabbrev {
                    start,
                    prefix,
                    end: point,
                    candidates,
                    next: 0,
                });
            }
            Some(_) => return app.error("No dynamic expansion found"),
            None => return app.error("No word before point to expand"),
        }
    }
    let Some(mut d) = app.dabbrev.take() else {
        return;
    };
    if d.next >= d.candidates.len() {
        app.buf().replace_offsets(d.start, d.end, &d.prefix);
        app.error(format!(
            "No further dynamic expansion for \u{2018}{}\u{2019} found",
            d.prefix
        ));
        return;
    }
    let word = d.candidates[d.next].clone();
    d.next += 1;
    app.buf().replace_offsets(d.start, d.end, &word);
    d.end = d.start + word.chars().count();
    app.buf().set_offset(d.end);
    app.dabbrev = Some(d);
}

// ----------------------------------------------------------------- occur

/// Show the lines of the current buffer matching `pattern` in *Occur*.
pub fn occur(app: &mut App, pattern: &str) {
    let source = app.active_id();
    let name = app.buf_ref().name().to_string();
    let found = match app.buf_ref().occur(pattern) {
        Ok(f) => f,
        Err(e) => return app.error(format!("Invalid regexp: {e}")),
    };
    if found.is_empty() {
        return app.msg(format!(
            "Searched 1 buffer; no matches for \u{2018}{pattern}\u{2019}"
        ));
    }
    let width = (found.last().map_or(1, |f| f.0 + 1))
        .to_string()
        .len()
        .max(6);
    let mut text = format!(
        "{} match{} for \"{pattern}\" in buffer: {name}\n",
        found.len(),
        if found.len() == 1 { "" } else { "es" }
    );
    let mut lines = vec![None];
    for (l, t) in &found {
        text.push_str(&format!("{:>width$}:{t}\n", l + 1));
        lines.push(Some(*l));
    }
    app.show_special_buffer("*Occur*", "occur-mode", &text);
    app.buf().set_point(Pos::new(1, 0));
    let buffer = app.active_id();
    app.occur = Some(Occur {
        buffer,
        source,
        lines,
    });
}

/// RET in *Occur*: go to that line of the searched buffer.
fn occur_goto(app: &mut App) {
    let Some(o) = app.occur.as_ref().filter(|o| o.buffer == app.active_id()) else {
        return app.error("Not in an *Occur* buffer");
    };
    let line = app.buf_ref().point().line;
    let (source, target) = (o.source, o.lines.get(line).copied().flatten());
    let Some(target) = target else {
        return app.error("No occurrence on this line");
    };
    if !app.editor.state.buffers.contains_key(&source) {
        return app.error("The buffer was killed");
    }
    crate::windows::pop_to_other_window(app);
    app.switch_to(source);
    let b: &mut Buffer = app.buf();
    b.set_point(Pos::new(target, 0));
}

/// `org-sparse-tree` with the answered pattern.
pub fn org_sparse_tree(app: &mut App, pattern: &str) {
    let n = app.buf().org_sparse_tree(pattern);
    if n == 0 {
        app.msg(format!("No matches for \u{2018}{pattern}\u{2019}"));
    } else {
        app.msg(format!(
            "{n} match{} for \u{2018}{pattern}\u{2019}",
            if n == 1 { "" } else { "es" }
        ));
    }
}

/// `org-occur` with the answered pattern (an *Occur* listing).
pub fn org_occur(app: &mut App, pattern: &str) {
    let found = match app.buf_ref().org_occur_lines(pattern) {
        Ok(f) => f,
        Err(e) => return app.error(format!("Invalid regexp: {e}")),
    };
    if found.is_empty() {
        return app.msg(format!("No matches for \u{2018}{pattern}\u{2019}"));
    }
    let name = app.buf_ref().name().to_string();
    let source = app.active_id();
    let width = found.last().map_or(1, |f| f.0 + 1).to_string().len().max(6);
    let mut text = format!(
        "{} match{} for \"{pattern}\" in buffer: {name}\n",
        found.len(),
        if found.len() == 1 { "" } else { "es" }
    );
    let mut lines = vec![None];
    for (l, t) in &found {
        text.push_str(&format!("{:>width$}:{t}\n", l + 1));
        lines.push(Some(*l));
    }
    app.show_special_buffer("*Occur*", "occur-mode", &text);
    app.buf().set_point(Pos::new(1, 0));
    let buffer = app.active_id();
    app.occur = Some(Occur {
        buffer,
        source,
        lines,
    });
}

/// `org-insert-drawer` with the answered name.
pub fn org_insert_drawer(app: &mut App, name: &str) {
    let line = app.buf_ref().point().line;
    app.buf().org_insert_drawer(line, name);
}

/// `org-goto`: pick a heading to visit.
fn org_goto_prompt(app: &mut App) {
    let targets = app.buf_ref().org_goto_targets();
    if targets.is_empty() {
        return app.error("No headings in this buffer");
    }
    let candidates: Vec<String> = targets.iter().map(|(_, s)| s.clone()).collect();
    let places: Vec<Pos> = targets.iter().map(|(l, _)| Pos::new(*l, 0)).collect();
    app.prompt(
        "Goto heading: ",
        Purpose::Choose {
            what: Choice::OrgGoto(places),
            candidates,
        },
    );
}

/// Org commands (see forge_core::org). False if `cmd` isn't one.
fn org_command(app: &mut App, cmd: &str) -> bool {
    let line = app.buf_ref().point().line;
    let back = app.has_arg();
    let count = app.count();
    let goto = |app: &mut App, l: Option<usize>, what: &str| match l {
        Some(l) => {
            app.buf().set_point(forge_core::Pos::new(l, 0));
            let off = app.buf_ref().point_offset();
            app.buf().reveal(off);
        }
        None => app.msg(format!("No {what} heading")),
    };
    // Keys with a table meaning when point is in a table.
    let table_key = match cmd {
        "org-cycle" => Some("tab"),
        "org-shifttab" => Some("backtab"),
        "org-metaup" => Some("metaup"),
        "org-metadown" => Some("metadown"),
        "org-metaleft" => Some("metaleft"),
        "org-metaright" => Some("metaright"),
        "org-shiftmetaup" => Some("shiftmetaup"),
        "org-shiftmetadown" => Some("shiftmetadown"),
        "org-shiftmetaleft" => Some("shiftmetaleft"),
        "org-shiftmetaright" => Some("shiftmetaright"),
        "org-ctrl-c-ctrl-c" => Some("ctrl-c-ctrl-c"),
        "org-ctrl-c-star" => Some("ctrl-c-star"),
        _ => None,
    };
    if let Some(k) = table_key
        && crate::org_table_cmds::key(app, k)
    {
        return true;
    }
    if cmd == "org-todo" {
        crate::org_cmds::todo_command(app, line, !back);
        return true;
    }
    if crate::org_cmds::command(app, cmd) {
        return true;
    }
    match cmd {
        "org-cycle" => match app.buf().org_cycle(line) {
            Some(v) => app.msg(v.name().to_string()),
            None if app.buf().org_cycle_drawer(line).is_some() => {}
            None => return crate::commands::execute(app, "indent-for-tab-command"),
        },
        "org-shifttab" => {
            let v = app.buf().org_global_cycle();
            app.msg(v.name().to_string());
        }
        "org-todo" => {
            if let Err(e) = app.buf().org_todo(line, !back) {
                app.msg(e);
            }
        }
        "org-ctrl-c-ctrl-c" => crate::org_cmds::ctrl_c_ctrl_c(app, line),
        "org-meta-return" => app.buf().org_insert_heading_or_item(line),
        "org-metaleft" | "org-metaright" | "org-shiftmetaleft" | "org-shiftmetaright" => {
            let delta = if cmd.ends_with("left") { -1 } else { 1 };
            if !app
                .buf()
                .org_shift_level(line, delta, cmd.contains("shift"))
            {
                app.msg("Cannot promote or demote here".to_string());
            }
        }
        "org-metaup" | "org-metadown" => {
            let down = cmd == "org-metadown";
            if forge_core::org::heading_level(&app.buf_ref().line(line)).is_none()
                && forge_core::org::list_item(&app.buf_ref().line(line)).is_some()
            {
                if !app.buf().org_move_item(line, down) {
                    app.msg("Cannot move past the first or last item".to_string());
                }
                return true;
            }
            if !app.buf().org_move_subtree(line, cmd == "org-metadown") {
                app.msg("Cannot move past the first or last sibling".to_string());
            }
        }
        "org-next-visible-heading" => {
            let l = app.buf_ref().org_next_heading(line, true, false);
            goto(app, l, "next");
        }
        "org-previous-visible-heading" => {
            let l = app.buf_ref().org_next_heading(line, false, false);
            goto(app, l, "previous");
        }
        "org-forward-heading-same-level" => {
            let l = app.buf_ref().org_next_heading(line, true, true);
            goto(app, l, "next sibling");
        }
        "org-backward-heading-same-level" => {
            let l = app.buf_ref().org_next_heading(line, false, true);
            goto(app, l, "previous sibling");
        }
        "outline-up-heading" => {
            let l = app.buf_ref().org_up_heading(line);
            goto(app, l, "parent");
        }
        // ---------------- Document Structure (Phase 11.1)
        "org-up-element" => {
            let l = app.buf_ref().org_up_element(line);
            goto(app, l, "parent element");
        }
        "org-goto" => org_goto_prompt(app),
        "org-goto-sibling" => {
            let l = app.buf_ref().org_goto_sibling(line, !back);
            goto(app, l, "sibling");
        }
        "org-mark-subtree" => match app.buf_ref().org_mark_subtree(line) {
            Some((a, b)) => {
                let len = app.buf_ref().line_len(b);
                app.buf().push_mark(Pos::new(a, 0), false);
                app.buf().set_point(Pos::new(b, len));
                app.buf().exchange_point_and_mark();
                app.msg("Mark set".to_string());
            }
            None => app.msg("Not in an outline entry".to_string()),
        },
        "org-tree-to-indirect-buffer" => match app.buf_ref().org_tree_to_indirect_text(line) {
            Some((name, text)) => {
                let mut n = 0;
                let base = format!("{name} (indirect)");
                let final_name = loop {
                    let cand = if n == 0 {
                        base.clone()
                    } else {
                        format!("{base}<{n}>")
                    };
                    if app.editor.find_buffer(&cand).is_none() {
                        break cand;
                    }
                    n += 1;
                };
                let mut b = Buffer::new(&final_name);
                b.set_contents(&text);
                let mode = app.buf_ref().major_mode().to_string();
                b.set_major_mode(&mode);
                let id = app.editor.add_buffer(b);
                app.switch_to(id);
            }
            None => app.msg("Not in an outline entry".to_string()),
        },
        "org-cycle-set-startup-visibility" => {
            let v = app.buf().org_cycle_set_startup_visibility(if back {
                "showall"
            } else {
                "contents"
            });
            app.msg(v.name().to_string());
        }
        "org-fold-show-all" => {
            app.buf().org_fold_show_all();
            app.msg("SHOW ALL".to_string());
        }
        "org-indent-mode" => {
            if app.buf_ref().major_mode() != forge_core::org::MODE {
                app.error("Not in an Org buffer".to_string());
                return true;
            }
            let on = !app.buf_ref().org_display.indent;
            app.buf().org_display.indent = on;
            app.msg(format!(
                "Org-Indent mode {}",
                if on { "enabled" } else { "disabled" }
            ));
        }
        "org-num-mode" => {
            if app.buf_ref().major_mode() != forge_core::org::MODE {
                app.error("Not in an Org buffer".to_string());
                return true;
            }
            let on = !app.buf_ref().org_display.num;
            app.buf().org_display.num = on;
            app.msg(format!(
                "Org-Num mode {}",
                if on { "enabled" } else { "disabled" }
            ));
        }
        "org-reveal" => {
            if !app.buf().org_reveal_at_point() {
                app.msg("Nothing hidden here".to_string());
            }
        }
        "org-show-entry" => app.buf().org_show_entry(line),
        "org-show-children" => app.buf().org_show_children(line),
        "org-show-subtree" => app.buf().org_show_subtree(line),
        "org-hide-subtree" => {
            if !app.buf().org_hide_subtree(line) {
                app.msg("Nothing to hide".to_string());
            }
        }
        "org-cycle-force-archived" => match app.buf().org_cycle_force_archived(line) {
            Some(v) => app.msg(v.name().to_string()),
            None => return crate::commands::execute(app, "indent-for-tab-command"),
        },
        "org-insert-heading" => app.buf().org_insert_heading(line),
        "org-insert-heading-respect-content" => app.buf().org_insert_heading_respect_content(line),
        "org-insert-todo-heading" => app.buf().org_insert_todo_heading(line, false),
        "org-insert-todo-heading-respect-content" => app.buf().org_insert_todo_heading(line, true),
        "org-do-promote" => {
            if !app.buf().org_do_promote(line) {
                app.msg("Cannot promote here".to_string());
            }
        }
        "org-do-demote" => {
            if !app.buf().org_do_demote(line) {
                app.msg("Cannot demote here".to_string());
            }
        }
        "org-promote-subtree" => {
            if !app.buf().org_promote_subtree(line) {
                app.msg("Cannot promote here".to_string());
            }
        }
        "org-demote-subtree" => {
            if !app.buf().org_demote_subtree(line) {
                app.msg("Cannot demote here".to_string());
            }
        }
        "org-move-subtree-up" => {
            if !app.buf().org_move_subtree_up(line) {
                app.msg("Cannot move past the first sibling".to_string());
            }
        }
        "org-move-subtree-down" => {
            if !app.buf().org_move_subtree_down(line) {
                app.msg("Cannot move past the last sibling".to_string());
            }
        }
        "org-move-subtree" => {
            if !app.buf().org_move_subtree(line, !back) {
                app.msg("Cannot move past the first or last sibling".to_string());
            }
        }
        "org-sort" => {
            let text = app.buf_ref().line(line);
            if forge_core::org_table::is_table_line(&text) {
                return crate::commands::execute(app, "org-table-sort-lines");
            }
            if forge_core::org::heading_level(&text).is_none()
                && forge_core::org::list_item(&text).is_some()
            {
                if app.buf().org_sort_list(line, back) {
                    app.msg("Sorted list".to_string());
                } else {
                    app.msg("Nothing to sort".to_string());
                }
                return true;
            }
            let n = app.buf().org_sort_children(line, back);
            if n == 0 {
                app.msg("Nothing to sort".to_string());
            } else {
                app.msg(format!("Sorted {n} entries"));
            }
        }
        "org-copy-subtree" => match app.buf_ref().org_copy_subtree(line) {
            Some(t) => {
                app.editor.kill_ring.push(t);
                app.msg("Subtree copied".to_string());
            }
            None => app.msg("Not in an outline entry".to_string()),
        },
        "org-cut-subtree" => match app.buf().org_cut_subtree(line) {
            Some(t) => {
                app.kill(t, false);
                app.msg("Subtree cut".to_string());
            }
            None => app.msg("Not in an outline entry".to_string()),
        },
        "org-paste-subtree" | "org-yank" => {
            match app.editor.kill_ring.current().map(str::to_owned) {
                Some(t) => {
                    if !app.buf().org_paste_subtree(line, &t) {
                        app.msg("Nothing to paste".to_string());
                    }
                }
                None => app.error("Kill ring is empty"),
            }
        }
        "org-toggle-heading" | "org-ctrl-c-star" => {
            if !app.buf().org_toggle_heading(line) {
                app.msg("Not on a heading or list item".to_string());
            }
        }
        "org-clone-subtree-with-time-shift" => {
            let days = if app.has_arg() { count } else { 0 };
            if !app.buf().org_clone_subtree_with_time_shift(line, days) {
                app.msg("Not in an outline entry".to_string());
            }
        }
        "org-sparse-tree" | "org-match-sparse-tree" => {
            app.prompt("Sparse tree (regexp): ", Purpose::OrgSparse);
        }
        "org-occur" => {
            app.prompt("Occur (regexp): ", Purpose::OrgOccur);
        }
        "org-occur-in-agenda-files" => {
            app.prompt(
                "Occur in agenda files (regexp, this buffer): ",
                Purpose::OrgOccur,
            );
        }
        "org-list-indent-item-generic" => {
            if !app.buf().org_list_indent_item(line) {
                app.msg("Not on a list item".to_string());
            }
        }
        "org-list-outdent-item" => {
            if !app.buf().org_list_outdent_item(line) {
                app.msg("Not on a list item".to_string());
            }
        }
        "org-list-insert-item" => {
            if !app.buf().org_list_insert_item(line) {
                app.msg("Not on a list item".to_string());
            }
        }
        "org-list-item-to-subtree" => {
            if !app.buf().org_list_item_to_subtree(line) {
                app.msg("Not on a list item".to_string());
            }
        }
        "org-list-repair" => {
            let n = app.buf().org_list_repair();
            app.msg(if n == 0 {
                "Nothing repaired".to_string()
            } else {
                format!("Repaired {n} lines")
            });
        }
        "org-sort-list" => {
            if !app.buf().org_sort_list(line, back) {
                app.msg("Nothing to sort".to_string());
            }
        }
        "org-insert-drawer" => {
            app.prompt("Drawer name: ", Purpose::OrgDrawer);
        }
        _ => return false,
    }
    true
}

// --------------------------------------------------- tree-sitter structure

fn tree_command(app: &mut App, cmd: &str) -> bool {
    match cmd {
        "beginning-of-defun" => {
            tree_move(
                app,
                |b, o, n| treesit::beginning_of_defun(b, o, n, Tactic::Nested),
                app.count(),
                "Beginning of defun",
            );
        }
        "end-of-defun" => {
            tree_move(
                app,
                |b, o, n| treesit::end_of_defun(b, o, n, Tactic::Nested),
                app.count(),
                "End of defun",
            );
        }
        "mark-defun" => {
            let off = app.buf_ref().point_offset();
            match treesit::defun_at(app.buf(), off, Tactic::Nested) {
                Some(range) => {
                    let b = app.buf();
                    b.push_mark(b.offset_to_pos(range.end), true);
                    b.set_offset(range.start);
                    app.msg("Mark set");
                }
                None => app.error("No defun found"),
            }
        }
        "forward-sexp" => {
            tree_move(
                app,
                treesit::forward_sexp,
                app.count(),
                "Scan error: no sexp to move over",
            );
        }
        "backward-sexp" => {
            tree_move(
                app,
                treesit::backward_sexp,
                app.count(),
                "Scan error: no sexp to move over",
            );
        }
        "forward-list" => {
            tree_move(
                app,
                treesit::forward_list,
                app.count(),
                "Scan error: no list to move over",
            );
        }
        "backward-list" => {
            tree_move(
                app,
                treesit::backward_list,
                app.count(),
                "Scan error: no list to move over",
            );
        }
        "down-list" => {
            tree_move(
                app,
                treesit::down_list,
                app.count(),
                "Scan error: no list to descend into",
            );
        }
        "up-list" => {
            tree_move(
                app,
                treesit::up_list,
                app.count(),
                "Scan error: no enclosing list",
            );
        }
        "transpose-sexps" => {
            if !transpose_sexps(app, app.count()) {
                app.error("Don't have two sexps to transpose");
            }
        }
        "kill-sexp" => {
            if !kill_sexp(app, app.count()) {
                app.error("Scan error: no sexp to kill");
            }
        }
        "which-function" => {
            let off = app.buf_ref().point_offset();
            let defs = treesit::definitions(app.buf());
            match defs
                .into_iter()
                .filter(|d| treesit::is_defun_kind(&d.kind) && d.start <= off && off <= d.end)
                .min_by_key(|d| d.end.saturating_sub(d.start))
            {
                Some(d) => app.msg(d.name),
                None => app.msg("No function at point"),
            }
        }
        "add-log-current-defun" => {
            let off = app.buf_ref().point_offset();
            match treesit::add_log_current_defun(app.buf(), off, ".") {
                Some(name) => app.msg(name),
                None => app.error("No defun at point"),
            }
        }
        "beginning-of-thing" => {
            let arg = app.count();
            app.prompt("Thing: ", Purpose::Thing(ThingAction::Beginning { arg }));
        }
        "end-of-thing" => {
            let arg = app.count();
            app.prompt("Thing: ", Purpose::Thing(ThingAction::End { arg }));
        }
        "bounds-of-thing-at-point" => {
            app.prompt("Thing: ", Purpose::Thing(ThingAction::Bounds));
        }
        "thing-at-point" => {
            app.prompt("Thing: ", Purpose::Thing(ThingAction::Text));
        }
        "fill-reindent-defun" => {
            if !fill_reindent_defun(app) {
                app.error("No defun detected at point");
            }
        }
        "outline-next-heading" => {
            let off = app.buf_ref().point_offset();
            let arg = app.count();
            match treesit::next_heading(app.buf(), off, arg) {
                Some(to) => app.buf().set_offset(to),
                None => app.error("No next heading"),
            }
        }
        "outline-previous-heading" => {
            let off = app.buf_ref().point_offset();
            let arg = app.count();
            match treesit::previous_heading(app.buf(), off, arg) {
                Some(to) => app.buf().set_offset(to),
                None => app.error("No previous heading"),
            }
        }
        "outline-level" => {
            let off = app.buf_ref().point_offset();
            match treesit::heading_level(app.buf(), off) {
                Some(level) => app.msg(format!("Level {level}")),
                None => app.error("Not on a heading"),
            }
        }
        _ => return false,
    }
    true
}

fn fill_reindent_defun(app: &mut App) -> bool {
    let offset = app.buf_ref().point_offset();
    let text_at_point = treesit::thing_at(app.buf(), offset, treesit::Thing::Comment).is_some()
        || treesit::thing_at(app.buf(), offset, treesit::Thing::String).is_some();
    if text_at_point {
        let line = app.buf_ref().point().line;
        if let Some((start, end)) = fill::paragraph_at(app.buf_ref(), line) {
            fill::fill_line_range_arg(app.buf(), start, end, false);
        }
        return true;
    }
    let Some(range) = treesit::defun_at(app.buf(), offset, Tactic::ParentFirst) else {
        return false;
    };
    let (start_line, end_line) = {
        let b = app.buf_ref();
        let end = range.end.max(range.start + 1).min(b.char_count());
        (
            b.offset_to_pos(range.start).line,
            b.offset_to_pos(end.saturating_sub(1)).line,
        )
    };
    for line in start_line..=end_line {
        if !app.buf_ref().line(line).trim().is_empty() {
            app.editor.indent_line(line);
        }
    }
    true
}

fn tree_move(
    app: &mut App,
    mv: impl Fn(&mut Buffer, usize, i64) -> Option<usize>,
    arg: i64,
    failure: &str,
) -> bool {
    let offset = app.buf_ref().point_offset();
    match mv(app.buf(), offset, arg) {
        Some(to) => {
            app.buf().set_offset(to);
            true
        }
        None => {
            app.error(failure);
            true
        }
    }
}

fn transpose_sexps(app: &mut App, arg: i64) -> bool {
    for _ in 0..arg.unsigned_abs() {
        let offset = app.buf_ref().point_offset();
        let Some(left_start) = treesit::backward_sexp(app.buf(), offset, 1) else {
            return false;
        };
        let Some(right_end) = treesit::forward_sexp(app.buf(), offset, 1) else {
            return false;
        };
        let Some(left_end) = treesit::forward_sexp(app.buf(), left_start, 1) else {
            return false;
        };
        let Some(right_start) = treesit::backward_sexp(app.buf(), right_end, 1) else {
            return false;
        };
        if left_start >= right_start || left_end > right_start {
            return false;
        }
        let left = app.buf_ref().text_offsets(left_start, left_end);
        let middle = app.buf_ref().text_offsets(left_end, right_start);
        let right = app.buf_ref().text_offsets(right_start, right_end);
        let replacement = format!("{right}{middle}{left}");
        let b = app.buf();
        b.replace_range(
            b.offset_to_pos(left_start),
            b.offset_to_pos(right_end),
            &replacement,
        );
    }
    true
}

fn kill_sexp(app: &mut App, arg: i64) -> bool {
    if arg == 0 {
        return true;
    }
    let offset = app.buf_ref().point_offset();
    let target = if arg > 0 {
        treesit::forward_sexp(app.buf(), offset, arg)
    } else {
        treesit::backward_sexp(app.buf(), offset, arg)
    };
    let Some(target) = target else {
        return false;
    };
    let (a, b) = if target < offset {
        (target, offset)
    } else {
        (offset, target)
    };
    let text = {
        let buf = app.buf();
        let (a, b) = (buf.offset_to_pos(a), buf.offset_to_pos(b));
        buf.delete_range(a, b)
    };
    if text.is_empty() {
        return false;
    }
    app.kill(text, arg < 0);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::Editor;

    fn test_app() -> App {
        let mut app = App::new(Editor::default());
        let id = app.editor.add_buffer(Buffer::new("*scratch*"));
        app.editor.show_buffer(id);
        app
    }

    fn rust_test_app(text: &str) -> App {
        lang_test_app("rust", text)
    }

    fn lang_test_app(lang: &str, text: &str) -> App {
        let mut app = test_app();
        let def = forge_core::syntax::language(lang).unwrap();
        let b = app.buf();
        b.set_contents(text);
        b.set_major_mode(def.mode);
        b.set_language(Some(def));
        app
    }

    fn answer_prompt(app: &mut App, input: &str) {
        app.minibuffer.as_mut().unwrap().input = input.to_string();
        crate::minibuffer::cmd_exit(app);
    }

    #[test]
    fn tree_sitter_commands_navigate_and_edit() {
        let src = "fn f() { g(a, b); }\n";
        let mut app = rust_test_app(src);
        let g = app.buf_ref().byte_to_char(src.find('g').unwrap());
        let close = app.buf_ref().byte_to_char(src.find("b)").unwrap() + 1);
        app.buf().set_offset(g);
        assert!(execute(&mut app, "forward-list"));
        assert_eq!(app.buf_ref().point_offset(), close + 1);
        assert!(execute(&mut app, "backward-list"));
        assert_eq!(app.buf_ref().point_offset(), g + 1);

        let src = "fn f() { a b; }\n";
        let mut app = rust_test_app(src);
        let between = app.buf_ref().byte_to_char(src.find("a b").unwrap() + 1);
        app.buf().set_offset(between);
        assert!(execute(&mut app, "transpose-sexps"));
        assert_eq!(app.buf_ref().get_contents(), "fn f() { b a; }\n");

        let mut app = rust_test_app(src);
        let a = app.buf_ref().byte_to_char(src.find('a').unwrap());
        app.buf().set_offset(a);
        assert!(execute(&mut app, "kill-sexp"));
        assert_eq!(app.buf_ref().get_contents(), "fn f() {  b; }\n");
    }

    #[test]
    fn tree_sitter_names_things_and_outline() {
        let src = "class A:\n    def method(self):\n        return 1\n";
        let mut app = lang_test_app("python", src);
        let at = app.buf_ref().byte_to_char(src.find("return").unwrap());
        app.buf().set_offset(at);
        assert!(execute(&mut app, "add-log-current-defun"));
        assert_eq!(app.message.as_deref(), Some("A.method"));

        let src = "# One\ntext\n## Two\nbody\n";
        let mut app = lang_test_app("markdown", src);
        let one = app.buf_ref().byte_to_char(src.find("# One").unwrap());
        let two = app.buf_ref().byte_to_char(src.find("## Two").unwrap());
        app.buf().set_offset(one);
        assert!(execute(&mut app, "outline-next-heading"));
        assert_eq!(app.buf_ref().point_offset(), two);
        assert!(execute(&mut app, "outline-level"));
        assert_eq!(app.message.as_deref(), Some("Level 2"));
        assert!(execute(&mut app, "outline-previous-heading"));
        assert_eq!(app.buf_ref().point_offset(), one);
    }

    #[test]
    fn tree_sitter_thing_commands_prompt_and_move() {
        let src = "fn f() { g(1); }\n";
        let mut app = rust_test_app(src);
        let one = app.buf_ref().byte_to_char(src.find('1').unwrap());
        app.buf().set_offset(one + 1);
        assert!(execute(&mut app, "beginning-of-thing"));
        answer_prompt(&mut app, "number");
        assert_eq!(app.buf_ref().point_offset(), one);

        assert!(execute(&mut app, "end-of-thing"));
        answer_prompt(&mut app, "number");
        assert_eq!(app.buf_ref().point_offset(), one + 1);

        assert!(execute(&mut app, "bounds-of-thing-at-point"));
        answer_prompt(&mut app, "number");
        assert_eq!(app.message.as_deref(), Some("11..12"));

        assert!(execute(&mut app, "thing-at-point"));
        answer_prompt(&mut app, "number");
        assert_eq!(app.message.as_deref(), Some("1"));
    }

    #[test]
    fn fill_reindent_defun_indents_the_whole_defun() {
        let src = "fn f() {\nlet x = 1;\n}\n";
        let mut app = rust_test_app(src);
        app.buf().set_offset(0);
        assert!(execute(&mut app, "fill-reindent-defun"));
        assert_eq!(
            app.buf_ref().get_contents(),
            "fn f() {\n    let x = 1;\n}\n"
        );
    }

    fn org_test_app(text: &str) -> App {
        let mut app = test_app();
        let id = app.editor.find_buffer("*scratch*").unwrap();
        let b = app.editor.state.get_buffer_mut(id).unwrap();
        b.set_contents(text);
        b.set_major_mode(forge_core::org::MODE);
        app
    }

    #[test]
    fn org_indent_mode_toggles_this_buffer() {
        let mut app = org_test_app("* A\nbody\n");
        assert!(execute(&mut app, "org-indent-mode"));
        assert!(app.buf_ref().org_display.indent);
        assert!(execute(&mut app, "org-indent-mode"));
        assert!(!app.buf_ref().org_display.indent);
        // Outside Org it explains itself.
        let mut plain = test_app();
        assert!(execute(&mut plain, "org-indent-mode"));
        assert!(!plain.buf_ref().org_display.indent);
    }

    #[test]
    fn org_num_mode_toggles_this_buffer() {
        let mut app = org_test_app("* A\n** B\n");
        assert!(execute(&mut app, "org-num-mode"));
        assert!(app.buf_ref().org_display.num);
        assert!(execute(&mut app, "org-num-mode"));
        assert!(!app.buf_ref().org_display.num);
        let mut plain = test_app();
        assert!(execute(&mut plain, "org-num-mode"));
        assert!(!plain.buf_ref().org_display.num);
    }

    #[test]
    fn org_convert_levels_command() {
        let mut app = org_test_app("* A\nbody\n** B\n*** C\n");
        assert!(execute(&mut app, "org-convert-to-odd-levels"));
        assert_eq!(app.buf_ref().get_contents(), "* A\nbody\n*** B\n***** C\n");
        assert!(execute(&mut app, "org-convert-to-oddeven-levels"));
        assert_eq!(app.buf_ref().get_contents(), "* A\nbody\n** B\n*** C\n");
    }

    #[test]
    fn org_entities_user_defines_an_entity() {
        use forge_core::export::entities;
        entities::clear_user();
        let mut app = test_app();
        assert!(execute(&mut app, "org-entities-user"));
        // The prompt carries back through the Org ask channel.
        crate::org_cmds::answered(&mut app, crate::org_cmds::Ask::EntityUser, "forge-logo 🔨");
        assert_eq!(entities::find("forge-logo").map(|e| e.4), Some("🔨"));
        // name latex html ascii utf8 defines every backend.
        crate::org_cmds::answered(
            &mut app,
            crate::org_cmds::Ask::EntityUser,
            r"szlig \ss &szlig; ss ß",
        );
        let e = entities::find("szlig").unwrap();
        assert_eq!((e.1, e.2, e.3, e.4), (r"\ss", "&szlig;", "ss", "ß"));
        // A blank answer is rejected.
        crate::org_cmds::answered(&mut app, crate::org_cmds::Ask::EntityUser, "   ");
        entities::clear_user();
    }

    #[test]
    fn org_beamer_select_environment_sets_beamer_env() {
        let mut app = org_test_app("* Frame\n");
        assert!(execute(&mut app, "org-beamer-select-environment"));
        crate::org_cmds::answered(&mut app, crate::org_cmds::Ask::BeamerEnv, "block");
        let text = app.buf_ref().get_contents();
        assert!(text.contains(":BEAMER_ENV: block"), "{text}");
        crate::org_cmds::answered(&mut app, crate::org_cmds::Ask::BeamerEnv, "");
        assert!(!app.buf_ref().get_contents().contains("BEAMER_ENV"));
        // Choosing column also records a width.
        crate::org_cmds::answered(&mut app, crate::org_cmds::Ask::BeamerEnv, "column");
        crate::org_cmds::answered(
            &mut app,
            crate::org_cmds::Ask::BeamerCol {
                env: "column".into(),
            },
            "0.4",
        );
        let text = app.buf_ref().get_contents();
        assert!(text.contains(":BEAMER_ENV: column"), "{text}");
        assert!(text.contains(":BEAMER_COL: 0.4"), "{text}");
    }
}
