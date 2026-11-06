// SPDX-License-Identifier: MIT OR Apache-2.0
//! `~/.config/forge/init.rhai`: configuration and commands in Rhai.
//!
//! The API mirrors the plugin API's WIT interfaces (keymap, command,
//! buffer, mode, ui):
//!
//! ```rhai
//! forge.opt["prefix_help_delay"] = 0.5;
//! forge.keymap.set("C-c g", "goto-line");             // C-c <letter> is yours
//! forge.keymap.set("n", "next-line", #{ mode: "help-mode" });
//! forge.keymap.set("C-c t", || forge.notify("hi"), #{ desc: "Say hi." });
//! forge.keymap.remap("kill-line", "kill-region");
//!
//! forge.command.create("greet", || {
//!   forge.ui.input(#{ prompt: "Name: " }, |name| {
//!     if name != () { forge.buffer.current().insert("Hello, " + name); }
//!   });
//! }, #{ desc: "Insert a greeting." });
//!
//! forge.plugin.grant("spell", #{ network: true, directories: ["~/dict"] });
//! ```
//!
//! Rhai runs inside Forge with no file or OS access. A call that runs
//! longer than [`TIME_LIMIT`] is stopped, and strings, arrays and maps
//! are capped at roughly [`MEMORY_LIMIT`].
//!
//! Like plugins, Rhai code is given the editor for the length of a call
//! and never waits for the user: `forge.ui.input` and friends take a
//! function that runs when the answer arrives, and `forge.command.run` /
//! notifications / questions take effect after the call returns.
//!
//! Positions are (line, column) with lines counted from 1 and columns
//! from 0, the same numbers the mode line shows (L1 C0).

use crate::app::{BufferId, Editor};
use crate::buffer::{Buffer, Pos};
use crate::decoration::LineNumbers;
use crate::keymap::{Condition, Origin, Scope, parse_keys};
use crate::plugin::host::Outcome;
use crate::plugin::runtime::{Effect, Level, LogLine, Question};
use rhai::Scope as RhaiScope;

/// The major mode of `*scratch*`: Rhai, with the keys that evaluate the
/// expression before point.
pub const INTERACTION_MODE: &str = "rhai-interaction-mode";
use rhai::{AST, Dynamic, Engine, EvalAltResult, FnPtr, ImmutableString, Map, Position};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A thread-safe view of Rhai calls, for a watchdog: is Rhai running, for
/// how long, and a way to stop it (C-g).
#[derive(Clone, Default)]
pub struct RhaiMonitor {
    since: Arc<Mutex<Option<Instant>>>,
    interrupt: Arc<AtomicBool>,
}

impl RhaiMonitor {
    pub fn busy(&self) -> Option<Duration> {
        self.since
            .lock()
            .expect("rhai monitor")
            .map(|t| t.elapsed())
    }

    pub fn interrupt(&self) {
        if self.busy().is_some() {
            self.interrupt.store(true, Ordering::SeqCst);
        }
    }
}

/// The name Rhai commands, modes and questions are filed under.
pub const SOURCE: &str = "init.rhai";
/// Longest a single call into Rhai may run.
pub const TIME_LIMIT: Duration = Duration::from_secs(2);
pub const MEMORY_LIMIT: usize = 64 * 1024 * 1024;

/// Options init.rhai can set through `forge.opt`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Settings {
    pub prefix_help_delay: Option<f64>,
    pub mouse: Option<bool>,
    /// Reload a plugin when its file changes (default on).
    pub plugin_auto_reload: Option<bool>,
    /// Line numbers in buffers that don't set their own.
    pub line_numbers: Option<crate::decoration::LineNumbers>,
    /// Indent and re-indent as you type (RET, closing brackets).
    pub electric_indent: Option<bool>,
    /// Long lines: "off" (cut off), "char" or "word" (wrapped).
    pub wrap: Option<crate::decoration::Wrap>,
    /// Where package indexes are (forge.opt.package_index: one or a list).
    pub package_index: Option<Vec<String>>,
    /// How many lines *Messages* keeps: t (unlimited), false (off) or a
    /// number of lines.
    pub message_log_max: Option<crate::messages::MessageLogMax>,
    /// Save foreign clipboard text to kill ring before killing.
    pub save_interprogram_paste_before_kill:
        Option<crate::clipboard::SaveInterprogramPasteBeforeKill>,
    /// The on/off options in [`FLAGS`], by name.
    pub flags: std::collections::BTreeMap<String, bool>,
    /// The number options in [`NUMBERS`], by name.
    pub numbers: std::collections::BTreeMap<String, f64>,
    /// The text options in [`STRINGS`] (one string, or a list of them).
    pub strings: std::collections::BTreeMap<String, Vec<String>>,
    /// The table options in [`MAPS`] (string keys to string values).
    pub maps: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
    /// forge.mode.hook: (mode, command) -- run the command in each buffer
    /// whose major mode is or derives from the mode runs a
    /// mode's hook (`add-hook 'prog-mode-hook`).
    pub mode_hooks: Vec<(String, String)>,
}

/// Number options: auto-saving, numbered backups, and how much history VC logs show.
pub const NUMBERS: &[&str] = &[
    "auto_save_interval",
    "auto_save_timeout",
    "corfu_echo_delay",
    "kept_new_versions",
    "kept_old_versions",
    "mark_ring_max",
    "org_clock_idle_minutes",
    "org_clock_rounding_minutes",
    "org_deadline_warning_days",
    "org_indent_per_level",
    "term_scrollback",
    "vc_log_show_limit",
    "vc_make_backup_files",
    "auto_revert_interval",
    "uniquify_min_dir_components",
    "left_margin_width",
    "right_margin_width",
];

