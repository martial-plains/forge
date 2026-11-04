// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Org agenda buffer (Phase 11, stage 4): the dispatcher, the
//! views, moving around, acting on an entry from the agenda, filters,
//! bulk marks and the agenda file list.

use crate::app::App;
use crate::org_cmds::{Ask, ask, ask_with, choose, now};
use crate::org_flow::{
    agenda_buffers, buffer_label, clock_goto, clock_in_at, refile_to, save_agenda_files,
};
use forge_core::org_agenda::{self as ag, Entry, Filter, Options, Rendered, Source, Span};
use forge_core::org_time::{Date, MONTH_NAMES, Timestamp, hmm, parse_date_input};
use forge_core::org_work::PriorityAction;
use forge_core::{Buffer, BufferId, Pos};
use std::collections::HashSet;

pub const BUFFER_NAME: &str = "*Org Agenda*";
pub const MODE: &str = "org-agenda-mode";

pub const COMMANDS: &[forge_docs::CommandDoc] = &[
    forge_docs::CommandDoc::new(
        "org-agenda",
        "Org: open the agenda dispatcher (week, TODO list, match, search,",
    )
    .body("stuck projects, restriction, export, custom commands)."),
    forge_docs::CommandDoc::line("org-agenda-list", "Org: the agenda for the current week."),
    forge_docs::CommandDoc::line(
        "org-search-view",
        "Org: show entries whose text has all the words (+word, -word, {regexp}).",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-list-stuck-projects",
        "Org: list projects that have no NEXT or STARTED task.",
    ),
    forge_docs::CommandDoc::line("org-todo-list", "Org: the global list of TODO entries."),
    forge_docs::CommandDoc::line(
        "org-tags-view",
        "Org: show entries matching a tags/properties/todo match string.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-goto",
        "Org agenda: show the entry at point in another window.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-goto-date",
        "Org agenda: show the agenda for a date.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-goto-today",
        "Org agenda: show the agenda around today.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-goto-calendar",
        "Org agenda: show a calendar of the months around the agenda's date.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-next-line",
        "Org agenda: move to the next entry line.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-previous-line",
        "Org agenda: move to the previous entry line.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-show-and-scroll-up",
        "Org agenda: show the entry at point in another window, staying here.",
    ),
    forge_docs::CommandDoc::line("org-agenda-redo", "Org agenda: rebuild the agenda."),
    forge_docs::CommandDoc::line("org-agenda-quit", "Org agenda: bury the agenda."),
    forge_docs::CommandDoc::line("org-agenda-exit", "Org agenda: kill the agenda."),
    forge_docs::CommandDoc::line(
        "org-agenda-recenter",
        "Org agenda: put the entry at point at the center of the window.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-reset-view",
        "Org agenda: back to the week view for today, without filters.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-switch-to",
        "Org agenda: go to the entry at point in this window.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-tree-to-indirect-buffer",
        "Org agenda: show the entry's subtree in its own buffer.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-todo",
        "Org agenda: cycle the entry's TODO state.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-priority",
        "Org agenda: set the entry's priority (a letter, or SPC to remove).",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-priority-up",
        "Org agenda: raise the entry's priority.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-priority-down",
        "Org agenda: lower the entry's priority.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-do-date-earlier",
        "Org agenda: move the entry's date a day earlier (a week with C-u).",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-do-date-later",
        "Org agenda: move the entry's date a day later (a week with C-u).",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-schedule",
        "Org agenda: schedule the entry (with a prefix argument, remove it).",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-deadline",
        "Org agenda: set the entry's deadline (with a prefix argument, remove it).",
    ),
    forge_docs::CommandDoc::line("org-agenda-refile", "Org agenda: refile the entry."),
    forge_docs::CommandDoc::line(
        "org-agenda-add-note",
        "Org agenda: add a timestamped note to the entry's logbook.",
    ),
    forge_docs::CommandDoc::line("org-agenda-archive", "Org agenda: archive the entry."),
    forge_docs::CommandDoc::line("org-agenda-clock-in", "Org agenda: clock in on the entry."),
    forge_docs::CommandDoc::line(
        "org-agenda-clock-out",
        "Org agenda: stop the running clock.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-clock-cancel",
        "Org agenda: discard the running clock.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-clock-goto",
        "Org agenda: go to the clocked entry.",
    ),
    forge_docs::CommandDoc::line("org-agenda-capture", "Org agenda: capture with a template."),
    forge_docs::CommandDoc::line("org-agenda-set-tags", "Org agenda: set the entry's tags."),
    forge_docs::CommandDoc::line(
        "org-agenda-set-effort",
        "Org agenda: set the entry's Effort.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-toggle-archive-tag",
        "Org agenda: add or remove the ARCHIVE tag on the entry.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-toggle-diary",
        "Org agenda: toggle diary entries in the agenda (recorded; Forge has no diary).",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-filter",
        "Org agenda: filter by +tag, -tag, <category, =regexp or _<effort (empty clears).",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-filter-by-category",
        "Org agenda: show only (or with a - prefix, hide) a category.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-filter-by-effort",
        "Org agenda: show only entries with an effort like <1:00, >0:30 or =2:00.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-filter-by-regexp",
        "Org agenda: show only entries matching a regexp.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-filter-by-tag",
        "Org agenda: show only entries with (+tag) or without (-tag) a tag; empty clears.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-filter-by-top-headline",
        "Org agenda: show only the entries under the top headline of the one at point.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-limit-interactively",
        "Org agenda: show only the first N entries.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-set-restriction-lock",
        "Org: limit the agenda to the subtree at point (or the file).",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-remove-restriction-lock",
        "Org: remove the agenda restriction.",
    ),
    forge_docs::CommandDoc::line("org-agenda-day-view", "Org agenda: show one day."),
    forge_docs::CommandDoc::line("org-agenda-week-view", "Org agenda: show one week."),
    forge_docs::CommandDoc::line("org-agenda-month-view", "Org agenda: show one month."),
    forge_docs::CommandDoc::line("org-agenda-year-view", "Org agenda: show one year."),
    forge_docs::CommandDoc::line(
        "org-agenda-later",
        "Org agenda: move forward by the span shown.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-earlier",
        "Org agenda: move back by the span shown.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-follow-mode",
        "Org agenda: toggle showing the entry in another window as you move.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-log-mode",
        "Org agenda: toggle showing closed entries and clock lines.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-entry-text-mode",
        "Org agenda: toggle showing the first lines of each entry's text.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-archives-mode",
        "Org agenda: toggle including archived entries.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-clockreport-mode",
        "Org agenda: toggle a clocked-time summary under the agenda.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-bulk-mark",
        "Org agenda: mark the entry for a bulk action.",
    ),
    forge_docs::CommandDoc::line("org-agenda-bulk-mark-all", "Org agenda: mark every entry."),
    forge_docs::CommandDoc::line(
        "org-agenda-bulk-mark-regexp",
        "Org agenda: mark every entry matching a regexp.",
    ),
    forge_docs::CommandDoc::line("org-agenda-bulk-unmark", "Org agenda: unmark the entry."),
    forge_docs::CommandDoc::line(
        "org-agenda-bulk-unmark-all",
        "Org agenda: unmark every entry.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-bulk-toggle",
        "Org agenda: toggle the mark on the entry.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-bulk-toggle-all",
        "Org agenda: toggle the mark on every entry.",
    ),
    forge_docs::CommandDoc::new(
        "org-agenda-bulk-action",
        "Org agenda: do something to every marked entry (t todo, + tag, - untag,",
    )
    .body("s schedule, d deadline, r refile, $ archive, A archive sibling)."),
    forge_docs::CommandDoc::line(
        "org-agenda-undo",
        "Org agenda: undo the last change made from the agenda.",
    ),
    forge_docs::CommandDoc::line(
        "org-agenda-file-to-front",
        "Org: add this file to the front of the agenda files.",
    ),
    forge_docs::CommandDoc::line(
        "org-remove-file",
        "Org: remove this file from the agenda files.",
    ),
    forge_docs::CommandDoc::line("org-cycle-agenda-files", "Org: visit the next agenda file."),
    forge_docs::CommandDoc::line(
        "org-agenda-write",
        "Org agenda: write the agenda to a file (.txt, .org or .html).",
    ),
];

#[derive(Clone, Debug, PartialEq)]
pub enum View {
    Span(Span),
    Todo(Option<String>),
    Match { expr: String, todo_only: bool },
    Search(String),
    SearchTodo(String),
    Stuck,
}

pub struct AgendaState {
    pub view: View,
    pub anchor: Date,
    pub log: bool,
    pub archives: bool,
    pub entry_text: bool,
    pub follow: bool,
    pub clockreport: bool,
    pub diary: bool,
    pub filter: Filter,
    pub limit: Option<usize>,
    pub rendered: Rendered,
    pub ids: Vec<BufferId>,
    pub marks: HashSet<(BufferId, String)>,
    pub restriction: Option<(BufferId, Option<String>)>,
    pub buffer: Option<BufferId>,
    /// The entry an open question is about: (buffer, heading text).
    pub target: Option<(BufferId, String)>,
    pub bulk: Option<char>,
    pub last_edit: Option<BufferId>,
}

impl AgendaState {
    fn new(view: View, anchor: Date) -> Self {
        AgendaState {
            view,
            anchor,
            log: false,
            archives: false,
            entry_text: false,
            follow: false,
            clockreport: false,
            diary: false,
            filter: Filter::default(),
            limit: None,
            rendered: Rendered::default(),
            ids: Vec::new(),
            marks: HashSet::new(),
            restriction: None,
            buffer: None,
            target: None,
            bulk: None,
            last_edit: None,
        }
    }
}

