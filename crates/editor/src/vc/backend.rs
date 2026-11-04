// SPDX-License-Identifier: MIT OR Apache-2.0
//! The backend interface: what every version-control system Forge drives
//! (Git, Mercurial, ...) can be asked, so the commands in this module never
//! care which one is underneath. Modelled on vc.el backend API
//! (`vc-BACKEND-state`, `-diff`, `-print-log`, `-annotate-command`, ...).
//!
//! Operations come in two kinds. Queries the editor needs at once (a
//! file's state, the branch for the mode line, the revisions to complete)
//! run synchronously and are quick. Everything that can take a while or
//! print a lot (diffs, logs, annotations, commits, push and pull) is
//! returned as a [`Cmd`] for the asynchronous runner, so the editor keeps
//! responding while it runs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The state of one file, as `vc-state` names it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FileState {
    UpToDate,
    /// Changed in the working tree (or index) since the last commit.
    Edited,
    /// Scheduled to be added by the next commit.
    Added,
    /// Scheduled to be removed by the next commit.
    Removed,
    /// Tracked, but gone from the working tree.
    Missing,
    Ignored,
    /// Not under version control.
    Unregistered,
    /// Has unresolved merge conflicts.
    Conflict,
    /// A newer revision exists in the repository (centralized systems).
    NeedsUpdate,
    /// Both changed locally and in the repository.
    NeedsMerge,
    /// Locked by another user (lock-based systems).
    LockedBy(String),
}

impl FileState {
    /// The word vc-dir shows.
    pub fn label(&self) -> String {
        match self {
            FileState::UpToDate => "up-to-date".into(),
            FileState::Edited => "edited".into(),
            FileState::Added => "added".into(),
            FileState::Removed => "removed".into(),
            FileState::Missing => "missing".into(),
            FileState::Ignored => "ignored".into(),
            FileState::Unregistered => "unregistered".into(),
            FileState::Conflict => "conflict".into(),
            FileState::NeedsUpdate => "needs-update".into(),
            FileState::NeedsMerge => "needs-merge".into(),
            FileState::LockedBy(u) => format!("locked by {u}"),
        }
    }

    /// Under version control at all?
    pub fn registered(&self) -> bool {
        !matches!(self, FileState::Unregistered | FileState::Ignored)
    }

    /// Has changes a commit would record.
    pub fn committable(&self) -> bool {
        matches!(
            self,
            FileState::Edited
                | FileState::Added
                | FileState::Removed
                | FileState::Missing
                | FileState::NeedsMerge
        )
    }

    /// The section vc-dir groups it under.
    pub fn group(&self) -> &'static str {
        match self {
            FileState::Conflict | FileState::NeedsMerge => "Conflicted",
            FileState::Edited | FileState::LockedBy(_) => "Modified",
            FileState::Added => "Added",
            FileState::Removed | FileState::Missing => "Removed",
            FileState::Unregistered => "Unregistered",
            FileState::Ignored => "Ignored",
            FileState::NeedsUpdate => "Needs update",
            FileState::UpToDate => "Up to date",
        }
    }
}

/// The order vc-dir shows the groups in.
pub const GROUP_ORDER: &[&str] = &[
    "Conflicted",
    "Modified",
    "Added",
    "Removed",
    "Needs update",
    "Unregistered",
    "Up to date",
    "Ignored",
];

/// One file in a status listing: its path relative to the root, its
/// state, and anything extra to say (`renamed from x`, `staged`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    pub path: String,
    pub state: FileState,
    pub extra: Option<String>,
}

/// What a status listing knows about the repository as a whole.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoInfo {
    pub branch: Option<String>,
    /// The commit checked out (short).
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub stashes: usize,
    /// A merge, rebase, ... in progress.
    pub operation: Option<String>,
    pub remote_url: Option<String>,
}

/// A program to run and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cmd {
    pub program: String,
    pub args: Vec<String>,
    pub dir: PathBuf,
    /// Fed to its standard input (a commit message).
    pub stdin: Option<String>,
}

impl Cmd {
    pub fn new(program: &str, dir: &Path, args: &[&str]) -> Self {
        Self {
            program: program.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            dir: dir.to_path_buf(),
            stdin: None,
        }
    }

    pub fn arg(mut self, a: impl Into<String>) -> Self {
        self.args.push(a.into());
        self
    }

