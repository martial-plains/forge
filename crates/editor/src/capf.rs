// SPDX-License-Identifier: MIT OR Apache-2.0
//! Completion at point.
//!
//! `M-x completion-at-point` (C-M-i, M-TAB) collects candidates for the
//! word before point: words from the open buffers, plus whatever plugin
//! sources serve the buffer's mode (they may answer later, like a
//! language server). Without the popup, the common part is inserted and
//! the candidates are listed in the echo area. With it
//! (completion.set-at-point-ui), they're shown in a list at point that
//! filters as you type: C-n / C-p (or the arrows) move, TAB or RET
//! inserts, C-g closes, and anything else types as usual. In auto mode
//! the popup opens by itself once typing pauses after a few word
//! characters. Plugins are asked once per completion, never per key.
//!
//! This module is the popup itself, not a completion UI of its own: a
//! plugin (corfu is the one Forge ships) chooses how it behaves by
//! setting the popup's [`forge_core::completion::Policy`]
//! (completion.set-options) and drives it through
//! [`forge_core::completion::Control`] requests
//! (completion.next, completion.accept, ...). Every key a plugin wants
//! runs a command it registers; the commands here are the ones the
//! editor owns.

use crate::app::App;
use forge_core::buffer::is_word_char;
use forge_core::completion::{Control, Policy, QuickAction, Quit};
use forge_core::display::PopupSpec;
use forge_core::{BufferId, Pos};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

/// Most candidates the built-in word source offers.
const WORDS: usize = 200;

pub struct Session {
    buffer: BufferId,
    /// Where the completed text starts (char offset), at `version`.
    start: usize,
    /// The word the completion hangs off (char offset), at `version`: the
    /// same as `start`, unless a source asked for a region of its own (a
    /// path, an expression), which is larger than the word.
    word: usize,
    version: u64,
    /// No duplicates by `text`.
    candidates: Vec<forge_core::completion::Candidate>,
    matches: Vec<usize>,
    selected: usize,
    /// Plugin requests not answered yet.
    pending: BTreeSet<u32>,
    popup: bool,
    /// No candidate is selected (preselect 'prompt, or after a reset): RET
    /// and TAB expand or close instead of inserting.
    deselected: bool,
    /// Asked for by the user (M-TAB), not opened by auto mode.
    explicit: bool,
    /// The input `matches` are for.
    input: String,
}

/// Default cap on the auxiliary popup's height (corfu-popupinfo), when the
/// plugin asks for none.
const INFO_LINES: usize = 10;
/// Columns the auxiliary popup wraps at when the plugin asks for no
/// particular width.
const INFO_WIDTH: usize = 60;

#[derive(Default)]
pub struct State {
    pub session: Option<Session>,
    next_request: u32,
    /// Auto mode: when the last self-insert happened.
    typed_at: Option<Instant>,
    /// Quickselect: the first candidate of the group a first quick key
    /// chose, waiting for the second key.
    pub quick_start: Option<usize>,
    /// Quickselect: waiting for a quick key, and what to do with the
    /// candidate it names.
    pub quick_armed: Option<QuickAction>,
    /// History mode: candidates taken, most recent first.
    pub history: Vec<String>,
    history_loaded: bool,
    /// Where the history is kept (None: corfu-history in the state dir).
    pub history_file: Option<std::path::PathBuf>,
    /// Echo mode: the selection last looked at, when it changed, and
    /// whether its text was shown.
    echo_key: Option<(u32, String)>,
    echo_at: Option<Instant>,
    echo_shown: bool,
    /// corfu-popupinfo: turned off for the popup that is open (toggle).
    info_hidden: bool,
    /// The selection the auxiliary popup is for, and when it settled.
    info_key: Option<(u32, String)>,
    info_at: Option<Instant>,
    /// Show at once (a toggle), without waiting out the delay.
    info_now: bool,
    /// How far the auxiliary popup is scrolled into its lines.
    info_scroll: usize,
}

/// The popup's current policy (a plugin's, or the defaults).
fn policy(app: &App) -> Policy {
    app.editor.completion.policy()
}

/// The word before `offset`: (start, text).
fn word_before(app: &App, offset: usize) -> (usize, String) {
    let b = app.buf_ref();
    let p = b.offset_to_pos(offset);
    let line: Vec<char> = b.line(p.line).chars().collect();
    let mut s = p.col.min(line.len());
    while s > 0
        && (is_word_char(line[s - 1]) || line[s - 1] == '-' && s > 1 && is_word_char(line[s - 2]))
    {
        s -= 1;
    }
    (
        offset - (p.col - s),
        line[s..p.col.min(line.len())].iter().collect(),
    )
}

fn files_has(
    files: &[forge_core::completion::Candidate],
    c: &forge_core::completion::Candidate,
) -> bool {
    files.iter().any(|f| f.text == c.text)
}

/// cape-file: when point follows `dir/` (or `~/`, `./`, `../`), the
/// directory it names. Relative names are taken from the buffer's own.
fn path_dir(app: &App, start: usize) -> Option<std::path::PathBuf> {
    let b = app.buf_ref();
    let p = b.offset_to_pos(start);
    let line: Vec<char> = b.line(p.line).chars().collect();
    let col = p.col.min(line.len());
    if col == 0 || line[col - 1] != '/' {
        return None;
    }
    let mut s = col;
    while s > 0
        && !line[s - 1].is_whitespace()
        && !matches!(
            line[s - 1],
            '"' | '\''
                | '('
                | ')'
                | '<'
                | '>'
                | '['
                | ']'
                | '{'
                | '}'
                | '|'
                | ','
                | ';'
                | '`'
                | '='
        )
    {
        s -= 1;
    }
    let token: String = line[s..col].iter().collect();
    if token == "/" {
        return None;
    }
    let path = crate::minibuffer::expand_path(&token);
    let path = if path.is_absolute() {
        path
    } else {
        let base = b
            .path()
            .and_then(|f| f.parent().map(std::path::Path::to_path_buf))
            .filter(|d| !d.as_os_str().is_empty())
            .or_else(|| std::env::current_dir().ok())?;
        base.join(path)
    };
    path.is_dir().then_some(path)
}

/// The entries of `dir` as candidates, directories with a trailing `/`.
fn path_candidates(dir: &std::path::Path, hidden: bool) -> Vec<forge_core::completion::Candidate> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, bool)> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            (hidden || !name.starts_with('.')).then(|| (name, e.path().is_dir()))
        })
        .collect();
    out.sort();
    out.into_iter()
        .map(|(n, d)| {
            if d {
                forge_core::completion::Candidate::new(format!("{n}/"), Some("directory".into()))
            } else {
                forge_core::completion::Candidate::new(n, Some("file".into()))
            }
        })
        .collect()
}

pub fn execute(app: &mut App, cmd: &str) -> bool {
    match cmd {
        "completion-at-point" => start(app, false),
        _ => return false,
    }
    true
}

