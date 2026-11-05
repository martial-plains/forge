// SPDX-License-Identifier: MIT OR Apache-2.0
//! Asynchronous subprocesses: M-x compile, grep, async shell commands,
//! and later language servers.
//!
//! A process runs on its own; threads read its output into a channel,
//! and the frontend calls [`Processes::poll`] when idle (the event loop
//! wakes every [`POLL_INTERVAL`] while any is running). Output comes as
//! text chunks, never split inside a UTF-8 character. A process's exit is
//! reported only after all of its output.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

/// How often to look for output while processes run.
pub const POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProcessId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessEvent {
    /// Output from stdout.
    Output(ProcessId, String),
    /// Output from stderr (interleaved with stdout as it arrives).
    Stderr(ProcessId, String),
    /// The process ended: its exit code (None if killed by a signal).
    Exited(ProcessId, Option<i32>),
}

enum Msg {
    Output(ProcessId, String, bool),
    /// One of the output streams ended.
    Eof(ProcessId),
}

struct Proc {
    child: Child,
    stdin: Option<ChildStdin>,
    /// Output streams still open.
    open_streams: u8,
    command: String,
}

/// What a spawn runs.
pub enum Program<'a> {
    /// A shell command line (`sh -c`, or `cmd /C` on Windows).
    Shell(&'a str),
    /// A program and its arguments, no shell.
    Argv(&'a str, &'a [String]),
}

/// What a spawn runs, owned: `forge.process.run` queues one of these
/// per call, and the editor spawns it after Lua returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessSpec {
    /// A shell command line (`sh -c`, or `cmd /C` on Windows).
    Shell(String),
    /// A program and its arguments, no shell.
    Argv(String, Vec<String>),
}

/// A file operation for `forge.fs`: performed after the Lua call
/// returns (never inside it, so nothing blocks), answered through the
/// Lua callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsOp {
    Read(std::path::PathBuf),
    Write(std::path::PathBuf, Vec<u8>),
    List(std::path::PathBuf),
    Exists(std::path::PathBuf),
}

/// One directory entry from `forge.fs.list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsEntry {
    pub name: String,
    pub dir: bool,
}

/// What a `forge.fs` call answers with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsAnswer {
    Read(Result<Vec<u8>, String>),
    Written(Result<(), String>),
    Listed(Result<Vec<FsEntry>, String>),
    Exists(bool),
}

/// Runs `forge.fs` calls on one background thread, in the order they were
/// asked, so a slow disk or a network mount never freezes the editor. The
/// frontend calls [`poll`](Self::poll) when idle.
pub struct FsWorker {
    jobs: Option<Sender<(u32, FsOp)>>,
    tx: Sender<(u32, FsAnswer)>,
    rx: Receiver<(u32, FsAnswer)>,
    pending: usize,
}

impl Default for FsWorker {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            jobs: None,
            tx,
            rx,
            pending: 0,
        }
    }
}

impl std::fmt::Debug for FsWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FsWorker")
            .field("pending", &self.pending)
            .finish()
    }
}

impl FsWorker {
    /// Queue `op`; its answer comes back from `poll` under `request`.
    pub fn start(&mut self, request: u32, op: FsOp) {
        self.pending += 1;
        if self.jobs.is_none() {
            let (jobs, queue) = mpsc::channel::<(u32, FsOp)>();
            let tx = self.tx.clone();
            let spawned = std::thread::Builder::new()
                .name("forge-fs".into())
                .spawn(move || {
                    while let Ok((request, op)) = queue.recv() {
                        if tx.send((request, perform_fs(&op))).is_err() {
                            break;
                        }
                    }
                });
            if spawned.is_ok() {
                self.jobs = Some(jobs);
            }
        }
        match &self.jobs {
            Some(jobs) => {
                let _ = jobs.send((request, op));
            }
            // No thread to be had: do it here rather than lose the call.
            None => {
                let _ = self.tx.send((request, perform_fs(&op)));
            }
        }
    }