// ------------------------------------------------------------- building

/// (Re)build the agenda text for `st` and show it.
fn rebuild(app: &mut App, mut st: AgendaState) {
    let (today, now_t) = now();
    let mut ids = match &st.restriction {
        Some((id, _)) if app.editor.state.buffers.contains_key(id) => vec![*id],
        _ => agenda_buffers(app),
    };
    if ids.is_empty() {
        // No agenda files: every open Org buffer.
        let mut open: Vec<BufferId> = app
            .editor
            .state
            .buffers
            .iter()
            .filter(|(_, b)| b.major_mode() == forge_core::org::MODE)
            .map(|(id, _)| *id)
            .collect();
        open.sort();
        ids = open;
    }
    if ids.is_empty() {
        ids = vec![app.active_id()];
    }
    let names: Vec<String> = ids.iter().map(|id| buffer_label(app, *id)).collect();
    let opts = Options {
        log: st.log,
        archives: st.archives,
        now: Some(now().1),
        // The window the agenda is shown in; tags go to its right edge.
        width: app.win().width,
        stuck: app.org.stuck_projects.clone(),
        deadline_warning: crate::org_flow::setting_number(app, "org_deadline_warning_days")
            .map_or(Options::default().deadline_warning, |n| n as i64),
        ..Options::default()
    };
    let mut filter = st.filter.clone();
    if let Some((id, Some(text))) = &st.restriction
        && let Some(si) = ids.iter().position(|i| i == id)
        && let Some(b) = app.editor.state.get_buffer(*id)
        && let Some(h) =
            (0..b.line_count()).find(|&l| b.org_level(l).is_some() && b.line(l) == *text)
    {
        filter.top_headline = Some((si, h));
    }
    let mut rendered = {
        let sources: Vec<Source> = ids
            .iter()
            .zip(&names)
            .filter_map(|(id, name)| {
                app.editor
                    .state
                    .get_buffer(*id)
                    .map(|buf| Source { name, buf })
            })
            .collect();
        match &st.view {
            View::Span(span) => ag::render_span(
                &sources,
                *span,
                st.anchor,
                today,
                &opts,
                &filter,
                st.entry_text,
            ),
            View::Todo(k) => {
                let title = match k {
                    Some(k) => format!("Global list of TODO items of type: {k}"),
                    None => "Global list of TODO items of type: ALL".to_string(),
                };
                ag::render_list_in(
                    &title,
                    ag::collect_todos(&sources, k.as_deref(), &opts),
                    &sources,
                    &filter,
                    st.entry_text,
                    opts.width,
                )
            }
            View::Match { expr, todo_only } => {
                match ag::collect_match(&sources, expr, *todo_only, &opts) {
                    Ok(e) => ag::render_list_in(
                        &format!(
                            "Headlines with {}: {expr}",
                            if *todo_only {
                                "TODO and match"
                            } else {
                                "match"
                            }
                        ),
                        e,
                        &sources,
                        &filter,
                        st.entry_text,
                        opts.width,
                    ),
                    Err(e) => {
                        let mut r = Rendered::default();
                        r.lines.push(format!("Bad match: {e}"));
                        r.refs.push(None);
                        r.dates.push(None);
                        r
                    }
                }
            }
            View::Search(q) | View::SearchTodo(q) => match ag::collect_search(&sources, q, &opts) {
                Ok(mut e) => {
                    let todo_only = matches!(st.view, View::SearchTodo(_));
                    if todo_only {
                        e.retain(|x| x.keyword.is_some());
                    }
                    ag::render_list_in(
                        &format!(
                            "Search words{}: {q}",
                            if todo_only { " (TODO entries)" } else { "" }
                        ),
                        e,
                        &sources,
                        &filter,
                        true,
                        opts.width,
                    )
                }
                Err(e) => {
                    let mut r = Rendered::default();
                    r.lines.push(format!("Bad search: {e}"));
                    r.refs.push(None);
                    r.dates.push(None);
                    r
                }
            },
            View::Stuck => ag::render_list(
                "List of stuck projects:",
                ag::collect_stuck(&sources, &opts),
                &sources,
                &filter,
                false,
            ),
        }
    };
    if let Some(n) = st.limit {
        limit_entries(&mut rendered, n);
    }
    if !filter_description(&st).is_empty() {
        rendered.lines[0] = format!("{}  [{}]", rendered.lines[0], filter_description(&st));
    }
    if st.clockreport {
        let sources: Vec<Source> = ids
            .iter()
            .zip(&names)
            .filter_map(|(id, name)| {
                app.editor
                    .state
                    .get_buffer(*id)
                    .map(|buf| Source { name, buf })
            })
            .collect();
        for l in clock_summary(&sources) {
            rendered.lines.push(l);
            rendered.refs.push(None);
            rendered.dates.push(None);
        }
    }
    let _ = now_t;
    st.ids = ids;
    st.rendered = rendered;
    // Marks survive a rebuild; marks for entries that are gone do not.
    show(app, &mut st);
    app.org.agenda = Some(st);
}

fn filter_description(st: &AgendaState) -> String {
    let mut p = Vec::new();
    if !st.filter.is_empty() {
        p.push(st.filter.describe());
    }
    if let Some((_, Some(_))) = &st.restriction {
        p.push("restricted to subtree".into());
    } else if st.restriction.is_some() {
        p.push("restricted to file".into());
    }
    if let Some(n) = st.limit {
        p.push(format!("limit {n}"));
    }
    if st.log {
        p.push("log".into());
    }
    if st.archives {
        p.push("archives".into());
    }
    p.join(" ")
}

fn limit_entries(r: &mut Rendered, n: usize) {
    let mut seen = std::collections::BTreeSet::new();
    let mut keep = vec![true; r.lines.len()];
    for (i, rf) in r.refs.iter().enumerate() {
        if let Some(e) = rf {
            seen.insert(*e);
            if seen.len() > n && !seen.contains(&usize::MAX) {
                // Past the limit unless this entry was already counted.
                let order: Vec<usize> = seen.iter().copied().collect();
                if order.iter().position(|x| x == e).is_some_and(|p| p >= n) {
                    keep[i] = false;
                }
            }
        }
    }
    let (mut lines, mut refs, mut dates) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..r.lines.len() {
        if keep[i] {
            lines.push(std::mem::take(&mut r.lines[i]));
            refs.push(r.refs[i]);
            dates.push(r.dates[i]);
        }
    }
    r.lines = lines;
    r.refs = refs;
    r.dates = dates;
}

fn clock_summary(sources: &[Source]) -> Vec<String> {
    let mut out = vec![String::new(), "Clock summary:".to_string()];
    let mut total = 0;
    for s in sources {
        let mins: i64 = (0..s.buf.line_count())
            .filter(|&l| s.buf.org_level(l) == Some(1))
            .map(|h| s.buf.org_clock_total(h, true))
            .sum();
        if mins > 0 {
            out.push(format!("  {:<24}{}", s.name, hmm(mins)));
            total += mins;
        }
    }
    out.push(format!("  {:<24}{}", "Total", hmm(total)));
    out
}

/// Put `st.rendered` in the agenda buffer.
fn show(app: &mut App, st: &mut AgendaState) {
    let id = match st
        .buffer
        .filter(|id| app.editor.state.buffers.contains_key(id))
        .or_else(|| app.editor.find_buffer(BUFFER_NAME))
    {
        Some(id) => id,
        None => app.editor.add_buffer(Buffer::new(BUFFER_NAME)),
    };
    st.buffer = Some(id);
    let mut lines = st.rendered.lines.clone();
    for (i, rf) in st.rendered.refs.iter().enumerate() {
        if let Some(e) = rf.and_then(|e| st.rendered.entries.get(e))
            && let Some(buf) = st.ids.get(e.source).copied()
            && st.marks.contains(&(buf, e.heading.clone()))
            && lines[i].starts_with(' ')
        {
            lines[i].replace_range(0..1, ">");
        }
    }
    let keep_line = if app.active_id() == id {
        Some(app.buf_ref().point().line)
    } else {
        None
    };
    if let Some(b) = app.editor.state.get_buffer_mut(id) {
        b.set_locked(false);
        b.set_contents(&lines.join("\n"));
        b.set_major_mode(MODE);
        b.set_locked(true);
    }
    if app.active_id() != id {
        app.switch_to(id);
    }
    let line = keep_line.unwrap_or_else(|| first_item_line(st));
    let line = line.min(app.buf_ref().line_count().saturating_sub(1));
    app.buf().set_point(Pos::new(line, 0));
    app.win_mut().scroll_top = 0;
    app.win_mut().scroll_y_px = 0;
}

fn first_item_line(st: &AgendaState) -> usize {
    st.rendered
        .refs
        .iter()
        .position(Option::is_some)
        .unwrap_or(0)
}

/// Rebuild the agenda we have (after something changed).
pub fn redo(app: &mut App) {
    if let Some(mut st) = app.org.agenda.take() {
        st.target = None;
        rebuild(app, st);
    }
}

/// The day view of the agenda for `date` (what clicking a timestamp does).
pub fn show_date(app: &mut App, date: Date) {
    open_view_at(app, View::Span(Span::Day), date);
}

fn open_view(app: &mut App, view: View) {
    let (today, _) = now();
    open_view_at(app, view, today);
}

fn open_view_at(app: &mut App, view: View, anchor: Date) {
    let mut st = AgendaState::new(view, anchor);
    if let Some(old) = app.org.agenda.take() {
        // Settings carry over from the last agenda.
        st.log = old.log;
        st.archives = old.archives;
        st.entry_text = old.entry_text;
        st.follow = old.follow;
        st.restriction = old.restriction;
        st.marks = old.marks;
        st.buffer = old.buffer;
    }
    rebuild(app, st);
}