/// Drain the popup requests a plugin queued (completion.next, .accept,
/// ...), in order.
pub fn pump(app: &mut App) {
    if app.editor.completion_control.is_empty() {
        return;
    }
    // With no popup at point, the same controls drive the minibuffer's own
    // vertical list (a completion plugin like vertico asking to move it).
    for action in std::mem::take(&mut app.editor.completion_control) {
        let list = app.completion.session.is_none() && app.minibuffer.is_some();
        match action {
            Control::Next(n) if list => crate::minibuffer::cmd_list_step(app, n as isize),
            Control::Previous(n) if list => crate::minibuffer::cmd_list_step(app, -(n as isize)),
            Control::First if list => crate::minibuffer::cmd_list_first(app),
            Control::Last if list => crate::minibuffer::cmd_list_last(app),
            Control::Scroll { down: true } if list => crate::minibuffer::cmd_list_next_page(app),
            Control::Scroll { down: false } if list => crate::minibuffer::cmd_list_prev_page(app),
            Control::Next(n) => move_selection(app, n as isize),
            Control::Previous(n) => move_selection(app, -(n as isize)),
            Control::First => set_selected(app, |_| 0),
            Control::Last => set_selected(app, |n| n.saturating_sub(1)),
            Control::Scroll { down } => cmd_scroll(app, down),
            Control::Accept => cmd_insert(app),
            Control::Expand => cmd_expand(app),
            Control::Quit => cmd_abort(app),
            Control::Reset => cmd_reset(app),
            Control::AcceptRow(row) => accept_row(app, row as usize),
            Control::ArmQuick(action) => cmd_quick(app, action),
            Control::InsertSeparator => cmd_separator(app),
            Control::TogglePopupInfo => toggle_info(app),
            Control::PopupInfoScroll { down } => scroll_info(app, down),
            Control::PopupInfoBeginning => app.completion.info_scroll = 0,
            Control::PopupInfoEnd => app.completion.info_scroll = usize::MAX,
        }
    }
}

/// corfu-popupinfo-toggle: hide the auxiliary popup for this completion, or
/// bring it back at once.
fn toggle_info(app: &mut App) {
    app.completion.info_hidden = !app.completion.info_hidden;
    app.completion.info_now = true;
    app.completion.info_scroll = 0;
}

/// Scroll the auxiliary popup a page (the plugin's scroll keys).
fn scroll_info(app: &mut App, down: bool) {
    let page = app
        .editor
        .completion
        .info()
        .map(|i| i.max_height as usize)
        .filter(|h| *h > 0)
        .unwrap_or(INFO_LINES);
    let s = &mut app.completion.info_scroll;
    *s = if down {
        s.saturating_add(page)
    } else {
        s.saturating_sub(page)
    };
}

/// The popup policy of `forge.opt` (a plugin can also set it with
/// completion.set-options; both are the same policy).
pub fn apply_settings(app: &mut App, s: &forge_core::rhai::Settings) {
    let mut p = policy(app);
    let mut changed = false;
    let flag = |name: &str| s.flags.get(name).copied();
    if let Some(v) = flag("corfu_cycle") {
        p.cycle = v;
        changed = true;
    }
    if let Some(v) = flag("corfu_preselect_prompt") {
        p.preselect_prompt = v;
        changed = true;
    }
    if let Some(v) = flag("corfu_indexed") {
        p.indexed = v;
        changed = true;
    }
    if let Some(v) = flag("corfu_echo") {
        p.echo = v;
        changed = true;
    }
    if let Some(v) = flag("corfu_mouse") {
        p.mouse = v;
        changed = true;
    }
    if let Some(v) = flag("corfu_history") {
        p.history = v;
        changed = true;
        if v {
            load_history(app);
        }
    }
    if let Some(n) = s.numbers.get("corfu_echo_delay") {
        p.echo_delay_ms = (n * 1000.0).clamp(0.0, 60_000.0) as u32;
        changed = true;
    }
    for name in ["corfu_quick1", "corfu_quick2"] {
        if let Some(keys) = s
            .strings
            .get(name)
            .and_then(|v| v.first())
            .filter(|k| !k.is_empty())
        {
            if name == "corfu_quick1" {
                p.quick1 = keys.clone();
            } else {
                p.quick2 = keys.clone();
            }
            changed = true;
        }
    }
    if changed {
        let owner = app
            .editor
            .completion
            .policy
            .as_ref()
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| "config".into());
        app.editor.completion.policy = Some((p, owner));
    }
    // corfu-sort-override-function: inputs (regexp patterns) whose matches
    // keep the source order instead of being sorted.
    if let Some(pats) = s
        .strings
        .get("corfu_sort_override")
        .filter(|v| !v.is_empty())
    {
        app.editor.completion.sort_override = Some((pats.clone(), "config".into()));
    } else {
        app.editor.completion.sort_override = None;
    }
    // corfu-preselect: how the popup preselects.
    if let Some(name) = s.strings.get("corfu_preselect").and_then(|v| v.first())
        && let Ok(p) = forge_core::completion::Preselect::from_name(name)
    {
        app.editor.completion.preselect = Some((p, "config".into()));
    }
    // corfu-on-exact-match: what to do when the input exactly matches.
    if let Some(name) = s
        .strings
        .get("corfu_on_exact_match")
        .and_then(|v| v.first())
        && let Ok(e) = forge_core::completion::ExactMatch::from_name(name)
    {
        app.editor.completion.on_exact_match = Some((e, "config".into()));
    }
    // corfu-preview-current: commit the selected candidate when you type.
    if let Some(name) = s
        .strings
        .get("corfu_preview_current")
        .and_then(|v| v.first())
        && let Ok(p) = forge_core::completion::PreviewCurrent::from_name(name)
    {
        app.editor.completion.preview_current = Some((p, "config".into()));
    }
    // corfu-auto-trigger: characters that arm automatic completion.
    if let Some(parts) = s.strings.get("corfu_auto_trigger") {
        let chars: Vec<char> = parts.concat().chars().collect();
        app.editor.completion.auto_trigger = Some((chars, "config".into()));
    }
    // corfu-auto-commands: commands that may arm automatic completion.
    if let Some(cmds) = s
        .strings
        .get("corfu_auto_commands")
        .filter(|v| !v.is_empty())
    {
        app.editor.completion.auto_commands = Some((cmds.clone(), "config".into()));
    }
}

// ----------------------------------------------------------- history

const HISTORY_MAX: usize = 500;

fn history_path(app: &App) -> Option<std::path::PathBuf> {
    app.completion
        .history_file
        .clone()
        .or_else(|| forge_core::config::state_dir().map(|d| d.join("corfu-history")))
}

fn load_history(app: &mut App) {
    if app.completion.history_loaded {
        return;
    }
    app.completion.history_loaded = true;
    if let Some(text) = history_path(app).and_then(|p| std::fs::read_to_string(p).ok()) {
        app.completion.history = text
            .lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .take(HISTORY_MAX)
            .collect();
    }
}

fn remember(app: &mut App, text: &str) {
    if !policy(app).history || text.contains('\n') {
        return;
    }
    load_history(app);
    let h = &mut app.completion.history;
    h.retain(|t| t != text);
    h.insert(0, text.to_string());
    h.truncate(HISTORY_MAX);
    if let Some(path) = history_path(app) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, app.completion.history.join("\n") + "\n");
    }
}