    /// Answers that have arrived.
    pub fn poll(&mut self) -> Vec<(u32, FsAnswer)> {
        let mut out = Vec::new();
        while let Ok(a) = self.rx.try_recv() {
            out.push(a);
        }
        self.pending = self.pending.saturating_sub(out.len());
        out
    }

    pub fn any_running(&self) -> bool {
        self.pending > 0
    }
}

/// Files over this size are refused: init.rhai works with config-scale
/// files, and a call is capped at 64 MB anyway.
pub const FS_MAX_READ: u64 = 16 * 1024 * 1024;

/// Perform a `forge.fs` operation: fast, local work (regular files and
/// directories only — anything else errors rather than blocks), so the
/// editor thread stays responsive.
pub fn perform_fs(op: &FsOp) -> FsAnswer {
    use std::fs;
    match op {
        FsOp::Read(path) => match fs::symlink_metadata(path) {
            Ok(md) if md.is_file() => {
                if md.len() > FS_MAX_READ {
                    return FsAnswer::Read(Err(format!(
                        "{} is too big to read (over 16 MB)",
                        path.display()
                    )));
                }
                match fs::read(path) {
                    Ok(bytes) => FsAnswer::Read(Ok(bytes)),
                    Err(e) => FsAnswer::Read(Err(format!("cannot read {}: {e}", path.display()))),
                }
            }
            Ok(_) => FsAnswer::Read(Err(format!("{} isn't a regular file", path.display()))),
            Err(e) => FsAnswer::Read(Err(format!("cannot read {}: {e}", path.display()))),
        },
        FsOp::Write(path, data) => {
            if let Ok(md) = fs::symlink_metadata(path)
                && !md.is_file()
            {
                return FsAnswer::Written(Err(format!("{} isn't a regular file", path.display())));
            }
            if let Some(parent) = path.parent()
                && let Err(e) = fs::create_dir_all(parent)
            {
                return FsAnswer::Written(Err(format!("cannot create {}: {e}", parent.display())));
            }
            match fs::write(path, data) {
                Ok(()) => FsAnswer::Written(Ok(())),
                Err(e) => FsAnswer::Written(Err(format!("cannot write {}: {e}", path.display()))),
            }
        }
        FsOp::List(path) => {
            match fs::symlink_metadata(path) {
                Ok(md) if !md.is_dir() => {
                    return FsAnswer::Listed(Err(format!("{} isn't a directory", path.display())));
                }
                Err(e) => {
                    return FsAnswer::Listed(Err(format!("cannot list {}: {e}", path.display())));
                }
                _ => {}
            }
            match fs::read_dir(path) {
                Ok(entries) => {
                    let mut out: Vec<FsEntry> = entries
                        .filter_map(|e| e.ok())
                        .map(|e| FsEntry {
                            name: e.file_name().to_string_lossy().into_owned(),
                            dir: e.file_type().map(|t| t.is_dir()).unwrap_or(false),
                        })
                        .collect();
                    out.sort_by(|a, b| a.name.cmp(&b.name));
                    FsAnswer::Listed(Ok(out))
                }
                Err(e) => FsAnswer::Listed(Err(format!("cannot list {}: {e}", path.display()))),
            }
        }
        FsOp::Exists(path) => FsAnswer::Exists(fs::symlink_metadata(path).is_ok()),
    }
}

pub struct Processes {
    procs: BTreeMap<ProcessId, Proc>,
    next: u32,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

impl Default for Processes {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            procs: BTreeMap::new(),
            next: 1,
            tx,
            rx,
        }
    }
}

impl std::fmt::Debug for Processes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Processes")
            .field("running", &self.procs.len())
            .finish()
    }
}

