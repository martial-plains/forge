// SPDX-License-Identifier: MIT OR Apache-2.0
//! Terminal application state: the editor plus everything Forge keeps
//! around it (pending prefix keys, prefix argument, minibuffer, keyboard
//! macros, echo-area message, the last command).

use crate::commands;
use crate::help;
use crate::keymap::{self, describe_key, describe_key_sequence};
use crate::minibuffer::{self, Minibuffer, Purpose};
use crate::plugins::{self, PendingGrant, PendingQuestion};
use crate::query::QueryReplace;
use crate::watch::Watch;
use forge_core::keymap::{Continuation, OnUnbound};
use forge_core::{
    Buffer, BufferId, Editor, GrantStore, Keymaps, Lookup, PluginHost, Pos, RhaiConfig, Scope,
    Window, WindowId,
};
use forge_docs::DocRegistry;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::Duration;

/// Kill commands that, run back to back, accumulate into one kill-ring entry.
const KILL_COMMANDS: &[&str] = &[
    "clipboard-kill-region",
    "kill-line",
    "kill-region",
    "kill-word",
    "backward-kill-word",
    "kill-sentence",
    "kill-sexp",
    "backward-kill-sentence",
];

/// How long a half-typed prefix waits before listing the keys that can follow it.
pub const DEFAULT_PREFIX_HELP_DELAY: Duration = Duration::from_millis(1000);
/// Idle time before modified buffers are auto-saved.
pub const AUTO_SAVE_IDLE: Duration = Duration::from_secs(30);
/// How long each step of a smooth scroll waits (one line per step).
pub const SMOOTH_SCROLL_TICK: Duration = Duration::from_millis(8);

/// Why a key sequence is being read (C-h k, C-h c, global-set-key, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadKeyFor {
    DescribeKey,
    DescribeKeyBriefly,
    SetKey { local: bool },
}

impl ReadKeyFor {
    fn prompt(self) -> &'static str {
        match self {
            Self::DescribeKey => "Describe key: ",
            Self::DescribeKeyBriefly => "Describe key briefly: ",
            Self::SetKey { local: false } => "Set key globally: ",
            Self::SetKey { local: true } => "Set key locally: ",
        }
    }
}

/// Reading a whole key sequence without running it.
pub struct KeyReader {
    pub purpose: ReadKeyFor,
    pub keys: Vec<String>,
}

/// One key press after ESC-as-Meta handling.
enum KeyInput {
    Key(String),
    /// ESC started a Meta prefix, or the key can't be described.
    Consumed,
    /// ESC ESC: give up on the sequence.
    Cancelled,
}

/// The prefix argument a command was given (C-u, C-u C-u, M-5, C-u - 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixArg {
    /// C-u pressed n times: 4, 16, 64, ...
    Raw(i64),
    Number(i64),
    /// M-- (or C-u -) with no digits.
    Negative,
}

impl PrefixArg {
    pub fn value(self) -> i64 {
        match self {
            Self::Raw(n) | Self::Number(n) => n,
            Self::Negative => -1,
        }
    }
}

/// A prefix argument being typed.
#[derive(Debug, Clone, Copy, Default)]
struct ArgState {
    /// 4 per C-u (0: no C-u).
    raw: i64,
    digits: Option<i64>,
    negative: bool,
    /// C-u after digits ends the number, so digits after it are text
    /// (C-u 6 4 C-u 1 inserts 64 ones).
    closed: bool,
}

impl ArgState {
    fn finish(self) -> PrefixArg {
        match (self.digits, self.negative) {
            (Some(d), neg) => PrefixArg::Number(if neg { -d } else { d }),
            (None, true) => PrefixArg::Negative,
            (None, false) => PrefixArg::Raw(self.raw.max(4)),
        }
    }

    fn echo(self) -> String {
        let sign = if self.negative { "-" } else { "" };
        match self.digits {
            Some(d) => format!("C-u {sign}{d}-"),
            None if self.negative => "C-u --".to_string(),
            None => {
                let mut n = self.raw.max(4);
                let mut parts = Vec::new();
                while n >= 4 {
                    parts.push("C-u");
                    n /= 4;
                }
                format!("{}-", parts.join(" "))
            }
        }
    }
}

/// C-q: the next key is inserted as a character, even a control key.
/// Octal digits (C-q 1 7 7) give that character code.
#[derive(Debug, Clone, Default)]
pub struct QuotedInsert {
    count: usize,
    octal: String,
}

pub struct App {
    pub editor: Editor,
    /// Prefix keys typed so far (e.g. ["C-x"]).
    pub pending: Vec<String>,
    /// The keys that ran the current command (for `undefined` and
    /// `self-insert-command`).
    pub this_command_keys: Vec<String>,
    /// Set while C-h k, global-set-key, ... read a key sequence.
    pub key_reader: Option<KeyReader>,
    /// A one-key menu in its own window, waiting (see [`crate::one_key`]).
    pub one_key: Option<crate::one_key::OneKey>,
    /// Show the keys that can follow the pending prefix (which-key style).
    pub show_prefix_hints: bool,
    /// Zero turns automatic prefix hints off (C-h after a prefix still works).
    pub prefix_help_delay: Duration,
    /// init.rhai; None when started with -q.
    pub init_path: Option<PathBuf>,
    /// The Rhai state init.rhai ran in (its commands live here).
    pub rhai: Option<RhaiConfig>,
    /// Errors from the last init.rhai/theme load (used by the package doctor).
    pub init_errors: Vec<String>,
    /// ESC was pressed; the next key gets Meta.
    pub meta_pending: bool,
    pub minibuffer: Option<Minibuffer>,
    /// The prompt a `completing-read-over` question took over (suspended
    /// while its question runs, then restored).
    pub suspended_prompt: Option<Minibuffer>,
    /// Prompts a plugin suspended with minibuffer.suspend, newest first.
    /// completion.sessions lists them and completion.resume brings one back.
    pub suspended_prompts: Vec<Minibuffer>,
    /// Echo-area message, cleared on the next key.
    pub message: Option<String>,
    /// Name of the *Messages* buffer.
    pub messages_buffer_name: String,
    /// How many lines *Messages* keeps.
    pub message_log_max: forge_core::MessageLogMax,
    /// Don't show messages in the echo area;
    /// they are still recorded in *Messages*.
    pub inhibit_message: bool,
    /// The last line written to *Messages*, so runs of related messages
    /// (progress "..." and wait/answer questions) collapse into one, as
    /// message-log-max does.
    pub(crate) last_messages_log: Option<crate::messages::LogEntry>,
    pub last_command: String,
    /// Whether the last kill went backwards, so a following kill in the
    /// same direction accumulates into the same kill-ring entry and one
    /// in the other direction starts a new one.
    last_kill_backward: Option<bool>,
    /// The last keys typed, for C-h l (view-lossage).
    pub lossage: std::collections::VecDeque<String>,
    pub this_command: String,
    /// The name of the command that is running, which the command itself
    /// can change; `last_command` takes it when the command finishes.
    command_name: String,
    /// The prefix argument of the command being run.
    pub prefix_arg: Option<PrefixArg>,
    /// Frames whose tab bar `toggle-frame-tab-bar` forced on (true) or off
    /// (false), overriding the global `tab-bar-mode`.
    pub tab_bar_override: crate::tabs::Overrides,
    /// This frontend is a GUI, so it can show a tool bar. A terminal
    /// frontend leaves it false: Forge has no tool bar in a text frame.
    pub graphical: bool,
    /// How much of each tool bar item to draw.
    pub tool_bar_style: forge_core::toolbar::Style,
    /// Where the tool bar goes.
    pub tool_bar_position: forge_core::toolbar::Position,
    /// Keep the default tool bar in modes that define their own.
    pub tool_bar_always_show_default: bool,
    /// Modifier keys picked on the modifier bar, waiting for the next key.
    pub pending_modifiers: Vec<forge_core::toolbar::Modifier>,
    arg_state: Option<ArgState>,
    /// Handed to the command M-x runs.
    pub(crate) forced_arg: Option<PrefixArg>,
    /// A command failed ("End of buffer", "... is undefined"). Stops
    /// keyboard macros, like bell.
    pub bell: bool,
    pub quoted: Option<QuotedInsert>,
    pub query: Option<QueryReplace>,
    /// Last (from, to) for M-% defaults.
    pub last_replace: Option<(String, String)>,
    /// The last rectangle killed or copied (C-x r y inserts it).
    pub killed_rectangle: Vec<String>,
    /// auto-save-timeout: idle time before auto-saving.
    pub auto_save_idle: Duration,
    /// auto-save-interval: input events between auto-saves (0: never).
    pub auto_save_interval: u64,
    /// auto-save-default: auto-save file buffers at all.
    pub auto_save_default: bool,
    /// Input events since the last auto-save.
    pub input_since_auto_save: u64,
    /// When the last key or click arrived (the idle auto-save waits for
    /// auto_save_idle after it).
    pub last_input: std::time::Instant,
    /// Idle hooks (plugins' run-when-idle) that already ran since the
    /// last input.
    pub idle_ran: std::collections::HashSet<(String, String)>,
    /// Current isearch / query-replace match to highlight.
    pub highlight: Option<(Pos, Pos)>,
    pub last_isearch: String,
    /// Where the last C-y / M-y put its text.
    pub yank_range: Option<(Pos, Pos)>,
    /// Link kill and yank commands to the system clipboard (forge.opt.select_enable_clipboard).
    pub select_enable_clipboard: bool,
    /// Link kill and yank commands to the primary selection (forge.opt.select_enable_primary).
    pub select_enable_primary: bool,
    /// Save foreign clipboard text to kill ring before a kill (forge.opt.save_interprogram_paste_before_kill).
    pub save_interprogram_paste_before_kill: forge_core::SaveInterprogramPasteBeforeKill,
    /// Rotating kill ring with M-y saves the yanked text to clipboard (forge.opt.yank_pop_change_selection).
    pub yank_pop_change_selection: bool,
    /// Dragging a region with the mouse copies it to kill ring/clipboard (forge.opt.mouse_drag_copy_region).
    pub mouse_drag_copy_region: bool,
    /// Transfer clipboard data to an X11 clipboard manager on exit (forge.opt.x_select_enable_clipboard_manager).
    pub x_select_enable_clipboard_manager: bool,
    /// The last text placed on or read from the system clipboard.
    pub last_clipboard_out: Option<String>,
    /// The last text placed on or read from the primary selection.
    pub last_primary_out: Option<String>,
    /// System or mock clipboard provider.
    pub clipboard_provider: Box<dyn crate::clipboard::ClipboardProvider>,
    pub previous_buffer: Option<BufferId>,
    /// Capture the mouse (wheel scrolls Forge, not the terminal).
    pub mouse: bool,
    /// Animate scrolling a line at a time instead of jumping
    /// (forge.opt.smooth_scroll).
    pub smooth_scroll: bool,
    /// Scroll the wheel in pixels rather than whole lines
    /// (forge.opt.pixel_scroll_precision).
    pub pixel_scroll_precision: bool,
    /// A scroll in flight: the window, and the scroll_top it is heading
    /// for. Only while smooth_scroll is on.
    pub(crate) scroll_anim: Option<(WindowId, usize)>,
    /// Show a vertical scroll bar (forge.opt.scroll_bar).
    pub scroll_bar: bool,
    /// What `load_init_file` resets `scroll_bar` to, before init.rhai runs.
    /// The frontends turn the scroll bar on by default (see `start`), so a
    /// reload keeps it on unless the user says otherwise.
    pub default_scroll_bar: bool,
    /// Show a horizontal scroll bar (forge.opt.horizontal_scroll_bar).
    pub horizontal_scroll_bar: bool,
    /// A thumb being dragged on a scroll bar: the window and which bar
    /// (false: vertical, true: horizontal).
    pub(crate) scroll_bar_drag: Option<(WindowId, bool)>,
    /// Where a left-button drag started (char offset).
    pub mouse_anchor: Option<usize>,
    /// The screen cell the pointer last pressed or moved to (a plugin's
    /// popup menu opens there).
    pub pointer: (u16, u16),
    /// `tooltip-mode`: show help text when the pointer rests on something.
    pub tooltip_mode: bool,
    /// wdired: the entries (line, path) a Dired buffer had when editing began.
    pub wdired: std::collections::HashMap<forge_core::BufferId, Vec<(usize, std::path::PathBuf)>>,
    /// Loaded plugins (the runtime starts with the first one).
    pub plugin_host: Option<PluginHost>,
    /// Where compiled plugins and their summaries are cached (None: no cache).
    pub plugin_cache: Option<std::path::PathBuf>,
    /// Plugin questions waiting for the minibuffer.
    pub pending_questions: VecDeque<PendingQuestion>,
    /// Plugin commands currently running each other (loop guard).
    pub plugin_depth: usize,
    /// What the user has granted plugins (plugin-grants.toml).
    pub grants: GrantStore,
    /// A plugin waiting for the permission question to be answered.
    pub pending_grant: Option<PendingGrant>,
    /// Plugin files waiting for that question to be over.
    pub pending_loads: VecDeque<PathBuf>,
    /// Projects whose init.rhai has been asked about (or run) this session,
    /// so one is never asked about twice (project.rs).
    pub projects_asked: std::collections::HashSet<PathBuf>,
    /// Shared with the watchdog thread (busy plugins, C-g, stolen keys).
    pub watch: Watch,
    /// Reload a plugin when its file changes (forge.opt.plugin_auto_reload).
    pub plugin_auto_reload: bool,
    /// Line numbers for buffers that don't choose (forge.opt.line_numbers).
    pub line_numbers: forge_core::decoration::LineNumbers,
    /// RET indents the new line, closing brackets re-indent theirs
    /// (forge.opt.electric_indent).
    pub electric_indent: bool,
    /// Defaults for buffers that don't set their own (forge.opt.*).
    pub wrap: forge_core::decoration::Wrap,
    pub hl_line: bool,
    pub fill_column_indicator: bool,
    pub show_trailing_whitespace: bool,
    /// show-paren-mode (on by default).
    pub show_paren: bool,
    /// Whether a plain C-SPC right after C-u C-SPC pops the mark ring
    /// again.
    pub set_mark_command_repeat_pop: bool,
    /// electric-pair-mode: ( inserts (), and so on.
    pub electric_pair: bool,
    /// Image mode: loop an animation (forge.opt.image_animate_loop).
    pub image_animate_loop: bool,
    /// How a sentence ends (forge.opt.sentence_end_double_space and
    /// sentence_end_without_period).
    pub sentence_end: forge_core::text_nav::SentenceEnd,
    /// The `sentence-end` regexp, or empty for the built-in rules
    /// (forge.opt.sentence_end).
    pub sentence_end_pattern: String,
    /// The `page-delimiter` regexp (forge.opt.page_delimiter).
    pub page_delimiter: String,
    /// The `paragraph-start` and `paragraph-separate` regexps
    /// (forge.opt.paragraph_start, forge.opt.paragraph_separate).
    pub paragraph_start: String,
    pub paragraph_separate: String,
    /// electric-quote-mode: typing ` and ' makes curved quotes.
    pub electric_quote: bool,
    /// The four curved quotes Electric Quote mode types, in order:
    /// left single, right single, left double, right double
    /// (forge.opt.electric_quote_chars).
    pub electric_quote_chars: [char; 4],
    /// Whether `"` is also converted to a curved double quote
    /// (forge.opt.electric_quote_replace_double).
    pub electric_quote_replace_double: bool,
    /// Where Electric Quote mode converts quotes (forge.opt.*). Off by
    /// default for strings.
    pub electric_quote_paragraph: bool,
    pub electric_quote_comment: bool,
    pub electric_quote_string: bool,
    /// Save minibuffer history between sessions (savehist-mode).
    pub savehist: bool,
    /// Structured documentation registry (built from commands, etc).
    pub docs: DocRegistry,
    /// The Info-style manual browser's node and history (*Manual* buffer).
    pub info: crate::info::InfoState,
    /// The interactive tutorial's progress (*Tutorial* buffer).
    pub tutorial: crate::tutorial::TutorialState,
    /// M-/ in progress.
    pub dabbrev: Option<crate::builtins::Dabbrev>,
    /// The *Occur* buffer, if any.
    pub occur: Option<crate::builtins::Occur>,
    /// Org clocks, capture, agenda and friends.
    pub org: crate::org_cmds::OrgState,
    /// few, the text web browser.
    pub few: crate::few::FewState,
    /// The edit server (forgeclient), when started.
    pub server: Option<crate::server::Server>,
    /// term, the terminal emulator buffers.
    pub term: crate::term::TermState,
    /// Icon preference, platform and per-icon overrides.
    pub icons: forge_core::icons::Prefs,
    /// Mode line parts: line-number-mode, column-number-mode, size-indication-mode.
    pub line_number_mode: bool,
    pub column_number_mode: bool,
    pub size_indication_mode: bool,
    /// The embedded web pages (webkit), which a frontend draws.
    pub web: crate::webkit::WebState,
    /// The search engines set with commands.
    pub search: crate::search::Engines,
    /// winner, uniquify and auto-revert bookkeeping.
    pub extras: crate::window_extras::State,
    /// Running compilations, greps and shell commands.
    pub jobs: crate::compile::Jobs,
    /// The grep options.
    pub grep: crate::grep::GrepOptions,
    /// `case-fold-search`: a search that ignores case.
    pub case_fold_search: bool,
    /// Package indexes and downloads.
    pub packages: crate::packages::Packages,
    /// Web requests init.rhai made with forge.http (in flight).
    pub rhai_fs: forge_core::process::FsWorker,
    pub rhai_http: forge_core::http::HttpRequests,
    /// Maps each in-flight forge.http request's host id to the Rhai
    /// request id its callback was registered under.
    pub rhai_http_ids: std::collections::BTreeMap<forge_core::http::RequestId, u32>,
    /// Completion at point (capf.rs).
    pub completion: crate::capf::State,
    /// A plugin waiting for one key (minibuffer.read-key).
    pub plugin_key: Option<(String, u32)>,
    /// Version control: buffers' states, running commands, the status,
    /// log, diff and commit buffers (vc/mod.rs).
    pub vc: crate::vc::Vc,
    /// C-x 8 (key translations for characters, insert-char, emoji).
    pub iso: crate::iso_transl::State,
    /// Input methods (C-\\, C-x \\): each buffer's, and a translation in
    /// progress.
    pub im: crate::input_method::State,
    /// forge.mode.hook's hooks, and which have run in each buffer for the
    /// major mode it has (a new mode runs them again).
    pub mode_hooks: Vec<(String, String)>,
    mode_hooks_done: HashMap<BufferId, (String, std::collections::HashSet<usize>)>,
    /// Commands init.rhai ran before a plugin defined them.
    pub deferred_commands: Vec<(String, Option<i64>)>,
    /// Hook commands not defined once the plugins had loaded (said once).
    hooks_warned: std::collections::HashSet<String>,
    running_hooks: bool,
    /// The selected window's cursor shape at the last redisplay.
    pub cursor_shape: forge_core::decoration::CursorShape,
    /// What the terminal can display (set by the terminal frontend at
    /// startup; the default is the cautious "only the basics").
    pub quit: bool,
}

