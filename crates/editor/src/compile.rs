// SPDX-License-Identifier: MIT OR Apache-2.0
//! Working on a project from inside Forge: M-x compile and recompile,
//! next-error / previous-error through compiler and grep output, M-! and
//! M-& (shell commands), grep and project-find-regexp, project-find-file.
//!
//! Commands run as asynchronous processes (forge_core::process): output
//! streams into its buffer while you keep editing, and the event loop
//! wakes every POLL_INTERVAL while any runs.

use crate::app::App;
use crate::minibuffer::{Choice, Purpose};
use forge_core::decoration::DecoFace;
use forge_core::process::{self, Location, ProcessEvent, ProcessId, Processes, Program};
use forge_core::{Buffer, BufferId, Pos};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub const COMPILATION: &str = "*compilation*";
pub const GREP: &str = "*grep*";
pub const SHELL_OUTPUT: &str = "*Shell Command Output*";
pub const ASYNC_SHELL: &str = "*Async Shell Command*";

/// Most files project-find-file offers.
const PROJECT_FILES_LIMIT: usize = 100_000;

pub struct Job {
    pub buffer: BufferId,
    pub kind: JobKind,
    started: Instant,
}

pub enum JobKind {
    Compile,
    /// A grep: the output is rewritten as it comes in, and the matches
    /// it reports are kept (see grep.rs).
    Grep(crate::grep::GrepJob),
    /// M-&: output shown as it comes.
    AsyncShell,
    /// M-!: when done, one line goes to the echo area, more are shown;
    /// with C-u the output is inserted where point was instead.
    Shell {
        insert: Option<(BufferId, usize, u64)>,
    },
    /// A program init.rhai is running (`forge.process.run`): output is
    /// collected, and the Rhai callback answers on exit (or timeout).
    Rhai(RhaiJob),
}