/// Begin completing the word before point.
pub fn start(app: &mut App, auto: bool) {
    let buffer = app.active_id();
    let point = app.buf_ref().point_offset();
    let (start, prefix) = word_before(app, point);
    let settings = app.editor.completion.at_point();
    // Built-in: words from the open buffers.
    let mut candidates: Vec<forge_core::completion::Candidate> = Vec::new();
    if !prefix.is_empty()
        && let Some((_, words)) = forge_core::editing::dabbrev(&app.editor, buffer, point, WORDS)
    {
        candidates.extend(
            words
                .into_iter()
                .map(|w| forge_core::completion::Candidate::new(w, None)),
        );
    }
    // cape-file: file names after a directory.
    if let Some(dir) = path_dir(app, start) {
        let mut files = path_candidates(&dir, prefix.starts_with('.'));
        let extra: Vec<_> = candidates
            .drain(..)
            .filter(|c| !files_has(&files, c))
            .collect();
        files.extend(extra);
        candidates = files;
    }
    let version = app.buf_ref().version();
    app.completion.session = Some(Session {
        buffer,
        start,
        word: start,
        version,
        candidates,
        matches: Vec::new(),
        selected: 0,
        pending: BTreeSet::new(),
        popup: settings.popup,
        deselected: matches!(
            app.editor.completion.preselect(),
            forge_core::completion::Preselect::Prompt
        ),
        explicit: !auto,
        input: "\u{0}".into(),
    });
    let mode = app.buf_ref().major_mode().to_string();
    let sources: Vec<String> = app
        .editor
        .completion
        .sources
        .iter()
        .filter(|s| {
            s.modes.is_empty()
                || s.modes
                    .iter()
                    .any(|m| app.editor.modes.derives_from(&mode, m))
        })
        .map(|s| s.name.clone())
        .collect();
    let at = app.buf_ref().offset_to_pos(point);
    for source in sources {
        app.completion.next_request += 1;
        let request = app.completion.next_request;
        if let Some(s) = app.completion.session.as_mut() {
            s.pending.insert(request);
        }
        if let Some(mut h) = app.plugin_host.take() {
            let out = h.complete(&mut app.editor, &source, request, buffer, at);
            app.plugin_host = Some(h);
            crate::plugins::apply(app, out);
        }
    }
    update(app, !auto);
    publish_selection(app);
}

/// A plugin source answered.
pub fn delivered(
    app: &mut App,
    request: u32,
    start: Pos,
    candidates: Vec<forge_core::completion::Candidate>,
) {
    let prompt = matches!(
        app.editor.completion.preselect(),
        forge_core::completion::Preselect::Prompt
    );
    // The candidate the user had picked: a late answer must not throw
    // the choice away.
    let picked = app.completion.session.as_ref().and_then(|s| {
        if s.deselected {
            return None;
        }
        s.matches
            .get(s.selected)
            .map(|&i| s.candidates[i].text.clone())
    });
    // The region the source wants completed: it may be larger than the
    // word before point (a path, an expression).
    let region = match app.completion.session.as_ref() {
        Some(s) if app.active_id() == s.buffer => {
            let b = app.buf_ref();
            let region = b.pos_to_offset(start);
            let word = word_before(app, b.point_offset()).0;
            Some((region, word, b.version()))
        }
        _ => None,
    };
    if let Some(s) = app.completion.session.as_mut() {
        let merge = s.pending.remove(&request);
        if merge {
            // A session can have several sources: the new candidates join
            // the ones already there. A source that answers again for a
            // candidate already present replaces it, so the richer answer
            // (annotation, documentation, location) is the one shown.
            for c in candidates {
                if let Some(existing) = s.candidates.iter_mut().find(|e| e.text == c.text) {
                    *existing = c;
                } else {
                    s.candidates.push(c);
                }
            }
        } else {
            // Not an answer this session is waiting for: it is the whole
            // set, not an addition to what another source offered.
            s.candidates = candidates;
        }
        s.matches = (0..s.candidates.len()).collect();
        s.deselected = prompt;
        // The matches are every candidate again, so the next update has to
        // refilter them for what is typed.
        s.input = "\u{0}".into();
        // The source may want a region of its own, larger than the word
        // before point: that region is what the candidates replace.
        if let Some((region, word, version)) = region
            && region < s.start
        {
            s.start = region;
            s.word = word;
            s.version = version;
        }
    }
    update(app, true);
    // The candidate the user had picked is still there: keep it picked.
    if let (Some(text), Some(s)) = (picked, app.completion.session.as_mut())
        && let Some(at) = s.matches.iter().position(|&i| s.candidates[i].text == text)
    {
        s.selected = at;
        s.deselected = false;
    }
    publish_selection(app);
}

/// Does the policy keep the popup open when point left the completion
/// start? `start` is where the session started (the first component), so
/// the input `start..point` may span components the separator split.
fn continues(app: &App, p: &Policy, start: usize, point: usize) -> bool {
    if continued_command(&p.continue_commands, &app.last_command) {
        return true;
    }
    match p.quit_at_boundary {
        Quit::Never => true,
        Quit::Always => false,
        Quit::Separator => {
            // Stay once the input has crossed the component separator
            // (multi-component completion): the separator is what moved
            // point past the word, not leaving it behind.
            let contents = app.buf_ref().get_contents();
            contents
                .chars()
                .skip(start)
                .take(point.saturating_sub(start))
                .any(|c| c == p.separator)
        }
    }
}

/// Whether `name` is one of `commands` (corfu-continue-commands): an exact
/// name, or a `name*` prefix.
fn continued_command(commands: &[String], name: &str) -> bool {
    commands
        .iter()
        .any(|c| c == name || c.strip_suffix('*').is_some_and(|pre| name.starts_with(pre)))
}