/// What a synchronous run captured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOutput {
    /// The exit code (None if a signal killed it).
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Run `program` in `dir` to completion, capturing its output:
/// `forge.process.run`. `stdin` is fed to it; `timeout` kills it when it
/// runs too long; `stop` (C-g) stops it too. All three are answered while
/// waiting, so a run never hangs the editor.
pub fn run_sync(
    program: Program<'_>,
    dir: &Path,
    stdin: Option<&[u8]>,
    timeout: Duration,
    stop: &std::sync::atomic::AtomicBool,
) -> std::io::Result<SyncOutput> {
    use std::sync::atomic::Ordering;
    let mut cmd = match program {
        Program::Shell(line) => {
            #[cfg(windows)]
            let c = {
                use std::os::windows::process::CommandExt;
                let mut c = Command::new("cmd");
                c.raw_arg("/D /C ").raw_arg(line);
                c
            };
            #[cfg(not(windows))]
            let mut c = Command::new("sh");
            #[cfg(not(windows))]
            c.arg("-c").arg(line);
            c
        }
        Program::Argv(prog, args) => {
            let mut c = Command::new(prog);
            c.args(args);
            c
        }
    };
    cmd.current_dir(dir)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // A pager would hang the run waiting for a keypress.
        .env("PAGER", "cat");
    // Its own process group, so killing it stops what it started too (the
    // `sleep` under `sh -c`).
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = cmd.spawn()?;
    // Written on its own thread: a reader that never comes must not wedge
    // the wait below. The thread ends with the write, or with an error
    // once the process (and its end of the pipe) is gone.
    if let Some(input) = stdin.map(<[u8]>::to_vec)
        && let Some(mut pipe) = child.stdin.take()
    {
        std::thread::spawn(move || {
            let _ = pipe.write_all(&input);
        });
    }
    // Drained on their own threads, for the same reason: a child that
    // fills the pipe buffer would block writing and never exit, so
    // reading only after it is gone would wait for nothing.
    let stdout = child.stdout.take().map(drain);
    let stderr = child.stderr.take().map(drain);
    let start = std::time::Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) {
            kill_tree(&mut child);
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "stopped",
            ));
        }
        match child.try_wait()? {
            Some(status) => {
                return Ok(SyncOutput {
                    code: status.code(),
                    stdout: stdout.map_or_else(Vec::new, |t| t.join().unwrap_or_default()),
                    stderr: stderr.map_or_else(Vec::new, |t| t.join().unwrap_or_default()),
                });
            }
            None if start.elapsed() > timeout => {
                kill_tree(&mut child);
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("ran longer than {} seconds", timeout.as_secs()),
                ));
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

/// Kill the child and everything it started, and wait for it.
fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    // SAFETY: kill(2) with the child's process group (its pid, negated);
    // it has no memory effects.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Read a pipe to its end on its own thread, so the child never blocks
/// writing to it.
fn drain(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        buf
    })
}

impl Processes {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start `program` in `dir`. With `stdin`, the process gets a pipe to
    /// [`write`](Self::write) to; otherwise its stdin is empty. `env`
    /// adds variables on top of the inherited environment.
    pub fn spawn(
        &mut self,
        program: Program,
        dir: &Path,
        stdin: bool,
        env: &[(String, String)],
    ) -> std::io::Result<ProcessId> {
        self.spawn_with(program, dir, stdin, env, false)
    }

    /// Like [`spawn`](Self::spawn), but the process starts with an empty
    /// environment: only `env` (plus the sanitized few) is visible. This
    /// is how `forge.process.run` honors `forge.env`'s
    /// `process-environment`, which the caller owns whole.
    pub fn spawn_env(
        &mut self,
        program: Program,
        dir: &Path,
        stdin: bool,
        env: &[(String, String)],
    ) -> std::io::Result<ProcessId> {
        self.spawn_with(program, dir, stdin, env, true)
    }

