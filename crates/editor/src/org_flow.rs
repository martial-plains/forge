// SPDX-License-Identifier: MIT OR Apache-2.0
//! Org workflow commands (Phase 11, stage 4): capture, refile, archive,
//! clocking and the relative timer. The agenda is `org_agenda_ui.rs`.

use crate::app::App;
use crate::org_cmds::{Ask, ask, ask_with, choose, now};
use forge_core::org_time::{Date, Hm, hmm};
use forge_core::org_work::{
    CaptureCtx, CaptureTemplate, TemplateKind, default_capture_templates, expand_template,
    parse_capture_templates, template_prompts,
};
use forge_core::{Buffer, BufferId, Origin, Pos, Scope};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const COMMANDS: &[forge_docs::CommandDoc] = &[
    forge_docs::CommandDoc::new(
        "org-capture",
        "Org: capture a note, task or other item with a template",
    )
    .body("(forge.opt.org_capture_templates)."),
    forge_docs::CommandDoc::line(
        "org-capture-select-template",
        "Org: choose a capture template.",
    ),
    forge_docs::CommandDoc::line(
        "org-capture-finalize",
        "Org capture: file the captured text where its template says (C-c C-c).",
    ),
    forge_docs::CommandDoc::line(
        "org-capture-kill",
        "Org capture: throw the captured text away (C-c C-k).",
    ),
    forge_docs::CommandDoc::line(
        "org-capture-refile",
        "Org capture: file the captured text under a heading you pick (C-c C-w).",
    ),
    forge_docs::CommandDoc::line(
        "org-capture-goto-target",
        "Org capture: visit where this template files its text.",
    ),
    forge_docs::CommandDoc::new(
        "org-refile",
        "Org: move the subtree at point under another heading,",
    )
    .body("in this or any agenda file."),
    forge_docs::CommandDoc::new(
        "org-copy",
        "Org: copy the subtree at point under another heading,",
    )
    .body("in this or any agenda file."),
    forge_docs::CommandDoc::line(
        "org-archive-subtree",
        "Org: move the subtree to its archive file (with a prefix argument, ask where).",
    ),
    forge_docs::CommandDoc::line(
        "org-archive-subtree-default",
        "Org: move the subtree to its archive file, FILE_archive by default.",
    ),
    forge_docs::CommandDoc::line(
        "org-archive-to-archive-sibling",
        "Org: move the subtree under an Archive sibling heading tagged ARCHIVE.",
    ),
    forge_docs::CommandDoc::line(
        "org-toggle-archive-tag",
        "Org: add or remove the ARCHIVE tag on the current heading.",
    ),
    forge_docs::CommandDoc::line(
        "org-clock-in",
        "Org: start the clock on the current heading, stopping any other.",
    ),
    forge_docs::CommandDoc::line(
        "org-clock-in-last",
        "Org: start the clock on the heading clocked most recently.",
    ),
    forge_docs::CommandDoc::line(
        "org-clock-select-task",
        "Org: pick a recently clocked heading and clock in on it.",
    ),
    forge_docs::CommandDoc::line(
        "org-clock-update-time-maybe",
        "Org: recompute the duration of the CLOCK line at point.",
    ),
    forge_docs::CommandDoc::line("org-clock-out", "Org: stop the running clock."),
    forge_docs::CommandDoc::line("org-clock-cancel", "Org: discard the running clock."),
    forge_docs::CommandDoc::line(
        "org-clock-display",
        "Org: show the time clocked on this heading and the running clock.",
    ),
    forge_docs::CommandDoc::line(
        "org-clock-goto",
        "Org: go to the heading being clocked (with a prefix argument, the last clocked).",
    ),
    forge_docs::CommandDoc::line(
        "org-clock-modify-effort-estimate",
        "Org: set the Effort of the heading being clocked.",
    ),
    forge_docs::CommandDoc::line(
        "org-clock-report",
        "Org: insert (or update, inside one) a clock table.",
    ),
    forge_docs::CommandDoc::line(
        "org-resolve-clocks",
        "Org: close clocks that were left running, other than the current one.",
    ),
    forge_docs::CommandDoc::line(
        "org-clock-toggle-auto-clockout",
        "Org: toggle stopping the running clock after 15 minutes without input.",
    ),
    forge_docs::CommandDoc::new(
        "org-clocktable-try-shift",
        "Org: shift the time range of a clock table (not implemented:",
    )
    .body("clock tables here cover the whole file)."),
    forge_docs::CommandDoc::line("org-timer", "Org: insert the relative timer's time."),
    forge_docs::CommandDoc::line(
        "org-timer-start",
        "Org: start the relative timer at 0:00:00.",
    ),
    forge_docs::CommandDoc::line("org-timer-stop", "Org: stop the relative timer."),
    forge_docs::CommandDoc::line(
        "org-timer-pause-or-continue",
        "Org: pause the relative timer, or continue it.",
    ),
    forge_docs::CommandDoc::line(
        "org-timer-set-timer",
        "Org: set a countdown timer (minutes, or H:MM:SS).",
    ),
    forge_docs::CommandDoc::line(
        "org-timer-item",
        "Org: insert a list item starting with the relative timer's time.",
    ),
];

// ----------------------------------------------------------------- files

/// The agenda files, as buffers: the files named in `forge.opt.
/// org_agenda_files` and `~/.config/forge/org-agenda-files`, opened when
/// needed; with none named, the Org file buffers open now.
pub fn agenda_buffers(app: &mut App) -> Vec<BufferId> {
    app.editor.org_agenda_buffers()
}

fn file_list_path() -> Option<PathBuf> {
    forge_core::config::config_dir().map(|d| d.join("org-agenda-files"))
}

/// The configured agenda files.
pub fn agenda_files(app: &mut App) -> Vec<PathBuf> {
    app.editor.org_agenda_files()
}

pub fn save_agenda_files(app: &mut App) {
    let Some(p) = file_list_path() else { return };
    let text: String = app
        .editor
        .org_files
        .iter()
        .map(|f| format!("{}\n", f.display()))
        .collect();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Err(e) = std::fs::write(&p, text) {
        app.error(format!("Cannot write {}: {e}", p.display()));
    }
}