/// A program init.rhai is running: what to answer it with, and when to
/// give up waiting.
pub struct RhaiJob {
    pub request: u32,
    pub timeout: std::time::Duration,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

#[derive(Default)]
pub struct Jobs {
    pub processes: Processes,
    pub running: BTreeMap<ProcessId, Job>,
    /// The directory each output buffer's file names are relative to.
    pub(crate) dirs: BTreeMap<BufferId, PathBuf>,
    /// The command each output buffer ran (for recompile, `g`).
    pub(crate) commands: BTreeMap<BufferId, String>,
    /// The matches each *grep* buffer holds, in the order they were
    /// found (see grep.rs).
    pub grep_matches: BTreeMap<BufferId, Vec<crate::grep::GrepMatch>>,
    /// The lines of each *grep* buffer that are headings.
    pub grep_headings: BTreeMap<BufferId, Vec<usize>>,
    /// Whether each *grep* buffer's command line is shown abbreviated.
    pub grep_abbreviated: BTreeMap<BufferId, bool>,
    /// The *grep* buffer being edited (Grep Edit mode).
    pub grep_edit: Option<BufferId>,
    /// The buffer the last visited grep match is marked in.
    pub grep_highlight: Option<BufferId>,
    /// The last grep command that ran.
    pub last_grep: Option<String>,
    /// The grep waiting for an answer to "Save the file? ".
    pub pending_grep: Option<(String, PathBuf)>,
    /// Where next-error looks, and the line it's at there.
    errors: Option<BufferId>,
    error_line: Option<usize>,
    last_compile: Option<String>,
    last_shell: Option<String>,
}

impl Jobs {
    pub fn any_running(&self) -> bool {
        self.processes.any_running()
    }
}

/// The directory commands for the current buffer run in: the project
/// root, or the buffer's file's directory, or the current directory.
pub fn default_directory(app: &App) -> PathBuf {
    let b = app.buf_ref();
    if let Some(d) = app.jobs.dirs.get(&app.active_id()) {
        return d.clone();
    }
    if let Some(d) = crate::tether::default_dir(app) {
        return d;
    }
    let here = b
        .path()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    // The project is the nearest directory with a `.forge/` in it, else
    // what its build files say. A repository is not a project: Forge no
    // longer makes one out of a `.git`.
    forge_core::project::root(&here)
        .or_else(|| process::project_root(&here))
        .unwrap_or(here)
}

/// The files of the project at `dir`: the version-control system's list of
/// tracked files (and untracked ones it doesn't ignore), else a walk.
pub fn project_files(dir: &Path) -> Vec<String> {
    if let Some((kind, root)) = crate::vc::responsible(dir)
        && root == dir
    {
        let backend = kind.backend();
        if let Ok(mut files) = backend.list_tracked_files(&root) {
            // Untracked files that aren't ignored, as project.el includes.
            if let Ok((entries, _)) = backend.dir_status(&root, "", false) {
                files.extend(
                    entries
                        .into_iter()
                        .filter(|e| e.state == crate::vc::backend::FileState::Unregistered)
                        .map(|e| e.path),
                );
            }
            files.retain(|f| root.join(f).is_file());
            files.sort();
            files.dedup();
            files.truncate(PROJECT_FILES_LIMIT);
            return files;
        }
    }
    process::project_files(dir, PROJECT_FILES_LIMIT)
}

/// The buffer's own directory (M-! runs there).
fn buffer_directory(app: &App) -> PathBuf {
    if let Some(d) = app.jobs.dirs.get(&app.active_id()) {
        return d.clone();
    }
    if let Some(d) = crate::tether::default_dir(app) {
        return d;
    }
    app.buf_ref()
        .path()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Commands from this module. False if `cmd` isn't one.
pub fn execute(app: &mut App, cmd: &str) -> bool {
    match cmd {
        "compile" | "project-compile" => {
            let dir = default_directory(app);
            let initial = app
                .jobs
                .last_compile
                .clone()
                .unwrap_or_else(|| process::default_compile_command(&dir));
            prompt_with(app, "Compile command: ", Purpose::Compile, &initial);
        }
        "recompile" => {
            let id = app.active_id();
            let command = app
                .jobs
                .commands
                .get(&id)
                .cloned()
                .or_else(|| app.jobs.last_compile.clone());
            match command {
                Some(c) => {
                    let (name, kind) = if app.buf_ref().name() == GREP {
                        let options = app.grep.clone();
                        let display = crate::grep::abbreviate(&c, &options);
                        let job = crate::grep::GrepJob::new(options, &c, &display, 0);
                        (GREP, JobKind::Grep(job))
                    } else {
                        (COMPILATION, JobKind::Compile)
                    };
                    let dir = app
                        .jobs
                        .dirs
                        .get(&id)
                        .cloned()
                        .unwrap_or_else(|| default_directory(app));
                    start(app, name, &c, &dir, kind);
                }
                None => {
                    execute(app, "compile");
                }
            }
        }
        "kill-compilation" => {
            let id = app.active_id();
            let found: Vec<ProcessId> = app
                .jobs
                .running
                .iter()
                .filter(|(_, j)| j.buffer == id || matches!(j.kind, JobKind::Compile))
                .map(|(p, _)| *p)
                .collect();
            match found.first() {
                Some(p) => {
                    app.jobs.processes.kill(*p);
                    app.msg("Killed");
                }
                None => app.error("No compilation running"),
            }
        }
        "project-find-regexp" => {
            let word = word_at_point(app.buf_ref());
            prompt_with(app, "Find regexp in project: ", Purpose::ProjectGrep, &word);
        }
        "shell-command" => {
            let insert = app.has_arg();
            app.prompt("Shell command: ", Purpose::ShellCommand { insert });
        }
        "async-shell-command" => app.prompt("Async shell command: ", Purpose::AsyncShellCommand),
        "project-find-file" => {
            let dir = default_directory(app);
            let files = project_files(&dir);
            if files.is_empty() {
                app.error(format!("No files in {}", dir.display()));
            } else {
                let prompt = format!("Find file in {}: ", tilde(&dir));
                app.prompt(
                    prompt,
                    Purpose::Choose {
                        what: Choice::ProjectFile(dir),
                        candidates: files,
                    },
                );
            }
        }
        "next-error" => next_error(app, app.count() as isize, false),
        "previous-error" => next_error(app, -(app.count() as isize), false),
        "first-error" => next_error(app, 1, true),
        "compile-goto-error" => {
            let id = app.active_id();
            let line = app.buf_ref().point().line;
            app.jobs.errors = Some(id);
            match location_near(app, app.buf_ref(), id, line) {
                Some(_) => {
                    app.jobs.error_line = Some(line);
                    visit_error(app, id, line);
                }
                None => app.error("No error here"),
            }
        }
        "compilation-next-error" | "compilation-previous-error" => {
            let forward = cmd.ends_with("next-error");
            let b = app.buf_ref();
            let from = b.point().line;
            let n = b.line_count();
            let mut l = from;
            let found = loop {
                l = if forward {
                    l + 1
                } else {
                    match l.checked_sub(1) {
                        Some(x) => x,
                        None => break None,
                    }
                };
                if l >= n {
                    break None;
                }
                if process::parse_location(&b.line(l)).is_some() {
                    break Some(l);
                }
            };
            match found {
                Some(l) => app.buf().set_point(Pos::new(l, 0)),
                None => app.error(if forward {
                    "No more errors"
                } else {
                    "No earlier errors"
                }),
            }
        }
        _ => return false,
    }
    true
}

pub(crate) fn prompt_with(app: &mut App, prompt: &str, purpose: Purpose, initial: &str) {
    app.prompt(prompt, purpose);
    if let Some(mb) = app.minibuffer.as_mut() {
        mb.input = initial.to_string();
    }
}

fn tilde(p: &Path) -> String {
    let s = p.display().to_string();
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && s.starts_with(&h) => format!("~{}", &s[h.len()..]),
        _ => s,
    }
}

/// Run a grep command line in `dir`, its matches in *grep* (vc-git-grep).
pub fn grep_in(app: &mut App, command: &str, dir: &Path) {
    let options = app.grep.clone();
    let display = crate::grep::abbreviate(command, &options);
    let job = crate::grep::GrepJob::new(options, command, &display, 0);
    start(app, GREP, command, dir, JobKind::Grep(job));
}

/// The word at point (for prompts' defaults).
pub fn word_at_point_pub(b: &Buffer) -> String {
    word_at_point(b)
}

fn word_at_point(b: &Buffer) -> String {
    let p = b.point();
    let line: Vec<char> = b.line(p.line).chars().collect();
    let is_word = |c: &char| c.is_alphanumeric() || *c == '_';
    let mut a = p.col.min(line.len());
    while a > 0 && is_word(&line[a - 1]) {
        a -= 1;
    }
    let mut z = p.col.min(line.len());
    while z < line.len() && is_word(&line[z]) {
        z += 1;
    }
    line[a..z].iter().collect()
}

// ---------------------------------------------------------- answers

pub fn answered(app: &mut App, purpose: &Purpose, input: &str) {
    if input.trim().is_empty() {
        return;
    }
    match purpose {
        Purpose::Compile => {
            app.jobs.last_compile = Some(input.to_string());
            let dir = default_directory(app);
            let retry = crate::tether::Retry::Replay {
                compile: true,
                insert: false,
                input: input.to_string(),
            };
            if !crate::tether::ensure_auth(app, &dir, retry) {
                return;
            }
            start(app, COMPILATION, input, &dir, JobKind::Compile);
        }
        Purpose::ProjectGrep => {
            let dir = default_directory(app);
            let command = process::grep_command(&dir, input);
            let options = app.grep.clone();
            let display = crate::grep::abbreviate(&command, &options);
            let job = crate::grep::GrepJob::new(options, &command, &display, 0);
            start(app, GREP, &command, &dir, JobKind::Grep(job));
        }
        Purpose::ShellCommand { insert } => {
            app.jobs.last_shell = Some(input.to_string());
            let dir = buffer_directory(app);
            let retry = crate::tether::Retry::Replay {
                compile: false,
                insert: *insert,
                input: input.to_string(),
            };
            if !crate::tether::ensure_auth(app, &dir, retry) {
                return;
            }
            let command = input.trim_end();
            // "cmd &" runs asynchronously.
            if let Some(c) = command.strip_suffix('&') {
                start(app, ASYNC_SHELL, c.trim_end(), &dir, JobKind::AsyncShell);
                return;
            }
            let target = insert.then(|| {
                let b = app.buf_ref();
                (app.active_id(), b.point_offset(), b.version())
            });
            start(
                app,
                SHELL_OUTPUT,
                command,
                &dir,
                JobKind::Shell { insert: target },
            );
        }
        Purpose::AsyncShellCommand => {
            let dir = buffer_directory(app);
            start(app, ASYNC_SHELL, input, &dir, JobKind::AsyncShell);
        }
        _ => {}
    }
}

/// RET in project-find-file: `input` is a candidate, or a part of one.
pub fn visit_project_file(app: &mut App, root: &Path, candidates: &[String], input: &str) {
    let file = if candidates.iter().any(|c| c == input) {
        Some(input.to_string())
    } else {
        let matches: Vec<&String> = candidates.iter().filter(|c| c.contains(input)).collect();
        match matches.len() {
            1 => Some(matches[0].clone()),
            0 => None,
            n => return app.error(format!("{n} files match {input:?}; type more")),
        }
    };
    match file {
        Some(f) => {
            visit(app, &root.join(f), false);
        }
        None => app.error(format!("No file matching {input:?}")),
    }
}

fn visit(app: &mut App, path: &Path, other_window: bool) -> bool {
    match app.editor.open_file(path) {
        Ok((id, _)) => {
            if other_window {
                crate::windows::pop_to_other_window(app);
            }
            app.switch_to(id);
            true
        }
        Err(e) => {
            app.error(format!("Cannot open {}: {e}", path.display()));
            false
        }
    }
}

// -------------------------------------------------------------- running

pub(crate) fn start(app: &mut App, name: &str, command: &str, dir: &Path, mut kind: JobKind) {
    // One run per output buffer: a new one replaces the old.
    let old = app.editor.find_buffer(name);
    if let Some(old) = old {
        let running: Vec<ProcessId> = app
            .jobs
            .running
            .iter()
            .filter(|(_, j)| j.buffer == old)
            .map(|(p, _)| *p)
            .collect();
        for p in running {
            app.jobs.processes.kill(p);
            app.jobs.running.remove(&p);
        }
    }
    let id = old.unwrap_or_else(|| app.editor.add_buffer(Buffer::new(name)));
    let mode = match kind {
        JobKind::Compile => "compilation-mode",
        JobKind::Grep(_) => "grep-mode",
        _ => "shell-command-mode",
    };
    // A grep shows the command with the ignored directories concealed.
    let display = match &kind {
        JobKind::Grep(job) => job.display.clone(),
        _ => command.to_string(),
    };
    let header = match kind {
        JobKind::Compile | JobKind::Grep(_) => format!(
            "-*- mode: {}; default-directory: \"{}\" -*-\n{} started at {}\n\n{display}\n",
            mode.trim_end_matches("-mode"),
            tilde(dir),
            if matches!(kind, JobKind::Grep(_)) {
                "Grep"
            } else {
                "Compilation"
            },
            now()
        ),
        _ => String::new(),
    };
    {
        let b = app.editor.state.get_buffer_mut(id).expect("just made");
        b.set_contents(&header);
        b.set_major_mode(mode);
        b.clear_decorations("compile");
        b.set_offset(b.char_count());
    }
    // The first line of output goes below the header.
    if let JobKind::Grep(job) = &mut kind {
        job.next_line = app
            .editor
            .state
            .get_buffer(id)
            .map_or(0, |b| b.line_count());
    }
    app.jobs.dirs.insert(id, dir.to_path_buf());
    app.jobs.commands.insert(id, command.to_string());
    if matches!(kind, JobKind::Compile | JobKind::Grep(_)) {
        app.jobs.errors = Some(id);
        app.jobs.error_line = None;
    }
    if matches!(kind, JobKind::Grep(_)) {
        // The matches of the last run are gone.
        app.jobs.grep_matches.remove(&id);
        app.jobs.grep_headings.remove(&id);
        app.jobs.grep_abbreviated.remove(&id);
    }
    let show = !matches!(kind, JobKind::Shell { .. });
    let spawned = match crate::tether::parse_path(dir) {
        // A remote directory: run through Tether's hops.
        Some(remote) => {
            let (argv, env, lines) = crate::tether::job_command(&remote, command);
            let local = std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir());
            let res = app.jobs.processes.spawn(
                Program::Argv(&argv[0], &argv[1..]),
                &local,
                !lines.is_empty(),
                &env,
            );
            if let Ok(pid) = &res
                && !lines.is_empty()
                && let Some(mut pipe) = app.jobs.processes.take_stdin(*pid)
            {
                std::thread::spawn(move || {
                    use std::io::Write;
                    let _ = pipe.write_all(&lines);
                });
            }
            res
        }
        None => app
            .jobs
            .processes
            .spawn(Program::Shell(command), dir, false, &[]),
    };
    match spawned {
        Ok(pid) => {
            app.jobs.running.insert(
                pid,
                Job {
                    buffer: id,
                    kind,
                    started: Instant::now(),
                },
            );
            if show {
                display_buffer(app, id);
            }
        }
        Err(e) => {
            append(app, id, &format!("Cannot run {command}: {e}\n"));
            display_buffer(app, id);
        }
    }
}