    fn spawn_with(
        &mut self,
        program: Program,
        dir: &Path,
        stdin: bool,
        env: &[(String, String)],
        clear: bool,
    ) -> std::io::Result<ProcessId> {
        let (mut cmd, command) = match program {
            Program::Shell(line) => {
                #[cfg(windows)]
                let c = {
                    use std::os::windows::process::CommandExt;
                    let mut c = Command::new("cmd");
                    c.raw_arg("/D /C ").raw_arg(line);
                    c
                };
                #[cfg(not(windows))]
                let mut c = Command::new("sh");
                #[cfg(not(windows))]
                c.arg("-c").arg(line);
                (c, line.to_string())
            }
            Program::Argv(prog, args) => {
                let mut c = Command::new(prog);
                c.args(args);
                (
                    c,
                    std::iter::once(prog.to_string())
                        .chain(args.iter().cloned())
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            }
        };
        if clear {
            cmd.env_clear();
        }
        cmd.current_dir(dir)
            .stdin(if stdin { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Tools that color or page their output shouldn't here.
            .env("TERM", "dumb")
            .env("PAGER", "cat")
            .env("GIT_PAGER", "cat")
            .env("NO_COLOR", "1")
            .env("CARGO_TERM_COLOR", "never");
        for (k, v) in env {
            cmd.env(k, v);
        }
        // Its own process group, so kill stops what it started too (the
        // compiler under `make`, the `sleep` under `sh -c`).
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        let mut child = cmd.spawn()?;
        let id = ProcessId(self.next);
        self.next += 1;
        let out = child.stdout.take().expect("piped");
        let err = child.stderr.take().expect("piped");
        spawn_reader(id, out, false, self.tx.clone());
        spawn_reader(id, err, true, self.tx.clone());
        let stdin = child.stdin.take();
        self.procs.insert(
            id,
            Proc {
                child,
                stdin,
                open_streams: 2,
                command,
            },
        );
        Ok(id)
    }

    /// Send text to a process's stdin. Err if it has none or it's gone.
    pub fn write(&mut self, id: ProcessId, data: &[u8]) -> std::io::Result<()> {
        let p = self
            .procs
            .get_mut(&id)
            .ok_or_else(|| std::io::Error::other("no such process"))?;
        let stdin = p
            .stdin
            .as_mut()
            .ok_or_else(|| std::io::Error::other("the process has no input"))?;
        stdin.write_all(data)?;
        stdin.flush()
    }

    /// Close a process's stdin (end of input).
    pub fn close_stdin(&mut self, id: ProcessId) {
        if let Some(p) = self.procs.get_mut(&id) {
            p.stdin = None;
        }
    }

    /// Take a process's stdin, to feed it from another thread.
    pub fn take_stdin(&mut self, id: ProcessId) -> Option<std::process::ChildStdin> {
        self.procs.get_mut(&id).and_then(|p| p.stdin.take())
    }

    /// Stop a process. Its exit is still reported by [`poll`](Self::poll).
    pub fn kill(&mut self, id: ProcessId) -> bool {
        match self.procs.get_mut(&id) {
            Some(p) => {
                p.stdin = None;
                #[cfg(unix)]
                // SAFETY: kill(2) with the child's process group (its pid,
                // negated); it has no memory effects.
                unsafe {
                    libc::kill(-(p.child.id() as i32), libc::SIGKILL);
                }
                #[cfg(windows)]
                let _ = Command::new("taskkill")
                    .args(["/PID", &p.child.id().to_string(), "/T", "/F"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                p.child.kill().is_ok()
            }
            None => false,
        }
    }

    pub fn is_running(&self, id: ProcessId) -> bool {
        self.procs.contains_key(&id)
    }

    pub fn any_running(&self) -> bool {
        !self.procs.is_empty()
    }

    /// The command line a process was started with.
    pub fn command(&self, id: ProcessId) -> Option<&str> {
        self.procs.get(&id).map(|p| p.command.as_str())
    }

    /// Output and exits since the last call, in order.
    pub fn poll(&mut self) -> Vec<ProcessEvent> {
        let mut events = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Output(id, text, stderr) => match events.last_mut() {
                    Some(ProcessEvent::Output(last, t)) if *last == id && !stderr => {
                        t.push_str(&text)
                    }
                    Some(ProcessEvent::Stderr(last, t)) if *last == id && stderr => {
                        t.push_str(&text)
                    }
                    _ if stderr => events.push(ProcessEvent::Stderr(id, text)),
                    _ => events.push(ProcessEvent::Output(id, text)),
                },
                Msg::Eof(id) => {
                    if let Some(p) = self.procs.get_mut(&id) {
                        p.open_streams = p.open_streams.saturating_sub(1);
                    }
                }
            }
        }
        let done: Vec<ProcessId> = self
            .procs
            .iter_mut()
            .filter(|(_, p)| p.open_streams == 0)
            .filter_map(|(id, p)| p.child.try_wait().ok().flatten().map(|_| *id))
            .collect();
        for id in done {
            let mut p = self.procs.remove(&id).expect("listed");
            let status = p.child.wait().ok();
            events.push(ProcessEvent::Exited(id, status.and_then(|s| s.code())));
        }
        events
    }

    /// Wait (up to `timeout`) until process `id` has exited, collecting
    /// everything it does meanwhile. For tests and synchronous commands.
    pub fn wait(&mut self, id: ProcessId, timeout: Duration) -> Vec<ProcessEvent> {
        let deadline = std::time::Instant::now() + timeout;
        let mut all = Vec::new();
        while self.is_running(id) && std::time::Instant::now() < deadline {
            all.extend(self.poll());
            std::thread::sleep(Duration::from_millis(5));
        }
        all.extend(self.poll());
        all
    }
}

impl Drop for Processes {
    fn drop(&mut self) {
        let ids: Vec<ProcessId> = self.procs.keys().copied().collect();
        for id in ids {
            self.kill(id);
            // Wait for the killed child, so it does not stay a zombie.
            if let Some(p) = self.procs.get_mut(&id) {
                let _ = p.child.wait();
            }
        }
    }
}

fn spawn_reader(
    id: ProcessId,
    mut from: impl Read + Send + 'static,
    stderr: bool,
    tx: Sender<Msg>,
) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        let mut pending: Vec<u8> = Vec::new();
        loop {
            match from.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    pending.extend_from_slice(&buf[..n]);
                    // Send the complete characters; keep a split one.
                    let valid = match std::str::from_utf8(&pending) {
                        Ok(_) => pending.len(),
                        Err(e) if e.error_len().is_none() => e.valid_up_to(),
                        Err(_) => pending.len(), // invalid: sent lossily
                    };
                    let text = String::from_utf8_lossy(&pending[..valid]).into_owned();
                    pending.drain(..valid);
                    if !text.is_empty() && tx.send(Msg::Output(id, text, stderr)).is_err() {
                        return;
                    }
                }
            }
        }
        if !pending.is_empty() {
            let _ = tx.send(Msg::Output(
                id,
                String::from_utf8_lossy(&pending).into_owned(),
                stderr,
            ));
        }
        let _ = tx.send(Msg::Eof(id));
    });
}

