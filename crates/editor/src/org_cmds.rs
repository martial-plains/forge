// SPDX-License-Identifier: MIT OR Apache-2.0
//! Org commands beyond the outline (Phase 11, stages 2 to 4): the state
//! they share, the questions they ask, and the semantics commands (tags,
//! properties, timestamps, planning, links, blocks). Tables are in
//! `org_table_cmds.rs`, capture/refile/archive/clock/timers in
//! `org_flow.rs` and the agenda in `org_agenda_ui.rs`.

use crate::app::App;
use crate::minibuffer::{Choice, Purpose};
use forge_core::org::{self, keywords};
use forge_core::org_ext::{LinkTarget, resolve_link};
use forge_core::org_time::{Date, Hm, Timestamp, now_local, parse_date_input};
use forge_core::{BufferId, Pos};
use std::path::PathBuf;

/// Everything Org keeps between commands.
#[derive(Default)]
pub struct OrgState {
    pub clock: Option<crate::org_flow::RunningClock>,
    pub last_clock: Option<(BufferId, String)>,
    /// Headings clocked recently, newest first (`org-clock-history`).
    pub clock_history: Vec<(BufferId, String)>,
    /// Buffers already looked at for a clock left running.
    pub clock_checked: Vec<BufferId>,
    /// The longest idle stretch seen while clocking (seconds).
    pub idle_secs: u64,
    /// A prefix argument was given to `org-publish`.
    pub publish_force: bool,
    pub capture: Option<crate::org_flow::CaptureSession>,
    pub capture_pending: Option<crate::org_flow::PendingCapture>,
    pub agenda: Option<crate::org_agenda_ui::AgendaState>,
    /// `org-store-link`: (target, description), newest last.
    pub stored_links: Vec<(String, String)>,
    /// `org-open-at-point` leaves marks to come back to.
    pub mark_ring: Vec<(BufferId, Pos)>,
    pub timer: crate::org_flow::Timer,
    /// Fields copied or cut by the table region commands.
    pub table_clip: Option<String>,
    pub formula_debug: bool,
    pub coordinate_overlays: bool,
    pub header_line: bool,
    pub auto_clockout: bool,
    /// forge.opt.org_directory, org_capture_templates and friends.
    pub directory: Option<PathBuf>,
    /// forge.opt.org_crypt_key (None: unset) and org_crypt_tag_matcher.
    pub crypt_key: Option<String>,
    pub crypt_tag_matcher: Option<String>,
    pub columns: Option<crate::org_extra::ColumnsState>,
    pub capture_templates: Option<Vec<forge_core::org_work::CaptureTemplate>>,
    pub export: crate::org_export::State,
    pub rich: crate::org_rich::State,
    /// `org-stuck-projects`, as set by the dispatcher's `!`.
    pub stuck_projects: Option<(Vec<String>, Vec<String>)>,
    /// `org-agenda-custom-commands`: (key, view spec), set by `C`.
    pub custom_agenda: Vec<(char, String)>,
}

/// What a question is about. The `Purpose` carries it to its answer.
#[derive(Debug, Clone)]
pub enum Ask {
    // Tags, properties, dates.
    SetTags,
    ToggleTag,
    TagsSparse,
    TagsView,
    PropName,
    PropValue(String),
    PropDelete,
    PropAction,
    Timestamp {
        active: bool,
    },
    Schedule,
    Deadline,
    CheckBefore,
    CheckAfter,
    Effort,
    // Links and blocks.
    LinkTarget,
    LinkDesc {
        target: String,
    },
    LinkGlobal,
    OpenFile,
    Template,
    // Tables.
    TableCreate,
    TableConvertSep,
    TableSort,
    TableGotoLine,
    TableExportFormat,
    TableExportFile {
        format: String,
        text: String,
    },
    TableImportFile,
    TableFormula {
        column: bool,
    },
    TableEditFormulas,
    TableWrapWidth,
    // Workflow.
    Refile {
        copy: bool,
    },
    CaptureKey,
    CapturePrompt,
    CaptureRefile,
    ArchiveLocation,
    ClockEffort,
    ClockSelect,
    ClockIdle(u64),
    TimerSet,
    // The agenda.
    AgendaKey,
    AgendaTodoKeyword,
    AgendaMatch {
        todo_only: bool,
    },
    AgendaSearch,
    AgendaSearchTodo,
    AgendaOccur,
    AgendaStuckConfig,
    AgendaCustomConfig,
    AgendaGotoDate,
    AgendaFilter,
    AgendaFilterTag,
    AgendaFilterCategory,
    AgendaFilterRegexp,
    AgendaFilterEffort,
    AgendaNote,
    AgendaPriority,
    /// `org-priority` on the heading at point.
    Priority,
    /// `org-add-note` on the heading at point.
    Note,
    /// Fast selection of a TODO state: the answer starts with its key.
    TodoState,
    /// The closing note asked for by `lognotedone`, for the heading at
    /// the given line.
    DoneNote(usize),
    AgendaBulkRegexp,
    AgendaBulkAction,
    AgendaWrite,
    FewGo,
    /// term: which shell to run.
    TermShell,
    /// icons: describe, customize (name, then value), platform.
    IconDescribe,
    IconCustomize,
    IconValue(String),
    IconPlatform,
    /// canvas-export: the canvas, then where to write it.
    CanvasExport(String),
    WebGo,
    WebNew,
    WebFind,
    WebInsert,
    WebHistory,
    WebPage,
    WebShow(String),
    FewSearch,
    FewHistory,
    FewBookmark,
    FewUnbookmark,
    FewFind,
    FewLink,
    FewToc,
    FewField,
    FewDownload,
    FewFile,
    FewCharset,
    ColumnsEdit,
    ColumnsAllowed,
    ColumnsNew,
    AttachDispatch,
    AttachFile {
        mv: bool,
    },
    AttachBuffer,
    AttachNew,
    AttachOpen,
    AgendaFile,
    AgendaTodo,
    ExportDispatch,
    FootnoteAction,
    DblockInsert,
    SearchEngine(u8),
    PublishProject,
    Emphasize,
    BabelConfirm,
    GotoNamed,
    GotoNamedResult,
    HeaderArgKey,
    HeaderArgValue,
    TangleFile,
    LobIngest,
    CiteInsert,
    EntityUser,
    CdlatexSymbol,
    CdlatexEnvironment,
    BeamerEnv,
    BeamerCol {
        env: String,
    },
    /// org-odt-convert: the ODT file to read.
    OdtFile,
    /// org-odt-convert: the format to write.
    OdtFormat {
        file: String,
    },
    /// Org Crypt: what the passphrase prompt continues.
    CryptPassphrase(crate::org_crypt::CryptOp),
}

/// Beamer environments offered by org-beamer-select-environment (the
/// names the LaTeX/Beamer exporter understands).
const BEAMER_ENVIRONMENTS: &[&str] = &[
    "block",
    "alertblock",
    "exampleblock",
    "columns",
    "column",
    "theorem",
    "definition",
    "example",
    "proof",
    "lemma",
    "ignoreheading",
    "appendix",
    "note",
    "frame",
    "fullframe",
];