/// Start a program init.rhai is running (an Effect::Process): spawn it
/// now that the Rhai call returned, and answer the Rhai callback on exit.
/// A spawn failure answers at once, so the callback always runs.
pub fn start_lua(
    app: &mut App,
    request: u32,
    program: forge_core::process::ProcessSpec,
    dir: PathBuf,
    stdin: Option<Vec<u8>>,
    timeout: std::time::Duration,
    env: Vec<(String, String)>,
) {
    use forge_core::process::ProcessSpec;
    let what = match &program {
        ProcessSpec::Shell(line) => line.clone(),
        ProcessSpec::Argv(prog, args) => std::iter::once(prog.clone())
            .chain(args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" "),
    };
    let prog = match &program {
        ProcessSpec::Shell(line) => Program::Shell(line),
        ProcessSpec::Argv(prog, args) => Program::Argv(prog, args),
    };
    // `env` is the whole environment (forge.env's process-environment,
    // with this call's extras merged in), so start from scratch.
    match app
        .jobs
        .processes
        .spawn_env(prog, &dir, stdin.is_some(), &env)
    {
        Ok(pid) => {
            // Fed on its own thread: a reader that never comes must not
            // wedge anything. The thread ends with the write, or with an
            // error once the process (and its end of the pipe) is gone.
            if let Some(input) = stdin
                && let Some(mut pipe) = app.jobs.processes.take_stdin(pid)
            {
                std::thread::spawn(move || {
                    use std::io::Write;
                    let _ = pipe.write_all(&input);
                });
            }
            app.jobs.running.insert(
                pid,
                Job {
                    buffer: app.active_id(),
                    kind: JobKind::Rhai(RhaiJob {
                        request,
                        timeout,
                        stdout: String::new(),
                        stderr: String::new(),
                        timed_out: false,
                    }),
                    started: Instant::now(),
                },
            );
        }
        Err(e) => {
            crate::plugins::answer_rhai_process(
                app,
                request,
                None,
                String::new(),
                format!("cannot run {what}: {e}"),
            );
        }
    }
}