/// Text options (a string, or a list of them): numbered backups, and the
/// version-control programs, their extra switches and the backends to try.
pub const STRINGS: &[&str] = &[
    "auth_source_files",
    "backup_directory",
    "browse_url_browser",
    "corfu_auto_commands",
    "corfu_auto_trigger",
    "corfu_on_exact_match",
    "corfu_preselect",
    "corfu_preview_current",
    "corfu_quick1",
    "corfu_quick2",
    "corfu_sort_override",
    "default_input_method",
    "default_transient_input_method",
    "dired_listing",
    "electric_quote_chars",
    "few_search_engine",
    "grep_find_ignored_directories",
    "grep_find_template",
    "grep_match_regexp",
    "grep_save_buffers",
    "grep_template",
    "image_file_name_extensions",
    "image_file_name_regexps",
    "org_agenda_files",
    "org_agenda_span",
    "org_capture_templates",
    "org_clock_in_switch_to_state",
    "org_clock_out_switch_to_state",
    "org_clocktable_indent_string",
    "org_crypt_key",
    "org_crypt_tag_matcher",
    "org_directory",
    "org_display_remote_inline_images",
    "org_id_extra_files",
    "org_publish_projects",
    "org_startup_folded",
    "icon_platform",
    "insert_directory_program",
    "icon_preference",
    "page_delimiter",
    "paragraph_separate",
    "paragraph_start",
    "search_engine",
    "sentence_end",
    "term_shell",
    "tool_bar_position",
    "tool_bar_style",
    "vc_git_annotate_switches",
    "vc_git_diff_switches",
    "vc_git_log_switches",
    "vc_git_program",
    "vc_handled_backends",
    "vc_hg_program",
    "vc_ignore_dir_regexp",
    "version_control",
    "webkit_search_engine",
    "uniquify_buffer_name_style",
];

/// Table options: `vc_project_backends` maps a project root to the
/// backend to use there (`{ ["/src/mixed"] = "Hg" }`).
pub const MAPS: &[&str] = &[
    "grep_files_aliases",
    "icon_overrides",
    "vc_project_backends",
];

/// On/off options.
pub const FLAGS: &[&str] = &[
    "auto_save_default",
    "case_fold_search",
    "corfu_cycle",
    "corfu_echo",
    "corfu_history",
    "corfu_indexed",
    "corfu_mouse",
    "corfu_preselect_prompt",
    "context_menu",
    "create_lockfiles",
    "delete_old_versions",
    "diff_refine",
    "editorconfig",
    "electric_pair",
    "electric_quote",
    "electric_quote_comment",
    "electric_quote_paragraph",
    "electric_quote_replace_double",
    "electric_quote_string",
    "fill_column_indicator",
    "grep_find_abbreviate",
    "grep_highlight_matches",
    "grep_use_headings",
    "column_number_mode",
    "hl_line",
    "line_number_mode",
    "size_indication_mode",
    "horizontal_scroll_bar",
    "image_animate_loop",
    "inhibit_message",
    "input_method_completion_everywhere",
    "make_backup_files",
    "menu_bar",
    "modifier_bar",
    "mouse_drag_copy_region",
    "no_littering",
    "org_clock_persist",
    "org_log_done",
    "delete_trailing_whitespace_on_save",
    "require_final_newline",
    "make_scripts_executable_on_save",
    "org_startup_indented",
    "org_update_dblocks_on_save",
    "recentf",
    "save_place",
    "savehist",
    "scroll_bar",
    "select_enable_clipboard",
    "select_enable_primary",
    "sentence_end_double_space",
    "sentence_end_without_period",
    "set_mark_command_repeat_pop",
    "show_paren",
    "show_trailing_whitespace",
    "smooth_scroll",
    "pixel_scroll_precision",
    "pixel_scroll_precision_mode",
    "tab_bar",
    "tool_bar",
    "tool_bar_always_show_default",
    "vc_annotate_colors",
    "vc_make_backup_files",
    "x_select_enable_clipboard_manager",
    "yank_pop_change_selection",
    "winner_mode",
    "global_auto_revert_mode",
    "global_auto_revert_non_file_buffers",
    "windmove_wrap_around",
    "vc_follow_symlinks",
];

/// The names `forge.opt` knows.
pub const OPTIONS: &[&str] = &[
    "auth_source_files",
    "auto_save_default",
    "auto_save_interval",
    "auto_save_timeout",
    "backup_directory",
    "case_fold_search",
    "corfu_auto_commands",
    "corfu_auto_trigger",
    "corfu_cycle",
    "corfu_echo",
    "corfu_echo_delay",
    "corfu_history",
    "corfu_indexed",
    "corfu_mouse",
    "corfu_on_exact_match",
    "corfu_preselect",
    "corfu_preselect_prompt",
    "corfu_preview_current",
    "corfu_quick1",
    "corfu_quick2",
    "corfu_sort_override",
    "context_menu",
    "create_lockfiles",
    "default_input_method",
    "default_transient_input_method",
    "dired_listing",
    "delete_old_versions",
    "diff_refine",
    "editorconfig",
    "electric_indent",
    "electric_pair",
    "electric_quote",
    "electric_quote_chars",
    "electric_quote_comment",
    "electric_quote_paragraph",
    "electric_quote_replace_double",
    "electric_quote_string",
    "fill_column_indicator",
    "grep_find_abbreviate",
    "grep_find_ignored_directories",
    "grep_find_template",
    "grep_files_aliases",
    "grep_highlight_matches",
    "grep_match_regexp",
    "grep_save_buffers",
    "grep_template",
    "grep_use_headings",
    "column_number_mode",
    "hl_line",
    "line_number_mode",
    "size_indication_mode",
    "horizontal_scroll_bar",
    "image_animate_loop",
    "image_file_name_extensions",
    "image_file_name_regexps",
    "inhibit_message",
    "input_method_completion_everywhere",
    "kept_new_versions",
    "kept_old_versions",
    "line_numbers",
    "make_backup_files",
    "mark_ring_max",
    "menu_bar",
    "message_log_max",
    "modifier_bar",
    "mouse",
    "mouse_drag_copy_region",
    "no_littering",
    "org_agenda_files",
    "org_agenda_span",
    "org_capture_templates",
    "org_clock_idle_minutes",
    "org_clock_in_switch_to_state",
    "org_clock_out_switch_to_state",
    "org_clock_persist",
    "org_clock_rounding_minutes",
    "org_clocktable_indent_string",
    "org_crypt_key",
    "org_crypt_tag_matcher",
    "org_deadline_warning_days",
    "org_directory",
    "org_display_remote_inline_images",
    "org_id_extra_files",
    "org_indent_per_level",
    "org_log_done",
    "delete_trailing_whitespace_on_save",
    "require_final_newline",
    "make_scripts_executable_on_save",
    "org_startup_folded",
    "org_startup_indented",
    "org_update_dblocks_on_save",
    "package_index",
    "epa_file_encrypt_to",
    "page_delimiter",
    "paragraph_separate",
    "paragraph_start",
    "plugin_auto_reload",
    "prefix_help_delay",
    "recentf",
    "save_interprogram_paste_before_kill",
    "sentence_end",
    "save_place",
    "savehist",
    "scroll_bar",
    "select_enable_clipboard",
    "select_enable_primary",
    "sentence_end_double_space",
    "sentence_end_without_period",
    "set_mark_command_repeat_pop",
    "show_paren",
    "show_trailing_whitespace",
    "smooth_scroll",
    "pixel_scroll_precision",
    "pixel_scroll_precision_mode",
    "tab_bar",
    "icon_overrides",
    "icon_platform",
    "insert_directory_program",
    "icon_preference",
    "term_scrollback",
    "term_shell",
    "tool_bar",
    "tool_bar_always_show_default",
    "tool_bar_position",
    "tool_bar_style",
    "vc_annotate_colors",
    "vc_git_annotate_switches",
    "vc_git_diff_switches",
    "vc_git_log_switches",
    "vc_git_program",
    "vc_handled_backends",
    "vc_hg_program",
    "vc_ignore_dir_regexp",
    "vc_log_show_limit",
    "vc_make_backup_files",
    "vc_project_backends",
    "version_control",
    "wrap",
    "x_select_enable_clipboard_manager",
    "yank_pop_change_selection",
    "winner_mode",
    "global_auto_revert_mode",
    "global_auto_revert_non_file_buffers",
    "windmove_wrap_around",
    "auto_revert_interval",
    "uniquify_min_dir_components",
    "left_margin_width",
    "right_margin_width",
    "uniquify_buffer_name_style",
    "vc_follow_symlinks",
];