/// Names and documentation of the commands in this file.
pub const COMMANDS: &[forge_docs::CommandDoc] = &[
    forge_docs::CommandDoc::line(
        "org-set-tags-command",
        "Org: set the tags of the current heading (empty removes them).",
    ),
    forge_docs::CommandDoc::line(
        "org-set-tags-to",
        "Org: set the tags of the current heading, replacing the old ones.",
    ),
    forge_docs::CommandDoc::line(
        "org-complete-tags",
        "Org: set the tags of the current heading, offering the tags in use.",
    ),
    forge_docs::CommandDoc::line(
        "org-toggle-tag",
        "Org: add the tag to the current heading, or remove it if it has it.",
    ),
    forge_docs::CommandDoc::line(
        "org-tags-sparse-tree",
        "Org: show only the headings matching a tags/properties match string.",
    ),
    forge_docs::CommandDoc::line(
        "org-toggle-ordered-property",
        "Org: make the children's TODOs ORDERED (each blocks on the one before), or stop.",
    ),
    forge_docs::CommandDoc::line(
        "org-set-property",
        "Org: set a property of the current heading.",
    ),
    forge_docs::CommandDoc::line(
        "org-delete-property",
        "Org: delete a property of the current heading.",
    ),
    forge_docs::CommandDoc::line(
        "org-insert-property-drawer",
        "Org: add an empty property drawer to the current heading.",
    ),
    forge_docs::CommandDoc::line(
        "org-property-next-allowed-value",
        "Org: set the property at point to its next allowed value.",
    ),
    forge_docs::CommandDoc::line(
        "org-property-previous-allowed-value",
        "Org: set the property at point to its previous allowed value.",
    ),
    forge_docs::CommandDoc::new(
        "org-property-action",
        "Org: do something with the properties of this heading (set, delete, next,",
    )
    .body("previous, compute)."),
    forge_docs::CommandDoc::line(
        "org-compute-property-at-point",
        "Org: set the property at point to the sum of its children's.",
    ),
    forge_docs::CommandDoc::line(
        "org-set-effort",
        "Org: set the Effort property of the current heading.",
    ),
    forge_docs::CommandDoc::new(
        "org-time-stamp",
        "Org: insert an active timestamp, asking for the date.",
    )
    .body("With a prefix argument, with the time."),
    forge_docs::CommandDoc::new(
        "org-timestamp",
        "Org: insert an active timestamp, asking for the date.",
    )
    .body("With a prefix argument, with the time."),
    forge_docs::CommandDoc::line(
        "org-time-stamp-inactive",
        "Org: insert an inactive timestamp, asking for the date.",
    ),
    forge_docs::CommandDoc::line(
        "org-timestamp-inactive",
        "Org: insert an inactive timestamp, asking for the date.",
    ),
    forge_docs::CommandDoc::new(
        "org-timestamp-up",
        "Org: increase the part of the timestamp at point (with an argument,",
    )
    .body("by that much)."),
    forge_docs::CommandDoc::new(
        "org-timestamp-down",
        "Org: decrease the part of the timestamp at point (with an argument,",
    )
    .body("by that much)."),
    forge_docs::CommandDoc::line(
        "org-timestamp-up-day",
        "Org: move the timestamp at point one day later.",
    ),
    forge_docs::CommandDoc::line(
        "org-timestamp-down-day",
        "Org: move the timestamp at point one day earlier.",
    ),
    forge_docs::CommandDoc::new(
        "org-shiftup",
        "Org: on a timestamp, increase it; on a heading,",
    )
    .body("raise its priority; otherwise move up."),
    forge_docs::CommandDoc::new(
        "org-shiftdown",
        "Org: on a timestamp, decrease it; on a heading,",
    )
    .body("lower its priority; otherwise move down."),
    forge_docs::CommandDoc::new(
        "org-shiftleft",
        "Org: on a timestamp, one day earlier; on a heading,",
    )
    .body("the previous TODO state; otherwise move left."),
    forge_docs::CommandDoc::new(
        "org-shiftright",
        "Org: on a timestamp, one day later; on a heading,",
    )
    .body("the next TODO state; otherwise move right."),
    forge_docs::CommandDoc::new(
        "org-schedule",
        "Org: schedule the current heading, asking for the date (with a prefix argument,",
    )
    .body("remove it)."),
    forge_docs::CommandDoc::new(
        "org-deadline",
        "Org: set a deadline on the current heading,",
    )
    .body("asking for the date (with a prefix argument, remove it)."),
    forge_docs::CommandDoc::line(
        "org-check-deadlines",
        "Org: show the deadlines that are due or near, in a sparse tree.",
    ),
    forge_docs::CommandDoc::line(
        "org-check-before-date",
        "Org: show the entries with an active timestamp before a date.",
    ),
    forge_docs::CommandDoc::line(
        "org-check-after-date",
        "Org: show the entries with an active timestamp after a date.",
    ),
    forge_docs::CommandDoc::new(
        "org-open-at-point",
        "Org: follow the link at point (or visit what the timestamp or checkbox there",
    )
    .body("means)."),
    forge_docs::CommandDoc::line(
        "org-mouse-open",
        "Org: follow the link under a click; do nothing when the click is not on one.",
    ),
    forge_docs::CommandDoc::line(
        "org-open-at-point-global",
        "Org: follow the link at point, in any buffer.",
    ),
    forge_docs::CommandDoc::line(
        "org-open-file",
        "Org: open a file by name, going to a line or search with FILE::SEARCH.",
    ),
    forge_docs::CommandDoc::new(
        "org-insert-link",
        "Org: insert a link, or edit the one at point.",
    )
    .body("Offers the links stored with org-store-link."),
    forge_docs::CommandDoc::line(
        "org-insert-link-global",
        "Org: insert a link in any buffer.",
    ),
    forge_docs::CommandDoc::line(
        "org-store-link",
        "Org: remember a link to the current heading or line, for org-insert-link.",
    ),
    forge_docs::CommandDoc::line(
        "org-insert-all-links",
        "Org: insert all the links stored with org-store-link, one per line.",
    ),
    forge_docs::CommandDoc::line("org-next-link", "Org: move to the next link."),
    forge_docs::CommandDoc::line("org-previous-link", "Org: move to the previous link."),
    forge_docs::CommandDoc::new(
        "org-mark-ring-goto",
        "Org: go back to where org-open-at-point was last used (with an argument,",
    )
    .body("further back)."),
    forge_docs::CommandDoc::line(
        "org-remove-link",
        "Org: replace the link at point by its description.",
    ),
    forge_docs::CommandDoc::line(
        "org-toggle-checkbox",
        "Org: toggle the checkbox of the list item at point.",
    ),
    forge_docs::CommandDoc::line(
        "org-update-statistics-cookies",
        "Org: recount every [n/m] and [n%] cookie in the buffer.",
    ),
    forge_docs::CommandDoc::new(
        "org-insert-structure-template",
        "Org: wrap the region (or open a new block) in #+BEGIN_x ...",
    )
    .body("#+END_x. x is s, e, q, v, c, C, l, h, a."),
    forge_docs::CommandDoc::line(
        "org-priority",
        "Org: set the heading's priority (a letter, SPC to remove, + or -).",
    ),
    forge_docs::CommandDoc::line(
        "org-add-note",
        "Org: add a timestamped note to the heading's LOGBOOK.",
    ),
    forge_docs::CommandDoc::new(
        "org-ctrl-c-minus",
        "Org: in a table insert a rule; on a list item cycle the bullet;",
    )
    .body("on other text make the line an item."),
    forge_docs::CommandDoc::line(
        "org-evaluate-time-range",
        "Org: on a time range, insert the time between its ends (=> H:MM).",
    ),
    forge_docs::CommandDoc::line(
        "org-shiftmetaup",
        "Org: in a table delete the row; otherwise drag the line up.",
    ),
    forge_docs::CommandDoc::line(
        "org-shiftmetadown",
        "Org: in a table insert a row; otherwise drag the line down.",
    ),
    forge_docs::CommandDoc::line(
        "org-return",
        "Org: in a table move to the next row; otherwise insert a newline.",
    ),
    forge_docs::CommandDoc::line(
        "org-toggle-comment",
        "Org: comment or uncomment the current heading (a COMMENT keyword).",
    ),
    forge_docs::CommandDoc::new(
        "org-convert-to-odd-levels",
        "Org: rewrite every heading with odd levels (1, 3, 5, ...),",
    )
    .body("the double-star layout."),
    forge_docs::CommandDoc::line(
        "org-convert-to-oddeven-levels",
        "Org: rewrite every heading with consecutive levels (1, 2, 3, ...).",
    ),
    forge_docs::CommandDoc::line(
        "org-entities-user",
        "Org: define an extra entity: name [latex] [html] [ascii] [utf8].",
    ),
    forge_docs::CommandDoc::new(
        "org-beamer-select-environment",
        "Org: set the current heading's Beamer environment (BEAMER_env),",
    )
    .body("for Beamer export."),
    forge_docs::CommandDoc::line(
        "org-version",
        "Org: show the version of Forge's Org support.",
    ),
];