// ----------------------------------------------------- the entry at point

/// The entry the agenda point is on, with its source buffer and heading
/// line (re-found by text if the buffer changed).
fn entry_at(app: &mut App) -> Option<(BufferId, usize, Entry)> {
    let line = app.buf_ref().point().line;
    let st = app.org.agenda.as_ref()?;
    if app.buf_ref().major_mode() != MODE {
        return None;
    }
    let e = st
        .rendered
        .refs
        .get(line)
        .copied()
        .flatten()
        .and_then(|i| st.rendered.entries.get(i))?
        .clone();
    let id = *st.ids.get(e.source)?;
    let b = app.editor.state.get_buffer(id)?;
    let h = if e.line < b.line_count() && b.line(e.line) == e.heading {
        e.line
    } else {
        (0..b.line_count()).find(|&l| b.org_level(l).is_some() && b.line(l) == e.heading)?
    };
    Some((id, h, e))
}

fn need_entry(app: &mut App) -> Option<(BufferId, usize, Entry)> {
    let r = entry_at(app);
    if r.is_none() {
        app.error("No agenda entry at point");
    }
    r
}

/// Remember the entry a following question is about.
fn set_target(app: &mut App, id: BufferId, e: &Entry) {
    if let Some(st) = app.org.agenda.as_mut() {
        st.target = Some((id, e.heading.clone()));
    }
}

/// The target's buffer and current heading line.
fn target(app: &App) -> Option<(BufferId, usize)> {
    let st = app.org.agenda.as_ref()?;
    let (id, text) = st.target.clone()?;
    let b = app.editor.state.get_buffer(id)?;
    let h = (0..b.line_count()).find(|&l| b.org_level(l).is_some() && b.line(l) == text)?;
    Some((id, h))
}

fn edited(app: &mut App, id: BufferId) {
    if let Some(st) = app.org.agenda.as_mut() {
        st.last_edit = Some(id);
    }
}

/// Run `f` with the entry's buffer selected and point on its heading,
/// then come back to the agenda and rebuild it.
fn with_entry(app: &mut App, id: BufferId, h: usize, f: impl FnOnce(&mut App)) {
    let agenda = app.active_id();
    app.switch_to(id);
    app.buf().set_point(Pos::new(h, 0));
    f(app);
    edited(app, id);
    if app.editor.state.buffers.contains_key(&agenda) && app.active_id() != agenda {
        app.switch_to(agenda);
    }
    redo(app);
}

// ------------------------------------------------------------- commands

/// The span a new agenda shows: `forge.opt.org_agenda_span` ("day",
/// "week", "month" or "year"; week when unset), as `org-agenda-span`.
fn default_span(app: &App) -> Span {
    match crate::org_flow::setting_string(app, "org_agenda_span")
        .map(|s| s.to_lowercase())
        .as_deref()
    {
        Some("day") => Span::Day,
        Some("month") => Span::Month,
        Some("year") => Span::Year,
        _ => Span::Week,
    }
}

/// The dispatcher's window, laid out like `*Agenda Commands*`:
/// two columns of keys, then the custom commands. Also the keys it takes.
fn dispatcher_menu(custom: &[(char, String)]) -> (String, Vec<char>) {
    let left = [
        ('a', "Agenda for current week or day"),
        ('l', "Agenda for the week, with the log"),
        ('/', "Multi-occur"),
        ('t', "List of all TODO entries"),
        ('m', "Match a TAGS/PROP/TODO query"),
        ('s', "Search for keywords"),
    ];
    let right = [
        ('<', "Buffer, subtree/region restriction"),
        ('>', "Remove restriction"),
        ('e', "Export agenda views"),
        ('T', "Entries with special TODO kwd"),
        ('M', "Like m, but only TODO entries"),
        ('S', "Like s, but only TODO entries"),
        ('C', "Configure custom agenda command"),
        ('#', "List stuck projects"),
        ('!', "Configure stuck projects"),
    ];
    let mut out = String::from("Press key for an agenda command:\n");
    out.push_str(&"-".repeat(32));
    out.push('\n');
    for i in 0..left.len().max(right.len()) {
        let l = left
            .get(i)
            .map_or(String::new(), |(k, d)| format!("{k}   {d}"));
        let r = right
            .get(i)
            .map_or(String::new(), |(k, d)| format!("{k}   {d}"));
        if r.is_empty() {
            out.push_str(&l);
        } else {
            out.push_str(&format!("{l:<42}{r}"));
        }
        out.push('\n');
    }
    if !custom.is_empty() {
        out.push('\n');
        for (k, spec) in custom {
            out.push_str(&format!("{k}   {spec}\n"));
        }
    }
    let valid = left
        .iter()
        .chain(right.iter())
        .map(|(k, _)| *k)
        .chain(custom.iter().map(|(k, _)| *k))
        .collect();
    (out, valid)
}