/// Every command table declared with structured [`forge_docs::CommandDoc`]s.
/// `App::new` registers each command and overlays its structured doc, and
/// [`App::sync_docs`] re-overlays them after the legacy registry changes.
fn rich_command_tables() -> [&'static [forge_docs::CommandDoc]; 26] {
    [
        commands::COMMANDS,
        crate::vc::COMMANDS,
        crate::iso_transl::COMMANDS,
        crate::input_method::COMMANDS,
        crate::org_cmds::COMMANDS,
        crate::org_flow::COMMANDS,
        crate::org_agenda_ui::COMMANDS,
        crate::org_extra::COMMANDS,
        crate::org_export::COMMANDS,
        crate::org_rich::COMMANDS,
        crate::few::COMMANDS,
        crate::server::COMMANDS,
        crate::term::COMMANDS,
        crate::icon::COMMANDS,
        crate::canvas::COMMANDS,
        crate::dialog::COMMANDS,
        crate::mode_line::COMMANDS,
        crate::hover::COMMANDS,
        crate::tether::COMMANDS,
        crate::webkit::COMMANDS,
        crate::org_table_cmds::COMMANDS,
        crate::org_crypt::COMMANDS,
        crate::org_dblock::COMMANDS,
        crate::search::COMMANDS,
        crate::org_publish::COMMANDS,
        crate::window_extras::COMMANDS,
    ]
}