/// corfu-preview-current 'insert': when the command that just ran is not a
/// continue command, commit the selected candidate. The command has already
/// run, so any text it typed sits just past the completed word: the
/// candidate replaces the word and that text follows it, with point moved
/// past.
fn commit_on_input(app: &mut App) {
    if app.editor.completion.preview_current() != forge_core::completion::PreviewCurrent::Insert {
        return;
    }
    let p = policy(app);
    if continued_command(&p.continue_commands, &app.this_command) {
        return;
    }
    let Some(s) = app.completion.session.as_ref() else {
        return;
    };
    if !s.popup || s.deselected || s.matches.is_empty() {
        return;
    }
    let index = s.matches[s.selected.min(s.matches.len() - 1)];
    let text = s.candidates[index].text.clone();
    let old_len = s.input.chars().count();
    let start = app.buf_ref().transform_offset(s.start, s.version);
    let appended = app.buf_ref().point_offset().saturating_sub(start + old_len);
    app.completion.session = None;
    let b = app.buf();
    b.replace_offsets(start, start + old_len, &text);
    b.set_offset(start + text.chars().count() + appended);
    b.break_undo_group();
    remember(app, &text);
    commit(app, &text, "exact");
    publish_selection(app);
}
/// Refilter after typing; close when point left the word. `explicit`:
/// the user asked (insert a sole match, say when there are none).
fn update(app: &mut App, explicit: bool) {
    let p = policy(app);
    if p.history {
        load_history(app);
    }
    let Some(s) = app.completion.session.as_ref() else {
        return;
    };
    if app.active_id() != s.buffer {
        app.completion.session = None;
        return;
    }
    let b = app.buf_ref();
    let start = b.transform_offset(s.start, s.version);
    // The word the completion hangs off: the word before point, unless a
    // source asked for a region of its own, which the word sits at the end
    // of.
    let word = b.transform_offset(s.word, s.version);
    let point = b.point_offset();
    if point < start {
        app.completion.session = None;
        return;
    }
    let (word_start, _) = word_before(app, point);
    if word_start != word && !continues(app, &p, start, point) {
        app.completion.session = None;
        return;
    }
    let input: String = b
        .get_contents()
        .chars()
        .skip(start)
        .take(point - start)
        .collect();
    let styles = app.editor.completion.styles().to_vec();
    let sorting = if app.editor.completion.sort_overridden(&input) {
        // corfu-sort-override-function: keep the source order.
        vec![forge_core::completion::Sort::Position]
    } else {
        app.editor.completion.sorting().to_vec()
    };
    use forge_core::completion::{ExactMatch, Preselect};
    let preselect = app.editor.completion.preselect();
    let exact_mode = app.editor.completion.on_exact_match();
    // corfu-on-exact-match: a single candidate already equal to the input
    // (so nothing more can be completed). `insert` commits it, `quit`
    // closes; `show`/`nil` fall through (show keeps it in the list).
    // corfu-on-exact-match: exactly one candidate and nothing more can be
    // completed from the typed input (the input already completes to it).
    use std::cmp::Ordering;
    let sole_exact = if input.is_empty() {
        None
    } else {
        app.completion
            .session
            .as_ref()
            .filter(|s| s.candidates.len() == 1)
            .and_then(|s| {
                let t = &s.candidates[0].text;
                let (l, r) = (t.to_lowercase(), input.to_lowercase());
                (matches!(
                    l.as_bytes().cmp(r.as_bytes()),
                    Ordering::Equal | Ordering::Greater
                ) && l.starts_with(&r))
                .then(|| t.clone())
            })
    };
    if let Some(text) = sole_exact {
        match exact_mode {
            ExactMatch::Insert => {
                let b_point = app.buf_ref().point_offset();
                app.buf().replace_offsets(start, b_point, &text);
                app.completion.session = None;
                commit(app, &text, "sole");
                publish_selection(app);
                return;
            }
            ExactMatch::Quit => {
                app.completion.session = None;
                publish_selection(app);
                return;
            }
            ExactMatch::Ignore | ExactMatch::Show => {}
        }
    }
    let keep_exact = matches!(exact_mode, ExactMatch::Show);
    let history = if p.history {
        app.completion.history.clone()
    } else {
        Vec::new()
    };
    let s = app.completion.session.as_mut().expect("checked");
    if s.input != input {
        let texts: Vec<String> = s.candidates.iter().map(|c| c.text.clone()).collect();
        let rank = |t: &str| -> Option<usize> { history.iter().position(|h| h == t) };
        s.matches = forge_core::completion::filter_with(&styles, &sorting, &input, &texts, &rank)
            .into_iter()
            .filter(|c| keep_exact || **c != input)
            .map(|c| {
                texts
                    .iter()
                    .position(|t| std::ptr::eq(t, c))
                    .expect("from the list")
            })
            .collect();
        s.selected = 0;
        s.deselected = match preselect {
            Preselect::Prompt => true,
            Preselect::First | Preselect::Directory => false,
            // corfu's default 'valid: select nothing only when the input is
            // itself a completion that is not the first candidate.
            Preselect::Valid => {
                let valid = s.candidates.iter().any(|c| c.text == input);
                let first_is_input = s
                    .matches
                    .first()
                    .is_some_and(|&i| s.candidates[i].text == input);
                valid && !first_is_input
            }
        };
        s.input = input.clone();
    }
    let (popup, empty, waiting) = (s.popup, s.matches.is_empty(), !s.pending.is_empty());
    let explicit = explicit || s.explicit;
    if empty {
        if !waiting {
            // quit-no-match: 'never keeps the (empty) popup alive, and
            // 'separator keeps it when a separator has been inserted.
            let quit = match p.quit_no_match {
                Quit::Always => true,
                Quit::Never => false,
                Quit::Separator => !input.contains(p.separator),
            };
            if quit {
                app.completion.session = None;
                if explicit {
                    app.msg("No match");
                }
            }
        }
        return;
    }
    // Without the popup, wait for every source, then complete in place.
    if !popup && explicit && !waiting {
        inline(app, start, &input);
    }
}

/// Without the popup: insert the common part, or the only candidate, and
/// list the rest in the echo area.
fn inline(app: &mut App, start: usize, input: &str) {
    let Some(s) = app.completion.session.as_ref() else {
        return;
    };
    let texts: Vec<String> = s
        .matches
        .iter()
        .map(|&i| s.candidates[i].text.clone())
        .collect();
    let common = texts.iter().skip(1).fold(texts[0].clone(), |acc, t| {
        acc.chars()
            .zip(t.chars())
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a)
            .collect()
    });
    let point = app.buf_ref().point_offset();
    if texts.len() == 1 {
        let text = texts[0].clone();
        app.buf().replace_offsets(start, point, &text);
        app.completion.session = None;
        commit(app, &text, "sole");
        return;
    }
    if common.chars().count() > input.chars().count() && common.starts_with(input) {
        app.buf().replace_offsets(start, point, &common);
    }
    let shown: Vec<&str> = texts.iter().take(8).map(String::as_str).collect();
    let more = if texts.len() > 8 { " ..." } else { "" };
    app.msg(format!("{{{}{more}}}", shown.join(" | ")));
    app.completion.session = None;
}

/// The prompt map for an open completion popup. Its keys can be rebound.
pub const PROMPT: &str = "completion-popup";