// ---------------------------------------------------------------- projects

/// Is `start` (a file, or the directory it is in) inside a git repository?
/// Only `.git` counts here, unlike [`project_root`], which answers for
/// mercurial and the others too: this backs a condition that says "git".
pub fn in_git_repo(start: &Path) -> bool {
    let Some(start) = (if start.is_file() {
        start.parent()
    } else {
        Some(start)
    }) else {
        return false;
    };
    let Ok(start) = start.canonicalize() else {
        return false;
    };
    start.ancestors().any(|d| d.join(".git").exists())
}

/// The project a file or directory is in: the nearest directory above it
/// with a `.forge` (or `.project`), else the nearest with a build file (Cargo.toml, package.json, go.mod, Makefile, ...).
pub fn project_root(start: &Path) -> Option<std::path::PathBuf> {
    let start = if start.is_file() {
        start.parent()?
    } else {
        start
    };
    let start = start.canonicalize().ok()?;
    let has = |d: &Path, names: &[&str]| names.iter().any(|n| d.join(n).exists());
    // Version-control directories are deliberately not markers: a checkout
    // is not a project until it says so.
    let vcs = [".forge", ".project"];
    if let Some(d) = start.ancestors().find(|d| has(d, &vcs)) {
        return Some(d.to_path_buf());
    }
    let build = [
        "Cargo.toml",
        "package.json",
        "go.mod",
        "pyproject.toml",
        "Makefile",
        "CMakeLists.txt",
        "build.zig",
    ];
    start
        .ancestors()
        .find(|d| has(d, &build))
        .map(Path::to_path_buf)
}