impl App {
    /// An app with the default key bindings (init.rhai isn't run until
    /// [`App::load_init_file`]).
    pub fn new(mut editor: Editor) -> Self {
        editor.keymaps = Keymaps::new();
        keymap::install_defaults(&mut editor.keymaps);
        let mut docs = DocRegistry::new();
        let rich = rich_command_tables();
        for table in rich {
            for cd in table {
                editor
                    .commands
                    .register_builtin(cd.name(), &cd.legacy_string());
            }
        }
        crate::docs_bridge::sync_command_docs(&mut docs, &editor.commands);
        for table in rich {
            crate::docs_bridge::overlay_command_docs(&mut docs, &editor.commands, table);
        }
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "fundamental-mode",
            "The mode with no mode-specific behaviour: plain text, the global keys,\n\nnothing else.",
            forge_docs::Kind::Mode,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "text-mode",
            "For prose. Words are the unit of motion, and M-q fills paragraphs.",
            forge_docs::Kind::Mode,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "prog-mode",
            "The parent of every programming mode: syntax highlighting from the language's\n\ngrammar, indentation by the syntax tree, comment commands (M-;) and electric pairs.",
            forge_docs::Kind::Mode,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "special-mode",
            "For read-only buffers that a command filled in.\n\nq closes the window, SPC and DEL scroll.",
            forge_docs::Kind::Mode,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "help-mode",
            "The mode of *Help*: read-only, with q to close.\n\nHeadings, keys and command names are coloured. Ask for more with C-h f, C-h k, C-h o, C-h b and C-h m.",
            forge_docs::Kind::Mode,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "org-mode",
            "Outlines, TODO items, checkboxes, tables, links, timestamps and agenda.\n\nTAB folds the heading, S-TAB cycles the whole buffer, C-c C-t cycles the TODO state, M-arrows promote, demote and move subtrees.",
            forge_docs::Kind::Mode,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "compilation-mode",
            "The mode of *compilation* and *grep*: RET jumps to the error on the line,\n\nn and p move between errors, g reruns, C-c C-k stops the process.",
            forge_docs::Kind::Mode,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind("dired-mode", "Dired lists a directory and works on its files, using detailed columns.\n\nMoving and visiting:\n  RET f e   visit the file          o     visit in another window\n  ^         go up a directory       i     list a subdirectory inline\n  k         remove an inline subdirectory    C-M-n C-M-p   next and previous subdirectory\n\nMarking:\n  m   mark          u   unmark          U   unmark everything\n  d   flag for deletion          x   delete the flagged files          D   delete now\n\nChanging files:\n  C   copy          R   rename or move          +   create a directory          g   refresh\n\nLayout (these change how the listing looks, not the files):\n  s sort (name, size, time...)   r reverse   G directories first   a hidden files\n  T tree view   l tree depth   I inodes   c colour by size   B icons\n  h hide what git ignores   F only files   / only directories\n\nMouse: <mouse-1> visits the name under it, <mouse-2> in another window.\nPress ? for a one-line reminder.", forge_docs::Kind::Mode, forge_docs::Source::Builtin));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "Buffer-menu-mode",
            "Lists the buffers. Switch with C-x b.",
            forge_docs::Kind::Mode,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "menu-bar-mode",
            "Draws the menu bar across the top.\n\nF10 opens it; C-n, C-p, C-f, C-b move; RET runs; ESC closes.",
            forge_docs::Kind::Mode,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "scroll-bar-mode",
            "Draws a vertical scroll bar on the right of every window.\n\nClicking above or below the thumb scrolls by nearly a screen (like M-v and C-v); dragging the thumb scrolls continuously. The scroll-bar face sets its colours.",
            forge_docs::Kind::Mode,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "horizontal-scroll-bar-mode",
            "Draws a horizontal scroll bar on the bottom of every window.\n\nClicking left or right of the thumb scrolls by a few columns; dragging the thumb scrolls continuously. The scroll-bar face sets its colours.",
            forge_docs::Kind::Mode,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "context-menu-mode",
            "Right-click, or S-F10, pops up a menu of what makes sense at the pointer: undo,\n\ncut, copy, paste, select.",
            forge_docs::Kind::Mode,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "auto-fill-mode",
            "Breaks lines automatically when they pass the fill column while you type.",
            forge_docs::Kind::Mode,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "smerge-mode",
            "Resolves Git conflict markers: keep upper, lower or both,\n\nand move between conflicts.",
            forge_docs::Kind::Mode,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind("prefix_help_delay", "Seconds to wait after a prefix key before listing the keys that can follow it.\n\nValues: 0 or more", forge_docs::Kind::Option, forge_docs::Source::Config));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "mouse",
            "The wheel scrolls, a click moves point, a drag selects.\n\nValues: true or false",
            forge_docs::Kind::Option,
            forge_docs::Source::Config,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "plugin_auto_reload",
            "Reload a plugin by itself when its file is rebuilt.\n\nValues: true or false",
            forge_docs::Kind::Option,
            forge_docs::Source::Config,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "package_index",
            "Where package indexes live.\n\nValues: a path or URL, or a list of them",
            forge_docs::Kind::Option,
            forge_docs::Source::Config,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind("electric_indent", "RET indents the new line; a closing bracket re-indents its line.\n\nValues: true or false", forge_docs::Kind::Option, forge_docs::Source::Config));
        docs.add(crate::docs_bridge::legacy_to_doc_kind("line_numbers", "Line numbers in buffers that don't choose their own.\n\nValues: off, absolute or relative", forge_docs::Kind::Option, forge_docs::Source::Config));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "mark_ring_max",
            "How many marks a buffer's mark ring keeps (C-u C-SPC walks back through them).\n\nValues: a number",
            forge_docs::Kind::Option,
            forge_docs::Source::Config,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "set_mark_command_repeat_pop",
            "A plain C-SPC right after C-u C-SPC pops the mark ring again.\n\nValues: true or false",
            forge_docs::Kind::Option,
            forge_docs::Source::Config,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "wrap",
            "What long lines do in buffers that don't choose.\n\nValues: off, char or word",
            forge_docs::Kind::Option,
            forge_docs::Source::Config,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "editorconfig",
            ".Editorconfig files set indentation and end-of-line behavior.\n\nValues: true or false",
            forge_docs::Kind::Option,
            forge_docs::Source::Config,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "electric_pair",
            "( Inserts a matching ).\n\nValues: true or false",
            forge_docs::Kind::Option,
            forge_docs::Source::Config,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "fill_column_indicator",
            "A line at the fill column in every buffer.\n\nValues: true or false",
            forge_docs::Kind::Option,
            forge_docs::Source::Config,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "hl_line",
            "Highlight the current line everywhere.\n\nValues: true or false",
            forge_docs::Kind::Option,
            forge_docs::Source::Config,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "context_menu",
            "Right-click opens a context menu (context-menu-mode).\n\nValues: true or false",
            forge_docs::Kind::Option,
            forge_docs::Source::Config,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "menu_bar",
            "Draw the menu bar (menu-bar-mode).\n\nValues: true or false",
            forge_docs::Kind::Option,
            forge_docs::Source::Config,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "recentf",
            "Remember recently opened files.\n\nValues: true or false",
            forge_docs::Kind::Option,
            forge_docs::Source::Config,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "save_place",
            "Files open where you left them.\n\nValues: true or false",
            forge_docs::Kind::Option,
            forge_docs::Source::Config,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "savehist",
            "Keep minibuffer history between sessions.\n\nValues: true or false",
            forge_docs::Kind::Option,
            forge_docs::Source::Config,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "show_paren",
            "Highlight matching brackets.\n\nValues: true or false",
            forge_docs::Kind::Option,
            forge_docs::Source::Config,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "show_trailing_whitespace",
            "Blanks at line ends stand out everywhere.\n\nValues: true or false",
            forge_docs::Kind::Option,
            forge_docs::Source::Config,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "cursor",
            "How the cursor looks in this buffer.\n\nValues: box, bar, hbar",
            forge_docs::Kind::Option,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "fill-column",
            "The column M-q fills to.\n\nValues: a number of columns",
            forge_docs::Kind::Option,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "display-fill-column-indicator",
            "A line at the fill column.\n\nValues: true or false",
            forge_docs::Kind::Option,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "display-fill-column-indicator-column",
            "Where the fill column line goes.\n\nValues: t (the fill column), a number, or nil (no line)",
            forge_docs::Kind::Option,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "display-fill-column-indicator-character",
            "The character the fill column line is drawn with.\n\nValues: one character, or nil (no line)",
            forge_docs::Kind::Option,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "hl-line",
            "Highlight the line point is on.\n\nValues: true or false",
            forge_docs::Kind::Option,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "indent-tabs",
            "Indent with tabs.\n\nValues: true or false",
            forge_docs::Kind::Option,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "indent-width",
            "Columns per indentation level.\n\nValues: a number of columns",
            forge_docs::Kind::Option,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "insert-final-newline",
            "End the file with a newline on save.\n\nValues: true or false",
            forge_docs::Kind::Option,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "line-numbers",
            "Line numbers here.\n\nValues: off, absolute or relative",
            forge_docs::Kind::Option,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind("read-only", "Commands may not change the text; the plugin that owns the buffer still can.\n\nValues: true or false", forge_docs::Kind::Option, forge_docs::Source::Builtin));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "show-trailing-whitespace",
            "Blanks at the ends of lines stand out.\n\nValues: true or false",
            forge_docs::Kind::Option,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "trim-trailing-whitespace",
            "Delete trailing blanks on save.\n\nValues: true or false",
            forge_docs::Kind::Option,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "whitespace",
            "Draw spaces and tabs visibly.\n\nValues: true or false",
            forge_docs::Kind::Option,
            forge_docs::Source::Builtin,
        ));
        docs.add(crate::docs_bridge::legacy_to_doc_kind(
            "wrap",
            "What long lines do: cut off, wrap anywhere, or at words.\n\nValues: off, char or word",
            forge_docs::Kind::Option,
            forge_docs::Source::Builtin,
        ));
        crate::docs_tables::register_faces_and_events(&mut docs);
        Self {
            editor,
            pending: Vec::new(),
            this_command_keys: Vec::new(),
            key_reader: None,
            one_key: None,
            show_prefix_hints: false,
            prefix_help_delay: DEFAULT_PREFIX_HELP_DELAY,
            init_path: None,
            rhai: None,
            init_errors: Vec::new(),
            meta_pending: false,
            minibuffer: None,
            suspended_prompt: None,
            suspended_prompts: Vec::new(),
            message: None,
            messages_buffer_name: "*Messages*".to_string(),
            message_log_max: forge_core::MessageLogMax::default(),
            inhibit_message: false,
            last_messages_log: None,
            last_command: String::new(),
            last_kill_backward: None,
            lossage: Default::default(),
            this_command: String::new(),
            command_name: String::new(),
            prefix_arg: None,
            tab_bar_override: Default::default(),
            graphical: false,
            tool_bar_style: Default::default(),
            tool_bar_position: Default::default(),
            tool_bar_always_show_default: false,
            pending_modifiers: Vec::new(),
            arg_state: None,
            forced_arg: None,
            bell: false,
            quoted: None,
            query: None,
            last_replace: None,
            killed_rectangle: Vec::new(),
            auto_save_idle: AUTO_SAVE_IDLE,
            auto_save_interval: crate::files::AUTO_SAVE_INTERVAL,
            auto_save_default: true,
            input_since_auto_save: 0,
            last_input: std::time::Instant::now(),
            idle_ran: Default::default(),
            highlight: None,
            last_isearch: String::new(),
            yank_range: None,
            select_enable_clipboard: true,
            select_enable_primary: false,
            save_interprogram_paste_before_kill: forge_core::SaveInterprogramPasteBeforeKill::Off,
            yank_pop_change_selection: false,
            mouse_drag_copy_region: false,
            x_select_enable_clipboard_manager: true,
            last_clipboard_out: None,
            last_primary_out: None,
            // Tests must not read or write the machine's clipboard: what is
            // on it would change what a yank pastes.
            #[cfg(not(test))]
            clipboard_provider: Box::new(crate::clipboard::SystemClipboard::new()),
            #[cfg(test)]
            clipboard_provider: Box::new(crate::clipboard::InMemoryClipboard::new()),
            previous_buffer: None,
            mouse: true,
            smooth_scroll: false,
            pixel_scroll_precision: false,
            scroll_anim: None,
            scroll_bar: false,
            default_scroll_bar: false,
            horizontal_scroll_bar: false,
            scroll_bar_drag: None,
            mouse_anchor: None,
            pointer: (0, 0),
            tooltip_mode: true,
            wdired: Default::default(),
            plugin_host: None,
            plugin_cache: None,
            pending_questions: VecDeque::new(),
            plugin_depth: 0,
            grants: GrantStore::in_memory(),
            pending_grant: None,
            pending_loads: VecDeque::new(),
            projects_asked: std::collections::HashSet::new(),
            watch: Watch::default(),
            plugin_auto_reload: true,
            line_numbers: Default::default(),
            electric_indent: true,
            wrap: Default::default(),
            hl_line: false,
            fill_column_indicator: false,
            show_trailing_whitespace: false,
            show_paren: true,
            set_mark_command_repeat_pop: false,
            electric_pair: false,
            image_animate_loop: false,
            sentence_end: forge_core::text_nav::SentenceEnd::default(),
            sentence_end_pattern: String::new(),
            page_delimiter: forge_core::text_nav::DEFAULT_PAGE_DELIMITER.to_string(),
            paragraph_start: forge_core::text_nav::DEFAULT_PARAGRAPH_START.to_string(),
            paragraph_separate: forge_core::text_nav::DEFAULT_PARAGRAPH_SEPARATE.to_string(),
            electric_quote: false,
            electric_quote_chars: ['\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}'],
            electric_quote_replace_double: false,
            electric_quote_paragraph: true,
            electric_quote_comment: true,
            electric_quote_string: false,
            savehist: true,
            docs,
            info: Default::default(),
            tutorial: Default::default(),
            dabbrev: None,
            occur: None,
            org: Default::default(),
            few: Default::default(),
            server: None,
            term: Default::default(),
            icons: Default::default(),
            line_number_mode: true,
            column_number_mode: true,
            size_indication_mode: false,
            web: Default::default(),
            search: Default::default(),
            extras: Default::default(),
            jobs: Default::default(),
            grep: Default::default(),
            case_fold_search: true,
            packages: Default::default(),
            rhai_fs: Default::default(),
            rhai_http: Default::default(),
            rhai_http_ids: Default::default(),
            completion: Default::default(),
            plugin_key: None,
            vc: Default::default(),
            iso: Default::default(),
            im: Default::default(),
            mode_hooks: Vec::new(),
            mode_hooks_done: HashMap::new(),
            deferred_commands: Vec::new(),
            hooks_warned: Default::default(),
            running_hooks: false,
            cursor_shape: Default::default(),
            quit: false,
        }
    }

    pub fn buf(&mut self) -> &mut Buffer {
        self.editor
            .state
            .active_buffer_mut()
            .expect("there is always an active buffer")
    }

    pub fn buf_ref(&self) -> &Buffer {
        self.editor
            .state
            .active_buffer()
            .expect("there is always an active buffer")
    }

    pub fn active_id(&self) -> BufferId {
        self.editor
            .state
            .active_buffer
            .expect("there is always an active buffer")
    }

    /// Show a message in the echo area and record it in *Messages*, like
    /// `message`.
    pub fn msg(&mut self, text: impl Into<String>) {
        let text = text.into();
        crate::messages::log(
            self,
            "echo",
            forge_core::plugin::runtime::Level::Info,
            &text,
        );
        self.echo(text);
    }

    /// Show a message in the echo area only, without recording it (the
    /// display side of `message` when `inhibit-message` is set).
    pub fn echo(&mut self, text: impl Into<String>) {
        if !self.inhibit_message {
            self.message = Some(text.into());
        }
    }

    /// Show `text` in the echo area while `f` runs, restoring the previous
    /// message after.
    pub fn with_temp_message<T>(
        &mut self,
        text: impl Into<String>,
        f: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let saved = self.message.take();
        self.message = Some(text.into());
        let out = f(self);
        self.message = saved;
        out
    }

    /// Report a failure. Like bell, it stops a running keyboard macro.
    pub fn error(&mut self, text: impl Into<String>) {
        let text = text.into();
        crate::messages::log(
            self,
            "forge",
            forge_core::plugin::runtime::Level::Error,
            &text,
        );
        self.echo(text);
        self.bell = true;
    }

    pub fn prompt(&mut self, prompt: impl Into<String>, purpose: Purpose) {
        self.minibuffer = Some(Minibuffer::new(prompt, purpose));
        // minibuffer.on-open: run plugin hooks once the prompt is up, after
        // this key (or plugin call) has finished.
        for (_, command) in self.editor.completion.minibuffer_open_commands.clone() {
            self.deferred_commands.push((command, None));
        }
    }

    /// Mirror the open prompt into the core editor, so a plugin can read it
    /// (minibuffer.active / .history-kind / .input).
    fn sync_prompt(&mut self) {
        match self.minibuffer.as_ref() {
            Some(mb) => {
                self.editor.prompt_open = true;
                self.editor.prompt_history =
                    minibuffer::prompt_history_kind(mb).map(str::to_string);
                self.editor.prompt_input = mb.current_input();
            }
            None => {
                self.editor.prompt_open = false;
                self.editor.prompt_history = None;
                self.editor.prompt_input.clear();
            }
        }
    }

    // ------------------------------------------------------ prefix arg

    /// The numeric prefix argument, 1 if none.
    pub fn count(&self) -> i64 {
        self.prefix_arg.map_or(1, PrefixArg::value)
    }

    pub fn has_arg(&self) -> bool {
        self.prefix_arg.is_some()
    }

    /// Plain C-u (not a number).
    pub fn raw_arg(&self) -> bool {
        matches!(self.prefix_arg, Some(PrefixArg::Raw(_)))
    }

    /// C-u: multiply by 4, or end the digits typed so far.
    pub(crate) fn universal_argument(&mut self) {
        let mut st = self.arg_state.unwrap_or_default();
        if st.digits.is_some() || st.negative {
            st.closed = true;
        } else {
            // A count of C-u's can be huge: it stops at the largest
            // number there is, rather than overflowing.
            st.raw = if st.raw == 0 {
                4
            } else {
                st.raw.saturating_mul(4)
            };
        }
        self.echo(st.echo());
        self.arg_state = Some(st);
    }

    /// M-0 .. M-9 (and digits after C-u).
    pub(crate) fn digit_argument(&mut self, d: i64) {
        let mut st = self.arg_state.unwrap_or_default();
        st.digits = Some(st.digits.unwrap_or(0).saturating_mul(10).saturating_add(d));
        self.echo(st.echo());
        self.arg_state = Some(st);
    }

    /// M-- (and - right after C-u).
    pub(crate) fn negative_argument(&mut self) {
        let mut st = self.arg_state.unwrap_or_default();
        st.negative = !st.negative;
        self.echo(st.echo());
        self.arg_state = Some(st);
    }

    // ------------------------------------------------------------- keys

    /// Nothing half-typed: the next key starts a new command.
    pub(crate) fn at_top_level(&self) -> bool {
        self.pending.is_empty()
            && !self.meta_pending
            && self.minibuffer.is_none()
            && self.key_reader.is_none()
            && self.arg_state.is_none()
            && self.quoted.is_none()
            && self.query.is_none()
    }

    /// A paste arriving as text, not as keys: from the macOS app, a
    /// terminal's bracketed paste, an input method. One character is
    /// typed like a key (electric pairs etc.); several are inserted as
    /// is, in one undo step, so a paste can be undone at once.
    pub fn paste_text(&mut self, t: &str) {
        let mut chars = t.chars();
        match (chars.next(), chars.next()) {
            // One character: typed, like a key (electric pairs etc.).
            (Some(c), None) if c != '\n' && c != '\r' => {
                self.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
            }
            // One newline: the RET a terminal sends for it.  A prompt
            // takes no newline of its own, so it is the key that ends it.
            (Some('\n' | '\r'), None) if self.minibuffer.is_some() => {
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
            // Several (a paste, an input method): inserted as is,
            // one undo step.
            (Some(_), _) if self.minibuffer.is_none() => {
                let text = t.replace("\r\n", "\n").replace('\r', "\n");
                self.buf().insert_str(&text);
                self.buf().break_undo_group();
            }
            _ => {
                for c in t.chars().filter(|c| *c != '\n' && *c != '\r') {
                    self.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
                }
            }
        }
    }

    pub fn handle_key(&mut self, mut key: KeyEvent) {
        let prompt_input_before = self.minibuffer.as_ref().map(Minibuffer::current_input);
        // Modifier-bar buttons apply to the next key, before it is recorded
        // or looked up applies a modifier typed on the bar.
        crate::toolbar::apply_pending(self, &mut key);
        self.show_prefix_hints = false;
        self.note_input();
        if let Some(d) = describe_key(&key, false) {
            if self.lossage.len() >= 300 {
                self.lossage.pop_front();
            }
            self.lossage.push_back(d);
        }
        let is_cg = key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('g');
        if !self.editor.kmacro.executing {
            if is_cg && self.editor.kmacro.recording.is_some() {
                // C-g abandons a macro being defined.
                self.editor.kmacro.recording = None;
            }
            let top = self.at_top_level();
            if let Some(desc) = crate::kmacro::record_key(key)
                && let Some(rec) = self.editor.kmacro.recording.as_mut()
            {
                if top {
                    self.editor.kmacro.seq_start = rec.len();
                }
                rec.push(desc);
            }
        }

        if let Some((plugin, request)) = self.plugin_key.take() {
            let desc = if is_cg {
                None
            } else {
                describe_key(&key, false)
            };
            self.message = None;
            if desc.is_none() {
                self.msg("Quit");
            }
            plugins::answer(self, &plugin, request, desc.as_deref());
        } else if let Some(q) = self.quoted.take() {
            self.quoted_key(q, key);
        } else if let Some(k) = crate::iso_transl::filter(self, key) {
            if k == key {
                // The input method's turn (C-\, C-x \), then the keymaps'.
                crate::input_method::feed(self, k);
            } else {
                // C-x 8 made a character: key translations come after the
                // input method.
                self.route_key(k);
            }
        }
        plugins::after_key(self);
        crate::capf::after_key(self);
        let prompt_input_after = self.minibuffer.as_ref().map(Minibuffer::current_input);
        crate::capf::minibuffer_input_changed(
            self,
            prompt_input_before.as_deref(),
            prompt_input_after.as_deref(),
        );
        crate::files::ask_about_locks(self);
        if !self.editor.kmacro.executing {
            crate::files::count_input(self);
        }
        crate::vc::ensure_state(self);
        self.run_mode_hooks();
        self.show_decoration_message();
        self.editor.echo_area = self.message.clone();
    }

    /// Run `cmd` with buffer `id` current and a prefix argument, as Forge
    /// runs a hook function: what was being typed (a prefix argument, the
    /// last command, the echo area) is as it was afterwards.
    pub fn run_in_buffer(&mut self, id: BufferId, cmd: &str, arg: Option<i64>, quiet: bool) {
        let saved = (
            self.arg_state.take(),
            self.forced_arg.take(),
            self.prefix_arg.take(),
            self.last_command.clone(),
            self.this_command.clone(),
            std::mem::take(&mut self.this_command_keys),
            self.message.clone(),
            self.last_kill_backward,
        );
        let previous = self.editor.state.active_buffer;
        self.editor.state.active_buffer = Some(id);
        self.forced_arg = arg.map(PrefixArg::Number);
        self.run_command(cmd);
        if previous.is_some_and(|p| self.editor.state.buffers.contains_key(&p)) {
            self.editor.state.active_buffer = previous;
        }
        let (arg_state, forced, prefix, last, this, keys, message, kill_backward) = saved;
        self.arg_state = arg_state;
        self.forced_arg = forced;
        self.prefix_arg = prefix;
        self.last_command = last;
        self.last_kill_backward = kill_backward;
        self.this_command = this;
        self.this_command_keys = keys;
        if quiet && !self.bell {
            self.message = message;
        }
    }

    /// A problem with init.rhai found later: in *Messages* and the echo area.
    fn init_warning(&mut self, text: String) {
        plugins::log(
            self,
            forge_core::rhai::SOURCE,
            forge_core::plugin::runtime::Level::Warn,
            &text,
        );
        self.echo(format!("init.rhai: {text}"));
        self.bell = true;
    }

    /// Are plugins still to load (waiting for permission, or queued)?
    fn plugins_pending(&self) -> bool {
        !self.pending_loads.is_empty() || self.pending_grant.is_some()
    }

    /// forge.mode.hook and init.rhai's forge.command.run of plugin
    /// commands: run what can run now. Called after each key, when idle
    /// and once the plugins are loaded.
    pub fn run_mode_hooks(&mut self) {
        if self.running_hooks || (self.mode_hooks.is_empty() && self.deferred_commands.is_empty()) {
            return;
        }
        self.running_hooks = true;
        // init.rhai's commands, in order, once each exists.
        let mut later = Vec::new();
        for (cmd, arg) in std::mem::take(&mut self.deferred_commands) {
            if self.editor.commands.contains(&cmd) {
                let id = self.active_id();
                self.run_in_buffer(id, &cmd, arg, false);
            } else if self.plugins_pending() {
                later.push((cmd, arg));
            } else {
                self.init_warning(format!(
                    "forge.command.run: no command named {cmd} (no plugin defined it)"
                ));
            }
        }
        self.deferred_commands.extend(later);
        let hooks = self.mode_hooks.clone();
        for id in self.editor.buffer_ids() {
            let Some(mode) = self
                .editor
                .state
                .get_buffer(id)
                .map(|b| b.major_mode().to_string())
            else {
                continue;
            };
            let entry = self
                .mode_hooks_done
                .entry(id)
                .or_insert_with(|| (mode.clone(), Default::default()));
            if entry.0 != mode {
                *entry = (mode.clone(), Default::default());
            }
            // As run-mode-hooks: the most basic parent's hook first,
            // the mode's own last; within one mode, the hook added last
            // first (add-hook puts it at the front).
            let mut chain = vec![mode.clone()];
            while let Some(parent) = chain
                .last()
                .and_then(|m| self.editor.modes.get(m))
                .and_then(|d| d.parent.clone())
                .filter(|p| !chain.contains(p))
            {
                chain.push(parent);
            }
            chain.reverse();
            let mut order: Vec<(usize, std::cmp::Reverse<usize>)> = hooks
                .iter()
                .enumerate()
                .filter_map(|(i, (m, _))| {
                    chain
                        .iter()
                        .position(|c| c == m)
                        .map(|d| (d, std::cmp::Reverse(i)))
                })
                .collect();
            order.sort();
            for (_, std::cmp::Reverse(i)) in order {
                let (hook_mode, cmd) = &hooks[i];
                let done = self
                    .mode_hooks_done
                    .get(&id)
                    .is_some_and(|(_, d)| d.contains(&i));
                if done {
                    continue;
                }
                if !self.editor.commands.contains(cmd) {
                    if !self.plugins_pending() && self.hooks_warned.insert(cmd.clone()) {
                        self.init_warning(format!(
                            "forge.mode.hook({hook_mode:?}): no command named {cmd}"
                        ));
                    }
                    continue;
                }
                if let Some((_, d)) = self.mode_hooks_done.get_mut(&id) {
                    d.insert(i);
                }
                self.run_in_buffer(id, cmd, Some(1), true);
            }
        }
        self.mode_hooks_done
            .retain(|id, _| self.editor.state.buffers.contains_key(id));
        self.running_hooks = false;
    }

    /// The prefix argument being typed, as the echo area shows it before
    /// a key sequence ("C-u 3 "), or nothing.
    pub fn arg_echo(&self) -> String {
        self.arg_state
            .map(|st| format!("{} ", st.echo().trim_end_matches('-')))
            .unwrap_or_default()
    }

    /// Send a key to whatever is taking keys: the completion popup, a
    /// question, the key reader, the minibuffer, the menu, or the keymaps.
    pub fn route_key(&mut self, key: KeyEvent) {
        if self.one_key.is_some() {
            crate::one_key::handle_key(self, key);
        } else if self.query.is_none()
            && self.key_reader.is_none()
            && self.minibuffer.is_none()
            && crate::capf::handle_key(self, key)
        {
            // The completion popup took it.
        } else if self.query.is_some() {
            crate::query::handle_key(self, key);
        } else if self.key_reader.is_some() {
            self.reader_key(key);
        } else if self.minibuffer.is_some() {
            minibuffer::handle_key(self, key);
        } else if crate::menu::handle_key(self, key) {
            // An open menu took it.
        } else if crate::term::handle_key(self, key) {
            // A terminal buffer sent it to its program.
        } else {
            self.dispatch_key(key);
        }
    }

    /// Handle keys ("C-x C-f", "d w") as if typed (keymap.execute-keys).
    pub fn execute_keys(&mut self, keys: &str) {
        for desc in keys.split_whitespace() {
            match keymap::key_event(desc) {
                Some(k) => self.handle_key(k),
                None => return self.error(format!("Can't type {desc}")),
            }
        }
    }

    /// With nothing else in the echo area, say what a decoration under
    /// point means ("unknown word", a compiler error).
    fn show_decoration_message(&mut self) {
        if self.message.is_some() || self.minibuffer.is_some() || self.query.is_some() {
            return;
        }
        let b = self.buf_ref();
        if let Some(m) = b.decoration_message_at(b.point_offset()) {
            self.message = Some(m.to_string());
        }
    }

    /// Turn a key event into key notation, handling ESC as a Meta prefix
    /// (ESC f == M-f). `echo` is what the echo area shows before "ESC-".
    fn read_key(&mut self, key: KeyEvent, echo: &str) -> KeyInput {
        // ESC bound on its own (a modal plugin's insert state leaves with
        // it) is a key, not the Meta prefix.
        if key.code == KeyCode::Esc
            && key.modifiers.is_empty()
            && !self.meta_pending
            && self.pending.is_empty()
            && self.key_reader.is_none()
            && matches!(
                self.editor.active_keymaps().lookup(&["ESC".to_string()]),
                Lookup::Command { .. }
            )
        {
            return KeyInput::Key("ESC".into());
        }
        if key.code == KeyCode::Esc && key.modifiers.is_empty() {
            if self.meta_pending {
                self.meta_pending = false;
                self.message = None;
                return KeyInput::Cancelled;
            }
            self.meta_pending = true;
            self.echo(format!("{echo}ESC-"));
            return KeyInput::Consumed;
        }
        let meta = std::mem::take(&mut self.meta_pending);
        match describe_key(&key, meta) {
            Some(desc) => KeyInput::Key(desc),
            None => KeyInput::Consumed,
        }
    }

    /// Feed one key through the active keymaps.
    ///
    /// The conditional layers are answered from a snapshot taken once here,
    /// before the key is resolved: `Syntax::faces` needs `&mut` and the git
    /// check walks the filesystem, so neither can be asked from inside the
    /// lookup, which only has `&self`.
    pub fn dispatch_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let is_cg = ctrl && key.code == KeyCode::Char('g');

        // C-g cancels a half-typed key sequence or prefix argument.
        if is_cg && (self.meta_pending || !self.pending.is_empty() || self.arg_state.is_some()) {
            self.meta_pending = false;
            self.pending.clear();
            self.arg_state = None;
            self.msg("Quit");
            return;
        }

        // Digits and "-" right after C-u belong to the argument.
        if let (Some(st), true, false) =
            (self.arg_state, self.pending.is_empty(), self.meta_pending)
            && let KeyCode::Char(c) = key.code
            && !ctrl
            && !alt
            && !st.closed
        {
            if let Some(d) = c.to_digit(10) {
                return self.digit_argument(d as i64);
            }
            if c == '-' && st.digits.is_none() && !st.negative {
                return self.negative_argument();
            }
        }

        // After C-x e, a plain e runs the macro again.
        if self.pending.is_empty()
            && !self.meta_pending
            && self.arg_state.is_none()
            && self.last_command == "kmacro-end-and-call-macro"
            && key.code == KeyCode::Char('e')
            && !ctrl
            && !alt
        {
            self.this_command_keys = vec!["e".into()];
            return self.run_command("kmacro-end-and-call-macro");
        }

        // Typing in the text browser's text box edits the box, like eww:
        // plain keys there never reach few-mode's single-letter commands.
        if self.pending.is_empty()
            && !self.meta_pending
            && self.arg_state.is_none()
            && crate::few::handle_key(self, &key)
        {
            return;
        }

        let mut echo = describe_key_sequence(&self.pending);
        if !echo.is_empty() {
            echo.push(' ');
        }
        let meta = self.meta_pending;
        let desc = match self.read_key(key, &echo) {
            KeyInput::Key(d) => d,
            KeyInput::Consumed => return,
            KeyInput::Cancelled => {
                self.pending.clear();
                return;
            }
        };
        if self.arg_state.is_none() {
            self.message = None;
        }
        self.pending.push(desc.clone());
        // A transient map's exit policy takes the key and hands it back:
        // what it means can be different now that the map is gone.
        while self.resolve_pending(Some(key.code), !ctrl && !meta && !alt) {
            self.pending.push(desc.clone());
        }
    }

    /// A key that wasn't typed: a click, dispatched through the keymaps
    /// like any other, so a region map can bind it. Point is already where
    /// the click landed.
    pub fn dispatch_mouse(&mut self, desc: &str) {
        // A click abandons a half-typed sequence: it isn't the next key of
        // one, and leaving it pending would make the click mean whatever
        // those keys meant.
        self.pending.clear();
        self.message = None;
        self.pending.push(desc.to_string());
        // A click that no map binds is not an error, so it never inserts
        // anything either.
        while self.resolve_pending(None, true) {
            self.pending.push(desc.to_string());
        }
    }

    /// Look the pending sequence up and run what it means. `code` is None
    /// for a click, which no map need bind. True when a transient map's
    /// exit policy took the key, so the caller should look it up again
    /// (the sequence is cleared by then).
    fn resolve_pending(&mut self, code: Option<KeyCode>, plain: bool) -> bool {
        // While a transient map is up, a key that doesn't reach it means whatever
        // that map's policy says: pass it down, swallow it, or take the map
        // down and resolve the key again in what's left. A prefix key is
        // still on its way to something, so it never counts as unbound.
        if !self.editor.transients.is_empty() {
            let reached_transient = match self.editor.active_keymaps().lookup(&self.pending) {
                Lookup::Command { scope, .. } => matches!(scope, Scope::Transient(_)),
                Lookup::Prefix => true,
                Lookup::Undefined => false,
            };
            if !reached_transient {
                match self.editor.transient_policy() {
                    OnUnbound::Stay => {}
                    OnUnbound::Swallow => {
                        self.pending.clear();
                        self.arg_state = None;
                        return false;
                    }
                    OnUnbound::Exit => {
                        self.editor.pop_all_transients();
                        // Start the key over: what it means can be different
                        // now that the map that hid it is gone.
                        self.pending.clear();
                        self.arg_state = None;
                        return true;
                    }
                }
            }
        }

        match self.editor.active_keymaps().lookup(&self.pending) {
            Lookup::Command { command, scope, .. } => {
                // A one-shot map goes away once it has run something.
                if let Scope::Transient(name) = &scope {
                    let one_shot = self
                        .editor
                        .transients
                        .iter()
                        .find(|t| &t.name == name)
                        .is_some_and(|t| t.one_shot);
                    if one_shot {
                        self.editor.pop_transient(name);
                    }
                }
                self.this_command_keys = std::mem::take(&mut self.pending);
                // The echo of the keys typed so far ("C-u C-x-") is done
                // with once they run something.
                if self.message.as_deref().is_some_and(|m| m.ends_with('-')) {
                    self.message = None;
                }
                self.run_command(&command);
            }
            Lookup::Prefix => {
                let prefix = self
                    .arg_state
                    .map(|st| format!("{} ", st.echo().trim_end_matches('-')))
                    .unwrap_or_default();
                self.echo(format!("{prefix}{}-", describe_key_sequence(&self.pending)));
            }
            Lookup::Undefined => {
                // C-h after a prefix lists what can follow it (unless bound).
                if self.pending.len() > 1 && self.pending.last().is_some_and(|k| k == "C-h") {
                    self.pending.pop();
                    self.show_prefix_hints = true;
                    self.echo(format!("{}-", describe_key_sequence(&self.pending)));
                    return false;
                }
                let seq = std::mem::take(&mut self.pending);
                match code {
                    Some(KeyCode::Char(_)) if plain && seq.len() == 1 => {
                        self.this_command_keys = seq;
                        self.run_command("self-insert-command");
                    }
                    // A click that no map binds has already done its job:
                    // it moved point.
                    None => {}
                    _ => {
                        self.arg_state = None;
                        self.error(format!("{} is undefined", describe_key_sequence(&seq)));
                    }
                }
            }
        }
        false
    }

    pub fn run_command(&mut self, cmd: &str) {
        self.sync_prompt();
        let is_arg_cmd = matches!(
            cmd,
            "universal-argument" | "digit-argument" | "negative-argument"
        );
        if !is_arg_cmd {
            self.prefix_arg = self
                .forced_arg
                .take()
                .or_else(|| self.arg_state.take().map(ArgState::finish));
        }
        // The name of the command that is running. A command may change it
        // (set-mark-command with a prefix argument becomes pop-to-mark-command,
        // rewrites `this-command'), and `last-command' takes
        // the name it ended up with. It is kept apart from `this_command',
        // which a command running inside this one (a keyboard macro) sets
        // for itself.
        let outer_name = std::mem::replace(&mut self.command_name, cmd.to_string());
        self.this_command = cmd.to_string();
        self.highlight = None;
        self.buf().take_refused();
        if !commands::execute(self, cmd) {
            self.error(format!("Unknown command: {cmd}"));
        }
        // A command a plugin ran may have queued popup requests
        // (completion.next, .accept, ...).
        crate::capf::pump(self);
        self.report_refused_edit();
        if is_arg_cmd {
            // Prefix arguments are invisible to last-command (C-k C-u C-k still appends).
            self.command_name = outer_name;
            return;
        }
        // Typed characters undo in runs; everything else is one step each.
        if cmd != "self-insert-command" || self.prefix_arg.is_some() {
            self.buf().break_undo_group();
        }
        self.prefix_arg = None;
        self.last_command = self.command_name.clone();
        self.command_name = outer_name;
        self.after_command();
    }

    /// Rename the command that is running rewrites
    /// `this-command': `last-command' takes the new name when the command
    /// finishes.
    pub fn rename_command(&mut self, name: &str) {
        self.command_name = name.to_string();
        self.this_command = name.to_string();
    }

    /// A command tried to change a locked buffer (the `read-only` option):
    /// say so, the way barf-if-buffer-read-only does.
    fn report_refused_edit(&mut self) {
        if self.buf().take_refused() {
            let name = self.buf_ref().name().to_string();
            self.error(format!("Buffer is read-only: {name}"));
        }
    }

    fn after_command(&mut self) {
        crate::vc::after_command(self);
        crate::canvas::sync(self);
        commands::auto_fill(self);
        crate::tutorial::after_command(self);
        crate::window_extras::after_command(self);
        crate::files::ask_about_locks(self);
        // An Image-mode buffer's picture is rendered on demand; the
        // transform commands refresh it themselves.
        if self.buf_ref().major_mode() == crate::image_mode::MODE
            && self
                .buf_ref()
                .image
                .as_ref()
                .is_some_and(|s| s.rendered.is_none())
        {
            crate::image_mode::refresh(self);
        }
    }

    // ---------------------------------------------------- quoted insert

    pub fn start_quoted_insert(&mut self) {
        let count = self.count().max(0) as usize;
        self.quoted = Some(QuotedInsert {
            count,
            octal: String::new(),
        });
        self.msg("C-q-");
    }

    fn quoted_key(&mut self, mut q: QuotedInsert, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let c = match key.code {
            KeyCode::Char(d @ '0'..='7') if !ctrl => {
                q.octal.push(d);
                if q.octal.len() < 3 {
                    self.msg(format!("C-q {}-", q.octal));
                    self.quoted = Some(q);
                    return;
                }
                u32::from_str_radix(&q.octal, 8)
                    .ok()
                    .and_then(char::from_u32)
            }
            _ if !q.octal.is_empty() => {
                // Anything else ends the octal code; RET is used up by it.
                let c = u32::from_str_radix(&q.octal, 8)
                    .ok()
                    .and_then(char::from_u32);
                self.insert_quoted(c, q.count);
                if key.code != KeyCode::Enter {
                    self.handle_key(key);
                }
                return;
            }
            KeyCode::Char(' ') | KeyCode::Char('@') | KeyCode::Char('2') if ctrl => Some('\0'),
            KeyCode::Char(c) if ctrl && c.is_ascii_alphabetic() => {
                char::from_u32(c.to_ascii_lowercase() as u32 & 0x1f)
            }
            KeyCode::Char(c) => Some(c),
            KeyCode::Enter => Some('\r'),
            KeyCode::Tab => Some('\t'),
            KeyCode::Backspace => Some('\u{7f}'),
            KeyCode::Esc => Some('\u{1b}'),
            _ => None,
        };
        self.insert_quoted(c, q.count);
    }

    fn insert_quoted(&mut self, c: Option<char>, count: usize) {
        self.message = None;
        match c {
            Some(c) => {
                let s: String = std::iter::repeat_n(c, count).collect();
                self.buf().insert_str(&s);
                self.buf().break_undo_group();
                self.report_refused_edit();
            }
            None => self.error("C-q can't insert that key"),
        }
        self.last_command = "quoted-insert".into();
    }

    // ------------------------------------------------- reading key sequences

    /// Start reading a key sequence for C-h k, C-h c, global-set-key, ...
    pub fn read_key_sequence(&mut self, purpose: ReadKeyFor) {
        self.key_reader = Some(KeyReader {
            purpose,
            keys: Vec::new(),
        });
        self.msg(purpose.prompt());
    }

    fn reader_key(&mut self, key: KeyEvent) {
        let Some(mut r) = self.key_reader.take() else {
            return;
        };
        let is_cg = key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('g');
        // C-h k C-g describes C-g; setting a key can be cancelled.
        if is_cg && (self.meta_pending || matches!(r.purpose, ReadKeyFor::SetKey { .. })) {
            self.meta_pending = false;
            self.msg("Quit");
            return;
        }

        let mut echo = format!("{}{}", r.purpose.prompt(), describe_key_sequence(&r.keys));
        if !r.keys.is_empty() {
            echo.push(' ');
        }
        let meta = self.meta_pending;
        match self.read_key(key, &echo) {
            KeyInput::Key(d) => r.keys.push(d),
            KeyInput::Consumed => {
                self.key_reader = Some(r);
                return;
            }
            KeyInput::Cancelled => {
                self.msg("Quit");
                return;
            }
        }

        // C-x 8 ' e is a key translation (iso_transl), not a binding.
        match crate::iso_transl::describe_keys(self, &r.keys) {
            Some(crate::iso_transl::Step::Prefix) => {
                self.msg(format!(
                    "{}{}-",
                    r.purpose.prompt(),
                    describe_key_sequence(&r.keys)
                ));
                self.key_reader = Some(r);
                return;
            }
            Some(crate::iso_transl::Step::Char(c)) => {
                self.message = None;
                let keys = describe_key_sequence(&r.keys);
                let name = crate::iso_transl::char_name(c).unwrap_or_default();
                let text = format!(
                    "{keys} is translated to {c} ({name}, U+{:04X}) by C-x 8's key translations, and {c} runs the command self-insert-command",
                    c as u32
                );
                match r.purpose {
                    ReadKeyFor::SetKey { .. } => self.error(format!(
                        "{keys} is a C-x 8 key translation, not a key to bind"
                    )),
                    ReadKeyFor::DescribeKeyBriefly => self.msg(text),
                    ReadKeyFor::DescribeKey => {
                        let doc = help::doc_of(self, "self-insert-command");
                        self.show_special_buffer(
                            "*Help*",
                            "help-mode",
                            &format!("{text}.\n\n{doc}"),
                        );
                    }
                }
                return;
            }
            _ => {}
        }

        let lookup = self.editor.active_keymaps_now().lookup(&r.keys);
        // C-c is the user's prefix even before anything is bound under it.
        let is_prefix = lookup == Lookup::Prefix
            || (r.keys.len() == 1 && r.keys[0] == "C-c" && lookup == Lookup::Undefined);
        if is_prefix {
            self.msg(format!(
                "{}{}-",
                r.purpose.prompt(),
                describe_key_sequence(&r.keys)
            ));
            self.key_reader = Some(r);
            return;
        }

        self.message = None;
        let keys = describe_key_sequence(&r.keys);
        let self_inserting = !meta
            && r.keys.len() == 1
            && matches!(key.code, KeyCode::Char(_))
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match r.purpose {
            ReadKeyFor::SetKey { local } => {
                let where_ = if local { "locally" } else { "globally" };
                self.prompt(
                    format!("Set key {keys} {where_} to command: "),
                    Purpose::SetKeyCommand {
                        keys: r.keys,
                        local,
                    },
                );
            }
            ReadKeyFor::DescribeKeyBriefly => {
                let text = match &lookup {
                    Lookup::Command {
                        command,
                        remapped_from: Some(from),
                        ..
                    } => {
                        format!("{keys} runs the command {command} (remapped from {from})")
                    }
                    Lookup::Command { command, .. } => format!("{keys} runs the command {command}"),
                    _ if self_inserting => format!("{keys} runs the command self-insert-command"),
                    _ => format!("{keys} is undefined"),
                };
                self.msg(text);
            }
            ReadKeyFor::DescribeKey => match lookup {
                Lookup::Undefined if !self_inserting => self.msg(format!("{keys} is undefined")),
                _ => help::describe_key(self, &keys, &lookup),
            },
        }
    }

    // ------------------------------------------------------ prefix hints

    /// The prefix typed so far, in the editor or while reading a key sequence.
    pub fn current_prefix(&self) -> &[String] {
        match &self.key_reader {
            Some(r) => &r.keys,
            None if self.minibuffer.is_none() => &self.pending,
            None => &[],
        }
    }

    /// (next key, what it runs) for the current prefix. Sub-prefixes show
    /// as "+prefix".
    pub fn prefix_hints(&self) -> Vec<(String, String)> {
        let mut hints: Vec<(String, String)> = self
            .editor
            .active_keymaps_now()
            .continuations(self.current_prefix())
            .into_iter()
            .map(|(k, c)| match c {
                Continuation::Command(cmd) => (k, cmd),
                Continuation::Prefix => (k, "+prefix".to_string()),
            })
            .collect();
        // After C-x 8, the translations go on too.
        let prefix = self.current_prefix();
        if prefix.len() >= 2 && prefix[0] == "C-x" && prefix[1] == "8" {
            for (k, what) in crate::iso_transl::continuations(&prefix[2..], self.iso.language) {
                if !hints.iter().any(|(h, _)| *h == k) {
                    hints.push((k, what));
                }
            }
        }
        hints
    }

    /// A transient map that owns the keyboard while it is up (on-unbound
    /// `swallow`: a keypad, a popup of commands) lists its keys at once,
    /// the way a prefix's continuations show after a pause: the map's
    /// name and (keys, what they do). Each command is described by the
    /// first sentence of its doc, falling back to its name. None while a
    /// prefix is being typed (its own hints show then).
    pub fn keypad_hints(&self) -> Option<(String, Vec<(String, String)>)> {
        if !self.current_prefix().is_empty() {
            return None;
        }
        let t = self
            .editor
            .transients
            .iter()
            .rev()
            .find(|t| t.on_unbound == OnUnbound::Swallow)?;
        let map = self
            .editor
            .keymaps
            .get(&forge_core::Scope::Transient(t.name.clone()))?;
        let hints: Vec<(String, String)> = map
            .bindings()
            .map(|(keys, cmd)| {
                let what = self
                    .editor
                    .commands
                    .get(cmd)
                    .map(|c| {
                        c.doc
                            .split(". ")
                            .next()
                            .unwrap_or("")
                            .trim()
                            .trim_end_matches('.')
                            .to_string()
                    })
                    .filter(|d| !d.is_empty())
                    .unwrap_or_else(|| cmd.to_string());
                (keys.to_string(), what)
            })
            .collect();
        (!hints.is_empty()).then(|| (t.name.clone(), hints))
    }

    /// Move the selected window's top to buffer line `target`. With
    /// smooth scrolling on, this only starts an animation: point follows
    /// one visible line per tick (see [`App::tick_scroll`]). Off, the jump
    /// is immediate and point is dragged along if it would leave.
    pub(crate) fn set_scroll_target(&mut self, target: usize) {
        self.win_mut().scroll_y_px = 0;
        if self.smooth_scroll {
            let id = self.editor.windows.selected_id();
            self.scroll_anim = Some((id, target));
        } else {
            self.win_mut().scroll_top = target;
            commands::keep_point_in_window(self);
        }
    }

    /// Put buffer line `line` at the top of the selected window at once,
    /// dragging point along if it would leave (scroll-bar dragging, and
    /// the macOS scroller's jump). Cancels any smooth scroll in flight.
    pub fn scroll_to_line(&mut self, line: usize) {
        self.scroll_to_line_pixels(line, 0);
    }

    /// Like [`App::scroll_to_line`], but leaves `pixels` of the line
    /// clipped off the top: a scroll bar dragged between two lines.
    pub fn scroll_to_line_pixels(&mut self, line: usize, pixels: usize) {
        self.scroll_anim = None;
        let last = self.buf_ref().line_count().saturating_sub(1);
        let line = line.min(last);
        let w = self.win_mut();
        w.scroll_top = line;
        w.scroll_y_px = if line == last { 0 } else { pixels };
        w.recenter_step = 0;
        commands::keep_point_in_window(self);
    }

    /// Move the selected window by `delta` pixels, crossing visible lines
    /// as the offset passes a whole row (pixel-scroll-precision-mode).
    /// Positive `delta` scrolls toward the end of the buffer. `row_height`
    /// is the frontend's text row height in pixels.
    pub fn scroll_pixels(&mut self, delta: i32, row_height: u32) {
        if delta == 0 || row_height == 0 {
            return;
        }
        self.scroll_anim = None;
        let row_height = i64::from(row_height);
        let mut top = self.win().scroll_top;
        let mut px = self.win().scroll_y_px as i64 + i64::from(delta);
        while px >= row_height {
            let next = self.buf_ref().forward_visible_line(top, 1);
            if next == top {
                px = 0;
                break;
            }
            top = next;
            px -= row_height;
        }
        while px < 0 {
            let prev = self.buf_ref().backward_visible_line(top, 1);
            if prev == top {
                px = 0;
                break;
            }
            top = prev;
            px += row_height;
        }
        let w = self.win_mut();
        w.scroll_top = top;
        w.scroll_y_px = px as usize;
        w.recenter_step = 0;
        commands::keep_point_in_window(self);
    }

    /// Scroll the selected window sideways to display column `col` as the
    /// leftmost column (scroll-bar dragging). Typing or moving point
    /// brings point back into view.
    pub fn scroll_to_column(&mut self, col: usize) {
        self.scroll_anim = None;
        self.win_mut().hscroll = col;
        self.win_mut().recenter_step = 0;
    }

    /// A smooth scroll is waiting to run: the loop should wake for it.
    pub fn scrolling(&self) -> bool {
        self.scroll_anim.is_some()
    }

    /// One step of a smooth scroll: move the selected window a single
    /// visible line toward its target and keep point in the window. Does
    /// nothing (and cancels the animation) once the target is reached, the
    /// window changed, or the target window is no longer selected.
    pub fn tick_scroll(&mut self) {
        let Some((id, target)) = self.scroll_anim else {
            return;
        };
        if id != self.editor.windows.selected_id() {
            self.scroll_anim = None;
            return;
        }
        let top = self.win().scroll_top;
        if top == target {
            self.scroll_anim = None;
            return;
        }
        let next = if top < target {
            self.buf_ref().forward_visible_line(top, 1).min(target)
        } else {
            self.buf_ref().backward_visible_line(top, 1).max(target)
        };
        self.win_mut().scroll_top = next;
        commands::keep_point_in_window(self);
        if next == target {
            self.scroll_anim = None;
        }
    }

    /// How long the event loop should wait before calling [`App::on_idle`],
    /// or None to block until the next key.
    pub fn idle_timeout(&self) -> Option<Duration> {
        let waiting = !self.current_prefix().is_empty() && !self.show_prefix_hints;
        let hints =
            (waiting && !self.prefix_help_delay.is_zero()).then_some(self.prefix_help_delay);
        let unsaved = self
            .editor
            .state
            .buffers
            .values()
            .any(Buffer::needs_auto_save);
        // Wake when the idle time is up, counted from the last key.
        let auto_save =
            (unsaved && self.auto_save_default && !self.auto_save_idle.is_zero()).then(|| {
                self.auto_save_idle
                    .saturating_sub(self.last_input.elapsed())
                    .max(Duration::from_millis(10))
            });
        let events = plugins::changes_pending(self).then_some(plugins::CHANGE_EVENT_DELAY);
        let reload = plugins::watching_files(self).then_some(plugins::RELOAD_POLL);
        let jobs = (self.jobs.any_running()
            || self.vc.runner.any_running()
            || plugins::processes_running(self)
            || self.rhai_fs.any_running()
            || self.rhai_http.any_running()
            || self.packages.needs_poll()
            || self.few.needs_poll()
            || self.server.is_some()
            || self.term.needs_poll())
        .then_some(forge_core::process::POLL_INTERVAL);
        let completion = [crate::capf::auto_delay(self), crate::capf::echo_wait(self)]
            .into_iter()
            .flatten()
            .min();
        let idle = self.last_input.elapsed().as_secs_f64();
        let hooks = self
            .editor
            .idle_hooks
            .iter()
            .filter(|h| {
                !self
                    .idle_ran
                    .contains(&(h.plugin.clone(), h.command.clone()))
            })
            .map(|h| Duration::from_secs_f64((h.seconds - idle).max(0.01)))
            .min();
        // A smooth scroll steps on its own clock (one line per tick).
        let smooth = self.scrolling().then_some(SMOOTH_SCROLL_TICK);
        let image = crate::image_mode::animation_delay(self);
        // A canvas being drawn: look again for the picture soon.
        let canvas = crate::canvas::rendering(self).then_some(Duration::from_millis(16));
        [
            smooth, hints, auto_save, events, reload, jobs, completion, hooks, image, canvas,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// A key or click arrived: the idle timers start again, and any
    /// in-flight smooth scroll stops (a fresh scroll starts its own).
    pub fn note_input(&mut self) {
        self.last_input = std::time::Instant::now();
        self.idle_ran.clear();
        self.scroll_anim = None;
    }

    /// Run the plugins' idle hooks whose time has come.
    fn run_idle_hooks(&mut self) {
        if self.minibuffer.is_some() || self.editor.idle_hooks.is_empty() {
            return;
        }
        let idle = self.last_input.elapsed().as_secs_f64();
        let due: Vec<(String, String)> = self
            .editor
            .idle_hooks
            .iter()
            .filter(|h| idle >= h.seconds)
            .map(|h| (h.plugin.clone(), h.command.clone()))
            .filter(|k| !self.idle_ran.contains(k))
            .collect();
        for k in due {
            self.idle_ran.insert(k.clone());
            plugins::run_command(self, &k.1);
        }
    }

    /// No key arrived within [`App::idle_timeout`]: show prefix hints,
    /// auto-save modified buffers, notice files changed on disk.
    pub fn on_idle(&mut self) {
        self.tick_scroll();
        if !self.current_prefix().is_empty() {
            self.show_prefix_hints = true;
        }
        self.editor.echo_area = self.message.clone();
        crate::files::idle_auto_save(self);
        crate::org_flow::on_idle(self);
        crate::window_extras::on_idle(self);
        crate::image_mode::on_idle(self);
        crate::canvas::sync(self);
        self.run_mode_hooks();
        self.run_idle_hooks();
        self.check_visible_files();
        // A project asked about while a prompt was up gets its turn now.
        if self.minibuffer.is_none() && self.pending_grant.is_none() {
            crate::project::load_if_granted(self);
        }
        plugins::notice(self, true);
        plugins::reload_changed(self);
        crate::compile::poll(self);
        // The edits in the *grep* buffer go back into the files they
        // came from.
        crate::grep::sync_edits(self);
        crate::vc::runner::poll(self);
        plugins::poll_processes(self);
        plugins::poll_rhai_fs(self);
        plugins::poll_rhai_http(self);
        crate::packages::poll(self);
        crate::few::poll(self);
        crate::server::poll(self);
        crate::term::poll(self);
        crate::capf::on_idle(self);
        // A plugin or process may have modified a locked file's buffer.
        crate::files::ask_about_locks(self);
    }

    /// The terminal regained focus: another program may have changed files.
    pub fn on_focus(&mut self) {
        self.check_visible_files();
        crate::vc::on_focus(self);
    }

    /// Auto-save every modified file buffer now (do-auto-save), quietly:
    /// for when Forge is about to die.
    pub fn auto_save_all(&mut self) {
        crate::files::do_auto_save(self, true);
    }

    /// Check the files shown in windows for changes made by other programs.
    pub fn check_visible_files(&mut self) {
        let mut shown: Vec<BufferId> = self.editor.windows.iter().map(|w| w.buffer).collect();
        shown.sort();
        shown.dedup();
        for id in shown {
            self.check_file(id);
        }
    }

    /// A file changed on disk: reload an unmodified buffer (keeping point
    /// where it can), or warn about a modified one.
    pub fn check_file(&mut self, id: BufferId) {
        let Some(b) = self.editor.state.get_buffer_mut(id) else {
            return;
        };
        if !b.changed_on_disk() {
            return;
        }
        let name = b.name().to_string();
        if b.is_modified() {
            self.msg(format!(
                "{name} changed on disk; M-x revert-buffer to load it, or save to overwrite"
            ));
        } else if b.path().is_some_and(|p| p.exists()) {
            match b.revert() {
                Ok(()) => self.msg(format!("Reverted {name} (file changed on disk)")),
                Err(e) => self.error(format!("Can't reread {name}: {e}")),
            }
        }
    }

    /// Words added to the selected window's mode line.
    pub fn mode_line_extra(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.editor.kmacro.recording.is_some() {
            v.push("Def".to_string());
        }
        // Plugins working in the background, and running compilations.
        if let Some(h) = &self.plugin_host {
            v.extend(h.away_names().map(|n| format!("{n}...")));
        }
        if self
            .jobs
            .running
            .values()
            .any(|j| matches!(j.kind, crate::compile::JobKind::Compile))
        {
            v.push("Compiling".to_string());
        }
        if let Some(n) = crate::grep::match_count(self) {
            v.push(format!("Grep: {n}"));
        }
        v.extend(crate::org_flow::mode_line(self));
        v.extend(crate::vc::mode_line_extra(self));
        v
    }

    /// Waiting for a single key (quoted-insert).
    pub fn waiting_for_key(&self) -> bool {
        self.quoted.is_some()
    }

    // ------------------------------------------------------------ init.rhai

    /// Reset key bindings, settings and init.rhai's commands and modes to
    /// the defaults, then run init.rhai (if there is one) again. Plugins'
    /// bindings are kept; init.rhai's go on top of them. Buffer-local
    /// bindings are kept too. Errors go to the echo area and *Messages*.
    /// Reconcile the structured command docs with the command registry:
    /// add or refresh one per command (built-in, plugin or init.rhai) and
    /// drop docs for commands that are gone. Call this after anything that
    /// registers or unregisters commands.
    pub fn sync_docs(&mut self) {
        crate::docs_bridge::sync_command_docs(&mut self.docs, &self.editor.commands);
        for table in rich_command_tables() {
            crate::docs_bridge::overlay_command_docs(&mut self.docs, &self.editor.commands, table);
        }
    }

    pub fn load_init_file(&mut self) {
        self.editor.keymaps.reset_shared();
        keymap::install_defaults(&mut self.editor.keymaps);
        self.editor.commands.remove_config();
        self.editor.modes.remove_owned_by(forge_core::rhai::SOURCE);
        self.editor.menus.remove_plugin(forge_core::rhai::SOURCE);
        self.editor.theme.clear();
        // Themes init.rhai or the theme files defined go too; a plugin's
        // themes stay (they're re-checked on define, not here).
        self.editor.themes.remove_owner(forge_core::rhai::SOURCE);
        self.editor.plugin_config.clear();
        // Mode parents (plugins' modes too), then plugins' bindings; the
        // user's bindings go on top of both.
        let parents: Vec<(String, String)> = self
            .editor
            .modes
            .names(forge_core::ModeKind::Major)
            .into_iter()
            .filter_map(|m| {
                self.editor
                    .modes
                    .get(&m)
                    .and_then(|d| d.parent.clone())
                    .map(|p| (m, p))
            })
            .collect();
        for (m, p) in parents {
            self.editor.keymaps.set_mode_parent(&m, &p);
        }
        if let Some(h) = &self.plugin_host {
            h.reapply_bindings(&mut self.editor);
        }
        self.prefix_help_delay = DEFAULT_PREFIX_HELP_DELAY;
        self.mouse = true;
        self.smooth_scroll = false;
        self.pixel_scroll_precision = false;
        self.scroll_anim = None;
        self.scroll_bar = self.default_scroll_bar;
        self.horizontal_scroll_bar = false;
        self.scroll_bar_drag = None;
        self.plugin_auto_reload = true;
        self.line_numbers = Default::default();
        self.electric_indent = true;
        self.wrap = Default::default();
        self.hl_line = false;
        self.fill_column_indicator = false;
        self.show_trailing_whitespace = false;
        self.show_paren = true;
        self.electric_pair = false;
        self.image_animate_loop = false;
        self.sentence_end = forge_core::text_nav::SentenceEnd::default();
        self.sentence_end_pattern = String::new();
        self.page_delimiter = forge_core::text_nav::DEFAULT_PAGE_DELIMITER.to_string();
        self.paragraph_start = forge_core::text_nav::DEFAULT_PARAGRAPH_START.to_string();
        self.paragraph_separate = forge_core::text_nav::DEFAULT_PARAGRAPH_SEPARATE.to_string();
        self.electric_quote = false;
        self.electric_quote_chars = ['\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}'];
        self.electric_quote_replace_double = false;
        self.electric_quote_paragraph = true;
        self.electric_quote_comment = true;
        self.electric_quote_string = false;
        self.savehist = true;
        self.editor.editorconfig = true;
        self.editor.save_place = true;
        self.editor.recentf = true;
        self.auto_save_default = true;
        self.auto_save_interval = crate::files::AUTO_SAVE_INTERVAL;
        self.auto_save_idle = AUTO_SAVE_IDLE;
        self.select_enable_clipboard = true;
        self.select_enable_primary = false;
        self.save_interprogram_paste_before_kill = forge_core::SaveInterprogramPasteBeforeKill::Off;
        self.yank_pop_change_selection = false;
        self.mouse_drag_copy_region = false;
        self.x_select_enable_clipboard_manager = true;
        forge_core::filelock::set_create_lockfiles(true);
        forge_core::backup::set_config(Default::default());
        crate::vc::set_config(Default::default());
        self.rhai = None;
        self.init_errors.clear();

        let Some(path) = self.init_path.clone() else {
            self.sync_docs();
            return;
        };
        if !path.exists() {
            // Point people with an old keys.toml at its replacement.
            let old = path.with_file_name("keys.toml");
            if old.exists() {
                self.msg(format!(
                    "{} is no longer read; Forge now uses {} (see docs/init.example.rhai)",
                    old.display(),
                    path.display()
                ));
            }
            self.sync_docs();
            return;
        }
        let rhai = match RhaiConfig::new() {
            Ok(l) => l,
            Err(e) => {
                self.init_errors.push(format!("Can't start Rhai: {e}"));
                self.sync_docs();
                return self.error(format!("Can't start Rhai: {e}"));
            }
        };
        let out = rhai.load_file(&mut self.editor, &path);
        self.init_errors = out.errors.clone();
        // Theme files load next and only define themes; `forge.theme.load`
        // in init.rhai or in one of them applies one.
        let mut theme_outs = Vec::new();
        for theme_path in forge_core::config::theme_files() {
            let out = rhai.load_file(&mut self.editor, &theme_path);
            self.init_errors.extend(out.errors.iter().cloned());
            theme_outs.push(out);
        }
        self.watch.set_rhai_monitor(Some(rhai.monitor()));
        self.rhai = Some(rhai);
        self.apply_rhai_settings();
        plugins::apply(self, out);
        for out in theme_outs {
            plugins::apply(self, out);
        }
        self.sync_docs();
        // init.rhai is where a project's own init.rhai binds, so now that
        // there is a Rhai state the file on screen can be asked about.
        crate::project::load_if_granted(self);
    }

    /// The compiled `sentence-end` regexp, or `None` when it is unset.
    pub fn sentence_end_re(&self) -> Result<Option<regex::Regex>, String> {
        if self.sentence_end_pattern.is_empty() {
            return Ok(None);
        }
        forge_core::text_nav::sentence_end_regexp(&self.sentence_end_pattern).map(Some)
    }

    /// The compiled `page-delimiter` regexp.
    pub fn page_delimiter_re(&self) -> Result<regex::Regex, String> {
        forge_core::text_nav::page_delimiter(&self.page_delimiter)
    }

    /// The compiled `paragraph-start` and `paragraph-separate` regexps.
    pub fn paragraph_rules(&self) -> Result<(regex::Regex, regex::Regex), String> {
        Ok((
            forge_core::text_nav::paragraph_regexp(&self.paragraph_start, "paragraph-start")?,
            forge_core::text_nav::paragraph_regexp(&self.paragraph_separate, "paragraph-separate")?,
        ))
    }

    /// Take over what forge.opt set (after init.rhai, and after M-:).
    pub fn apply_rhai_settings(&mut self) {
        let Some(settings) = self.rhai.as_ref().map(|l| l.settings()) else {
            return;
        };
        if let Some(d) = settings.prefix_help_delay {
            self.prefix_help_delay = Duration::from_secs_f64(d);
        }
        if let Some(m) = settings.mouse {
            self.mouse = m;
        }
        if let Some(m) = settings.message_log_max {
            self.message_log_max = m;
        }
        self.packages.indexes = settings.package_index.clone().unwrap_or_default();
        if let Some(r) = settings.plugin_auto_reload {
            self.plugin_auto_reload = r;
        }
        if let Some(l) = settings.line_numbers {
            self.line_numbers = l;
        }
        if let Some(e) = settings.electric_indent {
            self.electric_indent = e;
        }
        if let Some(w) = settings.wrap {
            self.wrap = w;
        }
        if let Some(s) = settings.save_interprogram_paste_before_kill {
            self.save_interprogram_paste_before_kill = s;
        }
        // The grep options.
        if let Some(on) = settings.flags.get("grep_highlight_matches") {
            self.grep.highlight_matches = *on;
        }
        if let Some(on) = settings.flags.get("grep_use_headings") {
            self.grep.use_headings = *on;
        }
        if let Some(on) = settings.flags.get("grep_find_abbreviate") {
            self.grep.abbreviate = *on;
        }
        if let Some(v) = settings
            .strings
            .get("grep_match_regexp")
            .and_then(|v| v.first())
        {
            self.grep.match_regexp = v.clone();
        }
        if let Some(v) = settings
            .strings
            .get("grep_save_buffers")
            .and_then(|v| v.first())
        {
            match crate::grep::SaveBuffers::from_name(v) {
                Some(s) => self.grep.save_buffers = s,
                None => self.error(format!(
                    "forge.opt.grep_save_buffers must be nil, ask or always, not {v:?}"
                )),
            }
        }
        if let Some(v) = settings
            .strings
            .get("grep_template")
            .and_then(|v| v.first())
        {
            self.grep.template = v.clone();
        }
        if let Some(v) = settings
            .strings
            .get("grep_find_template")
            .and_then(|v| v.first())
        {
            self.grep.find_template = v.clone();
        }
        if let Some(v) = settings.strings.get("grep_find_ignored_directories") {
            self.grep.ignored_directories = v.clone();
        }
        if let Some(v) = settings.maps.get("grep_files_aliases") {
            self.grep.files_aliases = v
                .iter()
                .map(|(alias, files)| (alias.clone(), files.clone()))
                .collect();
        }
        for (name, on) in &settings.flags {
            let on = *on;
            match name.as_str() {
                "case_fold_search" => self.case_fold_search = on,
                "editorconfig" => self.editor.editorconfig = on,
                "electric_pair" => self.electric_pair = on,
                "electric_quote" => self.electric_quote = on,
                "electric_quote_paragraph" => self.electric_quote_paragraph = on,
                "image_animate_loop" => self.image_animate_loop = on,
                "sentence_end_double_space" => self.sentence_end.double_space = on,
                "sentence_end_without_period" => self.sentence_end.without_period = on,
                "electric_quote_comment" => self.electric_quote_comment = on,
                "electric_quote_string" => self.electric_quote_string = on,
                "electric_quote_replace_double" => {
                    self.electric_quote_replace_double = on;
                }
                "fill_column_indicator" => self.fill_column_indicator = on,
                "hl_line" => self.hl_line = on,
                "line_number_mode" => self.line_number_mode = on,
                "column_number_mode" => self.column_number_mode = on,
                "size_indication_mode" => self.size_indication_mode = on,
                "inhibit_message" => self.inhibit_message = on,
                // The bar frames the window, so this is a global mode, not
                // a per-buffer one.
                "menu_bar" => crate::menu::set_mode(self, on),
                "tab_bar" => crate::tabs::set_mode(self, on),
                "tool_bar" => crate::toolbar::set_mode(self, on),
                "modifier_bar" => crate::toolbar::set_modifier_mode(self, on),
                "tool_bar_always_show_default" => self.tool_bar_always_show_default = on,
                "context_menu" => crate::menu::set_context_mode(self, on),
                "org_startup_indented" => self.editor.org_startup_indented = on,
                "recentf" => self.editor.recentf = on,
                "save_place" => self.editor.save_place = on,
                "savehist" => self.savehist = on,
                "show_paren" => self.show_paren = on,
                "set_mark_command_repeat_pop" => self.set_mark_command_repeat_pop = on,
                "show_trailing_whitespace" => self.show_trailing_whitespace = on,
                "scroll_bar" => self.scroll_bar = on,
                "horizontal_scroll_bar" => self.horizontal_scroll_bar = on,
                "smooth_scroll" => self.smooth_scroll = on,
                "pixel_scroll_precision" | "pixel_scroll_precision_mode" => {
                    self.pixel_scroll_precision = on;
                }
                "auto_save_default" => self.auto_save_default = on,
                "create_lockfiles" => forge_core::filelock::set_create_lockfiles(on),
                "select_enable_clipboard" => self.select_enable_clipboard = on,
                "select_enable_primary" => self.select_enable_primary = on,
                "yank_pop_change_selection" => self.yank_pop_change_selection = on,
                "mouse_drag_copy_region" => self.mouse_drag_copy_region = on,
                "x_select_enable_clipboard_manager" => self.x_select_enable_clipboard_manager = on,
                _ => {}
            }
        }
        crate::icon::apply_settings(self);
        if let Some(v) = settings
            .strings
            .get("tool_bar_style")
            .and_then(|v| v.first())
        {
            match forge_core::toolbar::Style::parse(v) {
                Some(style) => self.tool_bar_style = style,
                None => self.error(format!(
                    "forge.opt.tool_bar_style must be image, text, both, both-horiz or text-image-horiz, not {v:?}"
                )),
            }
        }
        if let Some(v) = settings
            .strings
            .get("tool_bar_position")
            .and_then(|v| v.first())
        {
            match forge_core::toolbar::Position::parse(v) {
                Some(position) => self.tool_bar_position = position,
                None => self.error(format!(
                    "forge.opt.tool_bar_position must be top, bottom, left or right, not {v:?}"
                )),
            }
        }
        // Electric Quote mode's four curved quotes, in order: left
        // single, right single, left double, right double.
        if let Some(chars) = settings.strings.get("electric_quote_chars") {
            let mut all: Vec<char> = Vec::new();
            for s in chars {
                all.extend(s.chars());
            }
            if all.len() == 4 {
                self.electric_quote_chars = [all[0], all[1], all[2], all[3]];
            } else {
                self.error("forge.opt.electric_quote_chars must name four characters".to_string());
            }
        }
        // The regexp options: sentence-end, page-delimiter and the two
        // paragraph boundaries. Each may be unset.
        if let Some(v) = settings.strings.get("sentence_end").and_then(|v| v.first()) {
            self.sentence_end_pattern = v.clone();
        }
        if let Some(v) = settings
            .strings
            .get("page_delimiter")
            .and_then(|v| v.first())
        {
            self.page_delimiter = v.clone();
        }
        if let Some(v) = settings
            .strings
            .get("paragraph_start")
            .and_then(|v| v.first())
        {
            self.paragraph_start = v.clone();
        }
        if let Some(v) = settings
            .strings
            .get("paragraph_separate")
            .and_then(|v| v.first())
        {
            self.paragraph_separate = v.clone();
        }
        crate::org_flow::apply_settings(self, &settings);
        crate::capf::apply_settings(self, &settings);
        if let Some(n) = settings.numbers.get("auto_save_interval") {
            self.auto_save_interval = *n as u64;
        }
        if let Some(n) = settings.numbers.get("auto_save_timeout") {
            self.auto_save_idle = Duration::from_secs_f64(*n);
        }
        // How many marks a buffer's mark ring keeps.
        if let Some(n) = settings.numbers.get("mark_ring_max") {
            forge_core::buffer::set_mark_ring_max(*n as usize);
        }
        // Numbered backups.
        let mut backup = forge_core::backup::Config::default();
        if let Some(v) = settings
            .strings
            .get("version_control")
            .and_then(|v| v.first())
        {
            match forge_core::backup::VersionControl::from_name(v) {
                Ok(vc) => backup.version_control = vc,
                Err(e) => self.error(format!("forge.opt.{e}")),
            }
        }
        if let Some(n) = settings.numbers.get("kept_old_versions") {
            backup.kept_old_versions = *n as usize;
        }
        if let Some(n) = settings.numbers.get("kept_new_versions") {
            backup.kept_new_versions = *n as usize;
        }
        backup.delete_old_versions = settings.flags.get("delete_old_versions").copied();
        if let Some(b) = settings.flags.get("make_backup_files") {
            backup.make_backup_files = *b;
        }
        let no_littering = settings.flags.get("no_littering").copied().unwrap_or(false);
        backup.no_littering = no_littering;
        if let Some(d) = settings
            .strings
            .get("backup_directory")
            .and_then(|v| v.first())
        {
            backup.directory = Some(crate::vc::expand_home(d));
        }
        // How Org files fold on first visit (also per-file via #+STARTUP).
        if let Some(m) = settings
            .strings
            .get("org_startup_folded")
            .and_then(|v| v.first())
        {
            let mode = m.to_ascii_lowercase();
            if forge_core::org::STARTUP_FOLDED_MODES.contains(&mode.as_str()) {
                self.editor.org_startup_folded = mode;
            } else {
                self.error(format!(
                    "forge.opt.org_startup_folded: unknown {m:?} (see {})",
                    forge_core::org::STARTUP_FOLDED_MODES.join(", ")
                ));
            }
        }
        // What counts as an image, for link previews and export.
        if let Some(v) = settings.strings.get("image_file_name_extensions") {
            forge_core::org_display::set_image_file_extensions(v.clone());
        }
        if let Some(v) = settings.strings.get("image_file_name_regexps")
            && let Err(e) = forge_core::org_display::set_image_file_regexps(v.clone())
        {
            self.error(format!("forge.opt.{e}"));
        }
        // Whether remote image links are fetched and shown.
        if let Some(v) = settings
            .strings
            .get("org_display_remote_inline_images")
            .and_then(|v| v.first())
        {
            match forge_core::org_display::RemoteImages::from_name(v) {
                Some(m) => forge_core::org_display::set_remote_inline_images(m),
                None => self.error(format!(
                    "forge.opt.org_display_remote_inline_images: want \"cache\", \"download\" or \"skip\", not {v:?}"
                )),
            }
        }
        // The `ls` that insert-directory runs (empty: Forge lists itself).
        forge_core::fileops::set_insert_directory_program(
            settings
                .strings
                .get("insert_directory_program")
                .and_then(|v| v.first())
                .map_or("", String::as_str),
        );
        if let Some(n) = settings.numbers.get("org_indent_per_level") {
            if (1.0..=8.0).contains(n) {
                self.editor.org_indent_per_level = *n as usize;
            } else {
                self.error("forge.opt.org_indent_per_level: want 1 to 8".to_string());
            }
        }
        forge_core::backup::set_config(backup);
        if let Err(e) = crate::vc::apply_settings(&settings) {
            self.error(e);
        }
        if let Err(e) = crate::input_method::apply_settings(self, &settings) {
            self.error(e);
        }
        if self.mode_hooks != settings.mode_hooks {
            self.mode_hooks = settings.mode_hooks.clone();
            self.mode_hooks_done.clear();
        }
        self.editor.session.keep_history = self.savehist;
    }

    /// Warn about init.rhai bindings to commands that don't exist (after
    /// plugins have had a chance to define them).
    pub fn report_unknown_bindings(&mut self) {
        let Some(rhai) = &self.rhai else { return };
        let unknown = rhai.unknown_bindings(&self.editor);
        for w in &unknown {
            plugins::log(
                self,
                forge_core::rhai::SOURCE,
                forge_core::plugin::runtime::Level::Warn,
                w,
            );
        }
        if let Some(first) = unknown.first() {
            let more = if unknown.len() > 1 {
                format!(" (and {} more in *Messages*)", unknown.len() - 1)
            } else {
                String::new()
            };
            self.echo(format!("init.rhai: {first}{more}"));
        }
    }

    // ------------------------------------------------------ kill ring

    /// Replace the clipboard provider (used in tests for an in-memory clipboard).
    pub fn set_clipboard_provider(&mut self, p: Box<dyn crate::clipboard::ClipboardProvider>) {
        self.clipboard_provider = p;
    }

    /// Read the OS clipboard or primary selection if another program has put new text there.
    pub fn interprogram_paste(&mut self) -> Option<String> {
        if self.select_enable_clipboard
            && let Some(text) = self
                .clipboard_provider
                .get_text(forge_core::clipboard::SelectionTarget::Clipboard)
            && !text.is_empty()
            && self.last_clipboard_out.as_deref() != Some(&text)
        {
            return Some(text);
        }
        if self.select_enable_primary
            && let Some(text) = self
                .clipboard_provider
                .get_text(forge_core::clipboard::SelectionTarget::Primary)
            && !text.is_empty()
            && self.last_primary_out.as_deref() != Some(&text)
        {
            return Some(text);
        }
        None
    }

    /// Before a kill command destroys the clipboard, save foreign clipboard data
    /// to the kill ring if save-interprogram-paste-before-kill is set.
    pub fn interprogram_save_before_kill(&mut self) {
        if self.save_interprogram_paste_before_kill
            == forge_core::SaveInterprogramPasteBeforeKill::Off
        {
            return;
        }
        let Some(text) = self.interprogram_paste() else {
            return;
        };
        let should_save = match self.save_interprogram_paste_before_kill {
            forge_core::SaveInterprogramPasteBeforeKill::Always => true,
            forge_core::SaveInterprogramPasteBeforeKill::Off => false,
            forge_core::SaveInterprogramPasteBeforeKill::Limit(limit) => {
                text.chars().count() <= limit
            }
        };
        if should_save {
            self.last_clipboard_out = Some(text.replace('\0', "\\0"));
            self.last_primary_out = Some(text.replace('\0', "\\0"));
            self.editor.kill_ring.push(text);
        }
    }

    /// Put text onto the system clipboard and/or primary selection according to options.
    pub fn interprogram_cut(&mut self, text: &str) {
        let sanitized = text.replace('\0', "\\0");
        if self.select_enable_clipboard {
            self.clipboard_provider
                .set_text(forge_core::clipboard::SelectionTarget::Clipboard, text);
            self.last_clipboard_out = Some(sanitized.clone());
        }
        if self.select_enable_primary {
            self.clipboard_provider
                .set_text(forge_core::clipboard::SelectionTarget::Primary, text);
            self.last_primary_out = Some(sanitized);
        }
    }

    /// Save text into the kill ring (kill-ring-save or mouse-drag-copy-region),
    /// saving foreign clipboard contents first if configured, and pushing to OS clipboard/primary.
    pub fn kill_ring_save(&mut self, text: String) {
        self.kill_ring_save_with_clipboard_override(text, false);
    }

    /// Save text into the kill ring, optionally forcing export to clipboard even if select-enable-clipboard is false.
    pub fn kill_ring_save_with_clipboard_override(&mut self, text: String, force_clipboard: bool) {
        if text.is_empty() {
            return;
        }
        self.interprogram_save_before_kill();
        self.editor.kill_ring.push(text.clone());
        if force_clipboard {
            let sanitized = text.replace('\0', "\\0");
            self.clipboard_provider
                .set_text(forge_core::clipboard::SelectionTarget::Clipboard, &text);
            self.last_clipboard_out = Some(sanitized.clone());
            if self.select_enable_primary {
                self.clipboard_provider
                    .set_text(forge_core::clipboard::SelectionTarget::Primary, &text);
                self.last_primary_out = Some(sanitized);
            }
        } else {
            self.interprogram_cut(&text);
        }
    }

    /// Put killed text on the kill ring, merging consecutive kills, saving foreign clipboard
    /// contents first if configured, and updating the OS clipboard/primary selection.
    pub fn kill(&mut self, text: String, backward: bool) {
        self.kill_with_clipboard_override(text, backward, false);
    }

    /// Put killed text on the kill ring, optionally forcing export to clipboard.
    pub fn kill_with_clipboard_override(
        &mut self,
        text: String,
        backward: bool,
        force_clipboard: bool,
    ) {
        if text.is_empty() {
            return;
        }
        let is_consecutive = KILL_COMMANDS.contains(&self.last_command.as_str())
            && self.last_kill_backward == Some(backward);
        if !is_consecutive {
            self.interprogram_save_before_kill();
        }
        self.last_kill_backward = Some(backward);
        let ring = &mut self.editor.kill_ring;
        if is_consecutive {
            if backward {
                ring.prepend(&text);
            } else {
                ring.append(&text);
            }
        } else {
            ring.push(text);
        }
        if let Some(cur) = self.editor.kill_ring.current().map(str::to_string) {
            if force_clipboard {
                let sanitized = cur.replace('\0', "\\0");
                self.clipboard_provider
                    .set_text(forge_core::clipboard::SelectionTarget::Clipboard, &cur);
                self.last_clipboard_out = Some(sanitized.clone());
                if self.select_enable_primary {
                    self.clipboard_provider
                        .set_text(forge_core::clipboard::SelectionTarget::Primary, &cur);
                    self.last_primary_out = Some(sanitized);
                }
            } else {
                self.interprogram_cut(&cur);
            }
        }
    }

    /// Before yank (C-y), check if another application owns the clipboard and push it to the kill ring.
    pub fn prepare_yank(&mut self) {
        if (self.select_enable_clipboard || self.select_enable_primary)
            && let Some(text) = self.interprogram_paste()
        {
            self.last_clipboard_out = Some(text.replace('\0', "\\0"));
            self.last_primary_out = Some(text.replace('\0', "\\0"));
            self.editor.kill_ring.push(text);
        }
    }

    /// On exit, put the kill ring's current entry back on the system
    /// clipboard when x-select-enable-clipboard-manager is set, so the data
    /// outlives Forge.
    pub fn save_clipboard_on_exit(&mut self) {
        if !self.x_select_enable_clipboard_manager || !self.select_enable_clipboard {
            return;
        }
        let Some(text) = self.editor.kill_ring.current() else {
            return;
        };
        let sanitized = text.replace('\0', "\\0");
        self.clipboard_provider
            .set_text(forge_core::clipboard::SelectionTarget::Clipboard, text);
        self.last_clipboard_out = Some(sanitized);
    }

    // -------------------------------------------------------- buffers

    pub fn switch_to(&mut self, id: BufferId) {
        let current = self.active_id();
        if current != id {
            self.previous_buffer = Some(current);
            self.editor.show_buffer(id);
            self.check_file(id);
        }
        // A file from a project may bring that project's init.rhai. Not
        // while another question is up: on_idle asks again once the
        // editor is free.
        crate::project::load_if_granted(self);
        // An Image-mode buffer's picture is rendered on demand.
        if self.buf_ref().major_mode() == crate::image_mode::MODE
            && self
                .buf_ref()
                .image
                .as_ref()
                .is_some_and(|s| s.rendered.is_none())
        {
            crate::image_mode::refresh(self);
        }
    }

    /// Select a window (C-x o), checking its file for outside changes.
    pub fn select_window(&mut self, id: WindowId) {
        self.editor.select_window(id);
        let buf = self.active_id();
        self.check_file(buf);
        crate::project::load_if_granted(self);
    }

    pub fn kill_buffer(&mut self, id: BufferId) {
        crate::vc::forget_buffer(self, id);
        if let Some(st) = self
            .editor
            .state
            .get_buffer(id)
            .and_then(|b| b.image.as_ref())
        {
            crate::image_mode::forget_render(st.rendered.as_deref());
        }
        let fallback = self.other_buffer().filter(|b| *b != id);
        self.editor.kill_buffer(id, fallback);
        if self.previous_buffer == Some(id) {
            self.previous_buffer = None;
        }
    }

    /// The selected window.
    pub fn win(&self) -> &Window {
        self.editor.windows.selected()
    }

    pub fn win_mut(&mut self) -> &mut Window {
        self.editor.windows.selected_mut()
    }

    /// Show generated text (help, buffer list) in a named buffer.
    pub fn show_special_buffer(&mut self, name: &str, mode: &str, contents: &str) {
        let id = match self.editor.find_buffer(name) {
            Some(id) => id,
            None => self.editor.add_buffer(Buffer::new(name)),
        };
        let b = self.editor.state.get_buffer_mut(id).expect("just found");
        b.set_contents(contents);
        b.set_major_mode(mode);
        self.switch_to(id);
        self.win_mut().scroll_top = 0;
        self.win_mut().scroll_y_px = 0;
    }

    /// Show a help buffer and fill in its link layer (see [`forge_core::LinkSpan`]).
    pub fn show_help(&mut self, contents: &str, links: Vec<forge_core::LinkSpan>) {
        self.show_special_buffer("*Help*", "help-mode", contents);
        self.buf().set_links(links);
    }

    /// Other buffer for C-x b's default.
    pub fn other_buffer(&self) -> Option<BufferId> {
        let active = self.active_id();
        self.previous_buffer
            .filter(|id| *id != active && self.editor.state.buffers.contains_key(id))
            .or_else(|| {
                self.editor
                    .buffer_ids()
                    .into_iter()
                    .rev()
                    .find(|id| *id != active)
            })
    }
}

#[cfg(test)]
mod scroll_tests {
    use super::*;
    use forge_core::{Buffer, Pos};

    fn tall_app() -> App {
        let mut app = App::new(Editor::default());
        let id = app.editor.add_buffer(Buffer::new("*scratch*"));
        app.editor.show_buffer(id);
        let text: String = (0..200).map(|i| format!("line {i}\n")).collect();
        app.buf().set_contents(&text);
        app.buf().set_point(Pos::new(0, 0));
        app
    }

    #[test]
    fn smooth_scroll_steps_one_line_at_a_time() {
        let mut app = tall_app();
        app.smooth_scroll = true;
        app.set_scroll_target(5);
        assert_eq!(app.win().scroll_top, 0, "nothing moves until a tick");
        assert!(app.scrolling());
        let mut steps = 0;
        while app.scrolling() {
            app.tick_scroll();
            steps += 1;
            assert!(steps <= 5, "must not overshoot");
        }
        assert_eq!(app.win().scroll_top, 5);
        assert_eq!(steps, 5, "one visible line per tick");
    }

    #[test]
    fn without_smooth_scroll_the_jump_is_immediate() {
        let mut app = tall_app();
        app.smooth_scroll = false;
        app.set_scroll_target(5);
        assert_eq!(app.win().scroll_top, 5);
        assert!(!app.scrolling());
    }

    #[test]
    fn input_cancels_an_in_flight_scroll() {
        let mut app = tall_app();
        app.smooth_scroll = true;
        app.set_scroll_target(50);
        app.tick_scroll();
        assert!(app.scrolling());
        app.note_input();
        assert!(!app.scrolling(), "typing stops the animation");
    }

    #[test]
    fn pixel_scroll_keeps_the_remainder_across_lines() {
        let mut app = tall_app();
        app.scroll_pixels(15, 10);
        assert_eq!(app.win().scroll_top, 1);
        assert_eq!(app.win().scroll_y_px, 5);

        app.scroll_pixels(-7, 10);
        assert_eq!(app.win().scroll_top, 0);
        assert_eq!(app.win().scroll_y_px, 8);
    }

    #[test]
    fn pixel_scroll_stops_at_the_buffer_edges() {
        let mut app = tall_app();
        app.scroll_pixels(-10, 10);
        assert_eq!((app.win().scroll_top, app.win().scroll_y_px), (0, 0));

        let last = app.buf_ref().line_count() - 1;
        app.scroll_to_line(last);
        app.scroll_pixels(10, 10);
        assert_eq!((app.win().scroll_top, app.win().scroll_y_px), (last, 0));
    }

    #[test]
    fn a_line_scroll_drops_a_pixel_offset() {
        let mut app = tall_app();
        app.scroll_pixels(5, 10);
        assert_eq!(app.win().scroll_y_px, 5);
        app.set_scroll_target(3);
        assert_eq!(app.win().scroll_y_px, 0);
    }

    #[test]
    fn a_scroll_bar_between_lines_sets_the_pixel_offset() {
        let mut app = tall_app();
        app.scroll_to_line_pixels(4, 6);
        assert_eq!((app.win().scroll_top, app.win().scroll_y_px), (4, 6));
    }
}

#[cfg(test)]
mod command_name_tests {
    use super::*;
    use forge_core::Buffer;

    fn app() -> App {
        let mut app = App::new(Editor::default());
        let id = app.editor.add_buffer(Buffer::new("*scratch*"));
        app.editor.show_buffer(id);
        app.buf().set_contents("one\ntwo\n");
        app
    }

    #[test]
    fn a_command_that_runs_another_keeps_its_own_name() {
        let mut app = app();
        // A keyboard macro runs commands of its own; the command that ran
        // it is still the last command, the way `last-command'
        // says (that is what makes a plain e repeat the macro).
        app.execute_keys("C-x ( C-n C-x ) C-x e");
        assert_eq!(app.last_command, "kmacro-end-and-call-macro");
    }

    #[test]
    fn a_command_can_change_its_own_name() {
        let mut app = app();
        // A command that renames itself (set-mark-command with a prefix
        // argument becomes pop-to-mark-command) gives `last-command' the
        // new name, the way `last-command' follows `this-command'.
        app.buf().push_mark(Pos::new(0, 0), false);
        app.run_command("set-mark-command");
        assert_eq!(app.last_command, "set-mark-command");
        app.execute_keys("C-u C-SPC");
        assert_eq!(app.last_command, "pop-to-mark-command");
    }
}

#[cfg(test)]
mod transient_tests {
    use super::*;
    use forge_core::Origin;
    use forge_core::keymap::{OnUnbound, TransientOpts};

    /// An app with a transient map "pad" that binds `p` to a command that
    /// records it ran, and one global binding it will shadow.
    fn app_with_pad() -> App {
        let mut app = App::new(Editor::default());
        let id = app.editor.add_buffer(Buffer::new("*scratch*"));
        app.editor.show_buffer(id);
        // Two real commands, so dispatch runs them and `last_command` says which.
        let transient = Scope::Transient("pad".into());
        app.editor
            .keymaps
            .bind(&transient, "p", "forward-char", Origin::Package)
            .unwrap();
        app.editor
            .keymaps
            .bind(&Scope::Global, "p", "previous-line", Origin::Package)
            .unwrap();
        // z is bound only below the map, for the "resolved elsewhere" cases.
        app.editor
            .keymaps
            .bind(&Scope::Global, "z", "previous-line", Origin::Package)
            .unwrap();
        app
    }

    fn press(app: &mut App, c: char) {
        let k = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        app.handle_key(k);
    }

    #[test]
    fn transients_show_up_in_where_is_and_prefix_hints() {
        let mut app = app_with_pad();
        app.editor.push_transient("pad", TransientOpts::default());
        // where-is sees the transient binding, not just the global one.
        let keys = app.editor.active_keymaps_now().where_is("forward-char");
        assert!(
            keys.contains(&"p".to_string()),
            "the transient binding is found: {keys:?}"
        );
        assert!(
            !keys.contains(&"z".to_string()),
            "and global keys still resolve: {keys:?}"
        );
        // A prefix in the transient map is offered as a continuation.
        app.editor
            .keymaps
            .bind(
                &Scope::Transient("pad".into()),
                "C-x r",
                "forward-char",
                Origin::Package,
            )
            .unwrap();
        // After C-x, r is offered -- from the transient map, which C-x r binds.
        app.pending = vec!["C-x".to_string()];
        let hints = app.prefix_hints();
        assert!(
            hints.contains(&("r".to_string(), "forward-char".to_string())),
            "the transient binding is a hint: {hints:?}"
        );
        app.pending.clear();
        // Popped, it is gone from where-is even though the map still exists.
        app.editor.pop_transient("pad");
        // C-x r was only in the map, so it resolves to nothing now.
        let maps = app.editor.active_keymaps_now();
        assert!(
            !maps.where_is("forward-char").contains(&"C-x r".to_string()),
            "a popped map's bindings stop resolving"
        );
        // The menu's and vc-dir's help maps are there from the defaults; a
        // popped map stays too.
        assert_eq!(
            app.editor.keymaps.transient_names(),
            vec!["menu", "pad", "vc-dir-help"],
            "the maps themselves are remembered"
        );
    }

    #[test]
    fn a_transient_map_outranks_everything_below_it() {
        let mut app = app_with_pad();
        // Not up yet: the global binding runs.
        press(&mut app, 'p');
        assert_eq!(app.last_command, "previous-line");
        assert!(app.editor.live_transients().is_empty());

        app.editor
            .push_transient("pad", TransientOpts::default().on_unbound(OnUnbound::Stay));
        press(&mut app, 'p');
        assert_eq!(app.last_command, "forward-char", "the transient map wins");
    }

    #[test]
    fn one_shot_pops_after_the_key_it_resolves() {
        let mut app = app_with_pad();
        app.editor
            .push_transient("pad", TransientOpts::default().one_shot(true));
        assert_eq!(app.editor.live_transients(), vec!["pad".to_string()]);
        press(&mut app, 'p');
        assert_eq!(app.last_command, "forward-char");
        assert!(
            app.editor.live_transients().is_empty(),
            "a one-shot map is gone after its key"
        );
        // The next p is the ordinary one again.
        press(&mut app, 'p');
        assert_eq!(app.last_command, "previous-line");
    }

    #[test]
    fn exit_pops_the_map_and_lets_the_key_through() {
        let mut app = app_with_pad();
        app.editor
            .push_transient("pad", TransientOpts::default().on_unbound(OnUnbound::Exit));
        // q is bound nowhere; the map gives it up, then it resolves below.
        press(&mut app, 'q');
        assert!(
            app.editor.live_transients().is_empty(),
            "an unbound key ended the map"
        );
        // And the map is not consulted again.
        press(&mut app, 'p');
        assert_eq!(app.last_command, "previous-line");
    }

    #[test]
    fn exit_carries_on_normally_for_a_key_resolved_below() {
        let mut app = app_with_pad();
        app.editor
            .push_transient("pad", TransientOpts::default().on_unbound(OnUnbound::Exit));
        // z is bound only in global, so it resolves below the map: the map
        // gives it up, and the key still runs what it always did.
        press(&mut app, 'z');
        assert!(
            app.editor.live_transients().is_empty(),
            "the map gave up the key"
        );
        assert_eq!(
            app.last_command, "previous-line",
            "and the key carried on to what was below"
        );
    }

    #[test]
    fn swallow_keeps_the_map_up_and_drops_the_key() {
        let mut app = app_with_pad();
        app.editor.push_transient(
            "pad",
            TransientOpts::default().on_unbound(OnUnbound::Swallow),
        );
        press(&mut app, 'p');
        assert_eq!(app.last_command, "forward-char");
        press(&mut app, 'q');
        assert_eq!(
            app.editor.live_transients(),
            vec!["pad".to_string()],
            "swallow keeps the keypad open"
        );
        press(&mut app, 'p');
        assert_eq!(app.last_command, "forward-char", "still the map's key");
    }

    #[test]
    fn a_keypad_lists_its_keys_while_it_is_up() {
        let mut app = app_with_pad();
        assert_eq!(app.keypad_hints(), None);
        // A map that lets other keys through doesn't own the keyboard.
        app.editor
            .push_transient("pad", TransientOpts::default().on_unbound(OnUnbound::Stay));
        assert_eq!(app.keypad_hints(), None);
        app.editor.push_transient(
            "pad",
            TransientOpts::default().on_unbound(OnUnbound::Swallow),
        );
        let (name, hints) = app.keypad_hints().expect("a keypad shows its keys");
        assert_eq!(name, "pad");
        let p = hints.iter().find(|(k, _)| k == "p").expect("p is listed");
        assert_eq!(
            p.1, "Move point one character forward",
            "by its doc's first sentence"
        );
        app.editor.pop_transient("pad");
        assert_eq!(app.keypad_hints(), None);
    }

    #[test]
    fn a_prefix_key_is_never_taken_as_unbound() {
        let mut app = app_with_pad();
        app.editor
            .keymaps
            .bind(
                &Scope::Transient("pad".into()),
                "C-x r",
                "forward-char",
                Origin::Package,
            )
            .unwrap();
        app.editor
            .push_transient("pad", TransientOpts::default().on_unbound(OnUnbound::Exit));
        let ctrl = KeyModifiers::CONTROL;
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), ctrl));
        assert_eq!(
            app.editor.live_transients(),
            vec!["pad".to_string()],
            "C-x is a prefix here, so the map stays"
        );
        app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        assert_eq!(app.last_command, "forward-char");
    }

    #[test]
    fn an_expired_map_is_gone_before_the_next_lookup() {
        let mut app = app_with_pad();
        // A timeout already in the past: nothing has to fire, the map is
        // simply not there when the key is looked up.
        app.editor
            .push_transient("pad", TransientOpts::default().on_unbound(OnUnbound::Stay));
        app.editor.transients[0].timeout = Some(Duration::from_millis(0));
        press(&mut app, 'p');
        assert_eq!(
            app.last_command, "previous-line",
            "the map had already expired"
        );
        assert!(app.editor.live_transients().is_empty());
    }

    #[test]
    fn pushing_the_same_name_twice_makes_it_outermost() {
        let mut app = app_with_pad();
        app.editor.push_transient("pad", TransientOpts::default());
        app.editor.push_transient("pad", TransientOpts::default());
        assert_eq!(app.editor.live_transients().len(), 1, "not stacked twice");
        assert!(app.editor.pop_transient("pad"));
        assert!(app.editor.live_transients().is_empty());
        assert!(!app.editor.pop_transient("pad"), "it was already down");
    }
}