/// State shared between the Rhai functions and the host.
#[derive(Default)]
struct Shared {
    /// The editor, while it is lent to Rhai for a call.
    editor: RefCell<Option<Editor>>,
    effects: RefCell<Vec<Effect>>,
    logs: RefCell<Vec<LogLine>>,
    settings: RefCell<Settings>,
    commands: RefCell<HashMap<String, Callback>>,
    /// forge.completion.group(category, fn): category -> callback.
    groups: RefCell<HashMap<String, Callback>>,
    answers: RefCell<HashMap<u32, Pending>>,
    next_request: Cell<u32>,
    /// (keys, command) bound, to warn about unknown commands later.
    bound: RefCell<Vec<(String, String)>>,
    /// Permissions granted with forge.plugin.grant.
    grants: RefCell<std::collections::BTreeMap<String, crate::permissions::Grants>>,
    /// Plugins declared with use_package, in the order declared.
    uses: RefCell<Vec<crate::declare::PluginUse>>,
    /// What each use_package did, for `use_package_report()`.
    use_log: RefCell<Vec<String>>,
    deadline: Cell<Option<Instant>>,
    /// forge.on(event, fn) handlers: (event name, function).
    handlers: RefCell<Vec<(String, Callback)>>,
    /// What happened to buffers since the handlers last heard.
    tracker: RefCell<crate::events::EventTracker>,
    tracking: Cell<bool>,
    /// The project whose init.rhai is running, so `#{ project: true }` in
    /// it means this project (see `crate::project`).
    project: RefCell<Option<std::path::PathBuf>>,
    /// init.rhai is running (plugins aren't loaded yet).
    in_init: Cell<bool>,
    /// Numbers forge.mode.hook's function hooks.
    next_hook: Cell<u32>,
    /// The AST of the script being run, so callbacks it defines can be
    /// called later.
    ast: RefCell<Option<AST>>,
}

impl Shared {
    /// The project whose init.rhai is running, else the one the active
    /// buffer is in: what `forge.project.root()` answers, and what
    /// `#{ project: true }` means.
    fn project_root(&self) -> Option<std::path::PathBuf> {
        if let Some(p) = self.project.borrow().clone() {
            return Some(p);
        }
        self.with_editor(|ed| {
            let path = ed
                .state
                .active_buffer()
                .and_then(|b| b.path())
                .map(Path::to_path_buf);
            Ok(path.as_deref().and_then(crate::project::root))
        })
        .ok()
        .flatten()
    }

    fn with_editor<R>(&self, f: impl FnOnce(&mut Editor) -> Result<R, String>) -> RhaiResult<R> {
        let mut e = self.editor.borrow_mut();
        let ed = e
            .as_mut()
            .ok_or_else(|| err("the editor is only available while Rhai code runs"))?;
        f(ed).map_err(err)
    }

    fn ask(&self, question: Question, callback: Pending) -> u32 {
        let request = self.next_request.get() + 1;
        self.next_request.set(request);
        self.answers.borrow_mut().insert(request, callback);
        self.effects.borrow_mut().push(Effect::Ask {
            plugin: SOURCE.into(),
            request,
            question,
        });
        request
    }

    /// Hold a callback for an answer that arrives after the call returns
    /// (a file operation, a finished process): the effect carrying it is
    /// queued by the caller.
    fn reserve(&self, callback: Pending) -> u32 {
        let request = self.next_request.get() + 1;
        self.next_request.set(request);
        self.answers.borrow_mut().insert(request, callback);
        request
    }
}

/// A callback waiting for an answer, or a select that also needs its
/// choices to report the chosen index.
#[derive(Clone)]
enum Pending {
    Callback(Callback),
    Text(Callback),
    Select {
        callback: Callback,
        items: Vec<String>,
    },
    Confirm(Callback),
}

/// A script callback: the function pointer and the AST that defined it.
#[derive(Clone)]
struct Callback {
    shared: std::rc::Rc<Shared>,
    func: FnPtr,
    ast: AST,
}

impl Callback {
    fn new(shared: &std::rc::Rc<Shared>, func: FnPtr) -> RhaiResult<Self> {
        let ast = shared
            .ast
            .borrow()
            .clone()
            .ok_or_else(|| err("a callback was made outside a script"))?;
        Ok(Self {
            shared: shared.clone(),
            func,
            ast,
        })
    }

    fn call_dynamic(&self, engine: &Engine, args: impl rhai::FuncArgs) -> RhaiResult<Dynamic> {
        let previous = self.shared.ast.replace(Some(self.ast.clone()));
        let result = self.func.call::<Dynamic>(engine, &self.ast, args);
        self.shared.ast.replace(previous);
        result
    }