    pub fn args<I: IntoIterator<Item = S>, S: Into<String>>(mut self, a: I) -> Self {
        self.args.extend(a.into_iter().map(Into::into));
        self
    }

    pub fn stdin(mut self, text: impl Into<String>) -> Self {
        self.stdin = Some(text.into());
        self
    }

    /// As a shell would show it, for messages and the output buffer.
    pub fn line(&self) -> String {
        std::iter::once(self.program.clone())
            .chain(
                self.args
                    .iter()
                    .map(|a| forge_core::process::shell_quote(a)),
            )
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Run it to completion here and now. Output is decoded lossily.
    pub fn run(&self) -> Result<Output, String> {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut c = Command::new(&self.program);
        c.args(&self.args)
            .current_dir(&self.dir)
            .stdin(if self.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in super::runner::ENV {
            c.env(k, v);
        }
        let mut child = c.spawn().map_err(|e| spawn_error(&self.program, e))?;
        if let Some(input) = &self.stdin
            && let Some(mut pipe) = child.stdin.take()
        {
            let input = input.clone();
            std::thread::spawn(move || {
                let _ = pipe.write_all(input.as_bytes());
            });
        }
        let out = child
            .wait_with_output()
            .map_err(|e| format!("{}: {e}", self.program))?;
        Ok(Output {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    /// Run it, and fail with its error output unless it exits with 0.
    pub fn ok(&self) -> Result<String, String> {
        let out = self.run()?;
        if out.code == Some(0) {
            Ok(out.stdout)
        } else {
            Err(out.error_text(&self.line()))
        }
    }
}

/// "git is not installed" rather than "No such file or directory".
pub fn spawn_error(program: &str, e: std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::NotFound {
        format!(
            "Can't run {program}: it isn't installed, or isn't on PATH (see forge.opt.vc_{}_program)",
            program_key(program)
        )
    } else {
        format!("Can't run {program}: {e}")
    }
}

fn program_key(program: &str) -> &str {
    let base = Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(program);
    match base {
        "hg" => "hg",
        _ => "git",
    }
}

/// What a finished command printed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// The error to show for a failed run: its stderr (never swallowed),
    /// else its stdout, else the exit status.
    pub fn error_text(&self, what: &str) -> String {
        let err = self.stderr.trim();
        let out = self.stdout.trim();
        let detail = if !err.is_empty() {
            err
        } else if !out.is_empty() {
            out
        } else {
            ""
        };
        let status = match self.code {
            Some(c) => format!("exited with {c}"),
            None => "was killed".into(),
        };
        if detail.is_empty() {
            format!("{what} {status}")
        } else {
            // The last lines say what went wrong; the echo area has one.
            let last = detail
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or(detail);
            last.trim().to_string()
        }
    }
}

/// One entry of a log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogEntry {
    pub rev: String,
    pub short: String,
    pub author: String,
    pub email: String,
    pub date: String,
    pub refs: Vec<String>,
    pub parents: Vec<String>,
    /// The whole message (first line is the subject).
    pub message: String,
    /// The patch, for logs that carry one (region history).
    pub diff: Option<String>,
}

impl LogEntry {
    pub fn subject(&self) -> &str {
        self.message.lines().next().unwrap_or("")
    }
}

/// One line of an annotation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnnotatedLine {
    pub rev: String,
    pub author: String,
    /// Seconds since the epoch.
    pub time: i64,
    pub date: String,
    /// The file's name in that revision (it may have been renamed since).
    pub file: String,
    /// The revision before `rev`, and the file's name there.
    pub previous: Option<(String, String)>,
    pub summary: String,
    /// The line number in that revision (1-based).
    pub orig_line: usize,
    pub text: String,
}

/// Options for a commit (the log-edit headers).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckinOpts {
    pub amend: bool,
    pub author: Option<String>,
    pub date: Option<String>,
    pub signoff: bool,
    pub no_verify: bool,
}

/// What a backend can do beyond the basics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Caps {
    pub push: bool,
    pub pull: bool,
    pub fetch: bool,
    pub merge: bool,
    pub branches: bool,
    pub tags: bool,
    pub stash: bool,
    pub region_history: bool,
    /// A staging area (git's index).
    pub index: bool,
    /// Files must be checked out (locked) before editing.
    pub lock_based: bool,
    pub ignore: bool,
    pub amend: bool,
}