#[cfg(test)]
mod prompt_tests {
    use super::*;
    use crate::minibuffer::{Minibuffer, Purpose};
    use forge_core::Origin;

    fn prompt_app() -> App {
        let mut app = App::new(Editor::default());
        let id = app.editor.add_buffer(Buffer::new("*scratch*"));
        app.editor.show_buffer(id);
        app.prompt("Test: ", Purpose::GotoLine);
        app
    }

    fn press(app: &mut App, c: char) {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }

    #[test]
    fn a_prompt_map_answers_its_keys_and_types_the_rest() {
        let mut app = prompt_app();
        for c in "demo".chars() {
            press(&mut app, c);
        }
        let mb: &Minibuffer = app.minibuffer.as_ref().expect("still asking");
        assert_eq!(mb.input, "demo", "unbound keys are typed");
    }

    #[test]
    fn a_prompt_binding_can_be_replaced() {
        let mut app = prompt_app();
        // q ends the prompt, so it is no longer typed.
        app.editor
            .keymaps
            .bind(
                &forge_core::Scope::Prompt("goto-line".into()),
                "q",
                "minibuffer-abort",
                Origin::User,
            )
            .unwrap();
        press(&mut app, 'q');
        assert!(app.minibuffer.is_none(), "the rebound key ran its command");
    }