    fn call_unit(&self, engine: &Engine, args: impl rhai::FuncArgs) -> RhaiResult<()> {
        self.call_dynamic(engine, args).map(|_| ())
    }

    fn call_values_unit(&self, engine: &Engine, mut args: Vec<Dynamic>) -> RhaiResult<()> {
        loop {
            match self.call_unit(engine, args.clone()) {
                Err(e) if is_arity_error(&e) && !args.is_empty() => {
                    args.pop();
                }
                other => return other,
            }
        }
    }

    /// Call with one argument, retrying with none when the callback takes
    /// no arguments (Rhai is stricter than Lua about that).
    fn call_one_unit(&self, engine: &Engine, arg: Dynamic) -> RhaiResult<()> {
        match self.call_unit(engine, (arg.clone(),)) {
            Err(e) if is_arity_error(&e) => self.call_unit(engine, ()),
            other => other,
        }
    }
}

fn is_arity_error(e: &EvalAltResult) -> bool {
    matches!(e, EvalAltResult::ErrorFunctionNotFound(..))
}

type RhaiResult<T> = Result<T, Box<EvalAltResult>>;

fn err(msg: impl Into<String>) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        msg.into().into(),
        Position::NONE,
    ))
}

fn type_err(msg: impl Into<String>) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        msg.into().into(),
        Position::NONE,
    ))
}

fn as_string(v: &Dynamic, what: &str) -> RhaiResult<String> {
    v.clone()
        .try_cast::<ImmutableString>()
        .map(|s| s.to_string())
        .or_else(|| v.clone().try_cast::<String>())
        .ok_or_else(|| type_err(format!("{what} is a string, not {}", v.type_name())))
}

fn as_bool(v: &Dynamic, what: &str) -> RhaiResult<bool> {
    v.clone()
        .try_cast::<bool>()
        .ok_or_else(|| type_err(format!("{what} is true or false, not {}", v.type_name())))
}

fn as_int(v: &Dynamic, what: &str) -> RhaiResult<i64> {
    v.clone()
        .try_cast::<i64>()
        .ok_or_else(|| type_err(format!("{what} is a number, not {}", v.type_name())))
}

fn as_map(v: Dynamic, what: &str) -> RhaiResult<Map> {
    v.try_cast::<Map>()
        .ok_or_else(|| type_err(format!("{what} is a map like #{{ ... }}")))
}

fn as_array(v: Dynamic, what: &str) -> RhaiResult<Vec<Dynamic>> {
    v.try_cast::<Vec<Dynamic>>()
        .ok_or_else(|| type_err(format!("{what} is an array like [ ... ]")))
}

fn map_get<'a>(m: &'a Map, key: &str) -> Option<&'a Dynamic> {
    m.get(key)
}

fn map_string(m: &Map, key: &str) -> RhaiResult<Option<String>> {
    match map_get(m, key) {
        None => Ok(None),
        Some(d) if d.is_unit() => Ok(None),
        Some(d) => Ok(Some(as_string(d, key)?)),
    }
}

fn map_bool(m: &Map, key: &str) -> RhaiResult<Option<bool>> {
    match map_get(m, key) {
        None => Ok(None),
        Some(d) if d.is_unit() => Ok(None),
        Some(d) => Ok(Some(as_bool(d, key)?)),
    }
}

#[allow(dead_code)]
fn map_array(m: &Map, key: &str) -> RhaiResult<Option<Vec<Dynamic>>> {
    match map_get(m, key) {
        None => Ok(None),
        Some(d) if d.is_unit() => Ok(None),
        Some(d) => Ok(Some(as_array(d.clone(), key)?)),
    }
}

/// Read an HTTP headers map (`#{ "User-Agent": "..." }`), in key order.
fn rhai_headers(t: &Map) -> RhaiResult<Vec<(String, String)>> {
    let mut out = Vec::new();
    for (k, v) in t {
        let name = k.to_string();
        let value = match v {
            v if v.is::<ImmutableString>() => as_string(v, "header value")?,
            v if v.is::<i64>() => as_int(v, "header value")?.to_string(),
            v if v.is::<f64>() => v.clone().cast::<f64>().to_string(),
            v if v.is::<bool>() => as_bool(v, "header value")?.to_string(),
            other => {
                return Err(type_err(format!(
                    "header value is a string, not {}",
                    other.type_name()
                )));
            }
        };
        out.push((name, value));
    }
    Ok(out)
}

/// A `require` name that stays inside ~/.config/forge/rhai.
#[allow(dead_code)]
fn valid_module_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        && !name.split('.').any(|s| s.is_empty() || s == "..")
}

fn unknown_option(name: &str) -> Box<EvalAltResult> {
    err(format!(
        "unknown option forge.opt[\"{name}\"] (known: {})",
        OPTIONS.join(", ")
    ))
}

/// `#{ mode: "help-mode" }` -> the mode keymap; nothing -> global.
///
/// `project` is the one scope that can be named two ways: a path names
/// that project, and `true` means the one this file is in (the project's
/// own init.rhai, where writing its own path out would be silly).
fn scope_from(opts: Option<Map>, shared: &Shared) -> RhaiResult<Scope> {
    let Some(t) = opts else {
        return Ok(Scope::Global);
    };
    if let Some(m) = map_string(&t, "mode")? {
        return Ok(Scope::Mode(m));
    }
    if let Some(project) = map_get(&t, "project")
        && !project.is_unit()
    {
        return match project.clone().try_cast::<bool>() {
            Some(true) => shared
                .project_root()
                .map(Scope::Project)
                .ok_or_else(|| err("no project here: no .forge or .git above this file")),
            Some(false) => Ok(Scope::Global),
            None => {
                let path = as_string(project, "project")?;
                let path = std::path::PathBuf::from(path);
                Ok(Scope::Project(std::fs::canonicalize(&path).unwrap_or(path)))
            }
        };
    }
    if let Some(name) = map_string(&t, "transient")? {
        return Ok(Scope::Transient(name));
    }
    if let Some(name) = map_string(&t, "prompt")? {
        return Ok(Scope::Prompt(name));
    }
    if let Some(name) = map_string(&t, "layer")? {
        return Ok(Scope::Layer(name));
    }
    if let Some(name) = map_string(&t, "region")? {
        return Ok(Scope::Region(name));
    }
    Ok(Scope::Global)
}