pub fn now() -> (Date, Hm) {
    now_local()
}

/// Ask with some text already in the minibuffer.
pub fn ask_with(app: &mut App, prompt: &str, ask: Ask, initial: &str) {
    app.prompt(prompt, Purpose::OrgAsk(ask));
    if let Some(m) = app.minibuffer.as_mut() {
        m.input = initial.to_string();
    }
}

pub fn ask(app: &mut App, prompt: &str, ask: Ask) {
    app.prompt(prompt, Purpose::OrgAsk(ask));
}

/// Ask for one of `candidates` (any text is accepted too).
pub fn choose(app: &mut App, prompt: &str, ask: Ask, candidates: Vec<String>) {
    app.prompt(
        prompt,
        Purpose::Choose {
            what: Choice::Org(ask),
            candidates,
        },
    );
}

/// Run an Org command from this module's family. False if `cmd` isn't one.
pub fn command(app: &mut App, cmd: &str) -> bool {
    stage2(app, cmd)
        || crate::org_crypt::command(app, cmd)
        || crate::org_dblock::command(app, cmd)
        || crate::org_publish::command(app, cmd)
        || crate::search::command(app, cmd)
        || crate::org_table_cmds::command(app, cmd)
        || crate::org_flow::command(app, cmd)
        || crate::org_agenda_ui::command(app, cmd)
        || crate::org_extra::command(app, cmd)
        || crate::org_export::command(app, cmd)
        || crate::org_rich::command(app, cmd)
}

/// The prompt answered in `Purpose::OrgAsk`.
pub fn answered(app: &mut App, ask: Ask, input: &str) {
    let t = input.trim();
    let handled = stage2_answer(app, &ask, input)
        || crate::org_crypt::answer(app, &ask, input)
        || crate::org_dblock::answer(app, &ask, input)
        || crate::org_publish::answer(app, &ask, input)
        || crate::org_table_cmds::answer(app, &ask, input)
        || crate::org_flow::answer(app, &ask, input)
        || crate::org_agenda_ui::answer(app, &ask, input)
        || crate::org_extra::answer(app, &ask, input)
        || crate::org_export::answer(app, &ask, input)
        || crate::org_rich::answer(app, &ask, input)
        || crate::few::answer(app, &ask, input)
        || crate::term::answer(app, &ask, input)
        || crate::icon::answer(app, &ask, input)
        || crate::canvas::answer(app, &ask, input)
        || crate::search::answer(app, &ask, input)
        || crate::webkit::answer(app, &ask, input);
    let _ = (handled, t);
}

/// The prompt answered in `Choice::Org`: `input` is what was typed or
/// picked, `candidates` the choices.
pub fn chosen(app: &mut App, ask: Ask, input: &str, candidates: &[String]) {
    let _ = candidates;
    answered(app, ask, input);
}