    /// An open menu's keys come from its own transient map, so they can be
    /// rebound, and a key it doesn't bind closes the menu and carries on.
    #[test]
    fn a_menu_keys_binding_can_be_replaced() {
        let mut app = App::new(Editor::default());
        let id = app.editor.add_buffer(Buffer::new("*scratch*"));
        app.editor.show_buffer(id);
        crate::menu::set_mode(&mut app, true);
        crate::menu::open_next(&mut app);
        assert!(crate::menu::active(&app), "the menu is up");
        app.editor
            .keymaps
            .bind(
                &forge_core::Scope::Transient(crate::menu::TRANSIENT.into()),
                "x",
                "menu-bar-menu-quit",
                Origin::User,
            )
            .unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert!(
            !crate::menu::active(&app),
            "the rebound key ran menu-bar-menu-quit"
        );
        assert_eq!(app.message.as_deref(), Some("Quit"), "which says Quit");
    }

    #[test]
    fn the_menu_keys_defaults_move_in_the_menu() {
        let mut app = App::new(Editor::default());
        let id = app.editor.add_buffer(Buffer::new("*scratch*"));
        app.editor.show_buffer(id);
        crate::menu::set_mode(&mut app, true);
        crate::menu::open_next(&mut app);
        // C-n is the next item in a menu, and next-line in the buffer.
        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
        assert_eq!(
            app.last_command, "menu-bar-menu-next-item",
            "the menu's own map answers"
        );
        assert!(crate::menu::active(&app), "and the menu stays up");
    }