/// Show a buffer in a window other than the selected one (splitting if
/// there's only one), without selecting it. Nothing if one shows it.
pub fn display_buffer(app: &mut App, id: BufferId) {
    if app.editor.windows.iter().any(|w| w.buffer == id) {
        return;
    }
    let selected = app.editor.windows.selected_id();
    crate::windows::pop_to_other_window(app);
    app.switch_to(id);
    let end = app.buf_ref().char_count();
    app.buf().set_offset(end);
    app.select_window(selected);
}

fn now() -> String {
    // No date crate: ask the system, fall back to seconds since the epoch.
    std::process::Command::new("date")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            format!("{secs}")
        })
}

/// Add text at the end of an output buffer. Windows whose point was at
/// the end follow it.
pub(crate) fn append(app: &mut App, id: BufferId, text: &str) {
    let text = text.replace("\r\n", "\n");
    // A lone CR (progress bars) starts the line again: keep what follows.
    let text: String = text
        .split('\n')
        .map(|l| l.rsplit('\r').next().unwrap_or(l))
        .collect::<Vec<_>>()
        .join("\n");
    let selected = app.editor.windows.selected_id();
    let windows: Vec<_> = app
        .editor
        .windows
        .iter()
        .filter(|w| w.buffer == id)
        .map(|w| w.id)
        .collect();
    let Some(b) = app.editor.state.get_buffer(id) else {
        return;
    };
    let end = b.char_count();
    let at_end: Vec<_> = windows
        .iter()
        .copied()
        .filter(|w| app.editor.window_point(*w) == end)
        .collect();
    let point = b.point_offset();
    let b = app.editor.state.get_buffer_mut(id).expect("checked");
    b.replace_offsets(end, end, &text);
    b.break_undo_group();
    let new_end = b.char_count();
    if point != end || (windows.contains(&selected) && !at_end.contains(&selected)) {
        b.set_offset(point);
    }
    for w in at_end {
        app.editor.set_window_point(w, new_end);
    }
    if let Some(b) = app.editor.state.get_buffer_mut(id) {
        b.set_modified(false);
    }
}