/// Which backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Git,
    Hg,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Git => "Git",
            Kind::Hg => "Hg",
        }
    }

    pub fn from_name(s: &str) -> Option<Kind> {
        match s.to_ascii_lowercase().as_str() {
            "git" => Some(Kind::Git),
            "hg" | "mercurial" => Some(Kind::Hg),
            _ => None,
        }
    }

    /// What marks a working tree's root.
    pub fn markers(self) -> &'static [&'static str] {
        match self {
            // A file named .git is a worktree or a submodule.
            Kind::Git => &[".git"],
            Kind::Hg => &[".hg"],
        }
    }

    pub fn backend(self) -> &'static dyn Backend {
        match self {
            Kind::Git => &super::git::Git,
            Kind::Hg => &super::hg::Hg,
        }
    }
}

/// A conflict's three versions of a file.
pub struct ConflictVersions {
    pub base: Option<String>,
    pub mine: String,
    pub theirs: String,
}

/// The interface. Every method has the repository root; file paths are
/// relative to it. Optional operations default to "not supported".
#[allow(unused_variables)]
pub trait Backend: Sync {
    fn kind(&self) -> Kind;
    fn caps(&self) -> Caps;
    /// The program to run (forge.opt.vc_git_program, ...).
    fn program(&self) -> String;

    // --- state
    fn file_state(&self, root: &Path, file: &str) -> Result<FileState, String>;
    /// Every interesting file under `dir` (relative to the root; "" for
    /// all of it), plus what's known about the repository.
    fn dir_status(
        &self,
        root: &Path,
        dir: &str,
        ignored: bool,
    ) -> Result<(Vec<StatusEntry>, RepoInfo), String>;
    fn dir_status_cmd(&self, root: &Path, dir: &str, ignored: bool) -> Cmd;
    fn parse_dir_status(&self, root: &Path, out: &str) -> (Vec<StatusEntry>, RepoInfo);
    /// The branch (or revision) the mode line shows.
    fn mode_line_rev(&self, root: &Path) -> Option<String>;
    /// The revision the working tree is at.
    fn working_revision(&self, root: &Path) -> Option<String>;
    fn list_tracked_files(&self, root: &Path) -> Result<Vec<String>, String>;

    // --- per-file operations
    fn register(&self, root: &Path, files: &[String]) -> Cmd;
    fn unregister(&self, root: &Path, files: &[String]) -> Cmd;
    fn revert(&self, root: &Path, files: &[(String, FileState)]) -> Vec<Cmd>;
    fn checkin(&self, root: &Path, files: &[String], message: &str, opts: &CheckinOpts) -> Cmd;
    /// The message of the last commit (for amending).
    fn last_message(&self, root: &Path) -> Option<String> {
        None
    }
    fn find_revision(&self, root: &Path, file: &str, rev: &str) -> Result<String, String>;
    /// Recent revisions that touched `file` (or the repository), newest first.
    fn revision_completions(&self, root: &Path, file: Option<&str>) -> Vec<String>;
    fn delete_file(&self, root: &Path, file: &str) -> Cmd;
    fn rename_file(&self, root: &Path, from: &str, to: &str) -> Cmd;
    fn ignore(&self, root: &Path, pattern: &str) -> Result<PathBuf, String> {
        Err(format!("{} can't ignore files", self.kind().name()))
    }

    // --- history
    /// `rev1` against `rev2`; `rev2` None is the working tree, `rev1` None
    /// the working revision.
    fn diff(
        &self,
        root: &Path,
        files: &[String],
        rev1: Option<&str>,
        rev2: Option<&str>,
        ignore_ws: bool,
    ) -> Cmd;
    fn log(&self, root: &Path, files: &[String], limit: usize, start: Option<&str>) -> Cmd;
    fn parse_log(&self, out: &str) -> Vec<LogEntry>;
    fn region_history(
        &self,
        root: &Path,
        file: &str,
        from: usize,
        to: usize,
    ) -> Result<Cmd, String> {
        Err(format!("{} has no region history", self.kind().name()))
    }
    fn incoming(&self, root: &Path, remote: Option<&str>) -> Result<Cmd, String> {
        Err(format!(
            "{} can't show incoming changes",
            self.kind().name()
        ))
    }
    fn outgoing(&self, root: &Path, remote: Option<&str>) -> Result<Cmd, String> {
        Err(format!(
            "{} can't show outgoing changes",
            self.kind().name()
        ))
    }
    fn annotate(&self, root: &Path, file: &str, rev: Option<&str>) -> Cmd;
    fn parse_annotate(&self, out: &str) -> Vec<AnnotatedLine>;
    /// The revision before `rev` (for the file, when given).
    fn previous_revision(&self, root: &Path, file: Option<&str>, rev: &str) -> Option<String>;
    fn next_revision(&self, root: &Path, file: Option<&str>, rev: &str) -> Option<String>;
    fn mergebase(&self, root: &Path, a: &str, b: &str) -> Option<String> {
        None
    }