    #[test]
    fn one_prompts_map_does_not_leak_into_another() {
        let mut app = prompt_app();
        app.editor
            .keymaps
            .bind(
                &forge_core::Scope::Prompt("dired".into()),
                "q",
                "minibuffer-abort",
                Origin::User,
            )
            .unwrap();
        press(&mut app, 'q');
        assert!(
            app.minibuffer.is_some(),
            "another prompt's binding does not apply here"
        );
    }
}

#[cfg(test)]
mod layer_tests {
    use super::*;
    use forge_core::keymap::{Condition, Origin};

    fn app_with_text(text: &str, name: &str) -> App {
        let mut app = App::new(Editor::default());
        let id = app.editor.add_buffer(Buffer::new(name));
        app.editor
            .state
            .get_buffer_mut(id)
            .unwrap()
            .set_contents(text);
        app.editor.show_buffer(id);
        app
    }

    fn press_key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
        app.handle_key(KeyEvent::new(code, mods));
    }

    #[test]
    fn a_region_layer_answers_only_while_a_region_is_active() {
        let mut app = app_with_text("one two\n", "*scratch*");
        // C-f is forward-char globally; the layer gives it a different
        // command, so which one ran says whether the layer was up.
        app.editor
            .keymaps
            .bind(
                &Scope::Layer("region".into()),
                "C-f",
                "kill-line",
                Origin::User,
            )
            .unwrap();
        app.editor
            .keymaps
            .add_layer("region", Condition::RegionActive);
        press_key(&mut app, KeyCode::Char('f'), KeyModifiers::CONTROL);
        assert_eq!(app.last_command, "forward-char", "the global binding");

        app.buf().push_mark(Pos::new(0, 0), true);
        app.buf().set_point(Pos::new(0, 2));
        assert!(app.buf_ref().mark_active(), "the region is active");
        press_key(&mut app, KeyCode::Char('f'), KeyModifiers::CONTROL);
        assert_eq!(app.last_command, "kill-line", "the layer's binding");
    }

    #[test]
    fn a_read_only_layer_follows_the_buffer() {
        let mut app = app_with_text("hello\n", "*scratch*");
        app.editor.keymaps.add_layer("ro", Condition::ReadOnly);
        app.editor
            .keymaps
            .bind(
                &Scope::Layer("ro".into()),
                "C-x C-f",
                "find-file",
                Origin::User,
            )
            .unwrap();
        let up = |app: &App| {
            app.editor
                .active_keymaps_now()
                .maps
                .iter()
                .any(|m| matches!(&m.scope, Scope::Layer(n) if n == "ro"))
        };
        // The lookup is what refreshes the context, so ask before looking.
        app.editor.active_keymaps();
        assert!(!up(&app), "an editable buffer keeps the layer down");
        app.buf().set_read_only(true);
        app.editor.active_keymaps();
        assert!(up(&app), "a read-only buffer brings it up");
    }

    #[test]
    fn the_read_only_option_refuses_commands_and_says_so() {
        let mut app = app_with_text("listing\n", "*listing*");
        app.buf().set_option("read-only", "true").unwrap();
        app.run_command("kill-line");
        assert_eq!(app.buf_ref().get_contents(), "listing\n");
        assert_eq!(
            app.message.as_deref(),
            Some("Buffer is read-only: *listing*")
        );
        app.run_command("forward-char");
        app.buf().set_option("read-only", "").unwrap();
        app.run_command("kill-line");
        assert_eq!(app.buf_ref().get_contents(), "l\n");
    }

    #[test]
    fn a_comment_layer_follows_the_cursor() {
        // The syntax conditions need the face at point, which the context
        // gathers where it has &mut; the point moving is what changes it.
        let mut app = app_with_text("// a comment\nlet x = 1;\n", "test.rs");
        app.buf().set_language(forge_core::syntax::language("rust"));
        app.editor.keymaps.add_layer("c", Condition::InComment);
        app.editor
            .keymaps
            .bind(&Scope::Layer("c".into()), "C-f", "kill-line", Origin::User)
            .unwrap();
        // On "x", outside the comment.
        app.buf().set_point(Pos::new(1, 5));
        press_key(&mut app, KeyCode::Char('f'), KeyModifiers::CONTROL);
        assert_eq!(
            app.last_command, "forward-char",
            "outside the comment the layer is down"
        );
        // Inside it.
        app.buf().set_point(Pos::new(0, 5));
        press_key(&mut app, KeyCode::Char('f'), KeyModifiers::CONTROL);
        assert_eq!(
            app.last_command, "kill-line",
            "inside the comment the layer answers"
        );
    }

    #[test]
    fn a_mode_layer_is_up_only_in_the_modes_it_names() {
        let mut app = app_with_text("one\n", "*scratch*");
        app.editor.keymaps.add_layer(
            "not-help",
            Condition::Not(Box::new(Condition::MajorDerivesFrom("special-mode".into()))),
        );
        app.editor
            .keymaps
            .bind(
                &Scope::Layer("not-help".into()),
                "C-f",
                "kill-line",
                Origin::User,
            )
            .unwrap();
        let up = |app: &App| {
            app.editor
                .active_keymaps_now()
                .maps
                .iter()
                .any(|m| matches!(&m.scope, Scope::Layer(n) if n == "not-help"))
        };
        assert!(
            up(&app),
            "fundamental-mode doesn't derive from special-mode"
        );
        app.show_special_buffer("*Help*", "help-mode", "text");
        assert!(!up(&app), "help-mode does, so the layer is down");
    }
}