pub fn command(app: &mut App, cmd: &str) -> bool {
    if !(cmd.starts_with("org-agenda")
        || matches!(
            cmd,
            "org-search-view"
                | "org-todo-list"
                | "org-tags-view"
                | "org-remove-file"
                | "org-cycle-agenda-files"
        ))
    {
        return false;
    }
    let back = app.has_arg();
    match cmd {
        "org-agenda" => {
            let restricted = app
                .org
                .agenda
                .as_ref()
                .and_then(|s| s.restriction.as_ref())
                .is_some();
            let (text, valid) = dispatcher_menu(&app.org.custom_agenda);
            let prompt = format!(
                "Press key for agenda command ({}): ",
                if restricted {
                    "restricted"
                } else {
                    "unrestricted"
                }
            );
            crate::one_key::open(app, "*Agenda Commands*", &text, &prompt, valid, |app, c| {
                answer(app, &Ask::AgendaKey, &c.to_string());
            });
            return true;
        }
        "org-agenda-list" => {
            open_view(app, View::Span(default_span(app)));
            return true;
        }
        "org-todo-list" => {
            open_view(app, View::Todo(None));
            return true;
        }
        "org-agenda-list-stuck-projects" => {
            open_view(app, View::Stuck);
            return true;
        }
        "org-search-view" => {
            ask(
                app,
                "Search words (+must -not {regexp}): ",
                Ask::AgendaSearch,
            );
            return true;
        }
        "org-tags-view" => {
            ask(
                app,
                "Match (tags/properties/todo): ",
                Ask::AgendaMatch { todo_only: back },
            );
            return true;
        }
        "org-agenda-capture" => return crate::org_cmds::command(app, "org-capture"),
        "org-agenda-file-to-front" => {
            match app.buf_ref().path().map(|p| p.to_path_buf()) {
                Some(p) => {
                    crate::org_flow::agenda_files(app);
                    app.editor.org_files.retain(|f| *f != p);
                    app.editor.org_files.insert(0, p.clone());
                    save_agenda_files(app);
                    app.msg(format!(
                        "Added {} to the front of the agenda files",
                        p.display()
                    ));
                }
                None => app.error("This buffer has no file"),
            }
            return true;
        }
        "org-remove-file" => {
            match app.buf_ref().path().map(|p| p.to_path_buf()) {
                Some(p) => {
                    crate::org_flow::agenda_files(app);
                    let before = app.editor.org_files.len();
                    app.editor.org_files.retain(|f| *f != p);
                    save_agenda_files(app);
                    app.msg(if app.editor.org_files.len() < before {
                        format!("Removed {} from the agenda files", p.display())
                    } else {
                        "This file is not an agenda file".to_string()
                    });
                }
                None => app.error("This buffer has no file"),
            }
            return true;
        }
        "org-cycle-agenda-files" => {
            let ids = agenda_buffers(app);
            if ids.is_empty() {
                app.error("No agenda files");
            } else {
                let cur = app.active_id();
                let next = ids
                    .iter()
                    .position(|i| *i == cur)
                    .map_or(0, |p| (p + 1) % ids.len());
                app.switch_to(ids[next]);
            }
            return true;
        }
        "org-agenda-set-restriction-lock" => {
            let id = app.active_id();
            let heading = app
                .buf_ref()
                .org_heading_at(app.buf_ref().point().line)
                .filter(|_| !back)
                .map(|h| app.buf_ref().line(h));
            let what = if heading.is_some() { "subtree" } else { "file" };
            let st = app
                .org
                .agenda
                .get_or_insert_with(|| AgendaState::new(View::Span(Span::Week), now().0));
            st.restriction = Some((id, heading));
            app.msg(format!("Agenda restricted to this {what}"));
            return true;
        }
        "org-agenda-remove-restriction-lock" => {
            if let Some(st) = app.org.agenda.as_mut() {
                st.restriction = None;
            }
            app.msg("Agenda restriction removed".to_string());
            if app.buf_ref().major_mode() == MODE {
                redo(app);
            }
            return true;
        }
        _ => {}
    }
    if app.buf_ref().major_mode() != MODE || app.org.agenda.is_none() {
        if cmd.starts_with("org-agenda-") {
            app.error("Not in an Org agenda buffer; open one with org-agenda");
            return true;
        }
        return false;
    }
    let line = app.buf_ref().point().line;
    match cmd {
        "org-agenda-next-line" | "org-agenda-previous-line" => {
            let fwd = cmd.ends_with("next-line");
            let st = app.org.agenda.as_ref().expect("checked");
            let n = st.rendered.refs.len();
            let target = if fwd {
                (line + 1..n).find(|&l| st.rendered.refs[l].is_some())
            } else {
                (0..line).rev().find(|&l| st.rendered.refs[l].is_some())
            };
            match target {
                Some(l) => {
                    app.buf().set_point(Pos::new(l, 0));
                    if app.org.agenda.as_ref().is_some_and(|s| s.follow) {
                        show_entry(app, false);
                    }
                }
                None => app.msg(
                    if fwd {
                        "End of agenda"
                    } else {
                        "Start of agenda"
                    }
                    .to_string(),
                ),
            }
        }
        "org-agenda-goto" => show_entry(app, true),
        "org-agenda-show-and-scroll-up" => show_entry(app, false),
        "org-agenda-switch-to" => {
            if let Some((id, h, _)) = need_entry(app) {
                app.switch_to(id);
                app.buf().set_point(Pos::new(h, 0));
                let off = app.buf_ref().point_offset();
                app.buf().reveal(off);
            }
        }
        "org-agenda-redo" => redo(app),
        "org-agenda-quit" | "org-agenda-exit" => {
            let id = app.active_id();
            if let Some(st) = app.org.agenda.as_mut() {
                st.target = None;
            }
            if cmd.ends_with("exit") {
                app.kill_buffer(id);
                app.org.agenda = None;
            } else if let Some(prev) = app.other_buffer() {
                app.switch_to(prev);
            }
        }
        "org-agenda-recenter" => {
            let _ = crate::commands::execute(app, "recenter-top-bottom");
        }
        "org-agenda-reset-view" => {
            let (today, _) = now();
            let span = default_span(app);
            if let Some(st) = app.org.agenda.as_mut() {
                st.view = View::Span(span);
                st.anchor = today;
                st.filter = Filter::default();
                st.limit = None;
            }
            redo(app);
        }
        "org-agenda-tree-to-indirect-buffer" => {
            if let Some((id, h, _)) = need_entry(app) {
                let agenda = app.active_id();
                app.switch_to(id);
                app.buf().set_point(Pos::new(h, 0));
                crate::commands::execute(app, "org-tree-to-indirect-buffer");
                let _ = agenda;
            }
        }
        // --------------------------------------------- acting on an entry
        "org-agenda-todo" => {
            if let Some((id, h, _)) = need_entry(app) {
                let (d, n) = now();
                let r = app
                    .editor
                    .state
                    .get_buffer_mut(id)
                    .map(|b| b.org_todo_logged(h, !back, d, n));
                if let Some(Err(e)) = r {
                    app.error(e);
                }
                edited(app, id);
                redo(app);
            }
        }
        "org-agenda-do-date-earlier" | "org-agenda-do-date-later" => {
            if let Some((id, h, e)) = need_entry(app) {
                let step = if back { 7 } else { 1 };
                let delta = if cmd.ends_with("later") { step } else { -step };
                let leader = match e.kind {
                    ag::Kind::Scheduled => "SCHEDULED:",
                    ag::Kind::Deadline => "DEADLINE:",
                    _ => "",
                };
                let mut moved = false;
                if let Some(b) = app.editor.state.get_buffer_mut(id) {
                    let found = b.org_entry_timestamps(h).into_iter().find(|(l, t)| {
                        let before: String = b.line(*l).chars().take(t.start).collect();
                        t.active && (leader.is_empty() || before.trim_end().ends_with(leader))
                    });
                    if let Some((l, t)) = found {
                        moved = b.org_timestamp_shift(Pos::new(l, t.start + 1), delta, true);
                    }
                }
                if moved {
                    edited(app, id);
                    redo(app);
                } else {
                    app.error("Entry has no date to move");
                }
            }
        }
        "org-agenda-priority-up" | "org-agenda-priority-down" => {
            if let Some((id, h, _)) = need_entry(app) {
                let act = if cmd.ends_with("up") {
                    PriorityAction::Up
                } else {
                    PriorityAction::Down
                };
                if let Some(Err(e)) = app
                    .editor
                    .state
                    .get_buffer_mut(id)
                    .map(|b| b.org_set_priority(h, act))
                {
                    app.error(e);
                }
                edited(app, id);
                redo(app);
            }
        }
        "org-agenda-priority" => {
            if let Some((id, _, e)) = need_entry(app) {
                set_target(app, id, &e);
                ask(
                    app,
                    "Priority (letter, SPC to remove, +/- to change): ",
                    Ask::AgendaPriority,
                );
            }
        }
        "org-agenda-schedule" | "org-agenda-deadline" => {
            if let Some((id, h, e)) = need_entry(app) {
                let kind = if cmd.ends_with("schedule") {
                    "SCHEDULED"
                } else {
                    "DEADLINE"
                };
                if back {
                    if let Some(b) = app.editor.state.get_buffer_mut(id) {
                        b.org_set_planning(h, kind, None);
                    }
                    edited(app, id);
                    redo(app);
                } else {
                    set_target(app, id, &e);
                    ask(
                        app,
                        if kind == "SCHEDULED" {
                            "Schedule to (date): "
                        } else {
                            "Deadline (date): "
                        },
                        if kind == "SCHEDULED" {
                            Ask::Schedule
                        } else {
                            Ask::Deadline
                        },
                    );
                }
            }
        }
        "org-agenda-refile" => {
            if let Some((id, _, e)) = need_entry(app) {
                set_target(app, id, &e);
                let _ = e;
                // The shared refile prompt uses the same target list.
                let c = refile_candidates(app);
                choose(app, "Refile to: ", Ask::Refile { copy: false }, c);
            }
        }
        "org-agenda-add-note" => {
            if let Some((id, _, e)) = need_entry(app) {
                set_target(app, id, &e);
                ask(app, "Note: ", Ask::AgendaNote);
            }
        }
        "org-agenda-archive" => {
            if let Some((id, h, _)) = need_entry(app) {
                with_entry(app, id, h, |app| {
                    crate::commands::execute(app, "org-archive-subtree-default");
                });
            }
        }
        "org-agenda-clock-in" => {
            if let Some((id, h, _)) = need_entry(app) {
                clock_in_at(app, id, h);
                edited(app, id);
                redo(app);
            }
        }
        "org-agenda-clock-out" => {
            crate::commands::execute(app, "org-clock-out");
            redo(app);
        }
        "org-agenda-clock-cancel" => {
            crate::commands::execute(app, "org-clock-cancel");
            redo(app);
        }
        "org-agenda-clock-goto" => clock_goto(app, false),
        "org-agenda-set-tags" => {
            if let Some((id, h, e)) = need_entry(app) {
                set_target(app, id, &e);
                let tags = app
                    .editor
                    .state
                    .get_buffer(id)
                    .map(|b| b.org_heading_tags(h).join(":"))
                    .unwrap_or_default();
                ask_with(
                    app,
                    "Tags: ",
                    Ask::SetTags,
                    &if tags.is_empty() {
                        String::new()
                    } else {
                        format!(":{tags}:")
                    },
                );
            }
        }
        "org-agenda-set-effort" => {
            if let Some((id, h, e)) = need_entry(app) {
                set_target(app, id, &e);
                let cur = app
                    .editor
                    .state
                    .get_buffer(id)
                    .and_then(|b| b.org_get_property(h, "Effort", false))
                    .unwrap_or_default();
                ask_with(app, "Effort: ", Ask::Effort, &cur);
            }
        }
        "org-agenda-toggle-archive-tag" => {
            if let Some((id, h, _)) = need_entry(app) {
                if let Some(b) = app.editor.state.get_buffer_mut(id) {
                    b.org_toggle_tag(h, "ARCHIVE");
                }
                edited(app, id);
                redo(app);
            }
        }
        "org-agenda-toggle-diary" => {
            if let Some(st) = app.org.agenda.as_mut() {
                st.diary = !st.diary;
                let on = st.diary;
                app.msg(format!(
                    "Diary {} (recorded: Forge has no diary entries to show)",
                    if on { "on" } else { "off" }
                ));
            }
        }
        // ------------------------------------------------- dates, views
        "org-agenda-goto-date" => ask(app, "Show agenda for date: ", Ask::AgendaGotoDate),
        "org-agenda-goto-today" => {
            let (today, _) = now();
            if let Some(st) = app.org.agenda.as_mut() {
                st.anchor = today;
            }
            redo(app);
        }
        "org-agenda-goto-calendar" => show_calendar(app),
        "org-agenda-day-view"
        | "org-agenda-week-view"
        | "org-agenda-month-view"
        | "org-agenda-year-view" => {
            let span = match cmd {
                "org-agenda-day-view" => Span::Day,
                "org-agenda-week-view" => Span::Week,
                "org-agenda-month-view" => Span::Month,
                _ => Span::Year,
            };
            let anchor = entry_date(app)
                .unwrap_or_else(|| app.org.agenda.as_ref().map_or(now().0, |s| s.anchor));
            if let Some(st) = app.org.agenda.as_mut() {
                st.view = View::Span(span);
                st.anchor = anchor;
            }
            redo(app);
        }
        "org-agenda-later" | "org-agenda-earlier" => {
            if let Some(st) = app.org.agenda.as_mut()
                && let View::Span(sp) = st.view
            {
                st.anchor = sp.step(st.anchor, cmd.ends_with("later"));
            }
            redo(app);
        }
        "org-agenda-follow-mode"
        | "org-agenda-log-mode"
        | "org-agenda-entry-text-mode"
        | "org-agenda-archives-mode"
        | "org-agenda-clockreport-mode" => {
            let st = app.org.agenda.as_mut().expect("checked");
            let (flag, name) = match cmd {
                "org-agenda-follow-mode" => (&mut st.follow, "Follow"),
                "org-agenda-log-mode" => (&mut st.log, "Log"),
                "org-agenda-entry-text-mode" => (&mut st.entry_text, "Entry text"),
                "org-agenda-archives-mode" => (&mut st.archives, "Archives"),
                _ => (&mut st.clockreport, "Clock report"),
            };
            *flag = !*flag;
            let on = *flag;
            app.msg(format!("{name} mode {}", if on { "on" } else { "off" }));
            redo(app);
        }
        // ------------------------------------------------------ filters
        "org-agenda-filter" => ask(
            app,
            "Filter (+tag -tag <category =regexp _<effort; empty clears): ",
            Ask::AgendaFilter,
        ),
        "org-agenda-filter-by-tag" => ask(
            app,
            "Filter by tag (+tag or -tag; empty clears): ",
            Ask::AgendaFilterTag,
        ),
        "org-agenda-filter-by-category" => ask(
            app,
            "Filter by category (-cat to hide; empty clears): ",
            Ask::AgendaFilterCategory,
        ),
        "org-agenda-filter-by-regexp" => ask(
            app,
            "Filter by regexp (-re to hide; empty clears): ",
            Ask::AgendaFilterRegexp,
        ),
        "org-agenda-filter-by-effort" => ask(
            app,
            "Filter by effort (<1:00, >0:30, =2:00; empty clears): ",
            Ask::AgendaFilterEffort,
        ),
        "org-agenda-filter-by-top-headline" => {
            if let Some((id, h, _)) = need_entry(app) {
                let top = app.editor.state.get_buffer(id).map(|b| {
                    let mut cur = h;
                    while let Some(p) = b.org_up_heading(cur) {
                        cur = p;
                    }
                    cur
                });
                let si = app
                    .org
                    .agenda
                    .as_ref()
                    .and_then(|s| s.ids.iter().position(|i| *i == id));
                if let (Some(top), Some(si), Some(st)) = (top, si, app.org.agenda.as_mut()) {
                    st.filter.top_headline = Some((si, top));
                }
                redo(app);
            }
        }
        "org-agenda-limit-interactively" => ask(
            app,
            "Show only the first N entries (empty for all): ",
            Ask::AgendaFilterEffort,
        ),
        // --------------------------------------------------- bulk marks
        "org-agenda-bulk-mark" | "org-agenda-bulk-unmark" | "org-agenda-bulk-toggle" => {
            if let Some((id, _, e)) = need_entry(app) {
                let key = (id, e.heading.clone());
                if let Some(st) = app.org.agenda.as_mut() {
                    let has = st.marks.contains(&key);
                    match cmd {
                        "org-agenda-bulk-mark" => st.marks.insert(key),
                        "org-agenda-bulk-unmark" => st.marks.remove(&key),
                        _ if has => st.marks.remove(&key),
                        _ => st.marks.insert(key),
                    };
                }
                redo_keep_line(app, line);
                let _ = crate::commands::execute(app, "org-agenda-next-line");
            }
        }
        "org-agenda-bulk-mark-all"
        | "org-agenda-bulk-unmark-all"
        | "org-agenda-bulk-toggle-all" => {
            if let Some(st) = app.org.agenda.as_mut() {
                let all: Vec<(BufferId, String)> = st
                    .rendered
                    .entries
                    .iter()
                    .filter_map(|e| st.ids.get(e.source).map(|id| (*id, e.heading.clone())))
                    .collect();
                match cmd {
                    "org-agenda-bulk-mark-all" => st.marks.extend(all),
                    "org-agenda-bulk-unmark-all" => st.marks.clear(),
                    _ => {
                        for k in all {
                            if !st.marks.remove(&k) {
                                st.marks.insert(k);
                            }
                        }
                    }
                }
            }
            redo_keep_line(app, line);
        }
        "org-agenda-bulk-mark-regexp" => {
            ask(app, "Mark entries matching regexp: ", Ask::AgendaBulkRegexp)
        }
        "org-agenda-bulk-action" => {
            let n = app.org.agenda.as_ref().map_or(0, |s| s.marks.len());
            if n == 0 {
                app.error("No entries are marked");
            } else {
                if let Some(st) = app.org.agenda.as_mut() {
                    st.bulk = None;
                }
                choose(
                    app,
                    &format!(
                        "Bulk action on {n} entr{}: ",
                        if n == 1 { "y" } else { "ies" }
                    ),
                    Ask::AgendaBulkAction,
                    [
                        "t: set TODO state",
                        "+: add a tag",
                        "-: remove a tag",
                        "s: schedule",
                        "d: set deadline",
                        "r: refile",
                        "$: archive",
                        "A: archive to sibling",
                    ]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                );
            }
        }
        "org-agenda-undo" => match app.org.agenda.as_ref().and_then(|s| s.last_edit) {
            Some(id) if app.editor.state.buffers.contains_key(&id) => {
                let agenda = app.active_id();
                app.switch_to(id);
                crate::commands::execute(app, "undo");
                app.switch_to(agenda);
                redo(app);
            }
            _ => app.error("Nothing to undo"),
        },
        "org-agenda-write" => ask(
            app,
            "Write agenda to file (.txt, .org or .html): ",
            Ask::AgendaWrite,
        ),
        _ => return false,
    }
    true
}