/// The part of a scope a command name can carry: the mode or map name, so
/// the same keys bound in two maps don't collide.
fn scope_tag(scope: &Scope) -> Option<String> {
    match scope {
        Scope::Global => None,
        Scope::Mode(m)
        | Scope::Transient(m)
        | Scope::Prompt(m)
        | Scope::Layer(m)
        | Scope::Region(m) => Some(m.clone()),
        Scope::Project(root) => Some(root.display().to_string()),
        Scope::Buffer(_) => None,
    }
}

// ---------------------------------------------------------------- buffers

/// A buffer seen from Rhai: `forge.buffer.current()`, `forge.buffer.find(name)`.
#[derive(Clone)]
struct RhaiBuffer {
    id: BufferId,
    shared: std::rc::Rc<Shared>,
}

impl RhaiBuffer {
    fn with<R>(&self, f: impl FnOnce(&mut Buffer) -> R) -> RhaiResult<R> {
        let id = self.id;
        self.shared.with_editor(|ed| {
            let b = ed
                .state
                .get_buffer_mut(id)
                .ok_or_else(|| format!("buffer {} was killed", id.0))?;
            Ok(f(b))
        })
    }

    fn register(engine: &mut Engine) {
        engine.register_type_with_name::<RhaiBuffer>("Buffer");
        engine.register_fn("id", |b: &mut RhaiBuffer| b.id.0);
        engine.register_fn("name", |b: &mut RhaiBuffer| -> RhaiResult<String> {
            b.with(|b| b.name().to_string())
        });
        engine.register_fn("path", |b: &mut RhaiBuffer| -> RhaiResult<Dynamic> {
            b.with(|b| match b.path() {
                Some(p) => Dynamic::from(p.display().to_string()),
                None => Dynamic::UNIT,
            })
        });
        engine.register_fn("modified", |b: &mut RhaiBuffer| -> RhaiResult<bool> {
            b.with(|b| b.is_modified())
        });
        engine.register_fn("text", |b: &mut RhaiBuffer| -> RhaiResult<String> {
            b.with(|b| b.get_contents())
        });
        engine.register_fn("line_count", |b: &mut RhaiBuffer| -> RhaiResult<i64> {
            b.with(|b| b.line_count() as i64)
        });
        engine.register_fn("line", |b: &mut RhaiBuffer, n: i64| -> RhaiResult<String> {
            b.with(|b| {
                if n >= 1 && (n as usize) <= b.line_count() {
                    b.line(n as usize - 1)
                } else {
                    String::new()
                }
            })
        });
        engine.register_fn("point", |b: &mut RhaiBuffer| -> RhaiResult<Dynamic> {
            b.with(|b| {
                let p = b.point();
                Dynamic::from_array(vec![
                    Dynamic::from(p.line as i64 + 1),
                    Dynamic::from(p.col as i64),
                ])
            })
        });
        engine.register_fn(
            "set_point",
            |b: &mut RhaiBuffer, line: i64| -> RhaiResult<()> {
                b.with(|b| b.set_point(rhai_pos(line as usize, 0)))
            },
        );
        engine.register_fn(
            "set_point",
            |b: &mut RhaiBuffer, line: i64, col: i64| -> RhaiResult<()> {
                b.with(|b| b.set_point(rhai_pos(line as usize, col as usize)))
            },
        );
        engine.register_fn(
            "insert",
            |b: &mut RhaiBuffer, text: String| -> RhaiResult<()> {
                b.with(|b| b.as_owner(|b| b.insert_str(&text)))
            },
        );
        engine.register_fn("region", |b: &mut RhaiBuffer| -> RhaiResult<Dynamic> {
            b.with(|b| match b.active_region() {
                Some((a, z)) => Dynamic::from_array(vec![
                    Dynamic::from(a.line as i64 + 1),
                    Dynamic::from(a.col as i64),
                    Dynamic::from(z.line as i64 + 1),
                    Dynamic::from(z.col as i64),
                ]),
                None => Dynamic::UNIT,
            })
        });
        engine.register_fn("region_text", |b: &mut RhaiBuffer| -> RhaiResult<Dynamic> {
            b.with(|b| match b.active_region() {
                Some((a, z)) => Dynamic::from(b.text_range(a, z)),
                None => Dynamic::UNIT,
            })
        });
        engine.register_fn(
            "replace",
            |b: &mut RhaiBuffer,
             l1: i64,
             c1: i64,
             l2: i64,
             c2: i64,
             text: String|
             -> RhaiResult<()> {
                b.with(|b| {
                    b.as_owner(|b| {
                        b.replace_range(
                            rhai_pos(l1 as usize, c1 as usize),
                            rhai_pos(l2 as usize, c2 as usize),
                            &text,
                        )
                    })
                })
            },
        );
        engine.register_fn(
            "search_forward",
            |b: &mut RhaiBuffer, q: String| -> RhaiResult<bool> {
                b.with(|b| match b.search_forward(&q, b.point()) {
                    Some((_, end)) => {
                        b.set_point(end);
                        true
                    }
                    None => false,
                })
            },
        );
        engine.register_fn(
            "search_backward",
            |b: &mut RhaiBuffer, q: String| -> RhaiResult<bool> {
                b.with(|b| match b.search_backward(&q, b.point()) {
                    Some((start, _)) => {
                        b.set_point(start);
                        true
                    }
                    None => false,
                })
            },
        );
        engine.register_fn("major_mode", |b: &mut RhaiBuffer| -> RhaiResult<String> {
            b.with(|b| b.major_mode().to_string())
        });
        engine.register_fn(
            "set_major_mode",
            |b: &mut RhaiBuffer, mode: String| -> RhaiResult<()> {
                let known = b.shared.with_editor(|ed| Ok(ed.modes.is_major(&mode)))?;
                if !known {
                    return Err(err(format!("unknown major mode {mode}")));
                }
                b.with(|b| b.set_major_mode(mode))
            },
        );
        engine.register_fn(
            "enable_minor_mode",
            |b: &mut RhaiBuffer, mode: String| -> RhaiResult<()> {
                let known = b.shared.with_editor(|ed| Ok(ed.modes.is_minor(&mode)))?;
                if !known {
                    return Err(err(format!("unknown minor mode {mode}")));
                }
                b.with(|b| b.enable_minor_mode(mode))
            },
        );
        engine.register_fn(
            "disable_minor_mode",
            |b: &mut RhaiBuffer, mode: String| -> RhaiResult<()> {
                b.with(|b| b.disable_minor_mode(&mode)).map(|_| ())
            },
        );
        engine.register_fn(
            "set_option",
            |b: &mut RhaiBuffer, name: String, value: Dynamic| -> RhaiResult<()> {
                let value = match value {
                    v if v.is_unit() => String::new(),
                    v if v.is::<bool>() => as_bool(&v, "option")?.to_string(),
                    v if v.is::<i64>() => as_int(&v, "option")?.to_string(),
                    v if v.is::<ImmutableString>() => as_string(&v, "option")?,
                    other => {
                        return Err(type_err(format!(
                            "set_option: {} isn't a value",
                            other.type_name()
                        )));
                    }
                };
                b.with(|b| b.set_option(&name, &value))
                    .and_then(|r| r.map_err(err))
            },
        );
        engine.register_fn(
            "option",
            |b: &mut RhaiBuffer, name: String| -> RhaiResult<Dynamic> {
                b.with(|b| b.option(&name))
                    .map(|o| o.map(Dynamic::from).unwrap_or(Dynamic::UNIT))
            },
        );
        engine.register_fn(
            "get_display_property",
            |b: &mut RhaiBuffer, line: i64, col: i64, name: String| -> RhaiResult<Dynamic> {
                b.with(|b| {
                    let pos = b.pos_to_offset(rhai_pos(line as usize, col as usize));
                    match b.display_prop(pos, &name) {
                        Some(s) => display_spec_dynamic(s),
                        None => Dynamic::UNIT,
                    }
                })
            },
        );
        engine.register_fn(
            "add_display_text_property",
            |b: &mut RhaiBuffer,
             l1: i64,
             c1: i64,
             l2: i64,
             c2: i64,
             spec: Dynamic|
             -> RhaiResult<()> {
                let spec = display_spec_from(spec)?;
                b.with(|b| {
                    let start = b.pos_to_offset(rhai_pos(l1 as usize, c1 as usize));
                    let end = b.pos_to_offset(rhai_pos(l2 as usize, c2 as usize));
                    b.add_display_prop(start, end, spec);
                })
            },
        );
        engine.register_fn(
            "remove_display_text_property",
            |b: &mut RhaiBuffer,
             l1: i64,
             c1: i64,
             l2: i64,
             c2: i64,
             name: String|
             -> RhaiResult<()> {
                b.with(|b| {
                    let start = b.pos_to_offset(rhai_pos(l1 as usize, c1 as usize));
                    let end = b.pos_to_offset(rhai_pos(l2 as usize, c2 as usize));
                    b.remove_display_prop(start, end, &name);
                })
            },
        );
        engine.register_fn(
            "add_overlay",
            |b: &mut RhaiBuffer,
             l1: i64,
             c1: i64,
             l2: i64,
             c2: i64,
             spec: Dynamic|
             -> RhaiResult<i64> {
                let spec = display_spec_from(spec)?;
                b.with(|b| {
                    let start = b.pos_to_offset(rhai_pos(l1 as usize, c1 as usize));
                    let end = b.pos_to_offset(rhai_pos(l2 as usize, c2 as usize));
                    b.add_overlay(start, end, crate::buffer::DisplayProps(vec![spec])) as i64
                })
            },
        );
        engine.register_fn(
            "remove_overlay",
            |b: &mut RhaiBuffer, id: i64| -> RhaiResult<()> {
                b.with(|b| b.remove_overlay(id as usize))
            },
        );
        engine.register_fn(
            "indent_line",
            |b: &mut RhaiBuffer, line: i64| -> RhaiResult<bool> {
                let id = b.id;
                b.shared.with_editor(|ed| {
                    ed.sync_language(id);
                    Ok(ed
                        .state
                        .get_buffer_mut(id)
                        .is_some_and(|b| b.indent_line((line as usize).saturating_sub(1))))
                })
            },
        );
        engine.register_fn("save", |b: &mut RhaiBuffer| -> RhaiResult<()> {
            b.with(|b| b.save().map_err(|e| e.to_string()))?
                .map_err(|e| format!("cannot save: {e}").into())
        });
        engine.register_fn(
            "set_text",
            |b: &mut RhaiBuffer, text: String| -> RhaiResult<()> {
                b.with(|b| {
                    let last = b.line_count().saturating_sub(1);
                    let end = rhai_pos(last + 1, b.line_len(last));
                    b.as_owner(|b| b.replace_range(rhai_pos(1, 0), end, &text))
                })
            },
        );
        engine.register_fn("show", |b: &mut RhaiBuffer| -> RhaiResult<()> {
            let id = b.id;
            b.with(|_| ())?;
            b.shared.with_editor(|ed| {
                ed.show_buffer(id);
                Ok(())
            })
        });
    }
}