/// The output of a running job, as it goes into the output buffer.  A
/// grep rewrites its output as it comes in, and keeps the matches it
/// reports.
fn output(app: &mut App, pid: ProcessId, buffer: BufferId, text: &str) {
    let chunk = match app.jobs.running.get_mut(&pid).map(|j| &mut j.kind) {
        Some(JobKind::Grep(job)) => job.push(text),
        _ => crate::grep::GrepChunk {
            text: text.to_string(),
            matches: Vec::new(),
            headings: Vec::new(),
        },
    };
    if !chunk.text.is_empty() {
        append(app, buffer, &chunk.text);
    }
    if !chunk.matches.is_empty() {
        app.jobs
            .grep_matches
            .entry(buffer)
            .or_default()
            .extend(chunk.matches);
    }
    if !chunk.headings.is_empty() {
        app.jobs
            .grep_headings
            .entry(buffer)
            .or_default()
            .extend(chunk.headings);
    }
}

/// Handle output and exits (called when idle).
pub fn poll(app: &mut App) {
    if !app.jobs.any_running() {
        return;
    }
    // init.rhai's programs give up after their timeout; the exit still
    // answers the callback.
    let now = Instant::now();
    let late: Vec<ProcessId> = app
        .jobs
        .running
        .iter()
        .filter(|(_, j)| {
            matches!(&j.kind, JobKind::Rhai(job) if now.duration_since(j.started) > job.timeout)
        })
        .map(|(pid, _)| *pid)
        .collect();
    for pid in late {
        app.jobs.processes.kill(pid);
        if let Some(JobKind::Rhai(job)) = app.jobs.running.get_mut(&pid).map(|j| &mut j.kind) {
            job.timed_out = true;
        }
    }
    for ev in app.jobs.processes.poll() {
        match ev {
            ProcessEvent::Output(pid, text) => {
                if let Some(JobKind::Rhai(job)) =
                    app.jobs.running.get_mut(&pid).map(|j| &mut j.kind)
                {
                    job.stdout.push_str(&text);
                } else if let Some(buffer) = app.jobs.running.get(&pid).map(|j| j.buffer) {
                    output(app, pid, buffer, &text);
                }
            }
            ProcessEvent::Stderr(pid, text) => {
                if let Some(JobKind::Rhai(job)) =
                    app.jobs.running.get_mut(&pid).map(|j| &mut j.kind)
                {
                    job.stderr.push_str(&text);
                } else if let Some(buffer) = app.jobs.running.get(&pid).map(|j| j.buffer) {
                    output(app, pid, buffer, &text);
                }
            }
            ProcessEvent::Exited(pid, code) => {
                let Some(job) = app.jobs.running.remove(&pid) else {
                    continue;
                };
                match job.kind {
                    JobKind::Rhai(rhai) => {
                        let (code, stderr) = if rhai.timed_out {
                            (
                                None,
                                format!("ran longer than {} seconds", rhai.timeout.as_secs()),
                            )
                        } else {
                            (code, rhai.stderr)
                        };
                        crate::plugins::answer_rhai_process(
                            app,
                            rhai.request,
                            code,
                            rhai.stdout,
                            stderr,
                        );
                    }
                    kind => finished(
                        app,
                        Job {
                            buffer: job.buffer,
                            kind,
                            started: job.started,
                        },
                        code,
                    ),
                }
            }
        }
    }
}