/// Words in a tag answer: `a:b`, `a b`, `a,b`.
fn split_tags(s: &str) -> Vec<String> {
    s.split(|c: char| c == ':' || c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

fn heading_here(app: &App) -> Option<usize> {
    let b = app.buf_ref();
    b.org_heading_at(b.point().line)
}

fn no_heading(app: &mut App) {
    app.error("Not in an outline entry");
}

/// The Org buffer commands that act on the text of the current buffer.
fn stage2(app: &mut App, cmd: &str) -> bool {
    let p = app.buf_ref().point();
    let line = p.line;
    let back = app.has_arg();
    match cmd {
        "org-priority" => match heading_here(app) {
            Some(_) => ask(
                app,
                "Priority (A-C, SPC to remove, + or - to change): ",
                Ask::Priority,
            ),
            None => no_heading(app),
        },
        "org-add-note" => match heading_here(app) {
            Some(_) => ask(app, "Note: ", Ask::Note),
            None => no_heading(app),
        },
        "org-ctrl-c-minus" => {
            if forge_core::org_table::is_table_line(&app.buf_ref().line(line)) {
                return crate::commands::execute(app, "org-table-insert-hline");
            }
            if app.buf().org_cycle_bullet(line, back) {
                return true;
            }
            // Plain text becomes an item; an item stays as it is.
            let text = app.buf_ref().line(line);
            if !text.trim().is_empty() && org::heading_level(&text).is_none() {
                let indent = text.chars().take_while(|c| c.is_whitespace()).count();
                app.buf()
                    .replace_range(Pos::new(line, indent), Pos::new(line, indent), "- ");
            }
        }
        "org-evaluate-time-range" => {
            let text = app.buf_ref().line(line);
            match time_range_result(&text) {
                Some((at, shown)) => {
                    let end = text.chars().count();
                    app.buf()
                        .replace_range(Pos::new(line, at), Pos::new(line, end), &shown);
                    app.msg(shown.trim().to_string());
                }
                None => app.error("Not at a time range"),
            }
        }
        "org-shiftmetaup" | "org-shiftmetadown" => {
            let down = cmd.ends_with("down");
            let n = app.buf_ref().line_count();
            if (down && line + 1 >= n) || (!down && line == 0) {
                return true;
            }
            let other = if down { line + 1 } else { line - 1 };
            let (a, b) = (line.min(other), line.max(other));
            let (ta, tb) = (app.buf_ref().line(a), app.buf_ref().line(b));
            app.buf().replace_range(
                Pos::new(a, 0),
                Pos::new(b, tb.chars().count()),
                &format!("{tb}\n{ta}"),
            );
            let col = p.col.min(app.buf_ref().line_len(other));
            app.buf().set_point(Pos::new(other, col));
        }
        // ------------------------------------------------------- tags
        "org-set-tags-command" | "org-set-tags-to" | "org-complete-tags" => match heading_here(app)
        {
            Some(h) => {
                let tags = app.buf_ref().org_heading_tags(h).join(":");
                let initial = if tags.is_empty() {
                    String::new()
                } else {
                    format!(":{tags}:")
                };
                ask_with(app, "Tags: ", Ask::SetTags, &initial);
            }
            None => no_heading(app),
        },
        "org-toggle-tag" => {
            if heading_here(app).is_none() {
                no_heading(app);
            } else {
                let all = app.buf_ref().org_all_tags();
                choose(app, "Toggle tag: ", Ask::ToggleTag, all);
            }
        }
        "org-tags-sparse-tree" => ask(app, "Match (tags/properties/todo): ", Ask::TagsSparse),
        "org-toggle-ordered-property" => match heading_here(app) {
            Some(h) => {
                let on = app.buf().org_toggle_ordered(h);
                app.msg(
                    if on {
                        "ORDERED property set"
                    } else {
                        "ORDERED property removed"
                    }
                    .to_string(),
                );
            }
            None => no_heading(app),
        },
        // ------------------------------------------------ properties
        "org-set-property" => match heading_here(app) {
            Some(h) => {
                let mut names: Vec<String> = [
                    "Effort",
                    "CATEGORY",
                    "ORDERED",
                    "ARCHIVE",
                    "CUSTOM_ID",
                    "ID",
                    "COLUMNS",
                    "STYLE",
                    "LOGGING",
                    "VISIBILITY",
                ]
                .iter()
                .map(|s| s.to_string())
                .collect();
                for l in 0..app.buf_ref().line_count() {
                    if app.buf_ref().org_level(l).is_some() {
                        for (n, _) in app.buf_ref().org_properties(l) {
                            if !names.contains(&n) {
                                names.push(n);
                            }
                        }
                    }
                }
                let _ = h;
                choose(app, "Property: ", Ask::PropName, names);
            }
            None => no_heading(app),
        },
        "org-delete-property" => match heading_here(app) {
            Some(h) => {
                let names: Vec<String> = app
                    .buf_ref()
                    .org_properties(h)
                    .into_iter()
                    .map(|(n, _)| n)
                    .collect();
                if names.is_empty() {
                    app.msg("No properties here".to_string());
                } else {
                    choose(app, "Delete property: ", Ask::PropDelete, names);
                }
            }
            None => no_heading(app),
        },
        "org-insert-property-drawer" => match heading_here(app) {
            Some(h) => {
                let l = app.buf().org_insert_property_drawer(h);
                app.buf().set_point(Pos::new(l + 1, 0));
            }
            None => no_heading(app),
        },
        "org-property-next-allowed-value" | "org-property-previous-allowed-value" => {
            let Some(h) = heading_here(app) else {
                return no_heading_true(app);
            };
            let name = property_name_at(&app.buf_ref().line(line));
            match name {
                Some(n) => match app.buf().org_property_cycle(h, &n, cmd.contains("next")) {
                    Ok(Some(v)) => app.msg(format!("{n}: {v}")),
                    Ok(None) => app.msg(format!("{n}: unset")),
                    Err(e) => app.error(e),
                },
                None => app.error("Point is not on a property line"),
            }
        }
        "org-property-action" => {
            if heading_here(app).is_none() {
                return no_heading_true(app);
            }
            choose(
                app,
                "Property: ",
                Ask::PropAction,
                [
                    "s: set",
                    "d: delete",
                    "n: next allowed value",
                    "p: previous allowed value",
                    "c: compute",
                ]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            );
        }
        "org-compute-property-at-point" => match app.buf().org_compute_property(line) {
            Ok(v) => app.msg(format!("Sum: {v}")),
            Err(e) => app.error(e),
        },
        "org-set-effort" => match heading_here(app) {
            Some(h) => {
                let cur = app
                    .buf_ref()
                    .org_get_property(h, "Effort", false)
                    .unwrap_or_default();
                ask_with(app, "Effort (e.g. 1:30 or 2): ", Ask::Effort, &cur);
            }
            None => no_heading(app),
        },
        // ------------------------------------------------ timestamps
        "org-time-stamp"
        | "org-timestamp"
        | "org-time-stamp-inactive"
        | "org-timestamp-inactive" => {
            let active = !cmd.contains("inactive");
            let _ = back;
            ask(
                app,
                if active {
                    "Date (active): "
                } else {
                    "Date (inactive): "
                },
                Ask::Timestamp { active },
            );
        }
        "org-timestamp-up"
        | "org-timestamp-down"
        | "org-timestamp-up-day"
        | "org-timestamp-down-day" => {
            let n = if back { app.count() } else { 1 };
            let delta = if cmd.contains("up") { n } else { -n };
            if !app
                .buf()
                .org_timestamp_shift(p, delta, cmd.ends_with("-day"))
            {
                app.error("Not on a timestamp");
            }
        }
        "org-shiftup" | "org-shiftdown" => {
            let up = cmd == "org-shiftup";
            if app
                .buf()
                .org_timestamp_shift(p, if up { 1 } else { -1 }, false)
            {
            } else if org::heading_level(&app.buf_ref().line(line)).is_none()
                && let Some(to) = app.buf_ref().org_neighbour_item(line, !up)
            {
                // On an item, S-up and S-down visit the neighbouring items.
                let col = p.col.min(app.buf_ref().line_len(to));
                app.buf().set_point(Pos::new(to, col));
            } else if app.buf_ref().org_level(line).is_some() {
                use forge_core::org_work::PriorityAction;
                match app.buf().org_set_priority(
                    line,
                    if up {
                        PriorityAction::Up
                    } else {
                        PriorityAction::Down
                    },
                ) {
                    Ok(Some(c)) => app.msg(format!("Priority [#{c}]")),
                    Ok(None) => app.msg("Priority removed".to_string()),
                    Err(e) => app.error(e),
                }
                keep_point(app, p);
            } else {
                return crate::commands::execute(
                    app,
                    if up { "previous-line" } else { "next-line" },
                );
            }
        }
        "org-shiftleft" | "org-shiftright" => {
            let right = cmd == "org-shiftright";
            if app
                .buf()
                .org_timestamp_shift(p, if right { 1 } else { -1 }, true)
                || org::heading_level(&app.buf_ref().line(line)).is_none()
                    && app.buf().org_cycle_bullet(line, !right)
            {
            } else if app.buf_ref().org_level(line).is_some() {
                change_todo(app, line, right, None);
            } else {
                return crate::commands::execute(
                    app,
                    if right {
                        "forward-char"
                    } else {
                        "backward-char"
                    },
                );
            }
        }
        "org-schedule" | "org-deadline" => {
            let Some(h) = heading_here(app) else {
                return no_heading_true(app);
            };
            let kind = if cmd == "org-schedule" {
                "SCHEDULED"
            } else {
                "DEADLINE"
            };
            if back {
                app.buf().org_set_planning(h, kind, None);
                app.msg(format!("{kind} removed"));
            } else {
                let cur = app.buf_ref().org_planning(h);
                let ts = if cmd == "org-schedule" {
                    cur.scheduled
                } else {
                    cur.deadline
                };
                let initial = ts.map(|t| t.date.iso()).unwrap_or_default();
                let prompt = if cmd == "org-schedule" {
                    "Schedule to (date): "
                } else {
                    "Deadline (date): "
                };
                ask_with(
                    app,
                    prompt,
                    if cmd == "org-schedule" {
                        Ask::Schedule
                    } else {
                        Ask::Deadline
                    },
                    &initial,
                );
            }
        }
        "org-check-deadlines" => {
            let (today, _) = now();
            let found = app.buf_ref().org_check_deadlines(today, 14);
            if found.is_empty() {
                app.msg("No deadlines due or near".to_string());
            } else {
                let mut matched = vec![false; app.buf_ref().line_count()];
                for (h, _, _) in &found {
                    matched[*h] = true;
                }
                app.buf().org_apply_sparse(&matched);
                app.msg(format!("{} deadline(s) due or near", found.len()));
            }
        }
        "org-check-before-date" => ask(app, "Check before date: ", Ask::CheckBefore),
        "org-check-after-date" => ask(app, "Check after date: ", Ask::CheckAfter),
        // -------------------------------------------------- links
        "org-open-at-point" | "org-open-at-point-global" => open_at_point(app),
        "org-mouse-open" => {
            // A click already moved point. Only follow when it landed on a
            // link; clicking ordinary Org text should just move point.
            let p = app.buf_ref().point();
            if app.buf_ref().org_link_at_point(p).is_some()
                || app.buf_ref().org_timestamp_at_point(p).is_some()
            {
                open_at_point(app);
            }
        }
        "org-open-file" => ask(app, "Open file (FILE or FILE::SEARCH): ", Ask::OpenFile),
        "org-store-link" => {
            let file = app.buf_ref().path().map(|p| p.display().to_string());
            let (target, desc) = app.buf_ref().org_store_link(line, file.as_deref());
            app.msg(format!("Stored: {target}"));
            app.org.stored_links.retain(|(t, _)| *t != target);
            app.org.stored_links.push((target, desc));
        }
        "org-insert-link" | "org-insert-link-global" => {
            let existing = app.buf_ref().org_link_at_point(p);
            let mut cands: Vec<String> = app
                .org
                .stored_links
                .iter()
                .rev()
                .map(|(t, _)| t.clone())
                .collect();
            for pre in ["https://", "file:", "mailto:", "id:"] {
                cands.push(pre.to_string());
            }
            app.prompt(
                "Link: ",
                Purpose::Choose {
                    what: Choice::Org(Ask::LinkTarget),
                    candidates: cands,
                },
            );
            if let (Some(l), Some(m)) = (existing, app.minibuffer.as_mut()) {
                m.input = l.target;
            }
        }
        "org-insert-all-links" => {
            if app.org.stored_links.is_empty() {
                app.msg("No stored links".to_string());
            } else {
                let text = app
                    .org
                    .stored_links
                    .iter()
                    .map(|(t, d)| {
                        if d.is_empty() {
                            format!("- [[{t}]]")
                        } else {
                            format!("- [[{t}][{d}]]")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let at = app.buf_ref().point();
                app.buf().replace_range(at, at, &text);
            }
        }
        "org-next-link" | "org-previous-link" => {
            match app.buf_ref().org_next_link(p, cmd == "org-next-link") {
                Some(to) => app.buf().set_point(to),
                None => app.msg("No further link found".to_string()),
            }
        }
        "org-mark-ring-goto" => {
            let n = if back { app.count().max(1) as usize } else { 1 };
            let len = app.org.mark_ring.len();
            if len < n {
                app.msg("Mark ring is empty".to_string());
            } else {
                let (id, pos) = app.org.mark_ring[len - n];
                app.org.mark_ring.truncate(len - n);
                if app.editor.state.buffers.contains_key(&id) {
                    app.switch_to(id);
                    app.buf().set_point(pos);
                }
            }
        }
        "org-remove-link" => {
            if !app.buf().org_remove_link(p) {
                app.error("Not on a link");
            }
        }
        "org-toggle-checkbox" => {
            if !app.buf().org_toggle_checkbox(line) {
                app.msg("Statistics updated".to_string());
            }
        }
        "org-update-statistics-cookies" => {
            let n = app.buf().org_update_cookies();
            app.msg(format!("Updated {n} cookie(s)"));
        }
        // ------------------------------------------------- blocks
        "org-insert-structure-template" => {
            let names = [
                "s: src",
                "e: example",
                "q: quote",
                "v: verse",
                "c: center",
                "C: comment",
                "l: export latex",
                "h: export html",
                "a: export ascii",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
            choose(app, "Template key: ", Ask::Template, names);
        }
        "org-toggle-comment" => match heading_here(app) {
            Some(h) => toggle_comment(app, h),
            None => no_heading(app),
        },
        "org-convert-to-odd-levels" => {
            let n = app.buf().org_convert_to_odd_levels();
            app.msg(format!("Converted {n} headings to odd levels"));
        }
        "org-convert-to-oddeven-levels" => {
            let n = app.buf().org_convert_to_oddeven_levels();
            app.msg(format!("Converted {n} headings to consecutive levels"));
        }
        "org-entities-user" => {
            ask(
                app,
                "Entity (name [latex] [html] [ascii] [utf8]): ",
                Ask::EntityUser,
            );
        }
        "org-beamer-select-environment" => match heading_here(app) {
            Some(_) => choose(
                app,
                "Beamer environment (empty removes): ",
                Ask::BeamerEnv,
                BEAMER_ENVIRONMENTS.iter().map(|e| e.to_string()).collect(),
            ),
            None => no_heading(app),
        },
        "org-version" => {
            app.msg(format!(
                "Forge Org support (forge-core {})",
                forge_core::VERSION
            ));
        }
        "org-return" => {
            if forge_core::org_table::is_table_line(&app.buf_ref().line(line)) {
                crate::org_table_cmds::next_row(app);
            } else {
                // RET in an item continues under its text, like Forge's
                // electric indent does in Org.
                let above = app.buf_ref().line(line);
                let ok = crate::commands::execute(app, "newline");
                let indent = match org::list_item(&above) {
                    Some((i, bullet, _)) if org::heading_level(&above).is_none() => {
                        Some(i + bullet.chars().count() + 1)
                    }
                    _ if above.starts_with(' ') && !above.trim().is_empty() => {
                        Some(above.chars().take_while(|c| *c == ' ').count())
                    }
                    _ => None,
                };
                if let Some(n) = indent {
                    let at = app.buf_ref().point();
                    if at.line == line + 1 && at.col == 0 {
                        app.buf().replace_range(at, at, &" ".repeat(n));
                    }
                }
                return ok;
            }
        }
        _ => return false,
    }
    true
}

/// Change the TODO state of the heading at `line`: the next state in the
/// sequence, or `explicit`. Point stays put, and `lognotedone` asks for a
/// closing note.
pub fn change_todo(app: &mut App, line: usize, forward: bool, explicit: Option<Option<String>>) {
    let (d, n) = now();
    let at = app.buf_ref().point();
    let kw = keywords(app.buf_ref());
    let h = app.buf_ref().org_heading_at(line);
    let was =
        h.and_then(|h| org::parse_heading(&app.buf_ref().line(h), &kw).and_then(|hd| hd.keyword));
    let was_clocked = crate::org_flow::is_running_at(app, line);
    let result = app.buf().org_todo_logged_to(line, forward, explicit, d, n);
    keep_point(app, at);
    match result {
        Err(e) => app.msg(e),
        Ok(Some(new)) => {
            crate::org_flow::after_todo(app, was_clocked, Some(&new));
            let finished = kw.is_done(&new) && !was.as_deref().is_some_and(|w| kw.is_done(w));
            let repeated = h.is_some_and(|h| {
                org::parse_heading(&app.buf_ref().line(h), &keywords(app.buf_ref()))
                    .and_then(|hd| hd.keyword)
                    .is_some_and(|k| !kw.is_done(&k))
            });
            if finished && !repeated && org::log_options(app.buf_ref()).note {
                ask(
                    app,
                    &format!("Closing note for {new}: "),
                    Ask::DoneNote(line),
                );
            }
        }
        Ok(None) => {}
    }
}

/// `C-c C-c`: do what makes sense where point is. Tables are handled
/// before this is reached.
pub fn ctrl_c_ctrl_c(app: &mut App, line: usize) {
    let text = app.buf_ref().line(line);
    let heading = app.buf_ref().org_heading_at(line);
    if crate::org_rich::at_babel(app) {
        crate::org_rich::command(app, "org-babel-execute-src-block");
    } else if forge_core::org_work::parse_clock_line(&text, line).is_some_and(|c| c.end.is_some()) {
        crate::org_flow::command(app, "org-clock-update-time-maybe");
    } else if crate::org_dblock::at_block(app) {
        crate::org_dblock::command(app, "org-dblock-update");
    } else if heading
        .and_then(|h| app.buf_ref().org_property_drawer(h))
        .is_some_and(|(a, z)| (a..=z).contains(&line))
    {
        command(app, "org-property-action");
    } else if org::heading_level(&text).is_some() {
        app.buf().org_update_cookies();
        command(app, "org-set-tags-command");
    } else if org::list_item(&text).is_some() {
        if !app.buf().org_toggle_checkbox(line) {
            let n = app.buf().org_update_cookies();
            app.msg(format!("Updated {n} cookie(s)"));
        }
    } else if text.trim_start().starts_with("#+") {
        app.msg("Refreshed in-buffer settings".to_string());
    } else {
        app.error("C-c C-c can do nothing useful at this location");
    }
}

/// `org-todo`: with fast-selection keys in `#+TODO:` ask which state,
/// otherwise cycle.
pub fn todo_command(app: &mut App, line: usize, forward: bool) {
    let keys = org::fast_todo_keys(app.buf_ref());
    if keys.is_empty() || !forward {
        return change_todo(app, line, forward, None);
    }
    let mut cands: Vec<String> = keys.iter().map(|(k, c)| format!("{c} {k}")).collect();
    cands.push("SPC (no state)".to_string());
    choose(app, "Select todo state: ", Ask::TodoState, cands);
}

/// Put point back where it was (on the same line, as far along as that
/// line reaches) after a command that rewrote the heading it was on.
fn keep_point(app: &mut App, at: Pos) {
    let line = at.line.min(app.buf_ref().line_count().saturating_sub(1));
    let col = at.col.min(app.buf_ref().line_len(line));
    app.buf().set_point(Pos::new(line, col));
}

/// `org-evaluate-time-range`: where the result goes (a char column) and
/// the text to put there: ` => 26:30`.
fn time_range_result(line: &str) -> Option<(usize, String)> {
    let stamps = forge_core::org_time::timestamps_in(line);
    let chars: Vec<char> = line.chars().collect();
    for w in stamps.windows(2) {
        let between: String = chars[w[0].end..w[1].start].iter().collect();
        if between.trim() != "--" {
            continue;
        }
        let minutes = |t: &Timestamp| {
            let (h, m) = t.time.unwrap_or((0, 0));
            t.date.days() * 1440 + h * 60 + m
        };
        let diff = minutes(&w[1]) - minutes(&w[0]);
        let (sign, d) = if diff < 0 { ("-", -diff) } else { ("", diff) };
        let shown = if d >= 1440 {
            format!("{sign}{}d {}:{:02}", d / 1440, d % 1440 / 60, d % 60)
        } else {
            format!("{sign}{}:{:02}", d / 60, d % 60)
        };
        return Some((w[1].end, format!(" => {shown}")));
    }
    None
}

fn no_heading_true(app: &mut App) -> bool {
    no_heading(app);
    true
}

/// `:NAME: value` -> NAME.
fn property_name_at(line: &str) -> Option<String> {
    let t = line.trim();
    let rest = t.strip_prefix(':')?;
    let name = rest.split(':').next()?;
    (!name.is_empty()
        && !name.eq_ignore_ascii_case("PROPERTIES")
        && !name.eq_ignore_ascii_case("END"))
    .then(|| name.to_string())
}

fn toggle_comment(app: &mut App, h: usize) {
    let kw = keywords(app.buf_ref());
    let hd = org::parse_heading(&app.buf_ref().line(h), &kw);
    let Some(hd) = hd else { return };
    let line = app.buf_ref().line(h);
    let stars = &line[..hd.level];
    let rest = line[hd.level..].trim_start().to_string();
    let new = if let Some(r) = rest.strip_prefix("COMMENT ") {
        format!("{stars} {r}")
    } else if hd.keyword.is_some() {
        // After the keyword: `* TODO COMMENT title`.
        let k = hd.keyword.clone().unwrap_or_default();
        let after = rest[k.len()..].trim_start();
        if let Some(a) = after.strip_prefix("COMMENT ") {
            format!("{stars} {k} {a}")
        } else {
            format!("{stars} {k} COMMENT {after}")
        }
    } else {
        format!("{stars} COMMENT {rest}")
    };
    app.buf().org_set_line(h, new.trim_end());
}

/// Follow the link, timestamp-less: links open, a heading with no link
/// looks for the first link in its entry.
fn open_at_point(app: &mut App) {
    if crate::org_rich::follow(app) {
        return;
    }
    let p = app.buf_ref().point();
    let link = app.buf_ref().org_link_at_point(p).or_else(|| {
        let b = app.buf_ref();
        let h = b.org_heading_at(p.line)?;
        if p.line != h {
            return None;
        }
        forge_core::org_ext::links_in(&b.line(h)).into_iter().next()
    });
    let Some(link) = link else {
        if let Some(ts) = app.buf_ref().org_timestamp_at_point(p) {
            crate::org_agenda_ui::show_date(app, ts.date);
            return;
        }
        // A checkbox toggles, the way C-c C-o on one is C-c C-c in Org.
        if app.buf().org_toggle_checkbox(p.line) {
            return;
        }
        app.error("No link at point");
        return;
    };
    let from = (app.active_id(), p);
    match resolve_link(&link.target) {
        LinkTarget::Url(u) => crate::webkit::browse_url(app, &u),
        LinkTarget::Shell(c) => app.msg(format!("Shell links are not run: {c}")),
        LinkTarget::File { path, search } => {
            let base = app
                .buf_ref()
                .path()
                .and_then(|p| p.parent().map(|d| d.to_path_buf()));
            let full = {
                let pth = crate::vc::expand_home(&path);
                if pth.is_absolute() {
                    pth
                } else {
                    base.map(|b| b.join(&pth)).unwrap_or(pth)
                }
            };
            app.org.mark_ring.push(from);
            crate::minibuffer::visit_file(app, &full, false);
            if let Some(s) = search {
                let t = resolve_link(&s);
                jump_within(app, &s, t);
            }
        }
        t @ (LinkTarget::Heading(_)
        | LinkTarget::CustomId(_)
        | LinkTarget::Id(_)
        | LinkTarget::Fuzzy(_)) => {
            match app.buf_ref().org_find_link_target(&t) {
                Some(to) => {
                    app.org.mark_ring.push(from);
                    app.buf().set_point(to);
                    let off = app.buf_ref().point_offset();
                    app.buf().reveal(off);
                }
                None => {
                    // An `id:` link may live in another agenda file.
                    if let LinkTarget::Id(id) = &t
                        && goto_id_elsewhere(app, id)
                    {
                        app.org.mark_ring.push(from);
                        return;
                    }
                    app.error(format!("No match for link {}", link.target));
                }
            }
        }
    }
}

fn jump_within(app: &mut App, raw: &str, t: LinkTarget) {
    let target = match &t {
        LinkTarget::Fuzzy(_)
        | LinkTarget::Heading(_)
        | LinkTarget::CustomId(_)
        | LinkTarget::Id(_) => t,
        _ => LinkTarget::File {
            path: String::new(),
            search: Some(raw.to_string()),
        },
    };
    match app.buf_ref().org_find_link_target(&target) {
        Some(to) => {
            app.buf().set_point(to);
            let off = app.buf_ref().point_offset();
            app.buf().reveal(off);
        }
        None => app.msg(format!("No match for {raw}")),
    }
}

fn goto_id_elsewhere(app: &mut App, id: &str) -> bool {
    let target = LinkTarget::Id(id.to_string());
    for bid in crate::org_flow::agenda_buffers(app) {
        let found = app
            .editor
            .state
            .get_buffer(bid)
            .and_then(|b| b.org_find_link_target(&target));
        if let Some(to) = found {
            app.switch_to(bid);
            app.buf().set_point(to);
            let off = app.buf_ref().point_offset();
            app.buf().reveal(off);
            return true;
        }
    }
    // `org-id-extra-files`: files (or directories, with everything under
    // them) searched for the ID too. A file is read before it is opened, so
    // only the one holding the ID becomes a buffer.
    for entry in crate::org_flow::setting_strings(app, "org_id_extra_files") {
        for file in forge_core::app::org_files_in(&crate::vc::expand_home(&entry)) {
            let holds = std::fs::read_to_string(&file).is_ok_and(|t| t.contains(id));
            if !holds {
                continue;
            }
            let Ok((bid, _)) = app.editor.open_file(&file) else {
                continue;
            };
            let found = app
                .editor
                .state
                .get_buffer(bid)
                .and_then(|b| b.org_find_link_target(&target));
            if let Some(to) = found {
                app.switch_to(bid);
                app.buf().set_point(to);
                let off = app.buf_ref().point_offset();
                app.buf().reveal(off);
                return true;
            }
        }
    }
    false
}

/// Open a URL with the system opener.
pub(crate) fn open_external(app: &mut App, url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    match std::process::Command::new(opener)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => app.msg(format!("Opening {url}")),
        Err(e) => app.error(format!("Cannot open {url}: {e}")),
    }
}

/// Answers to the questions this file asks. True when `ask` is one of them.
fn stage2_answer(app: &mut App, ask: &Ask, input: &str) -> bool {
    let t = input.trim();
    let line = app.buf_ref().point().line;
    match ask {
        Ask::TodoState => {
            let key = t.chars().next();
            let keys = org::fast_todo_keys(app.buf_ref());
            let state = match key {
                None | Some(' ') => Some(None),
                Some(c) => keys
                    .iter()
                    .find(|(_, k)| *k == c)
                    .map(|(name, _)| Some(name.clone())),
            };
            match state {
                Some(state) => change_todo(app, line, true, Some(state)),
                None => app.error(format!("No state with the key {}", key.unwrap_or(' '))),
            }
        }
        Ask::DoneNote(h) => {
            if !t.is_empty() {
                let (d, n) = now();
                let stamp = Timestamp::new(false, d, Some(n)).text();
                let at = app.buf_ref().point();
                let entry = format!("- CLOSING NOTE {stamp} \\\\\n  {}", t.replace('\n', "\n  "));
                app.buf().org_add_log_entry(*h, &entry);
                keep_point(app, at);
            }
        }
        Ask::Priority => {
            use forge_core::org_work::PriorityAction;
            if let Some(h) = heading_here(app) {
                let act = match t.chars().next() {
                    None | Some(' ') => PriorityAction::Remove,
                    Some('+') | Some('p') => PriorityAction::Up,
                    Some('-') | Some('n') => PriorityAction::Down,
                    Some(c) => PriorityAction::Set(c),
                };
                let at = app.buf_ref().point();
                match app.buf().org_set_priority(h, act) {
                    Ok(Some(c)) => app.msg(format!("Priority [#{c}]")),
                    Ok(None) => app.msg("Priority removed".to_string()),
                    Err(e) => app.error(e),
                }
                keep_point(app, at);
            }
        }
        Ask::Note => {
            if let (Some(h), false) = (heading_here(app), t.is_empty()) {
                let (d, n) = now();
                let at = app.buf_ref().point();
                crate::org_agenda_ui::add_note(app.buf(), h, t, d, n);
                keep_point(app, at);
            }
        }
        Ask::SetTags => {
            if let Some(h) = heading_here(app) {
                let tags = split_tags(t);
                app.buf().org_set_tags(h, &tags);
            }
        }
        Ask::ToggleTag => {
            if let (Some(h), false) = (heading_here(app), t.is_empty()) {
                let added = app.buf().org_toggle_tag(h, t.trim_matches(':'));
                app.msg(if added {
                    format!("Tag {t} added")
                } else {
                    format!("Tag {t} removed")
                });
            }
        }
        Ask::TagsSparse => {
            if !t.is_empty() {
                match app.buf().org_match_sparse_tree(t, false) {
                    Ok(n) => app.msg(format!("{n} match(es)")),
                    Err(e) => app.error(e),
                }
            }
        }
        Ask::PropName => {
            if !t.is_empty() {
                let h = heading_here(app);
                let cur = h
                    .and_then(|h| app.buf_ref().org_get_property(h, t, false))
                    .unwrap_or_default();
                let name = t.to_string();
                ask_with(
                    app,
                    &format!("{name}: "),
                    Ask::PropValue(name.clone()),
                    &cur,
                );
            }
        }
        Ask::PropValue(name) => {
            if let Some(h) = heading_here(app) {
                app.buf().org_set_property(h, name, t);
                app.msg(format!("{name} set"));
            }
        }
        Ask::PropDelete => {
            if let Some(h) = heading_here(app) {
                let gone = app.buf().org_delete_property(h, t);
                app.msg(if gone {
                    format!("{t} deleted")
                } else {
                    format!("No property {t}")
                });
            }
        }
        Ask::PropAction => {
            let key = t.chars().next().unwrap_or(' ');
            match key {
                's' => {
                    let _ = command(app, "org-set-property");
                }
                'd' => {
                    let _ = command(app, "org-delete-property");
                }
                'n' => {
                    let _ = command(app, "org-property-next-allowed-value");
                }
                'p' => {
                    let _ = command(app, "org-property-previous-allowed-value");
                }
                'c' => {
                    let _ = command(app, "org-compute-property-at-point");
                }
                _ => app.error(format!("No property action {key}")),
            }
        }
        Ask::Effort => {
            if let Some(h) = heading_here(app) {
                if t.is_empty() {
                    app.buf().org_delete_property(h, "Effort");
                } else if forge_core::org_ext::parse_effort(t).is_none() {
                    app.error(format!("Bad effort: {t}"));
                } else {
                    app.buf().org_set_property(h, "Effort", t);
                }
            }
        }
        Ask::Timestamp { active } => {
            let (today, now) = now();
            let p = app.buf_ref().point();
            let default = app
                .buf_ref()
                .org_timestamp_at_point(p)
                .map_or(today, |t| t.date);
            match parse_date_input(t, today, default) {
                Some((d, time)) => {
                    let time = time.or(if app.has_arg() { Some(now) } else { None });
                    let ts = Timestamp::new(*active, d, time);
                    app.buf().org_put_timestamp(p, &ts);
                }
                None => app.error(format!("Bad date: {t}")),
            }
        }
        Ask::Schedule | Ask::Deadline => {
            let Some(h) = heading_here(app) else {
                return no_heading_true(app);
            };
            let (today, _) = now();
            if t.is_empty() {
                return true;
            }
            match parse_date_input(t, today, today) {
                Some((d, time)) => {
                    let ts = Timestamp::new(true, d, time);
                    let kind = if matches!(ask, Ask::Schedule) {
                        "SCHEDULED"
                    } else {
                        "DEADLINE"
                    };
                    app.buf().org_set_planning(h, kind, Some(ts));
                    app.msg(format!("{kind}: {}", d.iso()));
                }
                None => app.error(format!("Bad date: {t}")),
            }
        }
        Ask::CheckBefore | Ask::CheckAfter => {
            let (today, _) = now();
            match parse_date_input(t, today, today) {
                Some((d, _)) => {
                    let n = app
                        .buf()
                        .org_check_date_sparse(d, matches!(ask, Ask::CheckBefore));
                    app.msg(format!("{n} entr{}", if n == 1 { "y" } else { "ies" }));
                }
                None => app.error(format!("Bad date: {t}")),
            }
        }
        Ask::LinkTarget => {
            if t.is_empty() {
                return true;
            }
            let desc = app
                .org
                .stored_links
                .iter()
                .find(|(tg, _)| tg == t)
                .map(|(_, d)| d.clone())
                .or_else(|| {
                    let p = app.buf_ref().point();
                    app.buf_ref()
                        .org_link_at_point(p)
                        .and_then(|l| l.description)
                })
                .unwrap_or_default();
            ask_with(
                app,
                "Description (empty for none): ",
                Ask::LinkDesc {
                    target: t.to_string(),
                },
                &desc,
            );
        }
        Ask::LinkDesc { target } => {
            let p = app.buf_ref().point();
            app.buf().org_insert_link(p, target, t);
        }
        Ask::OpenFile => {
            if !t.is_empty() {
                let (file, search) = match t.split_once("::") {
                    Some((f, s)) => (f.to_string(), Some(s.to_string())),
                    None => (t.to_string(), None),
                };
                crate::minibuffer::visit_file(app, &crate::vc::expand_home(&file), false);
                if let Some(s) = search {
                    let t = resolve_link(&s);
                    jump_within(app, &s, t);
                }
            }
        }
        Ask::Template => {
            let key = t.chars().next().unwrap_or(' ').to_string();
            match forge_core::org_ext::structure_template(&key) {
                Some((kind, arg)) => {
                    let (first, last) = match app.buf_ref().active_region() {
                        Some((a, b)) => (
                            a.line,
                            Some(if b.col == 0 && b.line > a.line {
                                b.line - 1
                            } else {
                                b.line
                            }),
                        ),
                        None => (line, None),
                    };
                    let to = app.buf().org_insert_block(first, last, kind, arg);
                    app.buf().set_point(Pos::new(to, 0));
                }
                None => app.error(format!("No structure template {key}")),
            }
        }
        Ask::EntityUser => match t.split_whitespace().collect::<Vec<_>>().as_slice() {
            [name, utf8] => {
                forge_core::export::entities::add_user(name, utf8, utf8, utf8, utf8);
                app.msg(format!("Entity \\{name} defined"));
            }
            [name, latex, html, ascii, utf8] => {
                forge_core::export::entities::add_user(name, latex, html, ascii, utf8);
                app.msg(format!("Entity \\{name} defined"));
            }
            _ => app.error("Usage: name [latex] [html] [ascii] [utf8]"),
        },
        Ask::BeamerEnv => {
            if let Some(h) = heading_here(app) {
                if t.is_empty() {
                    app.buf().org_delete_property(h, "BEAMER_ENV");
                    app.msg("Beamer environment removed");
                } else if t.eq_ignore_ascii_case("column") {
                    app.buf().org_set_property(h, "BEAMER_ENV", t);
                    ask_with(
                        app,
                        "Column width: ",
                        Ask::BeamerCol { env: t.to_string() },
                        "0.5",
                    );
                } else {
                    app.buf().org_set_property(h, "BEAMER_ENV", t);
                    app.buf().org_delete_property(h, "BEAMER_COL");
                    app.msg(format!("Beamer environment {t}"));
                }
            }
        }
        Ask::BeamerCol { env } => {
            if let Some(h) = heading_here(app) {
                let w = if t.is_empty() { "0.5" } else { t };
                app.buf().org_set_property(h, "BEAMER_ENV", env);
                app.buf().org_set_property(h, "BEAMER_COL", w);
                app.msg(format!("Beamer column {w}"));
            }
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod ctrl_c_tests {
    use super::*;
    use forge_core::Buffer;

    fn app_with(text: &str) -> App {
        let mut app = App::new(forge_core::Editor::default());
        let mut b = Buffer::new("t.org");
        b.set_contents(text);
        b.set_major_mode(forge_core::org::MODE);
        let id = app.editor.add_buffer(b);
        app.editor.show_buffer(id);
        app
    }

    fn ctrl_c(app: &mut App, line: usize) {
        app.minibuffer = None;
        app.buf().set_point(Pos::new(line, 0));
        app.run_command("org-ctrl-c-ctrl-c");
    }

    #[test]
    fn on_a_property_drawer_it_asks_for_a_property_action() {
        let mut app = app_with("* A\n:PROPERTIES:\n:Effort: 1:00\n:END:\n");
        for line in 1..=3 {
            ctrl_c(&mut app, line);
            assert!(app.minibuffer.is_some(), "line {line}");
        }
    }

    #[test]
    fn on_a_heading_it_sets_tags_and_on_an_item_toggles() {
        let mut app = app_with("* A [0/1]\n- [ ] one\nplain\n#+TITLE: x\n");
        ctrl_c(&mut app, 0);
        assert!(app.minibuffer.is_some());
        ctrl_c(&mut app, 1);
        assert!(app.minibuffer.is_none());
        assert!(app.buf_ref().line(1).contains("[X]"));
        assert!(app.buf_ref().line(0).contains("[1/1]"));
        ctrl_c(&mut app, 3);
        assert!(app.minibuffer.is_none());
        ctrl_c(&mut app, 2);
    }

    #[test]
    fn a_web_link_opens_in_webkit_and_a_date_in_the_agenda() {
        let mut app = app_with("* A\nSee [[https://example.com/x]] on <2026-10-14 Wed>.\n");
        app.graphical = true;
        app.buf().set_point(Pos::new(1, 8));
        app.run_command("org-open-at-point");
        assert_eq!(app.web.pages.len(), 1, "{:?}", app.message);
        assert_eq!(app.web.pages[0].url, "https://example.com/x");
        // The page is its own buffer now; go back to the Org one.
        let org = app
            .editor
            .state
            .buffers
            .iter()
            .find(|(_, b)| b.major_mode() == forge_core::org::MODE)
            .map(|(id, _)| *id)
            .unwrap();
        app.switch_to(org);
        let col = app.buf_ref().line(1).find("<2026").unwrap() + 3;
        app.buf().set_point(Pos::new(1, col));
        app.run_command("org-open-at-point");
        let st = app.org.agenda.as_ref().expect("agenda");
        assert_eq!(st.anchor.iso(), "2026-10-14");
    }
}