/// Rhai positions: line from 1, column from 0.
fn rhai_pos(line: usize, col: usize) -> Pos {
    Pos::new(line.saturating_sub(1), col)
}

/// A display property value from Rhai (unit, a boolean, a number or a
/// string).
fn display_value(v: &Dynamic) -> RhaiResult<crate::buffer::DisplayValue> {
    Ok(if v.is_unit() {
        crate::buffer::DisplayValue::Nil
    } else if v.is::<bool>() {
        crate::buffer::DisplayValue::Bool(as_bool(v, "display value")?)
    } else if v.is::<i64>() {
        crate::buffer::DisplayValue::Int(as_int(v, "display value")?)
    } else if v.is::<f64>() {
        crate::buffer::DisplayValue::Real(v.clone().cast::<f64>())
    } else if v.is::<ImmutableString>() {
        crate::buffer::DisplayValue::Str(as_string(v, "display value")?)
    } else if let Some(list) = v.clone().try_cast::<Vec<Dynamic>>() {
        // `[N]` is N pixels and `["+", N]` / `["-", N]` are font steps,
        // as in `(N)`, `(+ N)` and `(- N)`.
        let num = |d: &Dynamic| {
            d.as_int()
                .ok()
                .map(|i| i as f64)
                .or_else(|| d.as_float().ok())
        };
        match list.as_slice() {
            [n] if num(n).is_some() => crate::buffer::DisplayValue::Pixels(num(n).unwrap_or(0.0)),
            [sign, n] if sign.is::<ImmutableString>() && num(n).is_some() => {
                let n = num(n).unwrap_or(0.0).round() as i64;
                match sign.clone().into_string().unwrap_or_default().as_str() {
                    "+" => crate::buffer::DisplayValue::Step(n),
                    "-" => crate::buffer::DisplayValue::Step(-n),
                    _ => return Err(type_err("a display step is [\"+\", N] or [\"-\", N]")),
                }
            }
            _ => {
                return Err(type_err(
                    "a display list is [pixels], [\"+\", steps] or [\"-\", steps]",
                ));
            }
        }
    } else {
        return Err(type_err(format!(
            "display property values are (), booleans, numbers or strings, not {}",
            v.type_name()
        )));
    })
}