fn finished(app: &mut App, mut job: Job, code: Option<i32>) {
    let secs = job.started.elapsed().as_secs_f64();
    let what = match code {
        Some(0) => "finished".to_string(),
        Some(n) => format!("exited abnormally with code {n}"),
        None => "was killed".to_string(),
    };
    match job.kind {
        JobKind::Compile | JobKind::Grep(_) => {
            let is_grep = matches!(job.kind, JobKind::Grep(_));
            let label = if is_grep { "Grep" } else { "Compilation" };
            if let JobKind::Grep(g) = &mut job.kind {
                // The last line, when the output didn't end with a newline.
                let chunk = g.flush();
                if !chunk.text.is_empty() {
                    append(app, job.buffer, &chunk.text);
                }
                if !chunk.matches.is_empty() {
                    app.jobs
                        .grep_matches
                        .entry(job.buffer)
                        .or_default()
                        .extend(chunk.matches);
                }
                if !chunk.headings.is_empty() {
                    app.jobs
                        .grep_headings
                        .entry(job.buffer)
                        .or_default()
                        .extend(chunk.headings);
                }
            }
            append(
                app,
                job.buffer,
                &format!("\n{label} {what} at {} ({secs:.1}s)\n", now()),
            );
            let errors = decorate(app, job.buffer);
            // A grep knows how many matches it found; a compilation
            // counts the locations in its output.
            let count = if is_grep {
                app.jobs
                    .grep_matches
                    .get(&job.buffer)
                    .map_or(0, |m| m.len())
            } else {
                errors
            };
            let summary = match (is_grep, count) {
                (true, n) => format!("Grep {what}: {n} match{}", if n == 1 { "" } else { "es" }),
                (false, 0) => format!("Compilation {what}"),
                (false, n) => format!(
                    "Compilation {what}: {n} location{} (M-g n to visit)",
                    if n == 1 { "" } else { "s" }
                ),
            };
            app.msg(summary);
        }
        // init.rhai's programs answer their callback on exit instead.
        JobKind::Rhai(_) => {}
        JobKind::AsyncShell => {
            append(app, job.buffer, &format!("\nProcess {what}\n"));
        }
        JobKind::Shell { insert } => {
            let output = app
                .editor
                .state
                .get_buffer(job.buffer)
                .map(Buffer::get_contents)
                .unwrap_or_default();
            if let Some((target, offset, version)) = insert {
                if let Some(b) = app.editor.state.get_buffer_mut(target) {
                    let at = b.transform_offset(offset, version);
                    b.replace_offsets(at, at, &output);
                    b.set_offset(at);
                    b.break_undo_group();
                }
                return;
            }
            let trimmed = output.trim_end_matches('\n');
            if trimmed.is_empty() {
                app.msg(match code {
                    Some(0) => "(Shell command succeeded with no output)".to_string(),
                    _ => format!("(Shell command {what} with no output)"),
                });
            } else if !trimmed.contains('\n') {
                app.msg(trimmed.to_string());
            } else {
                display_buffer(app, job.buffer);
                let start = 0;
                for w in app
                    .editor
                    .windows
                    .iter()
                    .filter(|w| w.buffer == job.buffer)
                    .map(|w| w.id)
                    .collect::<Vec<_>>()
                {
                    app.editor.set_window_point(w, start);
                }
            }
        }
    }
}