fn redo_keep_line(app: &mut App, line: usize) {
    redo(app);
    let l = line.min(app.buf_ref().line_count().saturating_sub(1));
    app.buf().set_point(Pos::new(l, 0));
}

fn entry_date(app: &App) -> Option<Date> {
    let st = app.org.agenda.as_ref()?;
    let line = app.buf_ref().point().line;
    st.rendered.dates.get(line).copied().flatten()
}

/// Show the entry at point: in another window (`select`) or leave focus.
fn show_entry(app: &mut App, select: bool) {
    let Some((id, h, _)) = need_entry(app) else {
        return;
    };
    let agenda_win = app.editor.windows.selected_id();
    crate::windows::pop_to_other_window(app);
    app.switch_to(id);
    app.buf().set_point(Pos::new(h, 0));
    let off = app.buf_ref().point_offset();
    app.buf().reveal(off);
    if !select {
        app.select_window(agenda_win);
    }
}

fn show_calendar(app: &mut App) {
    let anchor = app.org.agenda.as_ref().map_or(now().0, |s| s.anchor);
    let mut text = String::new();
    for k in -1..=1 {
        let first = Date::new(anchor.y, anchor.m, 1).add_months(k);
        text.push_str(&format!(
            "{:^28}\n",
            format!("{} {}", MONTH_NAMES[(first.m - 1) as usize], first.y)
        ));
        text.push_str(" Mo  Tu  We  Th  Fr  Sa  Su\n");
        let mut col = first.weekday();
        text.push_str(&"    ".repeat(col));
        for d in 1..=Date::days_in_month(first.y, first.m) {
            let mark = if first.y == anchor.y && first.m == anchor.m && d == anchor.d {
                '*'
            } else {
                ' '
            };
            text.push_str(&format!("{d:>3}{mark}"));
            col += 1;
            if col == 7 {
                text.push('\n');
                col = 0;
            }
        }
        if col != 0 {
            text.push('\n');
        }
        text.push('\n');
    }
    app.show_special_buffer("*Calendar*", "special-mode", &text);
}

fn refile_candidates(app: &mut App) -> Vec<String> {
    let mut ids = agenda_buffers(app);
    let cur = app.active_id();
    if !ids.contains(&cur) {
        ids.insert(0, cur);
    }
    let mut out = Vec::new();
    for id in ids {
        let name = buffer_label(app, id);
        out.push(format!("{name}/"));
        if let Some(b) = app.editor.state.get_buffer(id) {
            for (_, path) in b.org_refile_targets(3) {
                out.push(format!("{name}: {path}"));
            }
        }
    }
    out
}

fn refile_target(app: &mut App, text: &str) -> Option<(BufferId, Option<usize>)> {
    let mut ids = agenda_buffers(app);
    let cur = app.active_id();
    if !ids.contains(&cur) {
        ids.insert(0, cur);
    }
    for id in ids {
        let name = buffer_label(app, id);
        if text == format!("{name}/") {
            return Some((id, None));
        }
        if let Some(b) = app.editor.state.get_buffer(id) {
            for (l, path) in b.org_refile_targets(3) {
                if text == format!("{name}: {path}") {
                    return Some((id, Some(l)));
                }
            }
        }
    }
    None
}

// ------------------------------------------------------------ answers

fn parse_effort_filter(s: &str) -> Option<(char, i64)> {
    let op = s.chars().next().filter(|c| matches!(c, '<' | '>' | '='))?;
    let v = forge_core::org_ext::parse_effort(s[1..].trim())?;
    Some((op, (v * 60.0).round() as i64))
}

fn apply_filter_token(f: &mut Filter, tok: &str) -> Result<(), String> {
    if let Some(r) = tok.strip_prefix('=') {
        f.regexp = Some((true, r.to_string()));
    } else if let Some(r) = tok.strip_prefix("-=") {
        f.regexp = Some((false, r.to_string()));
    } else if let Some(c) = tok.strip_prefix('<') {
        f.category = Some((true, c.to_string()));
    } else if let Some(c) = tok.strip_prefix("-<") {
        f.category = Some((false, c.to_string()));
    } else if let Some(e) = tok.strip_prefix('_') {
        f.effort = Some(parse_effort_filter(e).ok_or_else(|| format!("Bad effort filter: {e}"))?);
    } else if let Some(t) = tok.strip_prefix('+') {
        f.tags.push((true, t.to_string()));
    } else if let Some(t) = tok.strip_prefix('-') {
        f.tags.push((false, t.to_string()));
    } else if !tok.is_empty() {
        f.tags.push((true, tok.to_string()));
    }
    Ok(())
}