/// A display specification: a map `#{ name: "face", fg: "..." }`, an
/// array `["face", ...]`, or `#{ "disable-eval": [ ... ] }`.
fn display_spec_from(v: Dynamic) -> RhaiResult<crate::buffer::DisplaySpec> {
    if let Some(map) = v.clone().try_cast::<Map>() {
        if let Some(inner) = map_get(&map, "disable-eval") {
            return display_spec_from(inner.clone());
        }
        let name =
            map_string(&map, "name")?.ok_or_else(|| err("a display specification needs a name"))?;
        let mut args = Vec::new();
        for (k, v) in &map {
            if k.as_str() == "name" || k.as_str() == "disable-eval" {
                continue;
            }
            args.push(crate::buffer::DisplayValue::Symbol(k.to_string()));
            args.push(display_value(v)?);
        }
        return Ok(crate::buffer::DisplaySpec { name, args });
    }
    let list = as_array(v, "a display specification")?;
    let mut it = list.into_iter();
    let name = match it.next() {
        Some(v) if v.is::<ImmutableString>() => as_string(&v, "display specification name")?,
        _ => return Err(err("a display specification needs a name")),
    };
    let mut args = Vec::new();
    for v in it {
        args.push(display_value(&v)?);
    }
    Ok(crate::buffer::DisplaySpec { name, args })
}

/// A display specification as a Rhai array: the name, then its arguments.
fn display_spec_dynamic(spec: &crate::buffer::DisplaySpec) -> Dynamic {
    let mut list = vec![Dynamic::from(spec.name.clone())];
    for v in &spec.args {
        list.push(match v {
            crate::buffer::DisplayValue::Nil => Dynamic::UNIT,
            crate::buffer::DisplayValue::Bool(b) => Dynamic::from(*b),
            crate::buffer::DisplayValue::Int(i) => Dynamic::from(*i),
            crate::buffer::DisplayValue::Real(r) => Dynamic::from(*r),
            crate::buffer::DisplayValue::Str(s) | crate::buffer::DisplayValue::Symbol(s) => {
                Dynamic::from(s.clone())
            }
            crate::buffer::DisplayValue::Pixels(p) => Dynamic::from_array(vec![Dynamic::from(*p)]),
            crate::buffer::DisplayValue::Step(n) => Dynamic::from_array(vec![
                Dynamic::from(if *n < 0 { "-" } else { "+" }.to_string()),
                Dynamic::from(n.abs()),
            ]),
        });
    }
    Dynamic::from_array(list)
}

// ------------------------------------------------------------------- host

pub struct RhaiConfig {
    engine: std::rc::Rc<std::cell::OnceCell<Engine>>,
    shared: std::rc::Rc<Shared>,
    monitor: RhaiMonitor,
    scope: RefCell<RhaiScope<'static>>,
}

impl RhaiConfig {
    fn engine(&self) -> &Engine {
        self.engine
            .get()
            .expect("the Rhai engine is built before a config is used")
    }
}

mod call;
mod setup;
mod state;

#[cfg(test)]
mod tests;

/// Show a value the way M-: does: strings and numbers bare (like
/// print), maps expanded to three deep, then `#{...}`. Long output is
/// cut at a thousand characters, with more behind it.
pub fn pretty(value: &Dynamic) -> String {
    let mut out = String::new();
    pretty_into(value, 0, false, &mut out);
    if out.chars().count() > 1000 {
        out.chars().take(1000).collect::<String>() + " …"
    } else {
        out
    }
}

fn pretty_into(value: &Dynamic, depth: usize, quote: bool, out: &mut String) {
    if value.is_unit() {
        out.push_str("()");
    } else if value.is::<bool>() {
        out.push_str(if value.clone().cast::<bool>() {
            "true"
        } else {
            "false"
        });
    } else if value.is::<ImmutableString>() {
        let s = as_string(value, "value").unwrap_or_default();
        if quote {
            out.push_str(&format!("{s:?}"));
        } else {
            out.push_str(&s);
        }
    } else if value.is::<i64>() {
        out.push_str(&format!("{}", value.clone().cast::<i64>()));
    } else if value.is::<f64>() {
        out.push_str(&format!("{}", value.clone().cast::<f64>()));
    } else if value.is::<Vec<Dynamic>>() {
        let list = value.clone().cast::<Vec<Dynamic>>();
        if depth >= 3 {
            out.push_str("[...]");
            return;
        }
        out.push('[');
        for (i, v) in list.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            pretty_into(v, depth + 1, true, out);
        }
        out.push(']');
    } else if value.is::<Map>() {
        let map = value.clone().cast::<Map>();
        if depth >= 3 {
            out.push_str("#{...}");
            return;
        }
        let mut keys: Vec<_> = map.keys().cloned().collect();
        keys.sort();
        out.push_str("#{");
        for (i, k) in keys.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(k.as_str());
            out.push_str(": ");
            if let Some(v) = map.get(k) {
                pretty_into(v, depth + 1, true, out);
            }
        }
        out.push('}');
    } else {
        out.push_str(&value.to_string());
    }
}

/// The useful part of a Rhai error.
fn describe(e: &EvalAltResult) -> String {
    e.to_string()
}