#[cfg(test)]
mod docs_tests {
    use super::*;
    use forge_docs::Kind;

    /// The same two-way check `cargo xtask docs --check` runs, so a stale
    /// name fails `cargo test` as well as CI.
    #[test]
    fn every_command_has_a_doc_and_every_doc_names_a_command() {
        let app = App::new(Editor::default());
        let commands: std::collections::BTreeSet<&str> = app.editor.commands.names().collect();
        let docd: std::collections::BTreeSet<&str> = app
            .docs
            .of_kind(Kind::Command)
            .iter()
            .map(|d| d.name.as_str())
            .collect();
        let missing: Vec<&&str> = commands.difference(&docd).collect();
        assert!(missing.is_empty(), "commands with no doc: {missing:?}");
        let stale: Vec<&&str> = docd.difference(&commands).collect();
        assert!(
            stale.is_empty(),
            "docs naming a command that does not exist: {stale:?}"
        );
    }

    /// The link layer keys off command names, so a summary that is not a
    /// single short line would make C-h f pages unreadable. The full lint
    /// runs in `xtask docs --check`; this catches the shape of it.
    #[test]
    fn every_summary_is_one_short_capitalised_line() {
        let app = App::new(Editor::default());
        for doc in app.docs.iter() {
            let s = doc.summary.trim();
            assert!(
                !s.is_empty(),
                "{}:{} has an empty summary",
                doc.kind,
                doc.name
            );
            assert!(
                !s.contains('\n'),
                "{}:{} summary must be one line",
                doc.kind,
                doc.name
            );
            assert!(
                s.chars().count() <= forge_docs::lint::SUMMARY_MAX,
                "{}:{} summary is {} columns",
                doc.kind,
                doc.name,
                s.chars().count()
            );
        }
    }
}