/// Run a custom agenda command's view spec ("tags +work", "stuck", …).
fn run_custom(app: &mut App, spec: &str) {
    let (ty, arg) = spec.split_once(' ').unwrap_or((spec, ""));
    let arg = arg.trim().to_string();
    let view = match ty {
        "agenda" => View::Span(default_span(app)),
        "todo" => View::Todo((!arg.is_empty()).then_some(arg)),
        "tags" => View::Match {
            expr: arg,
            todo_only: false,
        },
        "tags-todo" => View::Match {
            expr: arg,
            todo_only: true,
        },
        "search" => View::Search(arg),
        _ => View::Stuck,
    };
    open_view(app, view);
}

pub fn answer(app: &mut App, a: &Ask, input: &str) -> bool {
    let t = input.trim();
    match a {
        Ask::AgendaKey => {
            let key = t.chars().next().unwrap_or(' ');
            match key {
                'a' => open_view(app, View::Span(default_span(app))),
                'l' => {
                    open_view(app, View::Span(default_span(app)));
                    if let Some(st) = app.org.agenda.as_mut() {
                        st.log = true;
                    }
                    redo(app);
                }
                't' => open_view(app, View::Todo(None)),
                'T' => {
                    let kws: Vec<String> = app
                        .buf_ref()
                        .org_todo_entries()
                        .into_iter()
                        .map(|(_, k, _, _)| k)
                        .fold(Vec::new(), |mut acc, k| {
                            if !acc.contains(&k) {
                                acc.push(k);
                            }
                            acc
                        });
                    choose(app, "TODO keyword: ", Ask::AgendaTodoKeyword, kws);
                }
                'm' => ask(
                    app,
                    "Match (tags/properties/todo): ",
                    Ask::AgendaMatch { todo_only: false },
                ),
                'M' => ask(
                    app,
                    "Match (TODO entries only): ",
                    Ask::AgendaMatch { todo_only: true },
                ),
                's' => ask(
                    app,
                    "Search words (+must -not {regexp}): ",
                    Ask::AgendaSearch,
                ),
                'S' => ask(
                    app,
                    "Search words, TODO entries only (+must -not {regexp}): ",
                    Ask::AgendaSearchTodo,
                ),
                '/' => ask(
                    app,
                    "Regexp to find in the agenda files: ",
                    Ask::AgendaOccur,
                ),
                '#' => open_view(app, View::Stuck),
                '!' => {
                    let cur = match &app.org.stuck_projects {
                        Some((p, a)) => format!("{} | {}", p.join(" "), a.join(" ")),
                        None => "TODO | NEXT STARTED".to_string(),
                    };
                    ask(
                        app,
                        &format!("Stuck projects (PROJECT-KEYWORDS | ACTIVE-KEYWORDS) [{cur}]: "),
                        Ask::AgendaStuckConfig,
                    );
                }
                '<' => {
                    // Restrict to this file (or the subtree at point), then
                    // offer the dispatcher again.
                    if app.buf_ref().major_mode() == MODE {
                        app.error("Cannot restrict from the agenda buffer");
                    } else {
                        command(app, "org-agenda-set-restriction-lock");
                        command(app, "org-agenda");
                    }
                }
                '>' => {
                    command(app, "org-agenda-remove-restriction-lock");
                    command(app, "org-agenda");
                }
                'e' => {
                    if app.org.agenda.is_some() {
                        ask(
                            app,
                            "Write agenda to file (.txt, .html, .org): ",
                            Ask::AgendaWrite,
                        );
                    } else {
                        app.error("No agenda to write; open one first");
                    }
                }
                'C' => ask(
                    app,
                    "Custom command (KEY TYPE [ARG]; TYPE: agenda todo tags tags-todo search stuck; KEY alone removes): ",
                    Ask::AgendaCustomConfig,
                ),
                k => match app.org.custom_agenda.iter().find(|(c, _)| *c == k).cloned() {
                    Some((_, spec)) => run_custom(app, &spec),
                    None => app.error(format!("No agenda command {key}")),
                },
            }
        }
        Ask::AgendaTodoKeyword => {
            open_view(app, View::Todo((!t.is_empty()).then(|| t.to_string())))
        }
        Ask::AgendaMatch { todo_only } => {
            if !t.is_empty() {
                open_view(
                    app,
                    View::Match {
                        expr: t.to_string(),
                        todo_only: *todo_only,
                    },
                );
            }
        }
        Ask::AgendaSearch => {
            if !t.is_empty() {
                open_view(app, View::Search(t.to_string()));
            }
        }
        Ask::AgendaSearchTodo => {
            if !t.is_empty() {
                open_view(app, View::SearchTodo(t.to_string()));
            }
        }
        Ask::AgendaOccur => {
            if !t.is_empty() {
                open_view(app, View::Search(format!("{{{t}}}")));
            }
        }
        Ask::AgendaStuckConfig => {
            if t.is_empty() {
                app.org.stuck_projects = None;
                app.msg("Stuck projects reset to the default".to_string());
            } else {
                let (p, a) = t.split_once('|').unwrap_or((t, "NEXT STARTED"));
                let words = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
                app.org.stuck_projects = Some((words(p), words(a)));
                app.msg("Stuck projects configured".to_string());
            }
        }
        Ask::AgendaCustomConfig => {
            let mut it = t.splitn(3, char::is_whitespace);
            let key = it.next().and_then(|k| k.chars().next());
            let ty = it.next().unwrap_or("").trim();
            let arg = it.next().unwrap_or("").trim();
            match key {
                None => {}
                Some(k) if ty.is_empty() => {
                    app.org.custom_agenda.retain(|(c, _)| *c != k);
                    app.msg(format!("Removed custom agenda command {k}"));
                }
                Some(k) if "aStTmMsl#!/<>eC".contains(k) => {
                    app.error(format!("{k} is a built-in agenda key"));
                }
                Some(k) => {
                    if !matches!(
                        ty,
                        "agenda" | "todo" | "tags" | "tags-todo" | "search" | "stuck"
                    ) {
                        app.error(format!("Unknown type {ty}"));
                    } else {
                        let spec = format!("{ty} {arg}").trim().to_string();
                        app.org.custom_agenda.retain(|(c, _)| *c != k);
                        app.org.custom_agenda.push((k, spec));
                        app.msg(format!("Custom agenda command {k} defined"));
                    }
                }
            }
        }
        Ask::AgendaGotoDate => {
            let (today, _) = now();
            match parse_date_input(t, today, today) {
                Some((d, _)) => {
                    if let Some(st) = app.org.agenda.as_mut() {
                        st.anchor = d;
                    }
                    redo(app);
                }
                None => app.error(format!("Bad date: {t}")),
            }
        }
        Ask::AgendaFilter
        | Ask::AgendaFilterTag
        | Ask::AgendaFilterCategory
        | Ask::AgendaFilterRegexp => {
            let mut result = Ok(());
            if let Some(st) = app.org.agenda.as_mut() {
                if t.is_empty() {
                    st.filter = Filter::default();
                } else {
                    for tok in t.split_whitespace() {
                        let tok = match a {
                            Ask::AgendaFilterCategory if !tok.starts_with(['<', '-']) => {
                                format!("<{tok}")
                            }
                            Ask::AgendaFilterCategory => tok.replacen('-', "-<", 1),
                            Ask::AgendaFilterRegexp if !tok.starts_with(['=', '-']) => {
                                format!("={tok}")
                            }
                            Ask::AgendaFilterRegexp => tok.replacen('-', "-=", 1),
                            _ => tok.to_string(),
                        };
                        if let Err(e) = apply_filter_token(&mut st.filter, &tok) {
                            result = Err(e);
                        }
                    }
                }
            }
            match result {
                Ok(()) => redo(app),
                Err(e) => app.error(e),
            }
        }
        Ask::AgendaFilterEffort => {
            // Shared by the effort filter and the entry limit.
            let by_limit = t.parse::<usize>().ok();
            if t.is_empty() {
                if let Some(st) = app.org.agenda.as_mut() {
                    st.filter.effort = None;
                    st.limit = None;
                }
                redo(app);
            } else if let Some(n) = by_limit {
                if let Some(st) = app.org.agenda.as_mut() {
                    st.limit = Some(n);
                }
                redo(app);
            } else if let Some(e) = parse_effort_filter(t) {
                if let Some(st) = app.org.agenda.as_mut() {
                    st.filter.effort = Some(e);
                }
                redo(app);
            } else {
                app.error(format!("Bad effort filter or limit: {t}"));
            }
        }
        Ask::AgendaBulkRegexp => match regex::Regex::new(&format!("(?i){t}")) {
            Ok(re) => {
                let mut count = 0;
                if let Some(st) = app.org.agenda.as_mut() {
                    let hits: Vec<(BufferId, String)> = st
                        .rendered
                        .entries
                        .iter()
                        .filter(|e| re.is_match(&e.heading))
                        .filter_map(|e| st.ids.get(e.source).map(|id| (*id, e.heading.clone())))
                        .collect();
                    count = hits.len();
                    st.marks.extend(hits);
                }
                app.msg(format!(
                    "Marked {count} entr{}",
                    if count == 1 { "y" } else { "ies" }
                ));
                redo(app);
            }
            Err(e) => app.error(format!("Bad regexp: {e}")),
        },
        Ask::AgendaBulkAction => bulk_answer(app, t),
        Ask::AgendaPriority => {
            let Some((id, h)) = target(app) else {
                return true;
            };
            let act = match t.chars().next() {
                None | Some(' ') => PriorityAction::Remove,
                Some('+') | Some('p') => PriorityAction::Up,
                Some('-') | Some('n') => PriorityAction::Down,
                Some(c) => PriorityAction::Set(c),
            };
            if let Some(Err(e)) = app
                .editor
                .state
                .get_buffer_mut(id)
                .map(|b| b.org_set_priority(h, act))
            {
                app.error(e);
            }
            edited(app, id);
            redo(app);
        }
        Ask::AgendaNote => {
            let Some((id, h)) = target(app) else {
                return true;
            };
            if !t.is_empty() {
                let (d, n) = now();
                if let Some(b) = app.editor.state.get_buffer_mut(id) {
                    add_note(b, h, t, d, n);
                }
                edited(app, id);
            }
            redo(app);
        }
        Ask::AgendaWrite => write_agenda(app, t),
        // Questions shared with the plain commands act on the agenda's
        // target entry when one is set.
        Ask::SetTags | Ask::Effort | Ask::Schedule | Ask::Deadline | Ask::Refile { .. }
            if app.org.agenda.as_ref().is_some_and(|s| s.target.is_some())
                && app.buf_ref().major_mode() == MODE =>
        {
            let Some((id, h)) = target(app) else {
                return true;
            };
            let (today, _) = now();
            match a {
                Ask::SetTags => {
                    let tags: Vec<String> = t
                        .split(|c: char| c == ':' || c == ',' || c.is_whitespace())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect();
                    if let Some(b) = app.editor.state.get_buffer_mut(id) {
                        b.org_set_tags(h, &tags);
                    }
                }
                Ask::Effort => {
                    if let Some(b) = app.editor.state.get_buffer_mut(id) {
                        if t.is_empty() {
                            b.org_delete_property(h, "Effort");
                        } else {
                            b.org_set_property(h, "Effort", t);
                        }
                    }
                }
                Ask::Schedule | Ask::Deadline => match parse_date_input(t, today, today) {
                    Some((d, time)) if !t.is_empty() => {
                        let kind = if matches!(a, Ask::Schedule) {
                            "SCHEDULED"
                        } else {
                            "DEADLINE"
                        };
                        if let Some(b) = app.editor.state.get_buffer_mut(id) {
                            b.org_set_planning(h, kind, Some(Timestamp::new(true, d, time)));
                        }
                    }
                    Some(_) => {}
                    None => return err(app, &format!("Bad date: {t}")),
                },
                Ask::Refile { copy } => match refile_target(app, t) {
                    Some(dest) => refile_to(app, id, h, dest, *copy),
                    None => return err(app, &format!("No such refile target: {t}")),
                },
                _ => {}
            }
            edited(app, id);
            redo(app);
        }
        _ => return false,
    }
    true
}