/// A guess at the command that builds the project in `root`.
pub fn default_compile_command(root: &Path) -> String {
    let has = |n: &str| root.join(n).exists();
    if has("Cargo.toml") {
        "cargo build".into()
    } else if has("go.mod") {
        "go build ./...".into()
    } else if has("package.json") {
        "npm run build".into()
    } else if has("build.zig") {
        "zig build".into()
    } else {
        "make -k".into()
    }
}

/// The project's files, relative to `root`, sorted: `git ls-files` (plus
/// untracked files that aren't ignored) in a Git repository, else a walk
/// that skips hidden directories and build output. At most `limit`.
pub fn project_files(root: &Path, limit: usize) -> Vec<String> {
    if root.join(".git").exists() {
        let out = Command::new("git")
            .args([
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
            ])
            .current_dir(root)
            .stderr(Stdio::null())
            .output();
        if let Some(out) = out.ok().filter(|o| o.status.success()) {
            let mut files: Vec<String> = out
                .stdout
                .split(|&b| b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .filter(|f| root.join(f).is_file())
                .take(limit)
                .collect();
            files.sort();
            files.dedup();
            return files;
        }
    }
    const SKIP: &[&str] = &[
        "target",
        "node_modules",
        "__pycache__",
        "build",
        "dist",
        "DerivedData",
    ];
    let mut files = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = e.path();
            if name.starts_with('.') {
                continue;
            }
            match e.file_type() {
                Ok(t) if t.is_dir() && !SKIP.contains(&name.as_str()) => dirs.push(path),
                Ok(t) if t.is_dir() => {}
                Ok(t) if t.is_file() => {
                    if let Ok(rel) = path.strip_prefix(root) {
                        files.push(rel.to_string_lossy().replace('\\', "/"));
                        if files.len() >= limit {
                            files.sort();
                            return files;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    files.sort();
    files
}

/// A command line that searches the project for `pattern` (a regexp),
/// printing `file:line:text`: ripgrep if installed, else git grep in a
/// repository, else grep.
pub fn grep_command(root: &Path, pattern: &str) -> String {
    let q = shell_quote(pattern);
    let have = |prog: &str| {
        Command::new(prog)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    if have("rg") {
        format!("rg -n --no-heading --color=never -e {q}")
    } else if root.join(".git").exists() && have("git") {
        format!("git --no-pager grep -n -I -E -e {q}")
    } else {
        format!(
            "grep -rnIE --exclude-dir=.git --exclude-dir=target --exclude-dir=node_modules -e {q} ."
        )
    }
}

/// Quote for `sh -c`.
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

// ------------------------------------------------------------ error lines

/// A place in a file mentioned by compiler or grep output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub file: String,
    /// From 1.
    pub line: usize,
    /// From 1, if given.
    pub column: Option<usize>,
}

static RUST: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"^\s*--> ([^\s:][^:]*):(\d+):(\d+)").unwrap());
static PYTHON: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r#"^\s*File "([^"]+)", line (\d+)"#).unwrap());
/// file:line:col: or file:line: (GCC, Clang, Go, TypeScript with
/// --pretty false, grep -n, ripgrep, ...). The file can't start with a
/// blank and must look like a path.
static GNU: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^([^\s:][^:\n]*?):(\d+)(?::(\d+))?[:\s]").unwrap()
});
static TSC: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"^([^\s(][^(\n]*)\((\d+),(\d+)\): ").unwrap());