/// Mark the locations in an output buffer; returns how many there are.
fn decorate(app: &mut App, id: BufferId) -> usize {
    let Some(b) = app.editor.state.get_buffer_mut(id) else {
        return 0;
    };
    let mut decos = Vec::new();
    let mut n = 0;
    // The file names above the matches in a *grep* buffer.
    let headings: Vec<usize> = app.jobs.grep_headings.get(&id).cloned().unwrap_or_default();
    for l in 0..b.line_count() {
        let text = b.line(l);
        let start = b.pos_to_offset(Pos::new(l, 0));
        if headings.contains(&l) {
            let end = start + text.chars().count();
            decos.push((start, end, DecoFace::GrepHeading, None));
            continue;
        }
        if let Some(loc) = process::parse_location(&text) {
            n += 1;
            if let Some(i) = text.find(&loc.file) {
                let a = start + text[..i].chars().count();
                decos.push((
                    a,
                    a + loc.file.chars().count(),
                    DecoFace::Syntax(forge_core::syntax::SyntaxFace::Function),
                    None,
                ));
            }
        } else {
            let t = text.trim_start();
            let face = if t.starts_with("error") {
                Some(DecoFace::Error)
            } else if t.starts_with("warning") {
                Some(DecoFace::Warning)
            } else {
                None
            };
            if let Some(face) = face {
                let word = t.find([':', '[']).unwrap_or(t.len());
                let a = start + text[..text.len() - t.len()].chars().count();
                decos.push((a, a + t[..word].chars().count(), face, None));
            }
        }
    }
    b.set_decorations("compile", decos);
    n
}

// ----------------------------------------------------------- next-error

/// The location on `line` of an output buffer: a grep match's own, or
/// what the line says.
fn location_near(app: &App, b: &Buffer, errors: BufferId, line: usize) -> Option<Location> {
    if let Some(m) = grep_match_at(app, errors, line) {
        return Some(Location {
            file: m.file.clone(),
            line: m.line,
            column: m.column,
        });
    }
    process::parse_location(&b.line(line))
}

/// The grep match on `line` of an output buffer, if it is one.
fn grep_match_at(app: &App, errors: BufferId, line: usize) -> Option<&crate::grep::GrepMatch> {
    let matches = app.jobs.grep_matches.get(&errors)?;
    let i = matches.binary_search_by_key(&line, |m| m.out_line).ok()?;
    matches.get(i)
}