fn err(app: &mut App, m: &str) -> bool {
    app.error(m.to_string());
    true
}

/// `- Note taken on [ts] \\ text` in the entry's LOGBOOK.
pub(crate) fn add_note(b: &mut Buffer, h: usize, text: &str, d: Date, n: forge_core::org_time::Hm) {
    let stamp = Timestamp::new(false, d, Some(n)).text();
    let note = format!(
        "- Note taken on {stamp} \\\\\n  {}",
        text.replace('\n', "\n  ")
    );
    let limit = b.org_subtree_end(h).min(
        (h + 1..b.line_count())
            .find(|&l| b.org_level(l).is_some())
            .map_or(usize::MAX, |l| l - 1),
    );
    let logbook = (h + 1..=limit.min(b.line_count().saturating_sub(1)))
        .find(|&l| b.line(l).trim().eq_ignore_ascii_case(":LOGBOOK:"));
    match logbook {
        Some(l) => b.org_insert_line_after(l, &note),
        None => {
            let mut after = b.org_planning_line(h).unwrap_or(h);
            if let Some((_, z)) = b.org_property_drawer(h) {
                after = z;
            }
            b.org_insert_line_after(after, &format!(":LOGBOOK:\n{note}\n:END:"));
        }
    }
}

fn bulk_answer(app: &mut App, t: &str) {
    let pending = app.org.agenda.as_ref().and_then(|s| s.bulk);
    let key = pending.unwrap_or_else(|| t.chars().next().unwrap_or(' '));
    let arg = if pending.is_some() {
        t.to_string()
    } else {
        String::new()
    };
    let needs_arg = matches!(key, 't' | '+' | '-' | 's' | 'd' | 'r');
    if pending.is_none() && needs_arg {
        if let Some(st) = app.org.agenda.as_mut() {
            st.bulk = Some(key);
        }
        let (prompt, cands) = match key {
            't' => ("Set TODO state to (empty to clear): ", None),
            '+' => ("Add tag: ", None),
            '-' => ("Remove tag: ", None),
            's' => ("Schedule to (date): ", None),
            'd' => ("Deadline (date): ", None),
            _ => ("Refile to: ", Some(refile_candidates(app))),
        };
        match cands {
            Some(c) => choose(app, prompt, Ask::AgendaBulkAction, c),
            None => ask(app, prompt, Ask::AgendaBulkAction),
        }
        return;
    }
    let marks: Vec<(BufferId, String)> = app
        .org
        .agenda
        .as_ref()
        .map(|s| s.marks.iter().cloned().collect())
        .unwrap_or_default();
    let (today, now_t) = now();
    let dest = if key == 'r' {
        refile_target(app, &arg)
    } else {
        None
    };
    if key == 'r' && dest.is_none() {
        return app.error(format!("No such refile target: {arg}"));
    }
    let when = if matches!(key, 's' | 'd') {
        match parse_date_input(&arg, today, today) {
            Some(x) => Some(x),
            None => return app.error(format!("Bad date: {arg}")),
        }
    } else {
        None
    };
    let mut done = 0;
    for (id, text) in marks {
        let Some(b) = app.editor.state.get_buffer(id) else {
            continue;
        };
        let Some(h) = (0..b.line_count()).find(|&l| b.org_level(l).is_some() && b.line(l) == text)
        else {
            continue;
        };
        match key {
            't' => {
                if let Some(b) = app.editor.state.get_buffer_mut(id) {
                    set_todo_keyword(b, h, &arg);
                }
            }
            '+' | '-' => {
                if let Some(b) = app.editor.state.get_buffer_mut(id) {
                    let has = b.org_heading_tags(h).contains(&arg);
                    if (key == '+') != has {
                        b.org_toggle_tag(h, &arg);
                    }
                }
            }
            's' | 'd' => {
                if let (Some(b), Some((d, time))) = (app.editor.state.get_buffer_mut(id), when) {
                    b.org_set_planning(
                        h,
                        if key == 's' { "SCHEDULED" } else { "DEADLINE" },
                        Some(Timestamp::new(true, d, time)),
                    );
                }
            }
            'r' => {
                if let Some(dest) = dest {
                    refile_to(app, id, h, dest, false);
                }
            }
            '$' => with_entry_quiet(app, id, h, |app| {
                crate::commands::execute(app, "org-archive-subtree-default");
            }),
            'A' => {
                if let Some(b) = app.editor.state.get_buffer_mut(id) {
                    let _ = b.org_archive_to_sibling(h, today, now_t);
                }
            }
            _ => {
                app.error(format!("No bulk action {key}"));
                break;
            }
        }
        done += 1;
    }
    if let Some(st) = app.org.agenda.as_mut() {
        st.bulk = None;
        st.marks.clear();
    }
    app.msg(format!(
        "Acted on {done} entr{}",
        if done == 1 { "y" } else { "ies" }
    ));
    redo(app);
}

fn with_entry_quiet(app: &mut App, id: BufferId, h: usize, f: impl FnOnce(&mut App)) {
    let agenda = app.active_id();
    app.switch_to(id);
    app.buf().set_point(Pos::new(h, 0));
    f(app);
    if app.editor.state.buffers.contains_key(&agenda) {
        app.switch_to(agenda);
    }
}

/// Set the heading's TODO keyword to `kw` (empty clears it).
pub(crate) fn set_todo_keyword(b: &mut Buffer, h: usize, kw: &str) {
    let k = forge_core::org::keywords(b);
    let Some(hd) = forge_core::org::parse_heading(&b.line(h), &k) else {
        return;
    };
    let line = b.line(h);
    let stars = "*".repeat(hd.level);
    let mut rest = line[hd.level..].trim_start().to_string();
    if let Some(old) = &hd.keyword {
        rest = rest[old.len()..].trim_start().to_string();
    }
    let new = if kw.is_empty() {
        format!("{stars} {rest}")
    } else {
        format!("{stars} {kw} {rest}")
    };
    b.org_set_line(h, new.trim_end());
}

fn write_agenda(app: &mut App, file: &str) {
    if file.is_empty() {
        return;
    }
    let Some(st) = app.org.agenda.as_ref() else {
        return;
    };
    let text = st.rendered.text();
    let path = crate::vc::expand_home(file);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("txt")
        .to_lowercase();
    let out = match ext.as_str() {
        "html" | "htm" => {
            let esc = text
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            format!(
                "<!doctype html>\n<meta charset=\"utf-8\"><title>Org Agenda</title>\n<pre>\n{esc}\n</pre>\n"
            )
        }
        "org" => {
            text.lines()
                .map(|l| format!("  {l}"))
                .collect::<Vec<_>>()
                .join("\n")
                + "\n"
        }
        _ => format!("{text}\n"),
    };
    match std::fs::write(&path, out) {
        Ok(()) => app.msg(format!("Agenda written to {}", path.display())),
        Err(e) => app.error(format!("Cannot write {}: {e}", path.display())),
    }
}