/// `forge.opt` settings for Org.
pub fn apply_settings(app: &mut App, s: &forge_core::rhai::Settings) {
    forge_core::org_work::set_clocktable_indent_string(
        s.strings
            .get("org_clocktable_indent_string")
            .and_then(|v| v.first())
            .cloned(),
    );
    app.org.directory = s
        .strings
        .get("org_directory")
        .and_then(|v| v.first())
        .map(|d| crate::vc::expand_home(d));
    app.org.crypt_key = s
        .strings
        .get("org_crypt_key")
        .and_then(|v| v.first())
        .cloned();
    app.org.crypt_tag_matcher = s
        .strings
        .get("org_crypt_tag_matcher")
        .and_then(|v| v.first())
        .cloned();
    forge_core::org::set_default_log_done(s.flags.get("org_log_done").copied().unwrap_or(false));
    if let Some(list) = s.strings.get("org_agenda_files") {
        for f in list {
            app.editor.add_org_agenda_file(crate::vc::expand_home(f));
        }
    }
    app.org.capture_templates = match s.strings.get("org_capture_templates") {
        Some(list) => match parse_capture_templates(list) {
            Ok(t) => Some(t),
            Err(e) => {
                app.error(format!("forge.opt.org_capture_templates: {e}"));
                None
            }
        },
        None => None,
    };
}

fn org_directory(app: &App) -> PathBuf {
    app.org
        .directory
        .clone()
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("org")))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn resolve_in_org_dir(app: &App, file: &str) -> PathBuf {
    let f = crate::vc::expand_home(file);
    if f.is_absolute() {
        f
    } else {
        org_directory(app).join(f)
    }
}

/// A short name for a buffer in prompts.
pub fn buffer_label(app: &App, id: BufferId) -> String {
    app.editor
        .state
        .get_buffer(id)
        .map(|b| {
            b.path()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| b.name().to_string())
        })
        .unwrap_or_default()
}

fn buf_mut(app: &mut App, id: BufferId) -> Option<&mut Buffer> {
    app.editor.state.get_buffer_mut(id)
}

fn buf_of(app: &App, id: BufferId) -> Option<&Buffer> {
    app.editor.state.get_buffer(id)
}

fn heading_of_point(app: &App) -> Option<usize> {
    let b = app.buf_ref();
    b.org_heading_at(b.point().line)
}

// ------------------------------------------------------------------ clock

pub struct RunningClock {
    pub buffer: BufferId,
    /// The heading line when the clock started, to find it again.
    pub heading: String,
    pub since: (Date, Hm),
}

/// Heading line in `id` whose text is `text` and has an open clock; else
/// the first open clock's heading.
fn find_running(app: &App, id: BufferId, text: &str) -> Option<usize> {
    let b = buf_of(app, id)?;
    let opens = b.org_open_clocks();
    let hs: Vec<usize> = opens.iter().filter_map(|&l| b.org_heading_at(l)).collect();
    hs.iter()
        .copied()
        .find(|&h| b.line(h) == text)
        .or_else(|| hs.first().copied())
}