/// M-g n / M-g p: the `n`th next (or previous) location in the last
/// compilation or grep buffer.
fn next_error(app: &mut App, n: isize, from_start: bool) {
    let errors = app
        .jobs
        .errors
        .filter(|id| app.editor.state.buffers.contains_key(id))
        .or_else(|| app.editor.find_buffer(COMPILATION))
        .or_else(|| app.editor.find_buffer(GREP));
    let Some(errors) = errors else {
        return app.error("No compilation or grep output to go through");
    };
    if app.jobs.errors != Some(errors) {
        app.jobs.error_line = None;
    }
    app.jobs.errors = Some(errors);
    let b = app.editor.state.get_buffer(errors).expect("checked");
    let lines = b.line_count();
    let mut line = if from_start {
        None
    } else {
        app.jobs.error_line
    };
    let mut left = n.unsigned_abs().max(1);
    let forward = n >= 0;
    while left > 0 {
        let candidates: Box<dyn Iterator<Item = usize>> = match (forward, line) {
            (true, None) => Box::new(0..lines),
            (true, Some(l)) => Box::new(l + 1..lines),
            (false, None) => return app.error("No earlier locations"),
            (false, Some(l)) => Box::new((0..l).rev()),
        };
        let mut found = None;
        for l in candidates {
            if let Some(loc) = location_near(app, b, errors, l) {
                // Several lines pointing at the same place count once.
                let same = line
                    .and_then(|p| location_near(app, b, errors, p))
                    .is_some_and(|p| p == loc);
                if !same {
                    found = Some(l);
                    break;
                }
            }
        }
        match found {
            Some(l) => line = Some(l),
            None => {
                let running = app.jobs.running.values().any(|j| j.buffer == errors);
                return app.error(if !forward {
                    "No earlier locations"
                } else if running {
                    "No more locations yet"
                } else {
                    "No more locations"
                });
            }
        }
        left -= 1;
    }
    let line = line.expect("found one");
    app.jobs.error_line = Some(line);
    visit_error(app, errors, line);
}

/// Go to the location on `line` of output buffer `errors`: the source in
/// another window, the output buffer scrolled to the line.
fn visit_error(app: &mut App, errors: BufferId, line: usize) {
    let b = app.editor.state.get_buffer(errors).expect("alive");
    let Some(loc) = location_near(app, b, errors, line) else {
        return;
    };
    let showing_errors = app.active_id() == errors;
    // What the error says: the line itself, or (Rust) the "error: ..."
    // line above the --> one.
    let this = b.line(line);
    let message = if this.trim_start().starts_with("-->") {
        (line.saturating_sub(6)..line)
            .rev()
            .map(|l| b.line(l))
            .find(|t| t.starts_with("error") || t.starts_with("warning") || t.starts_with("note"))
            .unwrap_or(this)
    } else {
        this
    };
    let dir = app
        .jobs
        .dirs
        .get(&errors)
        .cloned()
        .unwrap_or_else(|| PathBuf::from("."));
    let path = if Path::new(&loc.file).is_absolute() {
        PathBuf::from(&loc.file)
    } else {
        dir.join(&loc.file)
    };
    let offset = b.pos_to_offset(Pos::new(line, 0));
    // The output buffer's window moves to the line.
    for w in app
        .editor
        .windows
        .iter()
        .filter(|w| w.buffer == errors)
        .map(|w| w.id)
        .collect::<Vec<_>>()
    {
        app.editor.set_window_point(w, offset);
    }
    if !path.exists() {
        return app.error(format!("No file {}", path.display()));
    }
    let exact = grep_match_at(app, errors, line).and_then(|m| m.text.clone());
    if !visit(app, &path, showing_errors) {
        return;
    }
    let col = loc.column.map_or(0, |c| c - 1);
    let b = app.buf();
    b.set_point(Pos::new(loc.line - 1, col));
    highlight(app, loc.line, exact);
    display_buffer(app, errors);
    app.msg(message.trim().to_string());
}

/// Mark the line the error is on in the source file, or just the exact
/// match in it, when the grep found one and the user asked for that.
fn highlight(app: &mut App, line: usize, exact: Option<String>) {
    // The mark of the last one goes, so only one is shown at a time.
    if let Some(previous) = app.jobs.grep_highlight.take()
        && let Some(b) = app.editor.state.get_buffer_mut(previous)
    {
        b.clear_decorations("grep");
    }
    let id = app.active_id();
    let highlight_matches = app.grep.highlight_matches;
    let Some(b) = app.editor.state.get_buffer_mut(id) else {
        return;
    };
    let text = b.line(line.saturating_sub(1));
    let start = b.pos_to_offset(Pos::new(line.saturating_sub(1), 0));
    let whole = (start, start + text.chars().count());
    let (from, to) = match (highlight_matches, exact) {
        (true, Some(exact)) => match text.find(&exact) {
            Some(at) => {
                let at = text[..at].chars().count();
                (start + at, start + at + exact.chars().count())
            }
            None => whole,
        },
        _ => whole,
    };
    if to > from {
        b.set_decorations("grep", vec![(from, to, DecoFace::Highlight, None)]);
        app.jobs.grep_highlight = Some(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_at_point() {
        let mut b = Buffer::new("t");
        b.set_contents("let foo_bar = 1;");
        b.set_point(Pos::new(0, 6));
        assert_eq!(word_at_point(&b), "foo_bar");
    }
}
