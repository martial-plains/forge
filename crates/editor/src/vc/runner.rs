// SPDX-License-Identifier: MIT OR Apache-2.0
//! Running version-control commands without blocking the editor.
//!
//! A command runs as a child process; the event loop polls it while it
//! runs (see [`App::idle_timeout`](crate::app::App::idle_timeout)). Its
//! stdout and stderr are collected separately, and can also stream into a
//! buffer as they arrive (push, pull). When it exits, a continuation gets
//! the output and the exit code. Every command, and anything it printed
//! to stderr, is also written to `*vc*`, so an error is never swallowed:
//! the echo area says what went wrong and `*vc*` has the whole story.
//!
//! The environment is fixed so the output parses the same everywhere: the
//! C locale, no pager, no color, no prompts for passwords (a prompt would
//! wait forever for input that can't come).

use super::backend::{Cmd, Output};
use crate::app::App;
use forge_core::process::{ProcessEvent, ProcessId, Processes, Program};
use forge_core::{Buffer, BufferId};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// Variables every VC command runs with.
pub const ENV: &[(&str, &str)] = &[
    ("LC_ALL", "C"),
    ("LANG", "C"),
    ("LANGUAGE", "C"),
    ("GIT_PAGER", "cat"),
    ("PAGER", "cat"),
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_EDITOR", "true"),
    ("GIT_MERGE_AUTOEDIT", "no"),
    // Status in the background shouldn't take the index lock away from
    // a git command the user runs at the same time.
    ("GIT_OPTIONAL_LOCKS", "0"),
    ("HGPLAIN", "1"),
    ("HGEDITOR", "true"),
    ("NO_COLOR", "1"),
    ("TERM", "dumb"),
];

/// The buffer every command's output is logged to.
pub const OUTPUT_BUFFER: &str = "*vc*";

pub type Then = Box<dyn FnOnce(&mut App, Output)>;

pub struct Job {
    pub cmd: Cmd,
    /// For the mode line while it runs ("Pushing").
    pub title: String,
    stdout: String,
    stderr: String,
    /// Output also goes here as it arrives.
    stream: Option<BufferId>,
    then: Option<Then>,
    started: Instant,
}

#[derive(Default)]
pub struct Runner {
    processes: Processes,
    jobs: BTreeMap<ProcessId, Job>,
}

impl Runner {
    pub fn any_running(&self) -> bool {
        !self.jobs.is_empty()
    }

    /// What the mode line says while commands run.
    pub fn titles(&self) -> Vec<String> {
        self.jobs
            .values()
            .map(|j| format!("{}...", j.title))
            .collect()
    }
}

/// Run `cmd`; when it exits, call `then` with what it printed. `title`
/// shows in the mode line meanwhile. With `stream`, output goes into that
/// buffer as it arrives too.
pub fn run(
    app: &mut App,
    cmd: Cmd,
    title: &str,
    stream: Option<BufferId>,
    then: impl FnOnce(&mut App, Output) + 'static,
) {
    log_command(app, &cmd);
    let env: Vec<(String, String)> = ENV
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let spawned = app.vc.runner.processes.spawn(
        Program::Argv(&cmd.program, &cmd.args),
        &cmd.dir,
        cmd.stdin.is_some(),
        &env,
    );
    match spawned {
        Ok(pid) => {
            if let Some(input) = cmd.stdin.clone()
                && let Some(mut pipe) = app.vc.runner.processes.take_stdin(pid)
            {
                std::thread::spawn(move || {
                    use std::io::Write;
                    let _ = pipe.write_all(input.as_bytes());
                });
            }
            app.vc.runner.jobs.insert(
                pid,
                Job {
                    cmd,
                    title: title.to_string(),
                    stdout: String::new(),
                    stderr: String::new(),
                    stream,
                    then: Some(Box::new(then)),
                    started: Instant::now(),
                },
            );
        }
        Err(e) => {
            let msg = super::backend::spawn_error(&cmd.program, e);
            append_output(app, &format!("{msg}\n"));
            app.error(msg.clone());
            then(
                app,
                Output {
                    code: None,
                    stdout: String::new(),
                    stderr: msg,
                },
            );
        }
    }
}

/// Run `cmd` and wait for it (with the event loop's polling, so other
/// commands' output keeps flowing). For quick commands whose result the
/// next step needs at once.
pub fn run_now(app: &mut App, cmd: &Cmd) -> Result<Output, String> {
    log_command(app, cmd);
    let out = cmd.run()?;
    log_output(app, &out);
    Ok(out)
}