/// Parse a permission map, the shape expected by `forge.plugin.grant` and
/// `use_package`'s `grant` take.
fn parse_grants(spec: &Map) -> RhaiResult<crate::permissions::Grants> {
    use crate::permissions::{DirGrant, Grants, MAX_MEMORY_MB};
    let mut g = Grants::default();
    for (k, v) in spec {
        match k.as_str() {
            "network" => g.network = as_bool(v, "network")?,
            "environment" => g.environment = as_bool(v, "environment")?,
            "memory_mb" => {
                let n = as_int(v, "memory_mb")?;
                if n > 0 && n <= MAX_MEMORY_MB as i64 {
                    g.memory_mb = Some(n as u32);
                } else {
                    return Err(err(format!(
                        "memory_mb must be a number from 1 to {MAX_MEMORY_MB}"
                    )));
                }
            }
            "directories" => {
                let list = as_array(v.clone(), "directories")?;
                for d in list {
                    if let Some(path) = d.clone().try_cast::<ImmutableString>() {
                        g.directories.push(DirGrant {
                            path: path.to_string(),
                            write: false,
                            why: None,
                        });
                    } else if let Some(map) = d.clone().try_cast::<Map>() {
                        let path = map_string(&map, "path")?
                            .ok_or_else(|| err("a directory grant needs a path"))?;
                        g.directories.push(DirGrant {
                            path,
                            write: map_bool(&map, "write")?.unwrap_or(false),
                            why: None,
                        });
                    } else {
                        return Err(err("directories are paths or #{ path: ..., write: true }"));
                    }
                }
            }
            other => {
                return Err(err(format!(
                    "unknown permission {other:?} (known: network, environment, \
                     directories, memory_mb)"
                )));
            }
        }
    }
    Ok(g)
}

/// A name or a list of names (`on.commands`, `after`).
fn name_or_names(v: &Dynamic, what: &str) -> RhaiResult<Vec<String>> {
    if let Some(s) = v.clone().try_cast::<ImmutableString>() {
        Ok(vec![s.to_string()])
    } else if let Some(list) = v.clone().try_cast::<Vec<Dynamic>>() {
        list.iter()
            .map(|d| as_string(d, what))
            .collect::<RhaiResult<Vec<_>>>()
    } else {
        Err(err(format!("{what} is a name or a list of names")))
    }
}

/// `use_package(name, spec)`: record a declared plugin.
fn use_spec(s: &Shared, name: String, spec: &Map) -> RhaiResult<()> {
    use crate::declare::PluginUse;
    const KEYS: [&str; 12] = [
        "git", "rev", "index", "url", "sha256", "config", "grant", "hook", "on", "after",
        "version", "freeze",
    ];
    for k in spec.keys() {
        if !KEYS.contains(&k.as_str()) {
            return Err(err(format!(
                "unknown key {k:?} in a plugin declaration (known: {})",
                KEYS.join(", ")
            )));
        }
    }
    let mut u = PluginUse {
        name: name.clone(),
        git: map_string(spec, "git")?,
        rev: map_string(spec, "rev")?,
        index: map_string(spec, "index")?,
        version: map_string(spec, "version")?,
        frozen: map_bool(spec, "freeze")?.unwrap_or(false),
        url: map_string(spec, "url")?,
        sha256: map_string(spec, "sha256")?,
        ..Default::default()
    };
    if let Some(config) = map_get(spec, "config") {
        let json = rhai_to_json(config, 0)?;
        u.config = Some(json.clone());
        s.with_editor(|ed| {
            ed.plugin_config.insert(name.clone(), json);
            Ok(())
        })?;
    }
    if let Some(grant) = map_get(spec, "grant") {
        let g = parse_grants(&as_map(grant.clone(), "grant")?)?;
        s.grants
            .borrow_mut()
            .entry(name.clone())
            .or_default()
            .merge(&g);
        u.grants = g;
    }
    if let Some(hook) = map_get(spec, "hook") {
        for (mode, command) in as_map(hook.clone(), "hook")? {
            let mode = mode.to_string();
            let command = as_string(&command, "hook command")?;
            s.settings
                .borrow_mut()
                .mode_hooks
                .push((mode.clone(), command.clone()));
            u.hooks.push((mode, command));
        }
    }
    if let Some(on) = map_get(spec, "on") {
        let on = as_map(on.clone(), "on")?;
        for k in on.keys() {
            if !["commands", "modes", "extensions"].contains(&k.as_str()) {
                return Err(err(format!(
                    "unknown on key {:?} (known: commands, modes, extensions)",
                    k.as_str()
                )));
            }
        }
        u.on_commands = map_get(&on, "commands")
            .map(|v| name_or_names(v, "on.commands"))
            .transpose()?
            .unwrap_or_default();
        u.on_modes = map_get(&on, "modes")
            .map(|v| name_or_names(v, "on.modes"))
            .transpose()?
            .unwrap_or_default();
        u.on_extensions = map_get(&on, "extensions")
            .map(|v| name_or_names(v, "on.extensions"))
            .transpose()?
            .unwrap_or_default();
    }
    if let Some(after) = map_get(spec, "after") {
        u.after = name_or_names(after, "after")?;
    }
    let mut uses = s.uses.borrow_mut();
    if let Some(slot) = uses.iter_mut().find(|x| x.name == name) {
        *slot = u;
    } else {
        uses.push(u);
    }
    Ok(())
}

fn rhai_to_json(v: &Dynamic, depth: usize) -> RhaiResult<String> {
    if depth > 16 {
        return Err(err("plugin config is nested too deeply"));
    }
    Ok(if v.is_unit() {
        "null".to_string()
    } else if v.is::<bool>() {
        as_bool(v, "plugin config")?.to_string()
    } else if v.is::<i64>() {
        as_int(v, "plugin config")?.to_string()
    } else if v.is::<f64>() {
        let n = v.clone().cast::<f64>();
        if !n.is_finite() {
            return Err(err("plugin config numbers must be finite"));
        }
        n.to_string()
    } else if v.is::<ImmutableString>() {
        json_string(&as_string(v, "plugin config")?)
    } else if v.is::<Vec<Dynamic>>() {
        let list = v.clone().cast::<Vec<Dynamic>>();
        let mut parts = Vec::new();
        for item in &list {
            parts.push(rhai_to_json(item, depth + 1)?);
        }
        format!("[{}]", parts.join(","))
    } else if v.is::<Map>() {
        let map = v.clone().cast::<Map>();
        let mut parts = Vec::new();
        for (k, val) in &map {
            parts.push(format!(
                "{}:{}",
                json_string(k.as_str()),
                rhai_to_json(val, depth + 1)?
            ));
        }
        parts.sort();
        format!("{{{}}}", parts.join(","))
    } else {
        return Err(err(format!(
            "plugin config holds text, numbers, booleans and arrays/maps, not {}",
            v.type_name()
        )));
    })
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