/// Keys while the popup is open: look the key up in the popup's prompt map
/// and run what it says. True if the key was the popup's. Quickselect is
/// data-driven, so its keys stay here.
pub fn handle_key(app: &mut App, key: KeyEvent) -> bool {
    match quick_key(app, &key) {
        Some(QuickPick::Candidate(row)) => {
            app.completion.quick_start = None;
            let action = app.completion.quick_armed.take();
            if app
                .completion
                .session
                .as_ref()
                .is_some_and(|s| row < s.matches.len())
            {
                if action == Some(QuickAction::Jump) {
                    if let Some(s) = app.completion.session.as_mut() {
                        s.selected = row;
                        s.deselected = false;
                    }
                    publish_selection(app);
                } else {
                    insert_at(app, row);
                }
                return true;
            }
        }
        Some(QuickPick::Group(start)) => {
            app.completion.quick_start = Some(start);
            return true;
        }
        None => {
            app.completion.quick_start = None;
            app.completion.quick_armed = None;
        }
    }
    let open = app
        .completion
        .session
        .as_ref()
        .is_some_and(|s| s.popup && !s.matches.is_empty());
    if !open {
        return false;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let found = {
        app.editor.prompt = Some(PROMPT.to_string());
        let answer = app.editor.active_keymaps_now().lookup_above(
            &[crate::minibuffer::describe(key, ctrl, alt)],
            forge_core::keymap::PRIORITY_PROMPT,
        );
        app.editor.prompt = None;
        match answer {
            forge_core::Lookup::Command { command, .. } => Some(command),
            _ => None,
        }
    };
    match found {
        Some(command) => {
            app.run_command(&command);
            true
        }
        None => false,
    }
}

/// Whether `key` is one of the quickselect keys, and which candidate it
/// names. None when quickselect is off or the key is something else.
///
/// The first `quick1` candidates take one key. Past that, candidates come
/// in groups of `quick1`: a `quick2` key picks the group, then a `quick1`
/// key the candidate in it, so `quick_start` remembers the group in
/// between.
fn quick_key(app: &App, key: &KeyEvent) -> Option<QuickPick> {
    let cfg = policy(app);
    if app.completion.quick_armed.is_none()
        || !app.editor.completion.at_point().popup
        || key.modifiers != KeyModifiers::NONE
    {
        return None;
    }
    let KeyCode::Char(c) = key.code else {
        return None;
    };
    let s = app.completion.session.as_ref().filter(|s| s.popup)?;
    let first: Vec<char> = cfg.quick1.chars().collect();
    let second: Vec<char> = cfg.quick2.chars().collect();
    let visible = popup_rows(s, app.editor.completion.at_point().lines.max(1) as usize);
    // Still waiting for the second key of a two-key pick: `quick_start` is
    // the first candidate of the group already chosen.
    if let Some(start) = app.completion.quick_start {
        let row = first.iter().position(|k| *k == c).map(|i| start + i)?;
        return visible.contains(&row).then_some(QuickPick::Candidate(row));
    }
    if let Some(row) = visible
        .iter()
        .find(|&&row| row < first.len() && first[row] == c)
    {
        return Some(QuickPick::Candidate(*row));
    }
    let group = second.iter().position(|k| *k == c)?;
    let start = first.len() + group * first.len();
    // Only offer the group if some of it is on screen.
    visible
        .iter()
        .any(|&row| row >= start && row < start + first.len())
        .then_some(QuickPick::Group(start))
}

/// What a quickselect keypress means.
enum QuickPick {
    /// Insert this candidate (a row of the popup).
    Candidate(usize),
    /// Wait for a second key; this is the group's first row.
    Group(usize),
}

/// How many rows the popup shows.
fn lines_shown(app: &App) -> usize {
    app.editor.completion.at_point().lines.max(1) as usize
}

/// The key (or two keys) that name candidate `row` in quick mode.
fn quick_label(cfg: &Policy, row: usize) -> Option<String> {
    let first: Vec<char> = cfg.quick1.chars().collect();
    let second: Vec<char> = cfg.quick2.chars().collect();
    if row < first.len() {
        return Some(first[row].to_string());
    }
    let group = (row - first.len()) / first.len();
    let g = second.get(group)?;
    Some(format!("{g}{}", first[(row - first.len()) % first.len()]))
}

/// The rows the popup is showing, as indices into `matches`.
fn popup_rows(s: &Session, lines: usize) -> Vec<usize> {
    let first = s
        .selected
        .saturating_sub(lines.saturating_sub(1))
        .min(s.matches.len().saturating_sub(lines));
    (first..first + lines)
        .filter(|&i| i < s.matches.len())
        .collect()
}

/// Insert the candidate at `index` among the matches.
fn insert_at(app: &mut App, index: usize) {
    let Some(s) = app.completion.session.as_mut() else {
        return;
    };
    s.selected = index;
    s.deselected = false;
    insert_selected(app);
}

/// Take the candidate under the cursor.
pub fn insert_selected(app: &mut App) {
    let Some(s) = app.completion.session.take() else {
        return;
    };
    let Some(&i) = s.matches.get(s.selected) else {
        return;
    };
    let text = s.candidates[i].text.clone();
    let status = commit_status(s.matches.len(), &text, &s.input);
    let b = app.buf();
    let start = b.transform_offset(s.start, s.version);
    let point = b.point_offset();
    b.replace_offsets(start, point, &text);
    b.break_undo_group();
    remember(app, &text);
    commit(app, &text, status);
}

/// The CAPF exit status of taking `text` (corfu's `:exit-function`):
/// "sole" when it was the only candidate, "exact" when it equalled the
/// input, else "finished".
fn commit_status(total: usize, text: &str, input: &str) -> &'static str {
    if total <= 1 {
        "sole"
    } else if text == input {
        "exact"
    } else {
        "finished"
    }
}

/// Tell the completion plugins a candidate was committed.
pub(crate) fn commit(app: &mut App, candidate: &str, status: &str) {
    let Some(mut host) = app.plugin_host.take() else {
        return;
    };
    let out = host.commit(&mut app.editor, candidate, status);
    app.plugin_host = Some(host);
    crate::plugins::apply(app, out);
}

/// After any other key: follow the typing (or close), and arm auto mode.
pub fn after_key(app: &mut App) {
    commit_on_input(app);
    if app.completion.session.is_some() {
        update(app, false);
    }
    publish_selection(app);
    let auto = app.editor.completion.at_point();
    let ch = app.this_command_keys.last().and_then(|k| k.chars().next());
    app.completion.typed_at = (auto.popup
        && auto.auto
        && app.completion.session.is_none()
        && app.editor.completion.auto_arms(&app.last_command, ch))
    .then(Instant::now);
}

/// How long until auto mode should look (None: not armed).
pub fn auto_delay(app: &App) -> Option<std::time::Duration> {
    let t = app.completion.typed_at?;
    let delay = std::time::Duration::from_millis(app.editor.completion.at_point().delay_ms as u64);
    Some(delay.saturating_sub(t.elapsed()))
}

/// Idle: open the popup if typing paused long enough after enough word
/// characters.
pub fn on_idle(app: &mut App) {
    echo_tick(app);
    info_tick(app);
    let Some(t) = app.completion.typed_at else {
        return;
    };
    let a = app.editor.completion.at_point();
    if t.elapsed().as_millis() < a.delay_ms as u128 || app.minibuffer.is_some() {
        return;
    }
    app.completion.typed_at = None;
    let point = app.buf_ref().point_offset();
    let (word_start, word) = word_before(app, point);
    if word.chars().count() >= a.min_prefix as usize || path_dir(app, word_start).is_some() {
        start(app, true);
    }
}

/// Echo mode: when the selected candidate has stayed put for the delay,
/// say what it is in the echo area.
fn echo_tick(app: &mut App) {
    if !policy(app).echo {
        return;
    }
    let Some(sel) = selected(app) else {
        app.completion.echo_key = None;
        app.completion.echo_shown = false;
        return;
    };
    let key = (sel.index, sel.text.clone());
    if app.completion.echo_key.as_ref() != Some(&key) {
        app.completion.echo_key = Some(key);
        app.completion.echo_at = Some(Instant::now());
        app.completion.echo_shown = false;
        return;
    }
    let delay = policy(app).echo_delay_ms;
    let waited = app
        .completion
        .echo_at
        .is_some_and(|t| t.elapsed().as_millis() >= delay as u128);
    if waited && !app.completion.echo_shown {
        app.completion.echo_shown = true;
        if let Some(note) = sel.annotation.filter(|n| !n.is_empty()) {
            app.msg(format!("{}: {note}", sel.text));
        }
    }
}

/// How soon the echo timer needs looking at (None: it doesn't).
pub fn echo_wait(app: &App) -> Option<Duration> {
    let echo = (policy(app).echo && !app.completion.echo_shown && selected(app).is_some())
        .then_some(Duration::from_millis(100));
    // The auxiliary popup waits out its delay on the same idle tick.
    [echo, info_wait(app)].into_iter().flatten().min()
}