#[cfg(test)]
mod prefix_arg_tests {
    use super::*;

    #[test]
    fn a_long_run_of_c_u_does_not_overflow() {
        let mut app = App::new(Editor::default());
        for _ in 0..64 {
            app.universal_argument();
        }
        // The count is as large as a count can be, not a wrapped-around
        // zero.
        let st = app.arg_state.expect("the argument is still being typed");
        assert_eq!(st.raw, i64::MAX);
    }

    #[test]
    fn a_short_run_of_c_u_multiplies_by_four() {
        let mut app = App::new(Editor::default());
        app.universal_argument();
        app.universal_argument();
        let st = app.arg_state.expect("the argument is still being typed");
        assert_eq!(st.raw, 16);
    }
}

#[cfg(test)]
mod paste_tests {
    use super::*;
    use crate::minibuffer::Purpose;

    fn app() -> App {
        let mut app = App::new(Editor::default());
        let id = app.editor.add_buffer(forge_core::Buffer::new("*scratch*"));
        app.editor.show_buffer(id);
        app
    }

    #[test]
    fn a_pasted_newline_in_a_buffer_is_a_new_line() {
        let mut app = app();
        app.paste_text("\n");
        assert_eq!(app.buf_ref().get_contents(), "\n");
    }

    #[test]
    fn a_pasted_newline_in_a_prompt_ends_it() {
        let mut app = app();
        app.prompt("Test: ", Purpose::GotoLine);
        app.paste_text("\n");
        assert!(app.minibuffer.is_none(), "the newline ended the prompt");
    }
}