/// Keymap helpers.
pub const AGENDA_BINDINGS: &[(&str, &str)] = &[
    ("n", "org-agenda-next-line"),
    ("p", "org-agenda-previous-line"),
    ("C-n", "org-agenda-next-line"),
    ("C-p", "org-agenda-previous-line"),
    ("<down>", "org-agenda-next-line"),
    ("<up>", "org-agenda-previous-line"),
    ("TAB", "org-agenda-goto"),
    ("RET", "org-agenda-switch-to"),
    ("<mouse-1>", "org-agenda-switch-to"),
    ("<mouse-2>", "org-agenda-switch-to"),
    ("SPC", "org-agenda-show-and-scroll-up"),
    ("g", "org-agenda-redo"),
    ("r", "org-agenda-redo"),
    ("q", "org-agenda-quit"),
    ("x", "org-agenda-exit"),
    ("t", "org-agenda-todo"),
    (",", "org-agenda-priority"),
    ("S-<up>", "org-agenda-priority-up"),
    ("S-<down>", "org-agenda-priority-down"),
    ("+", "org-agenda-priority-up"),
    ("-", "org-agenda-priority-down"),
    ("S-<left>", "org-agenda-do-date-earlier"),
    ("S-<right>", "org-agenda-do-date-later"),
    ("C-c C-t", "org-agenda-todo"),
    ("C-c ,", "org-agenda-priority"),
    ("C-c C-z", "org-agenda-add-note"),
    ("C-c C-x C-i", "org-agenda-clock-in"),
    ("C-c C-x C-o", "org-agenda-clock-out"),
    ("C-c C-x C-x", "org-agenda-clock-cancel"),
    ("C-c C-x C-j", "org-agenda-clock-goto"),
    ("C-c C-s", "org-agenda-schedule"),
    ("C-c C-d", "org-agenda-deadline"),
    ("C-c C-w", "org-agenda-refile"),
    ("z", "org-agenda-add-note"),
    ("$", "org-agenda-archive"),
    ("I", "org-agenda-clock-in"),
    ("O", "org-agenda-clock-out"),
    ("X", "org-agenda-clock-cancel"),
    ("J", "org-agenda-clock-goto"),
    ("k", "org-agenda-capture"),
    (":", "org-agenda-set-tags"),
    ("e", "org-agenda-set-effort"),
    ("j", "org-agenda-goto-date"),
    (">", "org-agenda-goto-date"),
    (".", "org-agenda-goto-today"),
    ("c", "org-agenda-goto-calendar"),
    ("d", "org-agenda-day-view"),
    ("w", "org-agenda-week-view"),
    ("v m", "org-agenda-month-view"),
    ("v y", "org-agenda-year-view"),
    ("v d", "org-agenda-day-view"),
    ("v w", "org-agenda-week-view"),
    ("v l", "org-agenda-log-mode"),
    ("v a", "org-agenda-archives-mode"),
    ("f", "org-agenda-later"),
    ("b", "org-agenda-earlier"),
    ("F", "org-agenda-follow-mode"),
    ("l", "org-agenda-log-mode"),
    ("E", "org-agenda-entry-text-mode"),
    ("R", "org-agenda-clockreport-mode"),
    ("D", "org-agenda-toggle-diary"),
    ("|", "org-agenda-filter"),
    ("/", "org-agenda-filter-by-tag"),
    ("<", "org-agenda-filter-by-category"),
    ("=", "org-agenda-filter-by-regexp"),
    ("_", "org-agenda-filter-by-effort"),
    ("^", "org-agenda-filter-by-top-headline"),
    ("~", "org-agenda-limit-interactively"),
    ("m", "org-agenda-bulk-mark"),
    ("u", "org-agenda-bulk-unmark"),
    ("U", "org-agenda-bulk-unmark-all"),
    ("*", "org-agenda-bulk-mark-all"),
    ("%", "org-agenda-bulk-mark-regexp"),
    ("B", "org-agenda-bulk-action"),
    ("C-_", "org-agenda-undo"),
    ("C-x C-w", "org-agenda-write"),
    ("C-c C-x <", "org-agenda-set-restriction-lock"),
    ("C-c C-x >", "org-agenda-remove-restriction-lock"),
    ("C-c C-x a", "org-agenda-toggle-archive-tag"),
    ("C-c C-o", "org-agenda-switch-to"),
    ("L", "org-agenda-recenter"),
    ("C-c C-x r", "org-agenda-reset-view"),
    ("C-c C-x b", "org-agenda-tree-to-indirect-buffer"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn registered() -> HashSet<&'static str> {
        let mut set: HashSet<&'static str> = HashSet::new();
        set.extend(
            crate::commands::COMMANDS
                .iter()
                .chain(crate::org_cmds::COMMANDS)
                .map(|cd| cd.name()),
        );
        set.extend(crate::org_table_cmds::COMMANDS.iter().map(|c| c.name()));
        set.extend(crate::org_crypt::COMMANDS.iter().map(|c| c.name()));
        set.extend(crate::org_dblock::COMMANDS.iter().map(|c| c.name()));
        set.extend(crate::org_publish::COMMANDS.iter().map(|c| c.name()));
        set.extend(
            crate::org_flow::COMMANDS
                .iter()
                .chain(COMMANDS)
                .chain(crate::org_extra::COMMANDS)
                .chain(crate::org_export::COMMANDS)
                .chain(crate::org_rich::COMMANDS)
                .chain(crate::few::COMMANDS)
                .map(|cd| cd.name()),
        );
        set
    }

    #[test]
    fn every_org_and_agenda_binding_is_a_registered_command() {
        let reg = registered();
        for (keys, cmd) in crate::keymap::ORG_MODE_BINDINGS
            .iter()
            .chain(AGENDA_BINDINGS)
            .chain(crate::org_extra::COLUMNS_BINDINGS)
            .chain(crate::few::BINDINGS)
        {
            assert!(reg.contains(cmd), "{keys} -> unknown command {cmd}");
        }
    }

    #[test]
    fn no_command_is_registered_twice_with_different_docs() {
        let mut seen = HashSet::new();
        for cd in crate::org_cmds::COMMANDS.iter() {
            assert!(seen.insert(cd.name()), "{} registered twice", cd.name());
        }
        for cd in crate::org_table_cmds::COMMANDS {
            assert!(seen.insert(cd.name()), "{} registered twice", cd.name());
        }
        for cd in crate::org_crypt::COMMANDS {
            assert!(seen.insert(cd.name()), "{} registered twice", cd.name());
        }
        for cd in crate::org_flow::COMMANDS.iter().chain(COMMANDS) {
            assert!(seen.insert(cd.name()), "{} registered twice", cd.name());
        }
    }

    fn app_with(text: &str) -> App {
        let mut app = App::new(forge_core::Editor::default());
        let mut b = Buffer::new("tasks.org");
        b.set_contents(text);
        b.set_major_mode(forge_core::org::MODE);
        let id = app.editor.add_buffer(b);
        app.editor.show_buffer(id);
        app.editor.org_files_loaded = true;
        app.editor.org_files.clear();
        app
    }

    fn agenda_text(app: &mut App) -> String {
        let id = app.editor.find_buffer(BUFFER_NAME).expect("agenda buffer");
        app.editor.state.get_buffer(id).unwrap().get_contents()
    }

    #[test]
    fn agenda_shows_scheduled_and_changes_state_from_the_agenda() {
        let (today, _) = now();
        let ts = Timestamp::new(true, today, None).text();
        let mut app = app_with(&format!(
            "* TODO Write report\nSCHEDULED: {ts}\n* TODO Other\n"
        ));
        assert!(crate::commands::execute(&mut app, "org-agenda-list"));
        let text = agenda_text(&mut app);
        assert!(text.contains("Write report"), "{text}");
        assert!(crate::commands::execute(&mut app, "org-agenda-todo"));
        let id = app.editor.find_buffer("tasks.org").unwrap();
        assert!(
            app.editor
                .state
                .get_buffer(id)
                .unwrap()
                .get_contents()
                .contains("* DONE Write report")
        );
        // The global TODO list shows what is left.
        crate::commands::execute(&mut app, "org-todo-list");
        assert!(agenda_text(&mut app).contains("Other"));
    }

    #[test]
    fn filters_and_marks_work_in_the_todo_list() {
        let mut app = app_with("* TODO One :a:\n* TODO Two :b:\n");
        crate::commands::execute(&mut app, "org-todo-list");
        assert!(answer(&mut app, &Ask::AgendaFilterTag, "+a"));
        let t = agenda_text(&mut app);
        assert!(t.contains("One") && !t.contains("Two"), "{t}");
        assert!(answer(&mut app, &Ask::AgendaFilterTag, ""));
        assert!(agenda_text(&mut app).contains("Two"));
        crate::commands::execute(&mut app, "org-agenda-bulk-mark-all");
        assert_eq!(app.org.agenda.as_ref().unwrap().marks.len(), 2);
        assert!(agenda_text(&mut app).contains(">"));
    }

    #[test]
    fn tables_recalculate_with_c_c_c_c() {
        let mut app = app_with("| 1 | 2 | |\n#+TBLFM: $3=$1+$2\n");
        app.buf().set_point(Pos::new(0, 0));
        assert!(crate::commands::execute(&mut app, "org-ctrl-c-ctrl-c"));
        assert!(
            app.buf_ref().line(0).contains('3'),
            "{}",
            app.buf_ref().line(0)
        );
    }

    #[test]
    fn agenda_keys_bind_without_panicking() {
        let mut km = forge_core::keymap::Keymaps::new();
        crate::keymap::install_defaults(&mut km);
    }
}