    // --- remote, branches, tags, stash
    fn pull(&self, root: &Path) -> Result<Cmd, String> {
        Err(format!("{} can't pull", self.kind().name()))
    }
    fn push(&self, root: &Path) -> Result<Cmd, String> {
        Err(format!("{} can't push", self.kind().name()))
    }
    fn fetch(&self, root: &Path) -> Result<Cmd, String> {
        Err(format!("{} can't fetch", self.kind().name()))
    }
    /// True when a plain push has somewhere to go.
    fn has_upstream(&self, root: &Path) -> bool {
        true
    }
    fn remotes(&self, root: &Path) -> Vec<String> {
        Vec::new()
    }
    fn merge(&self, root: &Path, rev: &str) -> Result<Cmd, String> {
        Err(format!("{} can't merge", self.kind().name()))
    }
    fn branches(&self, root: &Path, remote: bool) -> Vec<String> {
        Vec::new()
    }
    fn current_branch(&self, root: &Path) -> Option<String> {
        None
    }
    fn create_branch(&self, root: &Path, name: &str, start: Option<&str>) -> Result<Cmd, String> {
        Err(format!("{} has no branches", self.kind().name()))
    }
    fn switch_branch(&self, root: &Path, name: &str) -> Result<Cmd, String> {
        Err(format!("{} has no branches", self.kind().name()))
    }
    fn rename_branch(&self, root: &Path, from: &str, to: &str) -> Result<Cmd, String> {
        Err(format!("{} can't rename branches", self.kind().name()))
    }
    fn delete_branch(&self, root: &Path, name: &str, force: bool) -> Result<Cmd, String> {
        Err(format!("{} can't delete branches", self.kind().name()))
    }
    fn tags(&self, root: &Path) -> Vec<String> {
        Vec::new()
    }
    fn create_tag(&self, root: &Path, name: &str, rev: Option<&str>) -> Result<Cmd, String> {
        Err(format!("{} has no tags", self.kind().name()))
    }
    fn retrieve_tag(&self, root: &Path, name: &str) -> Result<Cmd, String> {
        Err(format!("{} has no tags", self.kind().name()))
    }
    /// (name, description), newest first.
    fn stashes(&self, root: &Path) -> Vec<(String, String)> {
        Vec::new()
    }
    fn stash(&self, root: &Path, message: &str) -> Result<Cmd, String> {
        Err(format!("{} has no stash", self.kind().name()))
    }
    fn stash_apply(&self, root: &Path, name: &str, pop: bool) -> Result<Cmd, String> {
        Err(format!("{} has no stash", self.kind().name()))
    }
    fn stash_drop(&self, root: &Path, name: &str) -> Result<Cmd, String> {
        Err(format!("{} has no stash", self.kind().name()))
    }
    fn stash_show(&self, root: &Path, name: &str) -> Result<Cmd, String> {
        Err(format!("{} has no stash", self.kind().name()))
    }

    // --- index, conflicts
    fn stage(&self, root: &Path, files: &[String]) -> Result<Cmd, String> {
        Err(format!("{} has no staging area", self.kind().name()))
    }
    fn unstage(&self, root: &Path, files: &[String]) -> Result<Cmd, String> {
        Err(format!("{} has no staging area", self.kind().name()))
    }
    fn mark_resolved(&self, root: &Path, files: &[String]) -> Cmd;
    fn conflict_versions(&self, root: &Path, file: &str) -> Result<ConflictVersions, String> {
        Err(format!(
            "{} can't show the sides of a conflict",
            self.kind().name()
        ))
    }
    /// A grep over tracked files only (`git grep`), as a shell command line.
    fn grep_command(&self, root: &Path, regexp: &str, files: &str) -> Option<String> {
        None
    }
}