/// corfu-popupinfo: when the selection changes, remember it and start the
/// delay; the popup appears once the delay passes with it unchanged.
fn info_tick(app: &mut App) {
    if app.editor.completion.info().is_none() {
        app.completion.info_key = None;
        return;
    }
    let Some(sel) = selected(app) else {
        app.completion.info_key = None;
        return;
    };
    let key = (sel.index, sel.text.clone());
    if app.completion.info_key.as_ref() != Some(&key) {
        app.completion.info_key = Some(key);
        app.completion.info_at = Some(Instant::now());
        app.completion.info_now = false;
        app.completion.info_scroll = 0;
    }
}

/// How soon the auxiliary popup's delay is up (None: nothing to wait for).
fn info_wait(app: &App) -> Option<Duration> {
    let info = app.editor.completion.info()?;
    if app.completion.info_hidden || app.completion.info_now || selected(app).is_none() {
        return None;
    }
    let delay = Duration::from_millis(info.delay_ms.min(5_000) as u64);
    let waited = app
        .completion
        .info_at
        .map_or(Duration::ZERO, |t| t.elapsed());
    Some(delay.saturating_sub(waited))
}

/// The popup to draw, if one is open.
pub fn popup(app: &App) -> Option<PopupSpec> {
    let s = app
        .completion
        .session
        .as_ref()
        .filter(|s| s.popup && !s.matches.is_empty())?;
    if app.active_id() != s.buffer {
        return None;
    }
    let cfg = policy(app);
    let lines = app.editor.completion.at_point().lines.max(1) as usize;
    let first = s
        .selected
        .saturating_sub(lines - 1)
        .min(s.matches.len().saturating_sub(lines));
    let armed = app.completion.quick_armed.is_some();
    let rows: Vec<forge_core::display::PopupRow> = s
        .matches
        .iter()
        .enumerate()
        .skip(first)
        .take(lines)
        .map(|(i, &k)| {
            let c = &s.candidates[k];
            // The key that names the row, when picking by key or number.
            let label = if armed {
                quick_label(&cfg, i)
            } else if cfg.indexed {
                (i - first < 10).then(|| (i - first).to_string())
            } else {
                None
            };
            let text = match label {
                Some(l) => format!("{l} {}", c.text),
                None if armed || cfg.indexed => format!("  {}", c.text),
                None => c.text.clone(),
            };
            forge_core::display::PopupRow {
                text,
                margin: c.margin.clone().unwrap_or_default(),
                annotation: c.annotation.clone().unwrap_or_default(),
                selected: i == s.selected && !s.deselected,
                deprecated: c.deprecated,
            }
        })
        .collect();
    let anchor = app.buf_ref().transform_offset(s.start, s.version);
    // Wide enough for the longest candidate and its annotation, then into
    // whatever range the plugin asked for.
    let wanted = rows
        .iter()
        .map(|r| r.text.chars().count() + r.annotation.chars().count() + 1)
        .max()
        .unwrap_or(1) as u32;
    // The window's text area is the hard limit on the popup's width.
    let available = app.editor.windows.selected().width as u32;
    let width = app.editor.completion.frame().width(wanted, available);
    let input = if s.input == "\u{0}" {
        String::new()
    } else {
        s.input.clone()
    };
    Some(PopupSpec {
        anchor,
        rows,
        input,
        width,
    })
}

/// The auxiliary popup to draw beside the completion popup
/// (corfu-popupinfo), if one should show: the selected candidate's
/// documentation and/or location, wrapped and scrolled. None when no
/// plugin asked, the selection is hidden, or it has neither to show.
pub fn popup_info(app: &App) -> Option<forge_core::display::InfoSpec> {
    let info = app.editor.completion.info()?;
    if app.completion.info_hidden {
        return None;
    }
    let sel = selected(app)?;
    // Wait for the selection to settle, unless a toggle said "now".
    if !app.completion.info_now {
        let waited = app
            .completion
            .info_at
            .map_or(Duration::ZERO, |t| t.elapsed());
        if waited.as_millis() < info.delay_ms as u128 {
            return None;
        }
    }
    let width = if info.max_width > 0 {
        info.max_width as usize
    } else {
        INFO_WIDTH
    };
    let height = if info.max_height > 0 {
        info.max_height as usize
    } else {
        INFO_LINES
    };
    let mut lines: Vec<String> = Vec::new();
    if info.show_documentation
        && let Some(doc) = sel.documentation.as_deref().filter(|d| !d.is_empty())
    {
        for raw in doc.lines() {
            lines.extend(wrap(raw, width));
        }
    }
    if info.show_location
        && let Some(loc) = &sel.location
    {
        lines.extend(wrap(&location_line(&sel, loc), width));
    }
    if lines.is_empty() {
        return None;
    }
    let max_scroll = lines.len().saturating_sub(info_height(&info));
    let scroll = app.completion.info_scroll.min(max_scroll);
    let visible: Vec<String> = lines.into_iter().skip(scroll).take(height).collect();
    Some(forge_core::display::InfoSpec {
        lines: visible,
        width: info.max_width,
    })
}

/// The auxiliary popup's height in rows (0 means Forge's default).
fn info_height(info: &forge_core::completion::Info) -> usize {
    if info.max_height > 0 {
        info.max_height as usize
    } else {
        INFO_LINES
    }
}

/// Where `loc` is, one line for the auxiliary popup: the file's name and
/// the 1-based line and column, after the candidate's text.
fn location_line(
    sel: &forge_core::completion::Selection,
    loc: &forge_core::completion::Location,
) -> String {
    let line = loc.line + 1;
    let col = loc.column + 1;
    match &loc.path {
        Some(p) => {
            let name = std::path::Path::new(p)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.to_string_lossy().into_owned());
            format!("{} ({}:{}:{})", sel.text, name, line, col)
        }
        None => format!("{} (line {}, column {})", sel.text, line, col),
    }
}

/// Wrap `text` to `width` columns at spaces. A word longer than `width`
/// stays on its own line (the frontend truncates it).
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if cur.is_empty() {
            cur.push_str(word);
        } else if forge_core::width::line_width(&cur) + 1 + forge_core::width::line_width(word)
            <= width
        {
            cur.push(' ');
            cur.push_str(word);
        } else {
            out.push(std::mem::take(&mut cur));
            cur.push_str(word);
        }
    }
    out.push(cur);
    out
}

/// The candidate under the cursor, for a plugin that wants to describe it
/// (corfu's M-h). None when no popup is open.
pub fn selected(app: &App) -> Option<forge_core::completion::Selection> {
    let s = app
        .completion
        .session
        .as_ref()
        .filter(|s| s.popup && !s.matches.is_empty())?;
    if app.active_id() != s.buffer {
        return None;
    }
    if s.deselected {
        return None;
    }
    let &i = s.matches.get(s.selected)?;
    let c = &s.candidates[i];
    Some(forge_core::completion::Selection {
        text: c.text.clone(),
        annotation: c.annotation.clone(),
        category: "point".into(),
        index: s.selected as u32 + 1,
        total: s.matches.len() as u32,
        documentation: c.documentation.clone(),
        location: c.location.clone(),
    })
}