fn setting_str(app: &App, key: &str) -> Option<String> {
    let s = app.rhai.as_ref()?.settings();
    s.strings
        .get(key)
        .and_then(|v| v.first())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn setting_num(app: &App, key: &str) -> i64 {
    app.rhai
        .as_ref()
        .and_then(|l| l.settings().numbers.get(key).copied())
        .map_or(0, |n| n as i64)
}

/// A number option, if the user set it.
pub(crate) fn setting_number(app: &App, key: &str) -> Option<f64> {
    app.rhai
        .as_ref()
        .and_then(|l| l.settings().numbers.get(key).copied())
}

/// A text option's first value, if the user set it.
pub(crate) fn setting_string(app: &App, key: &str) -> Option<String> {
    setting_strings(app, key).into_iter().next()
}

/// A text option's values (it may be a list), empty when unset.
pub(crate) fn setting_strings(app: &App, key: &str) -> Vec<String> {
    app.rhai
        .as_ref()
        .and_then(|l| l.settings().strings.get(key).cloned())
        .unwrap_or_default()
}

pub(crate) fn setting_flag(app: &App, key: &str) -> bool {
    app.rhai
        .as_ref()
        .and_then(|l| l.settings().flags.get(key).copied())
        .unwrap_or(false)
}

/// Now, rounded as `org-clock-rounding-minutes` asks.
fn clock_now(app: &App) -> (Date, Hm) {
    let (d, n) = now();
    forge_core::org_work::round_clock_time(d, n, setting_num(app, "org_clock_rounding_minutes"))
}

/// `org-clock-in-switch-to-state` / `-out-`: put an entry that is in a
/// not-done state into the state named by option `key`.
fn switch_state(app: &mut App, id: BufferId, h: usize, key: &str) {
    let Some(state) = setting_str(app, key) else {
        return;
    };
    let Some(b) = buf_mut(app, id) else { return };
    let kw = forge_core::org::keywords(b);
    let Some(hd) = forge_core::org::parse_heading(&b.line(h), &kw) else {
        return;
    };
    if let Some(k) = hd.keyword
        && !kw.is_done(&k)
        && k != state
    {
        let _ = b.org_todo_to(h, true, Some(Some(state)));
    }
}

/// Stop the running clock. Returns the minutes it ran.
pub fn clock_out_running(app: &mut App, quiet: bool) -> Option<i64> {
    let when = clock_now(app);
    clock_out_running_at(app, quiet, when)
}

/// Stop the running clock as of `(d, n)`.
pub fn clock_out_running_at(app: &mut App, quiet: bool, (d, n): (Date, Hm)) -> Option<i64> {
    let rc = app.org.clock.take()?;
    let h = find_running(app, rc.buffer, &rc.heading)?;
    let mins = buf_mut(app, rc.buffer)?.org_clock_out(h, d, n).ok()?;
    switch_state(app, rc.buffer, h, "org_clock_out_switch_to_state");
    let line = buf_of(app, rc.buffer)
        .map(|b| b.line(h))
        .unwrap_or_default();
    app.org.last_clock = Some((rc.buffer, line.clone()));
    if !quiet {
        app.msg(format!(
            "Clock stopped at {:02}:{:02} ({}) on {}",
            n.0,
            n.1,
            hmm(mins),
            line.trim_start_matches('*').trim()
        ));
    }
    Some(mins)
}

/// Start clocking heading `h` of buffer `id`.
pub fn clock_in_at(app: &mut App, id: BufferId, h: usize) {
    clock_out_running(app, true);
    let (d, n) = clock_now(app);
    switch_state(app, id, h, "org_clock_in_switch_to_state");
    let Some(b) = buf_mut(app, id) else { return };
    match b.org_clock_in(h, d, n) {
        Ok(()) => {
            let line = b.line(h);
            app.org.clock = Some(RunningClock {
                buffer: id,
                heading: line.clone(),
                since: (d, n),
            });
            app.org.last_clock = Some((id, line.clone()));
            app.org
                .clock_history
                .retain(|(i, l)| !(*i == id && *l == line));
            app.org.clock_history.insert(0, (id, line.clone()));
            app.org.clock_history.truncate(8);
            app.msg(format!(
                "Clock started at {:02}:{:02} on {}",
                n.0,
                n.1,
                line.trim_start_matches('*').trim()
            ));
        }
        Err(e) => app.error(e),
    }
}

/// `org-clock-persist`: a clock left running in a file (the editor was
/// closed) becomes the running clock again, once per buffer.
fn adopt_open_clock(app: &mut App) {
    if app.org.clock.is_some() || !setting_flag(app, "org_clock_persist") {
        return;
    }
    let ids: Vec<BufferId> = app
        .editor
        .state
        .buffers
        .keys()
        .copied()
        .filter(|id| !app.org.clock_checked.contains(id))
        .collect();
    for id in ids {
        app.org.clock_checked.push(id);
        let Some(b) = buf_of(app, id) else { continue };
        if b.major_mode() != forge_core::org::MODE {
            continue;
        }
        let open = b
            .org_open_clocks()
            .into_iter()
            .filter_map(|l| forge_core::org_work::parse_clock_line(&b.line(l), l))
            .max_by_key(|c| c.start.minutes());
        let Some(c) = open else { continue };
        let Some(h) = b.org_heading_at(c.line) else {
            continue;
        };
        let line = b.line(h);
        let since = (c.start.date, c.start.time.unwrap_or((0, 0)));
        app.org.clock = Some(RunningClock {
            buffer: id,
            heading: line.clone(),
            since,
        });
        app.org.last_clock = Some((id, line.clone()));
        app.msg(format!(
            "Resumed the clock on {} (since {:02}:{:02})",
            line.trim_start_matches('*').trim(),
            since.1.0,
            since.1.1
        ));
        return;
    }
}

/// `org-clock-idle-time`: when you come back after being idle longer
/// than `org_clock_idle_minutes` with a clock running, ask what to do
/// with the idle time.
fn check_idle(app: &mut App) {
    let limit = setting_num(app, "org_clock_idle_minutes").max(0) as u64 * 60;
    if limit == 0 || app.org.clock.is_none() {
        app.org.idle_secs = 0;
        return;
    }
    let idle = app.last_input.elapsed().as_secs();
    if idle >= limit {
        app.org.idle_secs = app.org.idle_secs.max(idle);
    } else if app.org.idle_secs >= limit && app.minibuffer.is_none() {
        let secs = std::mem::take(&mut app.org.idle_secs);
        choose(
            app,
            &format!("Clock running while idle for {} min: ", secs / 60),
            Ask::ClockIdle(secs),
            [
                "keep",
                "subtract and clock out",
                "subtract and keep clocking",
                "cancel clock",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        );
    }
}

/// The idle prompt's answer.
fn idle_answer(app: &mut App, secs: u64, t: &str) {
    let (d, n) = now();
    let back = forge_core::org_work::minutes_before(d, n, (secs / 60) as i64);
    match t {
        "keep" => app.msg("Idle time kept".to_string()),
        "cancel clock" => {
            clock_cancel_running(app);
        }
        "subtract and clock out" | "subtract and keep clocking" => {
            let again = app
                .org
                .clock
                .as_ref()
                .map(|rc| (rc.buffer, rc.heading.clone()));
            if clock_out_running_at(app, true, back).is_none() {
                return app.error("No clock to stop");
            }
            app.msg(format!("Subtracted {} min idle", secs / 60));
            if t.ends_with("clocking")
                && let Some((id, text)) = again
                && let Some(h) = find_heading_line(app, id, &text)
                    .or_else(|| find_heading_line_loose(app, id, &text))
            {
                clock_in_at(app, id, h);
            }
        }
        _ => {}
    }
}

/// The heading whose title matches `text`'s, when its keyword changed.
fn find_heading_line_loose(app: &App, id: BufferId, text: &str) -> Option<usize> {
    let want = text.trim_start_matches('*').trim().to_string();
    let b = buf_of(app, id)?;
    let kw = forge_core::org::keywords(b);
    (0..b.line_count()).find(|&l| {
        b.org_level(l).is_some()
            && forge_core::org::parse_heading(&b.line(l), &kw)
                .is_some_and(|h| want.ends_with(&h.title))
    })
}

/// The running clock for the mode line: `[0:12/1:00] (Heading)`, with the
/// Effort after a slash when the heading has one.
pub fn mode_line(app: &App) -> Option<String> {
    let rc = app.org.clock.as_ref()?;
    let (d, n) = now();
    let since = forge_core::org_time::Timestamp::new(false, rc.since.0, Some(rc.since.1));
    let here = forge_core::org_time::Timestamp::new(false, d, Some(n));
    let mins = (here.minutes() - since.minutes()).max(0);
    let mut out = format!("[{}", hmm(mins));
    let b = buf_of(app, rc.buffer)?;
    if let Some(h) = find_running(app, rc.buffer, &rc.heading)
        && let Some(e) = b.org_get_property(h, "Effort", false)
    {
        out.push('/');
        out.push_str(&e);
    }
    let kw = forge_core::org::keywords(b);
    let title = forge_core::org::parse_heading(&rc.heading, &kw).map_or_else(
        || rc.heading.trim_start_matches('*').trim().to_string(),
        |h| h.title,
    );
    let title: String = title.chars().take(30).collect();
    out.push_str(&format!("] ({title})"));
    Some(out)
}

/// After a state change: the running clock stops when its entry is done
/// (`org-clock-out-when-done`).
pub fn after_todo(app: &mut App, was_running: bool, new: Option<&str>) {
    let done = new.is_some_and(|k| forge_core::org::keywords(app.buf_ref()).is_done(k));
    if was_running && done {
        clock_out_running(app, false);
    }
}

/// Whether the entry at `line` is the one being clocked.
pub fn is_running_at(app: &App, line: usize) -> bool {
    let Some(rc) = app.org.clock.as_ref() else {
        return false;
    };
    rc.buffer == app.active_id()
        && app.buf_ref().org_heading_at(line).is_some()
        && find_running(app, rc.buffer, &rc.heading) == app.buf_ref().org_heading_at(line)
}

fn select_task(app: &mut App) {
    let labels: Vec<String> = app
        .org
        .clock_history
        .iter()
        .filter(|(id, _)| app.editor.state.buffers.contains_key(id))
        .map(|(_, l)| l.trim_start_matches('*').trim().to_string())
        .collect();
    if labels.is_empty() {
        app.error("Nothing has been clocked yet");
    } else {
        choose(app, "Clock in on: ", Ask::ClockSelect, labels);
    }
}

fn find_heading_line(app: &App, id: BufferId, text: &str) -> Option<usize> {
    let b = buf_of(app, id)?;
    (0..b.line_count()).find(|&l| b.org_level(l).is_some() && b.line(l) == text)
}

pub fn clock_cancel_running(app: &mut App) -> bool {
    let Some(rc) = app.org.clock.take() else {
        return false;
    };
    let Some(h) = find_running(app, rc.buffer, &rc.heading) else {
        return false;
    };
    let ok = buf_mut(app, rc.buffer).is_some_and(|b| b.org_clock_cancel(h));
    if ok {
        app.msg("Clock canceled".to_string());
    }
    ok
}

pub fn clock_goto(app: &mut App, last: bool) {
    let target = match (&app.org.clock, last) {
        (Some(rc), false) => Some((rc.buffer, rc.heading.clone())),
        _ => app.org.last_clock.clone(),
    };
    match target {
        Some((id, text)) if app.editor.state.buffers.contains_key(&id) => {
            match find_heading_line(app, id, &text) {
                Some(h) => {
                    app.switch_to(id);
                    app.buf().set_point(Pos::new(h, 0));
                    let off = app.buf_ref().point_offset();
                    app.buf().reveal(off);
                }
                None => app.error("The clocked heading was not found"),
            }
        }
        _ => app.error("No clock to go to"),
    }
}

/// Stop the clock after 15 minutes of nothing, if asked to.
pub fn on_idle(app: &mut App) {
    if let Some((deadline, label)) = app.org.timer.countdown.clone()
        && Instant::now() >= deadline
    {
        app.org.timer.countdown = None;
        app.msg(format!("Timer done: {label}"));
        app.bell = true;
    }
    adopt_open_clock(app);
    check_idle(app);
    if app.org.auto_clockout
        && app.org.clock.is_some()
        && app.last_input.elapsed().as_secs_f64() >= 15.0 * 60.0
        && let Some(m) = clock_out_running(app, true)
    {
        app.msg(format!(
            "Clock stopped automatically after being idle ({})",
            hmm(m)
        ));
    }
}

// ------------------------------------------------------------------ timer

#[derive(Default)]
pub struct Timer {
    /// When the timer was (virtually) started; moved on by pauses.
    pub base: Option<Instant>,
    pub paused_at: Option<Instant>,
    pub countdown: Option<(Instant, String)>,
}

impl Timer {
    pub fn elapsed(&self) -> Option<Duration> {
        let base = self.base?;
        Some(
            self.paused_at
                .unwrap_or_else(Instant::now)
                .saturating_duration_since(base),
        )
    }

    pub fn text(&self) -> Option<String> {
        let s = self.elapsed()?.as_secs();
        Some(format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60))
    }
}

fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if let Ok(m) = s.parse::<f64>() {
        return Some(Duration::from_secs_f64(m * 60.0));
    }
    let lower = s.to_lowercase();
    if let Some(n) = lower
        .strip_suffix("min")
        .or_else(|| lower.strip_suffix('m'))
    {
        return n
            .trim()
            .parse::<f64>()
            .ok()
            .map(|m| Duration::from_secs_f64(m * 60.0));
    }
    if let Some(n) = lower
        .strip_suffix("sec")
        .or_else(|| lower.strip_suffix('s'))
    {
        return n.trim().parse::<f64>().ok().map(Duration::from_secs_f64);
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() >= 2 && parts.len() <= 3 {
        let mut total = 0.0;
        for p in &parts {
            total = total * 60.0 + p.trim().parse::<f64>().ok()?;
        }
        // H:MM is hours and minutes; M:SS would be ambiguous.
        if parts.len() == 2 {
            total *= 60.0;
        }
        return Some(Duration::from_secs_f64(total));
    }
    None
}

// ---------------------------------------------------------------- capture

pub struct CaptureSession {
    pub buffer: BufferId,
    pub origin: BufferId,
    pub template: CaptureTemplate,
}

pub struct PendingCapture {
    pub template: CaptureTemplate,
    pub prompts: Vec<(String, String)>,
    pub answers: Vec<String>,
    pub origin: BufferId,
    pub initial: String,
    pub link: String,
}

fn templates(app: &App) -> Vec<CaptureTemplate> {
    app.org
        .capture_templates
        .clone()
        .unwrap_or_else(default_capture_templates)
}

fn start_capture(app: &mut App, key: &str) {
    let Some(t) = templates(app).into_iter().find(|t| t.key == key) else {
        return app.error(format!("No capture template {key:?}"));
    };
    let origin = app.active_id();
    let initial = match app.buf_ref().active_region() {
        Some((a, b)) => app.buf_ref().text_range(a, b),
        None => String::new(),
    };
    let path = app.buf_ref().path().map(|p| p.display().to_string());
    let (target, desc) = app
        .buf_ref()
        .org_store_link(app.buf_ref().point().line, path.as_deref());
    let link = if desc.is_empty() {
        format!("[[{target}]]")
    } else {
        format!("[[{target}][{desc}]]")
    };
    let prompts = template_prompts(&t.text);
    let pending = PendingCapture {
        template: t,
        prompts,
        answers: Vec::new(),
        origin,
        initial,
        link,
    };
    if pending.prompts.is_empty() {
        open_capture(app, pending);
    } else {
        let (name, default) = pending.prompts[0].clone();
        app.org.capture_pending = Some(pending);
        ask_with(app, &format!("{name}: "), Ask::CapturePrompt, &default);
    }
}

fn open_capture(app: &mut App, p: PendingCapture) {
    let (today, now) = now();
    let ctx = CaptureCtx {
        today: Some(today),
        now,
        initial: p.initial.clone(),
        link: p.link.clone(),
        file_name: app
            .buf_ref()
            .path()
            .and_then(|f| f.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        full_path: app
            .buf_ref()
            .path()
            .map(|f| f.display().to_string())
            .unwrap_or_default(),
        kill: app
            .editor
            .kill_ring
            .current()
            .map(str::to_owned)
            .unwrap_or_default(),
        user: std::env::var("USER").unwrap_or_default(),
        answers: p.answers.clone(),
    };
    let (text, cursor) = expand_template(&p.template.text, &ctx);
    let target = resolve_in_org_dir(app, &p.template.file);
    let base = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "capture".into());
    let mut name = format!("CAPTURE-{base}");
    let mut n = 1;
    while app.editor.find_buffer(&name).is_some() {
        n += 1;
        name = format!("CAPTURE{n}-{base}");
    }
    let mut b = Buffer::new(&name);
    b.set_contents(&text);
    b.set_major_mode(forge_core::org::MODE);
    let id = app.editor.add_buffer(b);
    app.switch_to(id);
    if let Some(c) = cursor {
        let pos = app
            .buf_ref()
            .offset_to_pos(c.min(app.buf_ref().char_count()));
        app.buf().set_point(pos);
    }
    let scope = Scope::Buffer(id);
    for (keys, cmd) in [
        ("C-c C-c", "org-capture-finalize"),
        ("C-c C-k", "org-capture-kill"),
        ("C-c C-w", "org-capture-refile"),
    ] {
        let _ = app.editor.keymaps.bind(&scope, keys, cmd, Origin::Package);
    }
    app.org.capture = Some(CaptureSession {
        buffer: id,
        origin: p.origin,
        template: p.template,
    });
    app.msg(
        "Capture buffer: finish with C-c C-c, refile with C-c C-w, abort with C-c C-k".to_string(),
    );
}

/// The text of the capture buffer, ready to file.
fn capture_text(app: &App, s: &CaptureSession) -> String {
    buf_of(app, s.buffer)
        .map(|b| {
            (0..b.line_count())
                .map(|l| b.line(l))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn close_capture(app: &mut App) -> Option<CaptureSession> {
    let s = app.org.capture.take()?;
    let origin = s.origin;
    app.kill_buffer(s.buffer);
    if app.editor.state.buffers.contains_key(&origin) {
        app.switch_to(origin);
    }
    Some(s)
}

fn finalize_capture(app: &mut App) {
    let Some(s) = app.org.capture.as_ref() else {
        return app.error("Not in a capture buffer");
    };
    let text = capture_text(app, s);
    if text.trim().is_empty() {
        close_capture(app);
        return app.msg("Empty capture discarded".to_string());
    }
    let template = s.template.clone();
    let path = resolve_in_org_dir(app, &template.file);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match app.editor.open_file(&path) {
        Ok((id, _)) => {
            let line = buf_mut(app, id)
                .map(|b| b.org_capture_insert(template.kind, template.headline.as_deref(), &text));
            close_capture(app);
            app.msg(format!(
                "Captured to {}{} (buffer modified: save it with C-x C-s)",
                path.display(),
                line.map(|l| format!(", line {}", l + 1))
                    .unwrap_or_default()
            ));
        }
        Err(e) => app.error(format!("Cannot open {}: {e}", path.display())),
    }
}

// --------------------------------------------------------------- refile

fn refile_targets(app: &mut App) -> Vec<(BufferId, Option<usize>, String)> {
    let mut ids = agenda_buffers(app);
    let cur = app.active_id();
    if !ids.contains(&cur) && app.buf_ref().major_mode() == forge_core::org::MODE {
        ids.insert(0, cur);
    }
    let mut out = Vec::new();
    for id in ids {
        let name = buffer_label(app, id);
        out.push((id, None, format!("{name}/")));
        if let Some(b) = buf_of(app, id) {
            for (l, path) in b.org_refile_targets(3) {
                out.push((id, Some(l), format!("{name}: {path}")));
            }
        }
    }
    out
}

/// Move or copy the subtree at (`src_id`, `src_line`) under `dest`.
pub fn refile_to(
    app: &mut App,
    src_id: BufferId,
    src_line: usize,
    dest: (BufferId, Option<usize>),
    copy: bool,
) {
    let Some(src) = buf_of(app, src_id) else {
        return;
    };
    let Some(h) = src.org_heading_at(src_line) else {
        return app.error("Not in an outline entry");
    };
    let title = src.line(h);
    if src_id == dest.0 && !copy {
        let b = buf_mut(app, src_id).expect("source exists");
        let r = match dest.1 {
            Some(d) => b.org_refile_within(h, d).map(|_| ()),
            None => {
                let t = b.org_cut_subtree(h);
                t.map(|t| {
                    b.org_insert_subtree_under(None, &t);
                })
                .ok_or_else(|| "Not in an outline entry".to_string())
            }
        };
        return match r {
            Ok(()) => app.msg(format!("Refiled {}", title.trim_start_matches('*').trim())),
            Err(e) => app.error(e),
        };
    }
    let text = if copy {
        src.org_copy_subtree(h)
    } else {
        buf_mut(app, src_id).and_then(|b| b.org_cut_subtree(h))
    };
    let Some(text) = text else {
        return app.error("Not in an outline entry");
    };
    if let Some(b) = buf_mut(app, dest.0) {
        b.org_insert_subtree_under(dest.1, &text);
    }
    app.msg(format!(
        "{} {} to {}",
        if copy { "Copied" } else { "Refiled" },
        title.trim_start_matches('*').trim(),
        buffer_label(app, dest.0)
    ));
}

// --------------------------------------------------------------- archive

fn archive_path(app: &App, pattern: &str) -> Option<PathBuf> {
    let path = app.buf_ref().path()?.to_path_buf();
    let full = pattern.replace("%s", &path.display().to_string());
    let p = PathBuf::from(&full);
    Some(if p.is_absolute() {
        p
    } else {
        path.parent().map(|d| d.join(&p)).unwrap_or(p)
    })
}

fn archive_subtree(app: &mut App, spec: Option<&str>) {
    let Some(h) = heading_of_point(app) else {
        return app.error("Not in an outline entry");
    };
    let (pattern, head) = match spec {
        Some(s) => {
            let (f, hd) = s.split_once("::").map_or((s, ""), |(f, h)| (f, h));
            let hd = hd.trim().trim_start_matches('*').trim();
            (
                f.trim().to_string(),
                (!hd.is_empty()).then(|| hd.to_string()),
            )
        }
        None => app.buf_ref().org_archive_location(h),
    };
    let Some(path) = archive_path(app, &pattern) else {
        return app.error("This buffer has no file to name an archive after");
    };
    let (d, n) = now();
    let src_path = app
        .buf_ref()
        .path()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let text = app.buf_ref().org_prepare_archive(h, &src_path, d, n);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let id = match app.editor.open_file(&path) {
        Ok((id, _)) => id,
        Err(e) => return app.error(format!("Cannot open {}: {e}", path.display())),
    };
    let was_modified = buf_of(app, id).is_some_and(|b| b.is_modified());
    {
        let Some(b) = buf_mut(app, id) else { return };
        match head {
            Some(title) => {
                let target = match b.org_find_heading(&title) {
                    Some(t) => t,
                    None => {
                        let last = (0..b.line_count())
                            .rev()
                            .find(|&l| !b.line(l).is_empty())
                            .unwrap_or(0);
                        if b.char_count() == 0 {
                            b.set_contents(&format!("* {title}\n"));
                            0
                        } else {
                            b.org_insert_line_after(last, &format!("* {title}"));
                            last + 1
                        }
                    }
                };
                b.org_insert_subtree_under(Some(target), &text);
            }
            None => {
                b.org_insert_subtree_under(None, &text);
            }
        }
        if !was_modified {
            let _ = b.save();
        }
    }
    app.buf().org_cut_subtree(h);
    app.msg(format!("Archived to {}", path.display()));
}

// ---------------------------------------------------------------- commands

pub fn command(app: &mut App, cmd: &str) -> bool {
    let line = app.buf_ref().point().line;
    let back = app.has_arg();
    match cmd {
        // -------------------------------------------------- capture
        "org-capture" | "org-capture-select-template" => {
            let t = templates(app);
            if t.is_empty() {
                return no_templates(app);
            }
            let cands = t
                .iter()
                .map(|t| format!("{}: {}", t.key, t.description))
                .collect();
            choose(app, "Capture template: ", Ask::CaptureKey, cands);
        }
        "org-capture-finalize" => finalize_capture(app),
        "org-capture-kill" => {
            if close_capture(app).is_some() {
                app.msg("Capture aborted".to_string());
            } else {
                app.error("Not in a capture buffer");
            }
        }
        "org-capture-refile" => {
            if app.org.capture.is_none() {
                return err_true(app, "Not in a capture buffer");
            }
            let c: Vec<String> = refile_targets(app).into_iter().map(|t| t.2).collect();
            choose(app, "Refile capture to: ", Ask::CaptureRefile, c);
        }
        "org-capture-goto-target" => {
            let template = app
                .org
                .capture
                .as_ref()
                .map(|s| s.template.clone())
                .or_else(|| templates(app).into_iter().next());
            match template {
                Some(t) => {
                    let path = resolve_in_org_dir(app, &t.file);
                    crate::minibuffer::visit_file(app, &path, false);
                    if let Some(h) = t.headline.as_deref()
                        && let Some(l) = app.buf_ref().org_find_heading(h)
                    {
                        app.buf().set_point(Pos::new(l, 0));
                    }
                }
                None => return no_templates(app),
            }
        }
        // ----------------------------------------------------- refile
        "org-refile" | "org-copy" => {
            if heading_of_point(app).is_none() {
                return err_true(app, "Not in an outline entry");
            }
            let c: Vec<String> = refile_targets(app).into_iter().map(|t| t.2).collect();
            if c.is_empty() {
                return err_true(
                    app,
                    "No refile targets: open Org files or set forge.opt.org_agenda_files",
                );
            }
            choose(
                app,
                if cmd == "org-copy" {
                    "Copy to: "
                } else {
                    "Refile to: "
                },
                Ask::Refile {
                    copy: cmd == "org-copy",
                },
                c,
            );
        }
        // ---------------------------------------------------- archive
        "org-archive-subtree" | "org-archive-subtree-default" => {
            if back && cmd == "org-archive-subtree" {
                ask_with(
                    app,
                    "Archive to (FILE::* Heading): ",
                    Ask::ArchiveLocation,
                    "%s_archive::",
                );
            } else {
                archive_subtree(app, None);
            }
        }
        "org-archive-to-archive-sibling" => {
            let (d, n) = now();
            match app.buf().org_archive_to_sibling(line, d, n) {
                Ok(_) => app.msg("Archived under the Archive sibling".to_string()),
                Err(e) => app.error(e),
            }
        }
        "org-toggle-archive-tag" => match heading_of_point(app) {
            Some(h) => {
                let added = app.buf().org_toggle_tag(h, "ARCHIVE");
                app.msg(
                    if added {
                        "ARCHIVE tag added"
                    } else {
                        "ARCHIVE tag removed"
                    }
                    .to_string(),
                );
            }
            None => return err_true(app, "Not in an outline entry"),
        },
        // ------------------------------------------------------- clock
        "org-clock-select-task" => select_task(app),
        "org-clock-update-time-maybe" => match app.buf().org_update_clock_line(line) {
            Some(t) => app.msg(format!("Clock updated: {}", t.trim())),
            None => return err_true(app, "Not on a finished CLOCK line"),
        },
        "org-clock-in" if app.raw_arg() && app.count() >= 16 => {
            return command(app, "org-clock-in-last");
        }
        "org-clock-in" if app.raw_arg() => select_task(app),
        "org-clock-in" => match heading_of_point(app) {
            Some(h) => {
                let id = app.active_id();
                clock_in_at(app, id, h);
            }
            None => return err_true(app, "Not in an outline entry"),
        },
        "org-clock-in-last" => match app.org.last_clock.clone() {
            Some((id, text)) => match find_heading_line(app, id, &text) {
                Some(h) => clock_in_at(app, id, h),
                None => app.error("The last clocked heading was not found"),
            },
            None => app.error("Nothing has been clocked yet"),
        },
        "org-clock-out" => {
            if clock_out_running(app, false).is_none() {
                // Not the running one: maybe this entry has one open.
                let (d, n) = now();
                match heading_of_point(app) {
                    Some(h) => match app.buf().org_clock_out(h, d, n) {
                        Ok(m) => app.msg(format!("Clock stopped ({})", hmm(m))),
                        Err(e) => app.error(e),
                    },
                    None => app.error("No active clock"),
                }
            }
        }
        "org-clock-cancel" => {
            if !clock_cancel_running(app) {
                match heading_of_point(app) {
                    Some(h) if app.buf().org_clock_cancel(h) => {
                        app.msg("Clock canceled".to_string())
                    }
                    _ => app.error("No active clock"),
                }
            }
        }
        "org-clock-display" => {
            let mut parts = Vec::new();
            if let Some(h) = heading_of_point(app) {
                let own = app.buf_ref().org_clock_total(h, false);
                let all = app.buf_ref().org_clock_total(h, true);
                parts.push(format!(
                    "This entry: {} (with subtree: {})",
                    hmm(own),
                    hmm(all)
                ));
            }
            if let Some(rc) = &app.org.clock {
                parts.push(format!(
                    "Running: {}",
                    rc.heading.trim_start_matches('*').trim()
                ));
            }
            app.msg(if parts.is_empty() {
                "No clock data".to_string()
            } else {
                parts.join("; ")
            });
        }
        "org-clock-goto" => clock_goto(app, back),
        "org-clock-modify-effort-estimate" => {
            let h = app
                .org
                .clock
                .as_ref()
                .and_then(|rc| find_running(app, rc.buffer, &rc.heading));
            if h.is_some() || heading_of_point(app).is_some() {
                ask(app, "Effort estimate (e.g. 1:30 or 2): ", Ask::ClockEffort);
            } else {
                return err_true(app, "No clock to modify");
            }
        }
        "org-clock-report" => {
            let (d, n) = now();
            let l = app.buf().org_clock_report(line, d, n);
            app.buf().set_point(Pos::new(l, 0));
        }
        "org-resolve-clocks" => {
            let (d, n) = now();
            let keep = app.org.clock.as_ref().and_then(|rc| {
                buf_of(app, rc.buffer).and_then(|b| {
                    find_running(app, rc.buffer, &rc.heading).and_then(|h| {
                        b.org_clock_lines(h)
                            .into_iter()
                            .find(|c| c.end.is_none())
                            .map(|c| (rc.buffer, c.line))
                    })
                })
            });
            let mut total = 0;
            for id in agenda_buffers(app) {
                let k = keep.filter(|(b, _)| *b == id).map(|(_, l)| l);
                if let Some(b) = buf_mut(app, id) {
                    total += b.org_close_stale_clocks(k, d, n);
                }
            }
            app.msg(format!("Closed {total} stale clock(s)"));
        }
        "org-clock-toggle-auto-clockout" => {
            app.org.auto_clockout = !app.org.auto_clockout;
            app.msg(format!(
                "Auto clock-out {}",
                if app.org.auto_clockout {
                    "on (15 minutes idle)"
                } else {
                    "off"
                }
            ));
        }
        "org-clocktable-try-shift" => {
            app.error("Clock tables cover the whole file or subtree; shifting a time range is not supported".to_string());
        }
        // ------------------------------------------------------- timer
        "org-timer-start" => {
            app.org.timer.base = Some(Instant::now());
            app.org.timer.paused_at = None;
            app.msg("Timer started at 0:00:00".to_string());
        }
        "org-timer-stop" => {
            let t = app.org.timer.text();
            app.org.timer.base = None;
            app.org.timer.paused_at = None;
            app.org.timer.countdown = None;
            app.msg(match t {
                Some(t) => format!("Timer stopped at {t}"),
                None => "No timer running".to_string(),
            });
        }
        "org-timer-pause-or-continue" => {
            if app.org.timer.base.is_none() {
                return err_true(app, "No timer running");
            }
            match app.org.timer.paused_at.take() {
                Some(p) => {
                    // Continue: slide the start by the pause.
                    if let Some(b) = app.org.timer.base.as_mut() {
                        *b += Instant::now().saturating_duration_since(p);
                    }
                    app.msg("Timer continues".to_string());
                }
                None => {
                    app.org.timer.paused_at = Some(Instant::now());
                    let t = app.org.timer.text().unwrap_or_default();
                    app.msg(format!("Timer paused at {t}"));
                }
            }
        }
        "org-timer" | "org-timer-item" => match app.org.timer.text() {
            Some(t) => {
                let at = app.buf_ref().point();
                let s = if cmd == "org-timer" {
                    t
                } else {
                    format!("- {t} :: ")
                };
                app.buf().replace_range(at, at, &s);
            }
            None => return err_true(app, "No timer running; start one with org-timer-start"),
        },
        "org-timer-set-timer" => ask_with(
            app,
            "Countdown (minutes, 5m, or H:MM:SS): ",
            Ask::TimerSet,
            "5",
        ),
        _ => return false,
    }
    true
}

fn err_true(app: &mut App, m: &str) -> bool {
    app.error(m.to_string());
    true
}

fn no_templates(app: &mut App) -> bool {
    err_true(app, "No capture templates")
}

pub fn answer(app: &mut App, a: &Ask, input: &str) -> bool {
    let t = input.trim();
    match a {
        Ask::CaptureKey => {
            let key = t.split(':').next().unwrap_or("").trim().to_string();
            start_capture(app, &key);
        }
        Ask::CapturePrompt => {
            let Some(mut p) = app.org.capture_pending.take() else {
                return true;
            };
            p.answers.push(input.to_string());
            if p.answers.len() < p.prompts.len() {
                let (name, default) = p.prompts[p.answers.len()].clone();
                app.org.capture_pending = Some(p);
                ask_with(app, &format!("{name}: "), Ask::CapturePrompt, &default);
            } else {
                open_capture(app, p);
            }
        }
        Ask::CaptureRefile => {
            let targets = refile_targets(app);
            match targets.into_iter().find(|t| t.2 == input) {
                Some((id, line, _)) => capture_refile(app, id, line),
                None => app.error(format!("No such refile target: {t}")),
            }
        }
        Ask::Refile { copy } => {
            let targets = refile_targets(app);
            match targets.into_iter().find(|x| x.2 == t) {
                Some((id, line, _)) => {
                    let src = app.active_id();
                    let sl = app.buf_ref().point().line;
                    refile_to(app, src, sl, (id, line), *copy);
                }
                None => app.error(format!("No such refile target: {t}")),
            }
        }
        Ask::ArchiveLocation => archive_subtree(app, Some(t)),
        Ask::ClockIdle(secs) => idle_answer(app, *secs, t),
        Ask::ClockSelect => {
            let hit = app
                .org
                .clock_history
                .iter()
                .find(|(_, l)| l.trim_start_matches('*').trim() == t)
                .cloned();
            match hit {
                Some((id, text)) => match find_heading_line(app, id, &text) {
                    Some(h) => clock_in_at(app, id, h),
                    None => app.error("That heading is gone"),
                },
                None => app.error("No such task"),
            }
        }
        Ask::ClockEffort => {
            if forge_core::org_ext::parse_effort(t).is_none() {
                return err_true(app, &format!("Bad effort: {t}"));
            }
            let running = app
                .org
                .clock
                .as_ref()
                .map(|rc| (rc.buffer, rc.heading.clone()));
            match running {
                Some((id, text)) => {
                    if let Some(h) = find_running(app, id, &text)
                        && let Some(b) = buf_mut(app, id)
                    {
                        b.org_set_property(h, "Effort", t);
                    }
                }
                None => {
                    if let Some(h) = heading_of_point(app) {
                        app.buf().org_set_property(h, "Effort", t);
                    }
                }
            }
        }
        Ask::TimerSet => match parse_duration(t) {
            Some(d) => {
                app.org.timer.countdown =
                    Some((Instant::now() + d, format!("{} elapsed", fmt_duration(d))));
                if app.org.timer.base.is_none() {
                    app.org.timer.base = Some(Instant::now());
                }
                app.msg(format!("Countdown of {} started", fmt_duration(d)));
            }
            None => app.error(format!("Bad duration: {t}")),
        },
        _ => return false,
    }
    true
}

fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

fn capture_refile(app: &mut App, id: BufferId, line: Option<usize>) {
    let Some(s) = app.org.capture.as_ref() else {
        return app.error("Not in a capture buffer");
    };
    let text = capture_text(app, s);
    if text.trim().is_empty() {
        close_capture(app);
        return app.msg("Empty capture discarded".to_string());
    }
    let kind = s.template.kind;
    let entry = if kind == TemplateKind::Entry {
        text
    } else {
        format!("* {}", text.lines().next().unwrap_or(""))
    };
    if let Some(b) = buf_mut(app, id) {
        b.org_insert_subtree_under(line, &entry);
    }
    close_capture(app);
    app.msg(format!("Refiled the capture to {}", buffer_label(app, id)));
}

#[allow(dead_code)]
fn unused(_: &Path) {}

#[cfg(test)]
mod clock_tests {
    use super::*;

    fn app_with(text: &str) -> App {
        let mut app = App::new(forge_core::Editor::default());
        let mut b = Buffer::new("t.org");
        b.set_contents(text);
        b.set_major_mode(forge_core::org::MODE);
        let id = app.editor.add_buffer(b);
        app.editor.show_buffer(id);
        app
    }

    #[test]
    fn mode_line_shows_the_clock_and_effort() {
        let mut app = app_with("* TODO Write\n:PROPERTIES:\n:Effort: 1:00\n:END:\n* Other\n");
        assert_eq!(mode_line(&app), None);
        app.buf().set_point(Pos::new(0, 0));
        app.run_command("org-clock-in");
        let m = mode_line(&app).unwrap();
        assert!(m.starts_with("[0:0") && m.contains("/1:00] (Write)"), "{m}");
        assert!(app.mode_line_extra().iter().any(|w| w.contains("(Write)")));
    }

    #[test]
    fn marking_done_stops_the_clock() {
        let mut app = app_with("* TODO Write\n* TODO Other\n");
        app.buf().set_point(Pos::new(0, 0));
        app.run_command("org-clock-in");
        // Another entry's state change leaves the clock alone.
        let other = (0..app.buf_ref().line_count())
            .find(|&l| app.buf_ref().line(l).contains("Other"))
            .unwrap();
        app.buf().set_point(Pos::new(other, 0));
        app.run_command("org-todo");
        assert!(app.org.clock.is_some());
        app.buf().set_point(Pos::new(0, 0));
        app.run_command("org-todo");
        assert!(app.org.clock.is_none());
        assert!(app.buf_ref().line(0).starts_with("* DONE"));
    }

    #[test]
    fn history_and_selecting_a_task() {
        let mut app = app_with("* A\n* B\n");
        app.run_command("org-clock-select-task");
        assert!(app.minibuffer.is_none());
        for name in ["* A", "* B"] {
            let l = (0..app.buf_ref().line_count())
                .find(|&l| app.buf_ref().line(l) == name)
                .unwrap();
            app.buf().set_point(Pos::new(l, 0));
            app.run_command("org-clock-in");
        }
        assert_eq!(app.org.clock_history.len(), 2);
        app.run_command("org-clock-select-task");
        assert!(app.minibuffer.is_some());
        app.minibuffer = None;
        assert!(answer(&mut app, &Ask::ClockSelect, "A"));
        assert!(app.org.clock.as_ref().unwrap().heading.contains('A'));
    }

    fn with_init(app: &mut App, src: &str) {
        let path = std::env::temp_dir().join(format!(
            "forge-clock-init-{}-{}.rhai",
            std::process::id(),
            src.len()
        ));
        std::fs::write(&path, src).unwrap();
        app.init_path = Some(path);
        app.load_init_file();
    }

    #[test]
    fn org_options_from_the_ported_config_are_accepted() {
        let mut app = app_with("* TODO A\n");
        with_init(
            &mut app,
            "forge.opt[\"org_deadline_warning_days\"] = 0;\nforge.opt[\"org_agenda_span\"] = \"month\";\nforge.opt[\"org_id_extra_files\"] = [\"~/org\"];\n",
        );
        assert_eq!(setting_number(&app, "org_deadline_warning_days"), Some(0.0));
        assert_eq!(
            setting_string(&app, "org_agenda_span").as_deref(),
            Some("month")
        );
        assert_eq!(setting_strings(&app, "org_id_extra_files"), ["~/org"]);
    }

    #[test]
    #[cfg(unix)]
    fn saving_can_tidy_the_buffer_and_mark_scripts_executable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("forge-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("run.sh");
        let mut app = app_with("#!/bin/sh   \necho hi \t\nexit");
        app.buf().set_visited_file(file.clone());
        with_init(
            &mut app,
            "forge.opt[\"delete_trailing_whitespace_on_save\"] = true;\nforge.opt[\"require_final_newline\"] = true;\nforge.opt[\"make_scripts_executable_on_save\"] = true;\n",
        );
        crate::commands::save(&mut app);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "#!/bin/sh\necho hi\nexit\n"
        );
        assert_ne!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o100,
            0
        );
    }

    #[test]
    fn clocking_switches_state_and_rounds() {
        let mut app = app_with("#+TODO: TODO STARTED | DONE\n* TODO Write\n");
        with_init(
            &mut app,
            "forge.opt[\"org_clock_in_switch_to_state\"] = \"STARTED\";\nforge.opt[\"org_clock_out_switch_to_state\"] = \"TODO\";\nforge.opt[\"org_clock_rounding_minutes\"] = 15;\n",
        );
        app.buf().set_point(Pos::new(1, 0));
        app.run_command("org-clock-in");
        assert!(
            app.buf_ref().line(1).starts_with("* STARTED Write"),
            "{}",
            app.buf_ref().get_contents()
        );
        let since = app.org.clock.as_ref().unwrap().since.1;
        assert_eq!(since.1 % 15, 0);
        app.run_command("org-clock-out");
        assert!(app.buf_ref().line(1).starts_with("* TODO Write"));
    }

    #[test]
    fn a_clock_left_running_is_resumed() {
        let mut app = app_with("* A\n:LOGBOOK:\nCLOCK: [2026-10-12 Mon 09:00]\n:END:\n");
        app.on_idle();
        assert!(app.org.clock.is_none());
        with_init(&mut app, "forge.opt[\"org_clock_persist\"] = true;\n");
        app.org.clock_checked.clear();
        crate::org_flow::on_idle(&mut app);
        let rc = app.org.clock.as_ref().expect("resumed");
        assert_eq!(rc.since.1, (9, 0));
    }

    #[test]
    fn idle_time_can_be_subtracted() {
        let mut app = app_with("* A\n");
        app.buf().set_point(Pos::new(0, 0));
        app.run_command("org-clock-in");
        app.minibuffer = None;
        assert!(answer(
            &mut app,
            &Ask::ClockIdle(1800),
            "subtract and keep clocking"
        ));
        assert!(app.org.clock.is_some());
        let t = app.buf_ref().get_contents();
        assert_eq!(t.matches("CLOCK:").count(), 2, "{t}");
        assert!(t.contains("=>"), "{t}");
        assert!(answer(&mut app, &Ask::ClockIdle(60), "cancel clock"));
        assert!(app.org.clock.is_none());
    }

    #[test]
    fn ctrl_c_ctrl_c_on_a_clock_line_recomputes_it() {
        let mut app = app_with(
            "* A\n:LOGBOOK:\nCLOCK: [2026-10-12 Mon 09:00]--[2026-10-12 Mon 10:00] =>  9:99\n:END:\n",
        );
        app.buf().set_point(Pos::new(2, 0));
        app.run_command("org-ctrl-c-ctrl-c");
        assert!(app.buf_ref().line(2).ends_with("=>  1:00"));
    }
}