/// Configuration the backends read.
#[derive(Debug, Clone)]
pub struct Config {
    /// vc-handled-backends: which to try, in order.
    pub handled: Vec<Kind>,
    pub git_program: String,
    pub hg_program: String,
    pub git_diff_switches: Vec<String>,
    pub git_log_switches: Vec<String>,
    pub git_annotate_switches: Vec<String>,
    /// vc-log-show-limit.
    pub log_limit: usize,
    /// Directories never looked at (network mounts, say).
    pub ignore_dir: Option<regex::Regex>,
    /// A project root's own backend (vc_project_backends).
    pub project_backends: BTreeMap<PathBuf, Kind>,
    pub annotate_colors: bool,
    pub diff_refine: bool,
    /// vc-make-backup-files: back up files under version control
    /// too (off: the repository is their backup).
    pub make_backup_files: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            handled: vec![Kind::Git, Kind::Hg],
            git_program: "git".into(),
            hg_program: "hg".into(),
            git_diff_switches: Vec::new(),
            git_log_switches: Vec::new(),
            git_annotate_switches: Vec::new(),
            log_limit: 2000,
            ignore_dir: None,
            project_backends: BTreeMap::new(),
            annotate_colors: true,
            diff_refine: true,
            make_backup_files: false,
        }
    }
}

/// Which backend controls `dir`, and its root: the nearest directory at
/// or above it with a marker of a handled backend (so a nested repository
/// wins over the one around it). A project's own choice comes first.
pub fn detect(dir: &Path, cfg: &Config) -> Option<(Kind, PathBuf)> {
    if let Some(re) = &cfg.ignore_dir
        && re.is_match(&dir.to_string_lossy())
    {
        return None;
    }
    for d in dir.ancestors() {
        if let Some(kind) = cfg.project_backends.get(d)
            && kind.markers().iter().any(|m| d.join(m).exists())
        {
            return Some((*kind, d.to_path_buf()));
        }
        for kind in &cfg.handled {
            if kind.markers().iter().any(|m| d.join(m).exists()) {
                return Some((*kind, d.to_path_buf()));
            }
        }
    }
    None
}

/// `path` relative to `root`, with `/` separators.
pub fn relative(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let s = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_takes_the_nearest_root_and_honours_overrides() {
        let d = std::env::temp_dir().join(format!("forge-vc-detect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("outer/.git")).unwrap();
        std::fs::create_dir_all(d.join("outer/inner/.hg")).unwrap();
        std::fs::create_dir_all(d.join("outer/inner/src")).unwrap();
        // A worktree: .git is a file.
        std::fs::create_dir_all(d.join("wt")).unwrap();
        std::fs::write(d.join("wt/.git"), "gitdir: /elsewhere\n").unwrap();
        let cfg = Config::default();
        assert_eq!(
            detect(&d.join("outer/inner/src"), &cfg),
            Some((Kind::Hg, d.join("outer/inner")))
        );
        assert_eq!(
            detect(&d.join("outer"), &cfg),
            Some((Kind::Git, d.join("outer")))
        );
        assert_eq!(detect(&d.join("wt"), &cfg), Some((Kind::Git, d.join("wt"))));
        let only_git = Config {
            handled: vec![Kind::Git],
            ..Default::default()
        };
        assert_eq!(
            detect(&d.join("outer/inner/src"), &only_git),
            Some((Kind::Git, d.join("outer")))
        );
        let ignoring = Config {
            ignore_dir: Some(regex::Regex::new("outer").unwrap()),
            ..Default::default()
        };
        assert_eq!(detect(&d.join("outer"), &ignoring), None);
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn errors_show_stderr() {
        let o = Output {
            code: Some(1),
            stdout: String::new(),
            stderr: "hint: blah\nerror: failed to push some refs\n".into(),
        };
        assert_eq!(o.error_text("git push"), "error: failed to push some refs");
        let o = Output {
            code: Some(2),
            ..Default::default()
        };
        assert_eq!(o.error_text("git x"), "git x exited with 2");
    }
}