/// Run `cmd` now; on failure report its error and return None.
pub fn run_ok(app: &mut App, cmd: &Cmd) -> Option<String> {
    match run_now(app, cmd) {
        Ok(out) if out.success() => Some(out.stdout),
        Ok(out) => {
            app.error(out.error_text(&cmd.line()));
            None
        }
        Err(e) => {
            app.error(e);
            None
        }
    }
}

/// Handle output and exits (called while idle).
pub fn poll(app: &mut App) {
    poll_split(app);
}

/// Wait for every running command (for tests, and for steps that need the
/// result before going on), up to `timeout`.
pub fn wait_all(app: &mut App, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while app.vc.runner.any_running() {
        if Instant::now() > deadline {
            return false;
        }
        poll_split(app);
        std::thread::sleep(Duration::from_millis(5));
    }
    true
}

/// Collect output, keeping stdout and stderr apart, and finish the
/// commands that have exited.
fn poll_split(app: &mut App) {
    if !app.vc.runner.any_running() {
        return;
    }
    for ev in app.vc.runner.processes.poll() {
        let (pid, text, is_err) = match ev {
            ProcessEvent::Output(pid, t) => (pid, t, false),
            ProcessEvent::Stderr(pid, t) => (pid, t, true),
            ProcessEvent::Exited(pid, code) => {
                finish(app, pid, code);
                continue;
            }
        };
        let Some(job) = app.vc.runner.jobs.get_mut(&pid) else {
            continue;
        };
        if is_err {
            job.stderr.push_str(&text);
        } else {
            job.stdout.push_str(&text);
        }
        if let Some(b) = job.stream {
            crate::compile::append(app, b, &text);
        }
    }
}

fn finish(app: &mut App, pid: ProcessId, code: Option<i32>) {
    let Some(mut job) = app.vc.runner.jobs.remove(&pid) else {
        return;
    };
    let out = Output {
        code,
        stdout: std::mem::take(&mut job.stdout),
        stderr: std::mem::take(&mut job.stderr),
    };
    log_output(app, &out);
    if let Some(b) = job.stream {
        let secs = job.started.elapsed().as_secs_f64();
        let what = match code {
            Some(0) => "finished".to_string(),
            Some(n) => format!("exited abnormally with code {n}"),
            None => "was killed".to_string(),
        };
        crate::compile::append(app, b, &format!("\n{} {what} ({secs:.1}s)\n", job.title));
    }
    if let Some(then) = job.then.take() {
        then(app, out);
    }
}

/// Stop every running command (their continuations see them killed).
pub fn kill_all(app: &mut App) -> usize {
    let pids: Vec<ProcessId> = app.vc.runner.jobs.keys().copied().collect();
    for p in &pids {
        app.vc.runner.processes.kill(*p);
    }
    pids.len()
}

/// The `*vc*` buffer, made if need be.
pub fn output_buffer(app: &mut App) -> BufferId {
    match app.editor.find_buffer(OUTPUT_BUFFER) {
        Some(id) => id,
        None => {
            let mut b = Buffer::new(OUTPUT_BUFFER);
            b.set_major_mode("vc-output-mode");
            app.editor.add_buffer(b)
        }
    }
}

fn append_output(app: &mut App, text: &str) {
    let id = output_buffer(app);
    crate::compile::append(app, id, text);
    // Keep it from growing without end.
    if let Some(b) = app.editor.state.get_buffer_mut(id)
        && b.line_count() > 5000
    {
        let cut = b.pos_to_offset(forge_core::Pos::new(b.line_count() - 4000, 0));
        b.as_owner(|b| b.replace_offsets(0, cut, ""));
        b.set_modified(false);
    }
}

fn log_command(app: &mut App, cmd: &Cmd) {
    let dir = crate::files::abbreviate(&cmd.dir);
    append_output(app, &format!("\n$ cd {dir} && {}\n", cmd.line()));
}

fn log_output(app: &mut App, out: &Output) {
    // Only stderr and the status: stdout is often a whole diff or log,
    // which has its own buffer.
    let mut text = String::new();
    if !out.stderr.is_empty() {
        text.push_str(&out.stderr);
        if !out.stderr.ends_with('\n') {
            text.push('\n');
        }
    }
    if out.code != Some(0) {
        text.push_str(&match out.code {
            Some(c) => format!("[exit status {c}]\n"),
            None => "[killed]\n".into(),
        });
    }
    if !text.is_empty() {
        append_output(app, &text);
    }
}

/// A command failed: say why in the echo area and show `*vc*`.
pub fn report_failure(app: &mut App, cmd_line: &str, out: &Output, show: bool) {
    app.error(out.error_text(cmd_line));
    if show {
        let id = output_buffer(app);
        crate::compile::display_buffer(app, id);
    }
}