/// Keep `editor.selection` and `editor.completion_session` in step with
/// whichever list is open, so `completion.selected` and `completion.current`
/// answer for plugins. When the selection changed, queue the commands
/// plugins registered with `completion.on-selection-changed` (consult's
/// live preview).
pub fn publish_selection(app: &mut App) {
    let before = app.editor.selection.clone();
    app.editor.selection = selected(app).or_else(|| crate::minibuffer::selected(app));
    app.editor.completion_session = session_info(app);
    if selection_changed(&before, &app.editor.selection) {
        queue_selection_commands(app);
    }
}

fn selection_changed(
    a: &Option<forge_core::completion::Selection>,
    b: &Option<forge_core::completion::Selection>,
) -> bool {
    match (a, b) {
        (None, None) => false,
        (Some(x), Some(y)) => x.text != y.text || x.index != y.index,
        _ => true,
    }
}

fn queue_selection_commands(app: &mut App) {
    let cmds: Vec<String> = app
        .editor
        .completion
        .selection_commands
        .iter()
        .map(|(_, c)| c.clone())
        .collect();
    for cmd in cmds {
        if app.editor.commands.contains(&cmd) {
            app.deferred_commands.push((cmd, None));
        }
    }
}

/// The popup's session for a plugin (completion.current).
fn session_info(app: &App) -> Option<forge_core::completion::Session> {
    let s = app.completion.session.as_ref()?;
    let input = if s.input == "\u{0}" {
        String::new()
    } else {
        s.input.clone()
    };
    Some(forge_core::completion::Session {
        open: true,
        popup: s.popup,
        index: s.selected as u32,
        total: s.matches.len() as u32,
        deselected: s.deselected,
        lines: lines_shown(app) as u32,
        input,
    })
}

// ------------------------------------------------ popup key commands

fn move_selection(app: &mut App, by: isize) {
    let cycle = policy(app).cycle;
    let Some(s) = app.completion.session.as_mut() else {
        return;
    };
    let n = s.matches.len();
    if n == 0 {
        return;
    }
    if s.deselected {
        // The first move picks an end rather than stepping from the top.
        s.deselected = false;
        s.selected = if by > 0 { 0 } else { n - 1 };
    } else {
        let to = s.selected as isize + by;
        s.selected = if cycle {
            to.rem_euclid(n as isize)
        } else {
            to.clamp(0, n as isize - 1)
        } as usize;
    }
    publish_selection(app);
}

/// C-n / Down: the next candidate.
pub fn cmd_next(app: &mut App) {
    move_selection(app, 1);
}

/// C-p / Up: the previous candidate.
pub fn cmd_prev(app: &mut App) {
    move_selection(app, -1);
}

/// M-m (corfu-move-to-minibuffer): move the open completion to the
/// minibuffer, where it is listed and filtered with the full prompt UI.
/// Picking a candidate puts it where the completed word was and completes
/// it as the popup would have; C-g leaves the buffer as it was.
pub fn cmd_move_to_minibuffer(app: &mut App) {
    let Some(s) = app.completion.session.take() else {
        app.error("No completion popup");
        return;
    };
    if !s.popup || s.matches.is_empty() {
        app.completion.session = Some(s);
        app.error("No completion popup");
        return;
    }
    let (start, version, point) = {
        let b = app.buf_ref();
        (
            b.transform_offset(s.start, s.version),
            b.version(),
            b.point_offset(),
        )
    };
    let input = if s.input == "\u{0}" {
        String::new()
    } else {
        s.input.clone()
    };
    let candidates: Vec<String> = s.candidates.iter().map(|c| c.text.clone()).collect();
    let notes: Vec<String> = s
        .candidates
        .iter()
        .map(|c| c.annotation.clone().unwrap_or_default())
        .collect();
    app.prompt(
        "Move to minibuffer: ",
        crate::minibuffer::Purpose::Transfer {
            buffer: s.buffer,
            start,
            len: point.saturating_sub(start),
            version,
            candidates,
            notes,
        },
    );
    if let Some(mb) = &mut app.minibuffer {
        mb.input = input;
    }
    publish_selection(app);
}

/// M-TAB: grow the input to the prefix every candidate shares. True if
/// the input grew.
fn expand(app: &mut App) -> bool {
    let Some(s) = app.completion.session.as_ref() else {
        return false;
    };
    let texts: Vec<String> = s
        .matches
        .iter()
        .map(|&i| s.candidates[i].text.clone())
        .collect();
    if texts.is_empty() {
        return false;
    }
    let common = texts.iter().skip(1).fold(texts[0].clone(), |acc, t| {
        acc.chars()
            .zip(t.chars())
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a)
            .collect()
    });
    // The text to grow is the one from the start of the completion to
    // point, wherever the buffer has moved it to since.
    let (start, input) = (
        app.buf_ref().transform_offset(s.start, s.version),
        s.input.clone(),
    );
    let point = app.buf_ref().point_offset();
    if common.chars().count() > input.chars().count() && common.starts_with(&input) {
        app.buf().replace_offsets(start, point, &common);
        return true;
    }
    false
}

/// M-TAB: grow the input to the prefix every candidate shares.
pub fn cmd_expand(app: &mut App) {
    if expand(app) {
        publish_selection(app);
    }
}

/// TAB and RET: take the candidate under the cursor. With indexed mode, a
/// numeric prefix names the row to take (C-u 2 TAB). With nothing
/// selected: grow the input, or close.
pub fn cmd_insert(app: &mut App) {
    let Some(s) = app.completion.session.as_ref() else {
        return;
    };
    if policy(app).indexed && app.has_arg() {
        let lines = lines_shown(app);
        let row = popup_rows(s, lines)
            .get(app.count().max(0) as usize)
            .copied();
        match row {
            Some(row) => insert_at(app, row),
            None => app.error("No such row"),
        }
        return;
    }
    if s.deselected {
        if !expand(app) {
            cmd_abort(app);
        }
        return;
    }
    insert_selected(app);
}

/// The candidate on popup row `row` (a plugin's indexed or mouse pick).
fn accept_row(app: &mut App, row: usize) {
    let lines = lines_shown(app);
    let Some(s) = app.completion.session.as_ref() else {
        return;
    };
    match popup_rows(s, lines).get(row).copied() {
        Some(i) => insert_at(app, i),
        None => app.error("No such row"),
    }
}

/// C-g and ESC: close the popup without taking anything.
pub fn cmd_abort(app: &mut App) {
    app.completion.session = None;
    app.completion.quick_start = None;
    app.completion.quick_armed = None;
    publish_selection(app);
}

/// M-<: the first candidate.
pub fn cmd_first(app: &mut App) {
    set_selected(app, |_| 0);
}

/// M->: the last candidate.
pub fn cmd_last(app: &mut App) {
    set_selected(app, |n| n.saturating_sub(1));
}

/// C-v / M-v: a page down or up the list, without wrapping.
pub fn cmd_scroll(app: &mut App, down: bool) {
    let page = (lines_shown(app) as isize - 1).max(1);
    let Some(s) = app.completion.session.as_mut() else {
        return;
    };
    let n = s.matches.len() as isize;
    if n == 0 {
        return;
    }
    let from = if s.deselected {
        if down { -1 } else { n }
    } else {
        s.selected as isize
    };
    s.selected = (from + if down { page } else { -page }).clamp(0, n - 1) as usize;
    s.deselected = false;
    publish_selection(app);
}