/// The location a line of compiler/grep output points at, if any.
pub fn parse_location(line: &str) -> Option<Location> {
    let num = |s: &str| s.parse::<usize>().ok().filter(|&n| n > 0);
    if let Some(c) = RUST.captures(line).or_else(|| TSC.captures(line)) {
        return Some(Location {
            file: c[1].to_string(),
            line: num(&c[2])?,
            column: num(&c[3]),
        });
    }
    if let Some(c) = PYTHON.captures(line) {
        return Some(Location {
            file: c[1].to_string(),
            line: num(&c[2])?,
            column: None,
        });
    }
    let c = GNU.captures(line)?;
    let file = c[1].trim();
    // Not "Compilation started at 12:30:05" or "warning: 3 things".
    if file.contains(char::is_whitespace) || file.starts_with("http") || file.is_empty() {
        return None;
    }
    Some(Location {
        file: file.to_string(),
        line: num(&c[2])?,
        column: c.get(3).and_then(|m| num(m.as_str())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_processes_and_reports_output_then_exit() {
        let mut p = Processes::new();
        let dir = std::env::temp_dir();
        #[cfg(unix)]
        let command = "printf 'hello\\n'; echo err >&2; exit 3";
        #[cfg(windows)]
        let command = "echo \"quoted value\" & echo err 1>&2 & exit /b 3";
        let id = p.spawn(Program::Shell(command), &dir, false, &[]).unwrap();
        let events = p.wait(id, Duration::from_secs(10));
        let out: String = events
            .iter()
            .filter_map(|e| match e {
                ProcessEvent::Output(_, t) | ProcessEvent::Stderr(_, t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            out.contains(if cfg!(windows) {
                "quoted value"
            } else {
                "hello"
            }) && out.contains("err"),
            "{out:?}"
        );
        assert_eq!(events.last(), Some(&ProcessEvent::Exited(id, Some(3))));
        assert!(!p.any_running());

        // stdin, and kill.
        #[cfg(unix)]
        let cat = p.spawn(Program::Argv("cat", &[]), &dir, true, &[]).unwrap();
        #[cfg(windows)]
        let cat = p
            .spawn(Program::Shell("findstr ."), &dir, true, &[])
            .unwrap();
        p.write(cat, b"ping\n").unwrap();
        p.close_stdin(cat);
        let events = p.wait(cat, Duration::from_secs(10));
        assert!(
            events.iter().any(|e| matches!(
                e,
                ProcessEvent::Output(_, text) if text.contains("ping")
            )),
            "{events:?}"
        );
        #[cfg(unix)]
        let command = "sleep 30";
        #[cfg(windows)]
        let command = "ping -n 30 127.0.0.1 >NUL";
        let sleep = p.spawn(Program::Shell(command), &dir, false, &[]).unwrap();
        assert!(p.kill(sleep));
        let events = p.wait(sleep, Duration::from_secs(10));
        assert!(matches!(events.last(), Some(ProcessEvent::Exited(_, _))));
    }

    #[test]
    fn locations() {
        let l = |s: &str| parse_location(s).map(|l| (l.file, l.line, l.column));
        assert_eq!(
            l("  --> crates/core/src/app.rs:12:5"),
            Some(("crates/core/src/app.rs".into(), 12, Some(5)))
        );
        assert_eq!(
            l("main.c:3:10: error: expected ';'"),
            Some(("main.c".into(), 3, Some(10)))
        );
        assert_eq!(
            l("src/lib.rs:40:fn main() {"),
            Some(("src/lib.rs".into(), 40, None))
        );
        assert_eq!(
            l(r#"  File "app.py", line 7, in <module>"#),
            Some(("app.py".into(), 7, None))
        );
        assert_eq!(
            l("src/a.ts(3,14): error TS2322"),
            Some(("src/a.ts".into(), 3, Some(14)))
        );
        assert_eq!(l("Compilation started at Mon Sep 28 12:30:05"), None);
        assert_eq!(l("error[E0308]: mismatched types"), None);
        assert_eq!(l("https://example.com:443/x"), None);
    }

    #[test]
    fn projects() {
        let root = std::env::temp_dir().join(format!("forge-project-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        std::fs::write(root.join("src/deep/a.rs"), "").unwrap();
        std::fs::write(root.join("target/junk"), "").unwrap();
        assert_eq!(
            project_root(&root.join("src/deep/a.rs")),
            Some(root.canonicalize().unwrap())
        );
        assert_eq!(default_compile_command(&root), "cargo build");
        assert_eq!(project_files(&root, 100), ["Cargo.toml", "src/deep/a.rs"]);
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote("foo_bar"), "foo_bar");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