fn set_selected(app: &mut App, to: fn(usize) -> usize) {
    let Some(s) = app.completion.session.as_mut() else {
        return;
    };
    if s.matches.is_empty() {
        return;
    }
    s.selected = to(s.matches.len());
    s.deselected = false;
    publish_selection(app);
}

/// Reset: back to nothing selected; with nothing selected, close.
pub fn cmd_reset(app: &mut App) {
    let Some(s) = app.completion.session.as_mut() else {
        return;
    };
    if s.deselected {
        cmd_abort(app);
    } else {
        s.deselected = true;
        s.selected = 0;
        publish_selection(app);
    }
}

/// The next key names a candidate by the letter beside it (corfu-quick-*).
pub fn cmd_quick(app: &mut App, action: QuickAction) {
    if app
        .completion
        .session
        .as_ref()
        .is_some_and(|s| s.popup && !s.matches.is_empty())
    {
        app.completion.quick_armed = Some(action);
        app.completion.quick_start = None;
    } else {
        app.error("No completion popup");
    }
}

/// Insert the component separator and keep completing.
pub fn cmd_separator(app: &mut App) {
    if app.completion.session.is_none() {
        return;
    }
    let sep = policy(app).separator;
    app.buf().insert_str(&sep.to_string());
    update(app, true);
    publish_selection(app);
}

/// Mouse mode: a click on a row of the popup takes that candidate. True if
/// the click was the popup's.
pub fn click(app: &mut App, column: u16, row: u16) -> bool {
    if !policy(app).mouse {
        return false;
    }
    let Some(s) = app
        .completion
        .session
        .as_ref()
        .filter(|s| s.popup && !s.matches.is_empty())
    else {
        return false;
    };
    let Some((x, y, w, h)) = app.editor.windows.selected().popup_area else {
        return false;
    };
    if column < x || column >= x + w || row < y || row >= y + h {
        return false;
    }
    let lines = app.editor.completion.at_point().lines.max(1) as usize;
    let first = s
        .selected
        .saturating_sub(lines - 1)
        .min(s.matches.len().saturating_sub(lines));
    let index = first + (row - y) as usize;
    if index < s.matches.len() {
        insert_at(app, index);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::completion::{AtPoint, Candidate, Source};

    /// An app with one buffer holding `text`, the point at its end, and
    /// the completion popup on.
    fn app_with(text: &str) -> App {
        let mut app = App::new(forge_core::Editor::default());
        app.editor.completion.at_point = Some((
            AtPoint {
                popup: true,
                ..Default::default()
            },
            "test".to_string(),
        ));
        // A source that has not answered yet: the session waits for it.
        app.editor.completion.sources.push(Source {
            name: "test".to_string(),
            plugin: "test".to_string(),
            modes: vec![],
        });
        // The history of the taken candidates goes to a file of its own.
        app.completion.history_file = Some(std::env::temp_dir().join("forge-capf-test-history"));
        let id = app.editor.create_buffer("*test*");
        let b = app.editor.state.get_buffer_mut(id).unwrap();
        b.set_contents(text);
        b.set_offset(text.chars().count());
        app
    }

    fn candidate(text: &str) -> Candidate {
        Candidate::new(text, None)
    }

    /// The candidates of the session, in the order they are in.
    fn candidates(app: &App) -> Vec<String> {
        let s = app.completion.session.as_ref().expect("a session");
        s.candidates.iter().map(|c| c.text.clone()).collect()
    }

    /// The candidate the session has selected.
    fn selected(app: &App) -> String {
        let s = app.completion.session.as_ref().expect("a session");
        s.candidates[s.matches[s.selected]].text.clone()
    }

    #[test]
    fn a_source_that_answers_late_keeps_the_candidates_already_there() {
        let mut app = app_with("bazel banjo foo ba");
        start(&mut app, true);
        // The built-in source offers the words of the buffer.
        assert!(
            candidates(&app).contains(&"bazel".to_string()),
            "{:?}",
            candidates(&app)
        );
        // A plugin source answers as well.
        delivered(
            &mut app,
            1,
            Pos::new(0, 16),
            vec![candidate("banana"), candidate("bazel")],
        );
        let texts = candidates(&app);
        assert!(texts.contains(&"bazel".to_string()), "{texts:?}");
        assert!(texts.contains(&"banjo".to_string()), "{texts:?}");
        assert!(texts.contains(&"banana".to_string()), "{texts:?}");
        // A candidate that is already there is not added twice.
        assert_eq!(
            texts.iter().filter(|t| *t == "bazel").count(),
            1,
            "{texts:?}"
        );
    }

    #[test]
    fn a_late_answer_keeps_the_choice_the_user_made() {
        let mut app = app_with("bazel banjo foo ba");
        start(&mut app, true);
        cmd_next(&mut app);
        let picked = selected(&app);
        // A source answers late: the choice stands.
        delivered(&mut app, 1, Pos::new(0, 16), vec![candidate("banana")]);
        assert_eq!(selected(&app), picked);
    }

    #[test]
    fn a_source_can_complete_a_region_larger_than_a_word() {
        let mut app = app_with("long /usr/lo");
        start(&mut app, true);
        // The source wants the whole path, not just its last component.
        delivered(
            &mut app,
            1,
            Pos::new(0, 5),
            vec![candidate("/usr/local/bin")],
        );
        insert_selected(&mut app);
        assert_eq!(app.buf_ref().get_contents(), "long /usr/local/bin");
    }

    #[test]
    fn expand_grows_the_text_from_the_start_of_the_completion() {
        let mut app = app_with("X ba");
        start(&mut app, true);
        delivered(
            &mut app,
            1,
            Pos::new(0, 2),
            vec![candidate("banana"), candidate("band")],
        );
        // The text before the completion is edited: the start of the
        // completion moves with it.
        app.buf().replace_offsets(0, 0, "z");
        app.buf().set_offset(5);
        cmd_expand(&mut app);
        assert_eq!(app.buf_ref().get_contents(), "zX ban");
    }
}

#[cfg(test)]
mod path_tests {
    use super::*;

    #[test]
    fn a_path_in_the_buffer_completes_file_names() {
        let dir = std::env::temp_dir().join(format!("forge-capf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("main.rs"), "").unwrap();
        std::fs::write(dir.join(".hidden"), "").unwrap();
        let text = format!("let p = \"{}/", dir.display());
        let mut app = App::new(forge_core::Editor::default());
        app.editor.completion.at_point = Some((
            forge_core::completion::AtPoint {
                popup: true,
                ..Default::default()
            },
            "test".to_string(),
        ));
        let mut b = forge_core::Buffer::new("t.rs");
        b.set_contents(&text);
        b.set_point(forge_core::Pos::new(0, text.chars().count()));
        let id = app.editor.add_buffer(b);
        app.editor.show_buffer(id);
        start(&mut app, false);
        let names: Vec<String> = app
            .completion
            .session
            .as_ref()
            .expect("a session")
            .candidates
            .iter()
            .map(|c| c.text.clone())
            .collect();
        assert!(
            names.contains(&"src/".to_string()) && names.contains(&"main.rs".to_string()),
            "{names:?}"
        );
        assert!(!names.contains(&".hidden".to_string()));
    }
}
