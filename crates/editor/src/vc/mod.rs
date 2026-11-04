// SPDX-License-Identifier: MIT OR Apache-2.0
//! Version control, the way VC does it: one set of commands under
//! `C-x v` that works the same whatever system is underneath (Git and
//! Mercurial here; see [`backend`]).
//!
//! The commands work on a *fileset*: the file the selected buffer visits,
//! or the files marked in `*vc-dir*` or Dired.
//!
//! | Keys        | Command                                                   |
//! |-------------|-----------------------------------------------------------|
//! | C-x v v     | do the next sensible thing (register, commit, resolve...) |
//! | C-x v i     | register the file                                         |
//! | C-x v =     | diff against the last commit (C-u: pick revisions)        |
//! | C-x v D     | diff the whole repository                                 |
//! | C-x v l / L | the file's / repository's log                             |
//! | C-x v I / O | incoming / outgoing changes                               |
//! | C-x v g     | annotate (blame)                                          |
//! | C-x v ~     | visit another revision                                    |
//! | C-x v u     | revert (discard changes)                                  |
//! | C-x v h     | history of the region                                     |
//! | C-x v d     | the status buffer (vc-dir)                                |
//! | C-x v + / P | pull / push                                               |
//! | C-x v m     | merge                                                     |
//! | C-x v G     | ignore a file                                             |
//! | C-x v s / r | make / check out a tag                                    |
//! | C-x v b c/s/l | create / switch to / log a branch                       |
//! | C-x v x     | delete the file                                           |
//! | C-x v !     | edit the next command's command line                      |
//! | C-x v M D/L | diff / log against the merge base                         |
//!
//! The state of each file shows in its mode line (`Git-main` up to date,
//! `Git:main` edited, `Git@main` added, `Git!main` conflicted or removed,
//! `Git?main` missing), refreshed on visiting, saving and regaining focus.

pub mod annotate;
pub mod backend;
pub mod branches;
pub mod diff;
pub mod dir;
pub mod git;
pub mod hg;
pub mod log;
pub mod log_edit;
pub mod runner;
pub mod smerge;

use crate::app::App;
use crate::minibuffer::{ConfirmAction, Purpose};
use backend::{Backend, Cmd, Config, FileState, Kind};
use forge_core::{Buffer, BufferId, Pos};
use forge_docs::CommandDoc;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

// --------------------------------------------------------------- config

fn config_cell() -> &'static Mutex<Config> {
    static C: OnceLock<Mutex<Config>> = OnceLock::new();
    C.get_or_init(Default::default)
}

pub fn config() -> Config {
    config_cell().lock().map(|c| c.clone()).unwrap_or_default()
}

pub fn set_config(c: Config) {
    if let Ok(mut cur) = config_cell().lock() {
        *cur = c;
    }
}

/// `~/x` to the home directory.
pub fn expand_home(s: &str) -> PathBuf {
    match (s.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(h)) => PathBuf::from(h).join(rest),
        _ if s == "~" => std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| s.into()),
        _ => PathBuf::from(s),
    }
}

/// Take over the version-control options from forge.opt.
pub fn apply_settings(s: &forge_core::rhai::Settings) -> Result<(), String> {
    crate::authinfo::set_files(s.strings.get("auth_source_files").cloned());
    let mut c = Config::default();
    let one = |k: &str| s.strings.get(k).and_then(|v| v.first().cloned());
    if let Some(list) = s.strings.get("vc_handled_backends") {
        let mut kinds = Vec::new();
        for name in list {
            match Kind::from_name(name) {
                Some(k) => kinds.push(k),
                None => {
                    return Err(format!(
                        "forge.opt.vc_handled_backends: unknown backend {name:?} (Git or Hg)"
                    ));
                }
            }
        }
        c.handled = kinds;
    }
    if let Some(p) = one("vc_git_program") {
        c.git_program = p;
    }
    if let Some(p) = one("vc_hg_program") {
        c.hg_program = p;
    }
    if let Some(v) = s.strings.get("vc_git_diff_switches") {
        c.git_diff_switches = v.clone();
    }
    if let Some(v) = s.strings.get("vc_git_log_switches") {
        c.git_log_switches = v.clone();
    }
    if let Some(v) = s.strings.get("vc_git_annotate_switches") {
        c.git_annotate_switches = v.clone();
    }
    if let Some(n) = s.numbers.get("vc_log_show_limit") {
        c.log_limit = *n as usize;
    }
    if let Some(re) = one("vc_ignore_dir_regexp") {
        c.ignore_dir = Some(
            regex::Regex::new(&re).map_err(|e| format!("forge.opt.vc_ignore_dir_regexp: {e}"))?,
        );
    }
    if let Some(m) = s.maps.get("vc_project_backends") {
        for (dir, name) in m {
            let kind = Kind::from_name(name).ok_or_else(|| {
                format!("forge.opt.vc_project_backends: unknown backend {name:?}")
            })?;
            let dir = expand_home(dir);
            let dir = dir.canonicalize().unwrap_or(dir);
            c.project_backends.insert(dir, kind);
        }
    }
    if let Some(b) = s.flags.get("vc_annotate_colors") {
        c.annotate_colors = *b;
    }
    if let Some(b) = s.flags.get("diff_refine") {
        c.diff_refine = *b;
    }
    if let Some(b) = s.flags.get("vc_make_backup_files") {
        c.make_backup_files = *b;
    }
    set_config(c);
    Ok(())
}

// ---------------------------------------------------------------- state

/// What VC knows about a buffer's file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    pub kind: Kind,
    pub root: PathBuf,
    /// Relative to the root.
    pub rel: String,
    pub state: FileState,
}

/// Everything VC keeps, per buffer and overall.
#[derive(Default)]
pub struct Vc {
    pub runner: runner::Runner,
    pub files: HashMap<BufferId, FileInfo>,
    /// Buffers looked at already (with or without a VCS).
    checked: HashSet<BufferId>,
    pub dirs: HashMap<BufferId, dir::DirView>,
    pub logs: HashMap<BufferId, log::LogView>,
    pub diffs: HashMap<BufferId, diff::DiffView>,
    /// The version an editable diff's headers were last fixed at.
    pub diff_versions: HashMap<BufferId, u64>,
    pub annotations: HashMap<BufferId, annotate::AnnotateView>,
    pub log_edits: HashMap<BufferId, log_edit::Session>,
    pub history: log_edit::History,
    pub lists: HashMap<BufferId, branches::ListView>,
    /// A buffer showing an old revision: (kind, root, file, revision).
    pub revisions: HashMap<BufferId, (Kind, PathBuf, String, String)>,
    /// C-x v !: the next command's command line is edited first.
    pub edit_next: bool,
    pub last_apply: Option<diff::Applied>,
    pub merge: Option<smerge::MergeSession>,
}

/// The fileset a command works on.
#[derive(Debug, Clone)]
pub struct Fileset {
    pub kind: Kind,
    pub root: PathBuf,
    /// (path relative to the root, state); empty means the whole tree.
    pub files: Vec<(String, FileState)>,
}

impl Fileset {
    pub fn names(&self) -> Vec<String> {
        self.files.iter().map(|(f, _)| f.clone()).collect()
    }

    pub fn backend(&self) -> &'static dyn Backend {
        self.kind.backend()
    }

    /// "foo.rs", "3 files", or the repository's name.
    pub fn describe(&self) -> String {
        match self.files.as_slice() {
            [] => crate::files::abbreviate(&self.root).to_string(),
            [(one, _)] => one.clone(),
            many => format!("{} files", many.len()),
        }
    }
}

/// Which backend and root govern `path` (a file or directory).
pub fn responsible(path: &Path) -> Option<(Kind, PathBuf)> {
    let dir = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()?.to_path_buf()
    };
    let dir = dir.canonicalize().unwrap_or(dir);
    backend::detect(&dir, &config())
}

/// Find out (again) what VC says about a buffer's file, and update its
/// mode line.
pub fn refresh_buffer(app: &mut App, id: BufferId) {
    app.vc.checked.insert(id);
    let Some(path) = app
        .editor
        .state
        .get_buffer(id)
        .and_then(|b| b.path().map(Path::to_path_buf))
    else {
        app.vc.files.remove(&id);
        return;
    };
    let info = responsible(&path).and_then(|(kind, root)| {
        let path = path.canonicalize().unwrap_or(path.clone());
        let rel = backend::relative(&root, &path)?;
        let state = kind.backend().file_state(&root, &rel).ok()?;
        Some(FileInfo {
            kind,
            root,
            rel,
            state,
        })
    });
    let mode_line = info.as_ref().map(mode_line_string);
    let was_conflict = app
        .vc
        .files
        .get(&id)
        .is_some_and(|i| i.state == FileState::Conflict);
    match &info {
        Some(i) => {
            app.vc.files.insert(id, i.clone());
        }
        None => {
            app.vc.files.remove(&id);
        }
    }
    // Like vc-find-file-hook: a registered file's backups are
    // its repository, unless vc_make_backup_files says otherwise.
    let inhibit =
        info.as_ref().is_some_and(|i| i.state.registered()) && !config().make_backup_files;
    if let Some(b) = app.editor.state.get_buffer_mut(id) {
        b.vc_mode = mode_line;
        b.backup_inhibited = inhibit;
    }
    let conflict = info
        .as_ref()
        .is_some_and(|i| i.state == FileState::Conflict);
    if conflict && !was_conflict {
        smerge::maybe_enable(app, id, true);
    }
}

/// The mode line's VC part, as vc-default-mode-line-string makes
/// it: backend, a character for the state, and the branch.
pub fn mode_line_string(i: &FileInfo) -> String {
    let rev = i
        .kind
        .backend()
        .mode_line_rev(&i.root)
        .unwrap_or_else(|| "?".into());
    let name = i.kind.name();
    match &i.state {
        FileState::UpToDate | FileState::NeedsUpdate => format!("{name}-{rev}"),
        FileState::LockedBy(u) => format!("{name}:{u}:{rev}"),
        FileState::Added => format!("{name}@{rev}"),
        FileState::Conflict | FileState::Removed => format!("{name}!{rev}"),
        FileState::Missing => format!("{name}?{rev}"),
        FileState::Unregistered | FileState::Ignored => format!("{name}?{rev}"),
        _ => format!("{name}:{rev}"),
    }
}

/// Make sure the selected buffer has been looked at (cheap when it has).
pub fn ensure_state(app: &mut App) {
    let id = app.active_id();
    if !app.vc.checked.contains(&id) {
        refresh_buffer(app, id);
        if app.buf_ref().path().is_some() {
            smerge::maybe_enable(app, id, false);
        }
    }
}

/// Forget a buffer about to be killed (a commit message in it is kept as
/// a draft).
pub fn forget_buffer(app: &mut App, id: BufferId) {
    if app.vc.log_edits.contains_key(&id) {
        log_edit::save_draft(app, id);
    }
    app.vc.checked.remove(&id);
    app.vc.files.remove(&id);
    app.vc.dirs.remove(&id);
    app.vc.logs.remove(&id);
    app.vc.diffs.remove(&id);
    app.vc.diff_versions.remove(&id);
    app.vc.annotations.remove(&id);
    app.vc.lists.remove(&id);
    app.vc.revisions.remove(&id);
    app.vc.log_edits.remove(&id);
    if app.vc.merge.as_ref().is_some_and(|m| m.result == id) {
        app.vc.merge = None;
    }
}

/// After a VC operation in `root`: refresh every buffer there, reload the
/// unmodified ones whose files changed (a checkout, a pull), and refresh
/// the status buffers.
pub fn refresh_root(app: &mut App, root: &Path) {
    let ids: Vec<BufferId> = app.editor.buffer_ids();
    for id in ids {
        let in_root = app
            .editor
            .state
            .get_buffer(id)
            .and_then(|b| b.path())
            .is_some_and(|p| p.starts_with(root));
        if in_root {
            app.check_file(id);
            refresh_buffer(app, id);
        }
    }
    dir::refresh_for_root(app, root);
}

/// The terminal regained focus: files may have changed state.
pub fn on_focus(app: &mut App) {
    let shown: Vec<BufferId> = app.editor.windows.iter().map(|w| w.buffer).collect();
    for id in shown {
        if app.vc.files.contains_key(&id) {
            refresh_buffer(app, id);
        }
        if app.vc.dirs.contains_key(&id) {
            dir::refresh(app, id);
        }
    }
}

/// After saving a buffer.
pub fn after_save(app: &mut App, id: BufferId) {
    refresh_buffer(app, id);
    smerge::after_save(app, id);
    if let Some(root) = app.vc.files.get(&id).map(|i| i.root.clone()) {
        dir::refresh_for_root(app, &root);
    }
}

/// What the mode line says while VC commands run.
pub fn mode_line_extra(app: &App) -> Vec<String> {
    app.vc.runner.titles()
}

/// Deduce the fileset from where the user is:
/// marked files in `*vc-dir*` or Dired, a log or diff's files, or the
/// visited file. `whole_tree` gives the repository's root instead.
pub fn deduce_fileset(app: &mut App, whole_tree: bool) -> Result<Fileset, String> {
    let id = app.active_id();
    if let Some(v) = app.vc.dirs.get(&id) {
        let (kind, root) = (v.kind, v.root.clone());
        if whole_tree {
            return Ok(Fileset {
                kind,
                root,
                files: Vec::new(),
            });
        }
        let files = dir::selected(app);
        if files.is_empty() {
            return Err("No files selected (move to a file, or mark some with m)".into());
        }
        return Ok(Fileset { kind, root, files });
    }
    if let Some(s) = app.vc.log_edits.get(&id) {
        return Ok(s.fileset.clone());
    }
    if let Some(v) = app.vc.logs.get(&id) {
        let files = if whole_tree {
            Vec::new()
        } else {
            v.files
                .iter()
                .map(|f| (f.clone(), FileState::UpToDate))
                .collect()
        };
        return Ok(Fileset {
            kind: v.kind,
            root: v.root.clone(),
            files,
        });
    }
    if let Some(v) = app.vc.annotations.get(&id) {
        let files = if whole_tree {
            Vec::new()
        } else {
            vec![(v.file.clone(), FileState::UpToDate)]
        };
        return Ok(Fileset {
            kind: v.kind,
            root: v.root.clone(),
            files,
        });
    }
    if let Some((kind, root, file, _)) = app.vc.revisions.get(&id).cloned() {
        let files = if whole_tree {
            Vec::new()
        } else {
            vec![(file, FileState::UpToDate)]
        };
        return Ok(Fileset { kind, root, files });
    }
    if let Some(v) = app.vc.diffs.get(&id)
        && let Some(kind) = v.kind
    {
        let files = if whole_tree {
            Vec::new()
        } else {
            v.files
                .iter()
                .map(|f| (f.clone(), FileState::Edited))
                .collect()
        };
        return Ok(Fileset {
            kind,
            root: v.root.clone(),
            files,
        });
    }
    if app.buf_ref().major_mode() == crate::dired::MODE {
        let dirpath = crate::dired::directory(app.buf_ref()).ok_or("No directory here")?;
        let (kind, root) = responsible(&dirpath).ok_or_else(|| {
            format!(
                "{} is not under version control",
                crate::files::abbreviate(&dirpath)
            )
        })?;
        if whole_tree {
            return Ok(Fileset {
                kind,
                root,
                files: Vec::new(),
            });
        }
        let paths = crate::dired::selected_paths(app).unwrap_or_else(|| vec![dirpath.clone()]);
        let mut files = Vec::new();
        for p in paths {
            let p = p.canonicalize().unwrap_or(p);
            let rel = backend::relative(&root, &p).ok_or("A file outside the repository")?;
            let state = if p.is_dir() {
                FileState::Edited
            } else {
                kind.backend().file_state(&root, &rel)?
            };
            files.push((rel, state));
        }
        return Ok(Fileset { kind, root, files });
    }
    if !app.vc.checked.contains(&id) || !app.vc.files.contains_key(&id) {
        refresh_buffer(app, id);
    }
    match app.vc.files.get(&id) {
        Some(i) => Ok(Fileset {
            kind: i.kind,
            root: i.root.clone(),
            files: if whole_tree {
                Vec::new()
            } else {
                vec![(i.rel.clone(), i.state.clone())]
            },
        }),
        None => {
            // Not a file, or not in a repository: the directory then.
            let dir = match app.buf_ref().path() {
                Some(p) => p.parent().map(Path::to_path_buf),
                None => None,
            }
            .unwrap_or_else(|| crate::compile::default_directory(app));
            match responsible(&dir) {
                Some((kind, root)) if whole_tree || app.buf_ref().path().is_some() => {
                    let files = if whole_tree {
                        Vec::new()
                    } else {
                        let p = app.buf_ref().path().expect("checked").to_path_buf();
                        let p = p.canonicalize().unwrap_or(p);
                        let rel =
                            backend::relative(&root, &p).ok_or("A file outside the repository")?;
                        vec![(rel, FileState::Unregistered)]
                    };
                    Ok(Fileset { kind, root, files })
                }
                Some((kind, root)) => Ok(Fileset {
                    kind,
                    root,
                    files: Vec::new(),
                }),
                None => Err(match app.buf_ref().path() {
                    Some(p) => format!(
                        "{} is not under version control (M-x vc-create-repo makes a repository)",
                        crate::files::abbreviate(p)
                    ),
                    None => format!(
                        "Buffer {} is not associated with a version-controlled file",
                        app.buf_ref().name()
                    ),
                }),
            }
        }
    }
}

fn fileset_or_error(app: &mut App, whole: bool) -> Option<Fileset> {
    match deduce_fileset(app, whole) {
        Ok(f) => Some(f),
        Err(e) => {
            app.error(e);
            None
        }
    }
}

// ---------------------------------------------------------- questions

/// What a VC prompt is asking for.
#[derive(Debug, Clone)]
pub enum Ask {
    /// vc-diff with C-u: the older revision, then the newer.
    DiffRev1(Fileset),
    DiffRev2(Fileset, String),
    /// vc-print-log with C-u: how many entries, then from where.
    LogLimit(Fileset, bool),
    LogStart(Fileset, bool, usize),
    BranchLog(Fileset),
    /// vc-annotate with C-u.
    AnnotateRev(Fileset),
    RevisionOtherWindow(Fileset),
    /// vc-next-action with C-u on a registered file: check out a revision.
    CheckoutRev(Fileset),
    /// ... on an unregistered one: which backend to register with.
    RegisterBackend(Fileset),
    Dir,
    Merge(Fileset),
    CreateBranch(Fileset),
    SwitchBranch(Fileset),
    RenameBranch(Fileset),
    RenameBranchTo(Fileset, String),
    DeleteBranch(Fileset),
    CreateTag(Fileset),
    CreateTagAt(Fileset, String),
    RetrieveTag(Fileset),
    Ignore(Fileset),
    RenameFile(Fileset),
    StashMessage(Fileset),
    StashPick(Fileset, StashOp),
    /// C-x v ! (or C-u on push and pull): the command line.
    EditCommand(Fileset, String),
    CreateRepoBackend(PathBuf),
    GrepRegexp(Fileset),
    GrepFiles(Fileset, String),
    MergebaseDiff(Fileset),
    MergebaseLog(Fileset),
    PushTarget(Fileset),
    /// log-edit M-r / M-s: the regexp.
    LogEditSearch(BufferId, bool),
    /// vc-log-search: the regexp to look for in commit messages.
    LogSearch(Fileset),
    /// vc-rollback: type yes to undo the last commit.
    Rollback(Fileset),
    /// vc-clone: the repository, then the directory.
    CloneUrl,
    CloneDir(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StashOp {
    Apply,
    Pop,
    Drop,
    Show,
}

/// What a VC yes/no question is about.
#[derive(Debug, Clone)]
pub enum Confirm {
    /// Discard the changes in these files.
    Revert(Fileset),
    /// Save this buffer before committing.
    SaveBeforeCommit(Fileset, bool),
    /// Mark a conflicted file resolved.
    MarkResolved(Fileset),
    /// Pull before going on (a file needs an update).
    Pull(Fileset),
    /// Delete a file (and unregister it).
    DeleteFile(Fileset),
    /// Delete these unregistered files from disk (vc-dir `d`).
    CleanFiles(Fileset),
    /// Force-push.
    ForcePush(Fileset, String),
    /// Switch branches with uncommitted changes.
    SwitchDirty(Fileset, String),
    /// Delete a branch that isn't merged.
    ForceDeleteBranch(Fileset, String),
    /// Undo an applied hunk (diff-mode).
    UndoHunk { reverse: bool },
    /// Commit with a message that breaks the conventions.
    CommitAnyway(BufferId),
    /// Drop a stash.
    DropStash(Fileset, String),
}

/// Ask `prompt`, completing from `candidates`, with `initial` typed.
pub fn ask(app: &mut App, prompt: &str, what: Ask, candidates: Vec<String>, initial: &str) {
    app.prompt(prompt, Purpose::VcAsk { what, candidates });
    if let Some(mb) = app.minibuffer.as_mut() {
        mb.input = initial.to_string();
    }
}

fn confirm(app: &mut App, prompt: String, what: Confirm) {
    app.prompt(prompt, Purpose::Confirm(ConfirmAction::Vc(what)));
}

/// RET at a VC prompt.
pub fn answered(app: &mut App, what: Ask, input: String) {
    let input_t = input.trim().to_string();
    match what {
        Ask::DiffRev1(fs) => {
            let backend = fs.backend();
            let default = working_label(&fs);
            let rev1 = if input_t.is_empty() { default } else { input_t };
            let cands = backend.revision_completions(&fs.root, single(&fs).as_deref());
            ask(
                app,
                "Newer revision (default: the working tree): ",
                Ask::DiffRev2(fs, rev1),
                cands,
                "",
            );
        }
        Ask::DiffRev2(fs, rev1) => {
            let rev2 = (!input_t.is_empty()).then_some(input_t);
            show_diff(app, &fs, Some(rev1), rev2, false);
        }
        Ask::LogLimit(fs, whole) => {
            let limit = input_t.parse().unwrap_or(config().log_limit);
            ask(
                app,
                "Show log starting from revision (default: the working revision): ",
                Ask::LogStart(fs.clone(), whole, limit),
                fs.backend().revision_completions(&fs.root, None),
                "",
            );
        }
        Ask::LogStart(fs, whole, limit) => {
            let start = (!input_t.is_empty()).then_some(input_t);
            log::print_log(app, &fs, whole, limit, start, log::Source::Log);
        }
        Ask::BranchLog(fs) => {
            if input_t.is_empty() {
                return;
            }
            let fs = Fileset {
                files: Vec::new(),
                ..fs
            };
            log::print_log(
                app,
                &fs,
                true,
                config().log_limit,
                Some(input_t),
                log::Source::Log,
            );
        }
        Ask::AnnotateRev(fs) => {
            let rev = (!input_t.is_empty()).then_some(input_t);
            if let Some(f) = single(&fs) {
                annotate::annotate(app, fs.kind, &fs.root, &f, rev, None);
            }
        }
        Ask::RevisionOtherWindow(fs) => {
            let rev = if input_t.is_empty() {
                working_label(&fs)
            } else {
                input_t
            };
            if let Some(f) = single(&fs) {
                show_revision(app, fs.kind, &fs.root, &f, &rev, true, None);
            }
        }
        Ask::CheckoutRev(fs) => {
            if input_t.is_empty() {
                return;
            }
            let files = fs.names();
            let cmd = match fs.kind {
                Kind::Git => Cmd::new(
                    &fs.backend().program(),
                    &fs.root,
                    &["checkout", &input_t, "--"],
                )
                .args(files),
                Kind::Hg => Cmd::new(
                    &fs.backend().program(),
                    &fs.root,
                    &["revert", "--no-backup", "-r", &input_t, "--"],
                )
                .args(files),
            };
            run_then_refresh(
                app,
                cmd,
                "Checking out",
                &fs.root,
                format!("Checked out {} at {input_t}", fs.describe()),
            );
        }
        Ask::RegisterBackend(fs) => {
            let Some(kind) = Kind::from_name(&input_t) else {
                return app.error(format!("Unknown backend {input_t}"));
            };
            let fs = Fileset { kind, ..fs };
            register(app, fs);
        }
        Ask::Dir => {
            let dir = if input_t.is_empty() {
                crate::compile::default_directory(app)
            } else {
                expand_home(&input_t)
            };
            dir::open_dir(app, &dir);
        }
        Ask::Merge(fs) => {
            if input_t.is_empty() {
                return;
            }
            merge(app, &fs, &input_t);
        }
        Ask::CreateBranch(fs) => {
            if input_t.is_empty() {
                return;
            }
            match fs.backend().create_branch(&fs.root, &input_t, None) {
                Ok(cmd) => run_then_refresh(
                    app,
                    cmd,
                    "Branching",
                    &fs.root,
                    format!("Switched to a new branch {input_t}"),
                ),
                Err(e) => app.error(e),
            }
        }
        Ask::SwitchBranch(fs) => {
            if input_t.is_empty() {
                return;
            }
            switch_branch(app, &fs, &input_t, false);
        }
        Ask::RenameBranch(fs) => {
            if input_t.is_empty() {
                return;
            }
            ask(
                app,
                &format!("Rename branch {input_t} to: "),
                Ask::RenameBranchTo(fs, input_t),
                Vec::new(),
                "",
            );
        }
        Ask::RenameBranchTo(fs, from) => {
            if input_t.is_empty() {
                return;
            }
            match fs.backend().rename_branch(&fs.root, &from, &input_t) {
                Ok(cmd) => run_then_refresh(
                    app,
                    cmd,
                    "Renaming",
                    &fs.root,
                    format!("Renamed branch {from} to {input_t}"),
                ),
                Err(e) => app.error(e),
            }
        }
        Ask::DeleteBranch(fs) => {
            if input_t.is_empty() {
                return;
            }
            delete_branch(app, &fs, &input_t, false);
        }
        Ask::CreateTag(fs) => {
            if input_t.is_empty() {
                return;
            }
            let cands = fs.backend().revision_completions(&fs.root, None);
            ask(
                app,
                &format!("Tag {input_t} at revision (default: the working revision): "),
                Ask::CreateTagAt(fs, input_t),
                cands,
                "",
            );
        }
        Ask::CreateTagAt(fs, name) => {
            let rev = (!input_t.is_empty()).then_some(input_t);
            match fs.backend().create_tag(&fs.root, &name, rev.as_deref()) {
                Ok(cmd) => {
                    run_then_refresh(app, cmd, "Tagging", &fs.root, format!("Made tag {name}"))
                }
                Err(e) => app.error(e),
            }
        }
        Ask::RetrieveTag(fs) => {
            if input_t.is_empty() {
                return;
            }
            match fs.backend().retrieve_tag(&fs.root, &input_t) {
                Ok(cmd) => run_then_refresh(
                    app,
                    cmd,
                    "Retrieving",
                    &fs.root,
                    format!("Checked out {input_t}"),
                ),
                Err(e) => app.error(e),
            }
        }
        Ask::Ignore(fs) => {
            if input_t.is_empty() {
                return;
            }
            match fs.backend().ignore(&fs.root, &input_t) {
                Ok(file) => {
                    app.msg(format!(
                        "Ignoring {input_t} (in {})",
                        crate::files::abbreviate(&file)
                    ));
                    let root = fs.root.clone();
                    refresh_root(app, &root);
                }
                Err(e) => app.error(e),
            }
        }
        Ask::RenameFile(fs) => {
            let Some(from) = single(&fs) else { return };
            if input_t.is_empty() {
                return;
            }
            rename_file(app, &fs, &from, &input_t);
        }
        Ask::StashMessage(fs) => match fs.backend().stash(&fs.root, &input_t) {
            Ok(cmd) => {
                run_then_refresh(app, cmd, "Stashing", &fs.root, "Stashed the changes".into())
            }
            Err(e) => app.error(e),
        },
        Ask::StashPick(fs, op) => {
            let name = input_t.split(':').next().unwrap_or("").trim().to_string();
            if name.is_empty() {
                return;
            }
            stash_op(app, &fs, &name, op);
        }
        Ask::EditCommand(fs, what) => {
            let words = split_command_line(&input_t);
            let Some((prog, args)) = words.split_first() else {
                return;
            };
            let cmd = Cmd {
                program: prog.clone(),
                args: args.to_vec(),
                dir: fs.root.clone(),
                stdin: None,
            };
            remote_command(app, &fs, cmd, &what);
        }
        Ask::CreateRepoBackend(dir) => {
            let kind = Kind::from_name(if input_t.is_empty() { "Git" } else { &input_t });
            let Some(kind) = kind else {
                return app.error(format!("Unknown backend {input_t}"));
            };
            let program = kind.backend().program();
            let cmd = Cmd::new(&program, &dir, &["init", "-q"]);
            let d = dir.clone();
            runner::run(
                app,
                cmd.clone(),
                "Creating repository",
                None,
                move |app, out| {
                    if !out.success() {
                        return runner::report_failure(app, &cmd.line(), &out, true);
                    }
                    app.msg(format!(
                        "Created a {} repository in {}",
                        kind.name(),
                        crate::files::abbreviate(&d)
                    ));
                    app.vc.checked.clear();
                    refresh_root(app, &d);
                },
            );
        }
        Ask::GrepRegexp(fs) => {
            if input_t.is_empty() {
                return;
            }
            let word = input.trim().to_string();
            ask(
                app,
                "Search in files (default: all tracked): ",
                Ask::GrepFiles(fs, word),
                Vec::new(),
                "",
            );
        }
        Ask::GrepFiles(fs, regexp) => {
            let Some(line) = fs.backend().grep_command(&fs.root, &regexp, &input_t) else {
                return app.error(format!("{} has no grep", fs.kind.name()));
            };
            crate::compile::grep_in(app, &line, &fs.root);
        }
        Ask::MergebaseDiff(fs) => {
            if input_t.is_empty() {
                return;
            }
            match fs.backend().mergebase(&fs.root, &input_t, "HEAD") {
                Some(base) => {
                    let fs2 = Fileset {
                        files: Vec::new(),
                        ..fs
                    };
                    show_diff(app, &fs2, Some(base), Some(input_t), false);
                }
                None => app.error(format!("No merge base with {input_t}")),
            }
        }
        Ask::MergebaseLog(fs) => {
            if input_t.is_empty() {
                return;
            }
            match fs.backend().mergebase(&fs.root, &input_t, "HEAD") {
                Some(base) => {
                    let fs2 = Fileset {
                        files: Vec::new(),
                        ..fs
                    };
                    log::print_log(
                        app,
                        &fs2,
                        true,
                        config().log_limit,
                        Some(format!("{base}..{input_t}")),
                        log::Source::Log,
                    );
                }
                None => app.error(format!("No merge base with {input_t}")),
            }
        }
        Ask::LogSearch(fs) => {
            if input_t.is_empty() {
                return;
            }
            let cmd = match fs.kind {
                Kind::Git => Cmd::new(
                    "git",
                    &fs.root,
                    &[
                        "log",
                        "--no-color",
                        "--decorate=short",
                        "--date=format:%Y-%m-%d %H:%M",
                        "--format=%h %ad %an  %s%d",
                        "-i",
                        "-E",
                        "--grep",
                        &input_t,
                    ],
                ),
                Kind::Hg => Cmd::new("hg", &fs.root, &["log", "--keyword", &input_t]),
            };
            let line = cmd.line();
            let what = input_t.clone();
            runner::run(app, cmd, "Searching the log", None, move |app, out| {
                if !out.success() {
                    return runner::report_failure(app, &line, &out, true);
                }
                let text = if out.stdout.trim().is_empty() {
                    format!("No commits match \"{what}\"\n")
                } else {
                    format!("Commits matching \"{what}\":\n\n{}", out.stdout)
                };
                app.show_special_buffer("*vc-search-log*", "special-mode", &text);
            });
        }
        Ask::Rollback(fs) => {
            if input_t != "yes" {
                return app.msg("Rollback cancelled");
            }
            let cmd = match fs.kind {
                Kind::Git => Cmd::new("git", &fs.root, &["reset", "--soft", "HEAD~1"]),
                Kind::Hg => Cmd::new("hg", &fs.root, &["rollback"]),
            };
            run_then_refresh(
                app,
                cmd,
                "Rolling back",
                &fs.root,
                "Undid the last commit; its changes are staged again".into(),
            );
        }
        Ask::CloneUrl => {
            if input_t.is_empty() {
                return;
            }
            let name = input_t
                .trim_end_matches('/')
                .rsplit(['/', ':'])
                .next()
                .unwrap_or("repo")
                .trim_end_matches(".git")
                .to_string();
            let cwd = crate::compile::default_directory(app);
            let def = cwd.join(name).display().to_string();
            ask(
                app,
                "Clone into directory: ",
                Ask::CloneDir(input_t),
                Vec::new(),
                &def,
            );
        }
        Ask::CloneDir(url) => {
            if input_t.is_empty() {
                return;
            }
            let dest = expand_home(&input_t);
            let cwd = crate::compile::default_directory(app);
            let cwd = if cwd.is_dir() {
                cwd
            } else {
                PathBuf::from("/")
            };
            let hg = url.starts_with("hg") || url.contains("hg.");
            let cmd = if hg {
                Cmd::new("hg", &cwd, &["clone", &url, &dest.display().to_string()])
            } else {
                Cmd::new("git", &cwd, &["clone", &url, &dest.display().to_string()])
            };
            let line = cmd.line();
            app.msg(format!("Cloning {url}..."));
            runner::run(app, cmd, "Cloning", None, move |app, out| {
                if !out.success() {
                    return runner::report_failure(app, &line, &out, true);
                }
                app.msg(format!("Cloned into {}", dest.display()));
                crate::dired::open(app, &dest);
            });
        }
        Ask::LogEditSearch(id, backward) => {
            if !input_t.is_empty() {
                log_edit::search(app, id, backward, &input);
            }
        }
        Ask::PushTarget(fs) => {
            let words = split_command_line(&input_t);
            let Some((prog, args)) = words.split_first() else {
                return;
            };
            let cmd = Cmd {
                program: prog.clone(),
                args: args.to_vec(),
                dir: fs.root.clone(),
                stdin: None,
            };
            remote_command(app, &fs, cmd, "Pushing");
        }
    }
}

/// y to a VC question.
pub fn confirmed(app: &mut App, what: Confirm) {
    match what {
        Confirm::Revert(fs) => revert_now(app, fs),
        Confirm::SaveBeforeCommit(fs, amend) => {
            save_fileset_buffers(app, &fs);
            log_edit::start(app, fs, amend);
        }
        Confirm::MarkResolved(fs) => {
            let cmd = fs.backend().mark_resolved(&fs.root, &fs.names());
            run_then_refresh(
                app,
                cmd,
                "Resolving",
                &fs.root,
                format!("Marked {} resolved", fs.describe()),
            );
        }
        Confirm::Pull(fs) => pull(app, fs),
        Confirm::DeleteFile(fs) => {
            let Some(f) = single(&fs) else { return };
            // Gone from its buffer too.
            let path = fs.root.join(&f);
            if let Some(id) = app.editor.find_buffer_by_path(&path) {
                app.kill_buffer(id);
            }
            let cmd = fs.backend().delete_file(&fs.root, &f);
            let root = fs.root.clone();
            run_then_refresh(app, cmd, "Deleting", &root, format!("Deleted {f}"));
        }
        Confirm::CleanFiles(fs) => {
            let mut n = 0;
            for (f, _) in &fs.files {
                let p = fs.root.join(f);
                let ok = if p.is_dir() {
                    std::fs::remove_dir_all(&p).is_ok()
                } else {
                    std::fs::remove_file(&p).is_ok()
                };
                if ok {
                    n += 1;
                }
            }
            app.msg(format!("Deleted {n} file{}", if n == 1 { "" } else { "s" }));
            let root = fs.root.clone();
            refresh_root(app, &root);
        }
        Confirm::ForcePush(fs, line) => {
            let words = split_command_line(&line);
            if let Some((prog, args)) = words.split_first() {
                let cmd = Cmd {
                    program: prog.clone(),
                    args: args.to_vec(),
                    dir: fs.root.clone(),
                    stdin: None,
                };
                run_remote(app, &fs, cmd, "Pushing");
            }
        }
        Confirm::SwitchDirty(fs, name) => switch_branch(app, &fs, &name, true),
        Confirm::ForceDeleteBranch(fs, name) => delete_branch(app, &fs, &name, true),
        Confirm::UndoHunk { reverse } => diff::apply_hunk(app, reverse),
        Confirm::CommitAnyway(id) => log_edit::finish(app, id, true),
        Confirm::DropStash(fs, name) => stash_op(app, &fs, &name, StashOp::Drop),
    }
}

/// n to a VC question.
pub fn declined(app: &mut App, what: Confirm) {
    match what {
        Confirm::SaveBeforeCommit(..) => app.error("Aborted: save the buffer first (or answer y)"),
        Confirm::Revert(_) => app.msg("Revert canceled"),
        _ => {}
    }
}

/// "HEAD" for Git, "." for Mercurial.
fn working_label(fs: &Fileset) -> String {
    match fs.kind {
        Kind::Git => "HEAD".into(),
        Kind::Hg => ".".into(),
    }
}

/// The one file of a fileset (commands that take a single file).
fn single(fs: &Fileset) -> Option<String> {
    match fs.files.as_slice() {
        [(f, _)] => Some(f.clone()),
        _ => None,
    }
}

/// Split a command line the way a shell would, for the edited commands
/// (quotes group words; no expansion).
pub fn split_command_line(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut have = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                have = true;
            }
            (None, '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                    have = true;
                }
            }
            (None, c) if c.is_whitespace() => {
                if have || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    have = false;
                }
            }
            (None, c) => {
                cur.push(c);
                have = true;
            }
        }
    }
    if have || !cur.is_empty() {
        out.push(cur);
    }
    out
}

// ------------------------------------------------------------ running

/// Run `cmd`; when it succeeds say `done` and refresh `root`; when it
/// fails, show why.
pub fn run_then_refresh(app: &mut App, cmd: Cmd, title: &str, root: &Path, done: String) {
    let line = cmd.line();
    let root = root.to_path_buf();
    runner::run(app, cmd, title, None, move |app, out| {
        refresh_root(app, &root);
        if out.success() {
            app.msg(done);
        } else {
            runner::report_failure(app, &line, &out, true);
        }
    });
}

/// Run several commands one after another (stopping at the first that
/// fails), then `finally`.
pub fn run_sequence(
    app: &mut App,
    mut cmds: Vec<Cmd>,
    title: &str,
    finally: impl FnOnce(&mut App, bool) + 'static,
) {
    if cmds.is_empty() {
        return finally(app, true);
    }
    let first = cmds.remove(0);
    let line = first.line();
    let title2 = title.to_string();
    runner::run(app, first, title, None, move |app, out| {
        if !out.success() {
            runner::report_failure(app, &line, &out, true);
            return finally(app, false);
        }
        run_sequence(app, cmds, &title2, finally);
    });
}

// ------------------------------------------------------------ commands

/// Show a file as of a revision, read-only, in `name.~rev~`, optionally at a line.
pub fn show_revision(
    app: &mut App,
    kind: Kind,
    root: &Path,
    file: &str,
    rev: &str,
    other_window: bool,
    line: Option<usize>,
) {
    let short: String = if rev.len() == 40 && rev.bytes().all(|b| b.is_ascii_hexdigit()) {
        rev[..10].to_string()
    } else {
        rev.replace('/', "!")
    };
    let base = Path::new(file)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.to_string());
    let name = format!("{base}.~{short}~");
    let text = match kind.backend().find_revision(root, file, rev) {
        Ok(t) => t,
        Err(e) => return app.error(format!("Can't get {file} at {rev}: {e}")),
    };
    let id = match app.editor.find_buffer(&name) {
        Some(id) => id,
        None => app.editor.add_buffer(Buffer::new(&name)),
    };
    // Its major mode is the file's.
    let mode = app.editor.modes.major_for_file(&base).map(str::to_string);
    {
        let b = app.editor.state.get_buffer_mut(id).expect("made");
        b.set_locked(false);
        b.set_contents(&text);
        if let Some(m) = mode {
            b.set_major_mode(m);
        }
        b.set_locked(true);
    }
    app.editor.sync_language(id);
    app.vc.revisions.insert(
        id,
        (kind, root.to_path_buf(), file.to_string(), rev.to_string()),
    );
    if other_window {
        crate::windows::pop_to_other_window(app);
    }
    app.switch_to(id);
    if let Some(l) = line {
        let b = app.buf();
        let l = l.saturating_sub(1).min(b.line_count().saturating_sub(1));
        b.set_point(Pos::new(l, 0));
    }
}

/// vc-diff / vc-root-diff: run the diff and show it in *vc-diff*.
pub fn show_diff(
    app: &mut App,
    fs: &Fileset,
    rev1: Option<String>,
    rev2: Option<String>,
    quiet_if_none: bool,
) {
    let names = fs.names();
    let cmd = fs
        .backend()
        .diff(&fs.root, &names, rev1.as_deref(), rev2.as_deref(), false);
    let view = diff::DiffView {
        root: fs.root.clone(),
        kind: Some(fs.kind),
        files: names,
        rev1: rev1.clone(),
        rev2: rev2.clone(),
        ignore_ws: false,
        regenerable: true,
    };
    let line = cmd.line();
    let what = fs.describe();
    runner::run(app, cmd, "Diffing", None, move |app, out| {
        if !out.success() && out.stdout.is_empty() {
            return runner::report_failure(app, &line, &out, true);
        }
        if out.stdout.trim().is_empty() {
            if !quiet_if_none {
                app.msg(match (&rev1, &rev2) {
                    (None, None) => format!(
                        "No changes between the working revision and the working tree of {what}"
                    ),
                    (Some(a), None) => {
                        format!("No changes between {a} and the working tree of {what}")
                    }
                    (a, Some(b)) => format!(
                        "No changes between {} and {b}",
                        a.clone().unwrap_or_else(|| "the working revision".into())
                    ),
                });
            }
            return;
        }
        let id = diff::show(app, diff::VC_DIFF_BUFFER, &out.stdout, Some(view));
        if app.active_id() != id {
            crate::compile::display_buffer(app, id);
        }
    });
}

/// C-x v v.
fn next_action(app: &mut App) {
    let Some(fs) = fileset_or_error(app, false) else {
        return;
    };
    if fs.files.is_empty() {
        return app.error("No files to act on (move to a file, or mark some)");
    }
    let first = fs.files[0].1.clone();
    // Forge needs every file in the same state; edited, added and removed
    // all commit together.
    let commit_like = |s: &FileState| s.committable();
    let same = fs
        .files
        .iter()
        .all(|(_, s)| *s == first || (commit_like(s) && commit_like(&first)));
    if !same {
        return app.error(
            "The files are in different states: act on them separately, or mark ones alike",
        );
    }
    let arg = app.raw_arg() || app.has_arg();
    match first {
        FileState::Unregistered | FileState::Ignored => {
            if arg {
                let names: Vec<String> = config()
                    .handled
                    .iter()
                    .map(|k| k.name().to_string())
                    .collect();
                return ask(
                    app,
                    "Register with backend: ",
                    Ask::RegisterBackend(fs),
                    names,
                    "",
                );
            }
            register(app, fs);
        }
        FileState::Conflict => {
            let has_markers = fs
                .files
                .iter()
                .any(|(f, _)| smerge::file_has_markers(&fs.root.join(f)));
            if has_markers {
                // Visit it in smerge-mode to resolve.
                let (f, _) = &fs.files[0];
                let path = fs.root.join(f);
                if let Ok((id, _)) = app.editor.open_file(&path) {
                    app.switch_to(id);
                    smerge::maybe_enable(app, id, true);
                    let _ = smerge::next_conflict(app, true, true);
                }
                app.msg("Resolve the conflicts (C-c ^ u/l/b/a keep upper/lower/base/all, C-c ^ n next), save, then C-x v v");
            } else {
                let p = format!("Mark {} as resolved? (y or n) ", fs.describe());
                confirm(app, p, Confirm::MarkResolved(fs));
            }
        }
        FileState::NeedsUpdate | FileState::NeedsMerge => {
            let p = format!(
                "{} has newer revisions in the repository; pull them? (y or n) ",
                fs.describe()
            );
            confirm(app, p, Confirm::Pull(fs));
        }
        FileState::UpToDate => {
            if arg {
                let cands = fs
                    .backend()
                    .revision_completions(&fs.root, single(&fs).as_deref());
                return ask(
                    app,
                    &format!("Check out {} at revision: ", fs.describe()),
                    Ask::CheckoutRev(fs),
                    cands,
                    "",
                );
            }
            if fs.backend().caps().lock_based {
                return app.msg(format!("{} checked out for editing", fs.describe()));
            }
            app.msg(format!("Fileset is up-to-date ({})", fs.describe()));
        }
        FileState::LockedBy(u) => app.error(format!("{} is locked by {u}", fs.describe())),
        _ => checkin(app, fs, false),
    }
}

/// Commit: save the fileset's buffers first (asking), then the log buffer.
pub fn checkin(app: &mut App, fs: Fileset, amend: bool) {
    let unsaved: Vec<String> = fs
        .files
        .iter()
        .filter_map(|(f, _)| {
            let id = app.editor.find_buffer_by_path(&fs.root.join(f))?;
            let b = app.editor.state.get_buffer(id)?;
            b.is_modified().then(|| b.name().to_string())
        })
        .collect();
    match unsaved.as_slice() {
        [] => log_edit::start(app, fs, amend),
        [one] => {
            let p = format!("Buffer {one} modified; save it? (y or n) ");
            confirm(app, p, Confirm::SaveBeforeCommit(fs, amend));
        }
        many => {
            let p = format!("{} buffers modified; save them? (y or n) ", many.len());
            confirm(app, p, Confirm::SaveBeforeCommit(fs, amend));
        }
    }
}

fn save_fileset_buffers(app: &mut App, fs: &Fileset) {
    let active = app.active_id();
    for (f, _) in &fs.files {
        if let Some(id) = app.editor.find_buffer_by_path(&fs.root.join(f))
            && let Some(b) = app.editor.state.get_buffer_mut(id)
            && b.is_modified()
        {
            if let Err(e) = b.save() {
                let name = b.name().to_string();
                app.error(format!("Can't save {name}: {e}"));
            }
            if id == active {
                crate::files::after_save(app);
            } else {
                refresh_buffer(app, id);
            }
        }
    }
}

/// C-x v i.
fn register(app: &mut App, fs: Fileset) {
    let files: Vec<String> = fs
        .files
        .iter()
        .filter(|(_, s)| !s.registered())
        .map(|(f, _)| f.clone())
        .collect();
    if files.is_empty() {
        return app.msg(format!("{} is already registered", fs.describe()));
    }
    let cmd = fs.backend().register(&fs.root, &files);
    let msg = format!("Registered {} with {}", fs.describe(), fs.kind.name());
    run_then_refresh(app, cmd, "Registering", &fs.root.clone(), msg);
}

/// C-x v u: show what would be lost, then ask.
fn revert(app: &mut App) {
    let Some(fs) = fileset_or_error(app, false) else {
        return;
    };
    if fs.files.is_empty() {
        return app.error("No files to revert");
    }
    if fs.files.iter().all(|(_, s)| {
        matches!(
            s,
            FileState::UpToDate | FileState::Unregistered | FileState::Ignored
        )
    }) {
        return app.msg(format!("{} has no changes to revert", fs.describe()));
    }
    // vc-revert-show-diff: the changes first.
    show_diff(app, &fs, None, None, true);
    let p = format!("Discard changes in {}? (y or n) ", fs.describe());
    confirm(app, p, Confirm::Revert(fs));
}

fn revert_now(app: &mut App, fs: Fileset) {
    let files: Vec<(String, FileState)> = fs
        .files
        .iter()
        .filter(|(_, s)| s.registered() && *s != FileState::UpToDate)
        .cloned()
        .collect();
    let cmds = fs.backend().revert(&fs.root, &files);
    let root = fs.root.clone();
    let what = fs.describe();
    // Buffers visiting them follow the files, modified or not.
    let ids: Vec<BufferId> = files
        .iter()
        .filter_map(|(f, _)| app.editor.find_buffer_by_path(&root.join(f)))
        .collect();
    run_sequence(app, cmds, "Reverting", move |app, ok| {
        for id in ids {
            if let Some(b) = app.editor.state.get_buffer_mut(id)
                && b.path().is_some_and(Path::exists)
            {
                let _ = b.revert();
            }
        }
        refresh_root(app, &root);
        if ok {
            app.msg(format!("Reverted {what}"));
        }
        if let Some(id) = app.editor.find_buffer(diff::VC_DIFF_BUFFER) {
            for w in app
                .editor
                .windows
                .iter()
                .filter(|w| w.buffer == id)
                .map(|w| w.id)
                .collect::<Vec<_>>()
            {
                if app.editor.windows.count() > 1 {
                    let _ = app.editor.delete_window(w);
                }
            }
        }
    });
}

fn rename_file(app: &mut App, fs: &Fileset, from: &str, to: &str) {
    let to_rel = {
        let p = Path::new(to);
        if p.is_absolute() {
            match backend::relative(&fs.root, p) {
                Some(r) => r,
                None => return app.error("The new name must be in the same repository"),
            }
        } else {
            // Relative to the old file's directory.
            let dir = Path::new(from)
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            let joined = dir.join(to);
            joined.to_string_lossy().replace('\\', "/")
        }
    };
    let old_path = fs.root.join(from);
    if let Some(id) = app.editor.find_buffer_by_path(&old_path)
        && app
            .editor
            .state
            .get_buffer(id)
            .is_some_and(Buffer::is_modified)
    {
        return app.error("Save the buffer before renaming its file");
    }
    let cmd = fs.backend().rename_file(&fs.root, from, &to_rel);
    let root = fs.root.clone();
    let line = cmd.line();
    let new_path = root.join(&to_rel);
    runner::run(app, cmd, "Renaming", None, move |app, out| {
        if !out.success() {
            return runner::report_failure(app, &line, &out, true);
        }
        if let Some(id) = app.editor.find_buffer_by_path(&old_path) {
            app.kill_buffer(id);
            if let Ok((nid, _)) = app.editor.open_file(&new_path) {
                app.switch_to(nid);
            }
        }
        refresh_root(app, &root);
        app.msg(format!(
            "Renamed to {}",
            crate::files::abbreviate(&new_path)
        ));
    });
}

// ------------------------------------------------- remote and branches

/// C-x v +.
fn pull(app: &mut App, fs: Fileset) {
    match fs.backend().pull(&fs.root) {
        Ok(cmd) => remote_command(app, &fs, cmd, "Pulling"),
        Err(e) => app.error(e),
    }
}

/// Run a push/pull/fetch-like command, letting the user edit it first
/// after C-x v ! or with C-u.
fn remote_command(app: &mut App, fs: &Fileset, cmd: Cmd, title: &str) {
    if std::mem::take(&mut app.vc.edit_next) {
        let line = cmd.line();
        return ask(
            app,
            &format!("{title} with command: "),
            Ask::EditCommand(fs.clone(), title.to_string()),
            Vec::new(),
            &line,
        );
    }
    // Never force-push without saying so.
    if title == "Pushing"
        && cmd.args.iter().any(|a| {
            a == "--force"
                || a == "-f"
                || a.starts_with("--force-with-lease")
                || (a.starts_with('+') && a.len() > 1)
        })
    {
        let line = cmd.line();
        return confirm(
            app,
            format!("Really force-push ({line})? (y or n) "),
            Confirm::ForcePush(fs.clone(), line),
        );
    }
    run_remote(app, fs, cmd, title);
}

/// Run a long remote command, its output streaming into `*vc*`.
fn run_remote(app: &mut App, fs: &Fileset, cmd: Cmd, title: &str) {
    let out_buf = runner::output_buffer(app);
    let line = cmd.line();
    let root = fs.root.clone();
    let title_s = title.to_string();
    app.msg(format!("{title}..."));
    runner::run(app, cmd, title, Some(out_buf), move |app, out| {
        refresh_root(app, &root);
        if out.success() {
            app.msg(format!("{title_s}...done"));
            return;
        }
        // A pull or merge that stopped on conflicts: show them.
        let text = format!("{}\n{}", out.stdout, out.stderr);
        if text.contains("CONFLICT") || text.contains("conflicts") || text.contains("unresolved") {
            app.error(format!(
                "{title_s} left conflicts: resolve them (C-x v d lists them)"
            ));
            if let Some((kind, root2)) = responsible(&root) {
                let _ = kind;
                dir::open_dir(app, &root2);
            }
        } else if text.contains("non-fast-forward")
            || text.contains("fetch first")
            || text.contains("rejected")
        {
            app.error(format!(
                "{title_s} was rejected: the remote has changes you don't (pull first; see *vc*)"
            ));
            let id = runner::output_buffer(app);
            crate::compile::display_buffer(app, id);
        } else {
            runner::report_failure(app, &line, &out, true);
        }
    });
}

/// C-x v P: push; with no upstream, ask where (as `git push -u`).
fn push(app: &mut App, fs: Fileset) {
    let b = fs.backend();
    if !b.caps().push {
        return app.error(format!("{} can't push", fs.kind.name()));
    }
    if fs.kind == Kind::Git && !b.has_upstream(&fs.root) && !app.vc.edit_next {
        let branch = b.current_branch(&fs.root).unwrap_or_else(|| "HEAD".into());
        let remotes = b.remotes(&fs.root);
        let Some(remote) = remotes.first().cloned() else {
            return app.error("This repository has no remote to push to (git remote add ...)");
        };
        let line = format!("{} push --set-upstream {remote} {branch}", b.program());
        return ask(
            app,
            "No upstream branch; push with: ",
            Ask::PushTarget(fs),
            Vec::new(),
            &line,
        );
    }
    match b.push(&fs.root) {
        Ok(cmd) => remote_command(app, &fs, cmd, "Pushing"),
        Err(e) => app.error(e),
    }
}

fn merge(app: &mut App, fs: &Fileset, rev: &str) {
    match fs.backend().merge(&fs.root, rev) {
        Ok(cmd) => remote_command(app, fs, cmd, "Merging"),
        Err(e) => app.error(e),
    }
}

fn switch_branch(app: &mut App, fs: &Fileset, name: &str, force: bool) {
    let b = fs.backend();
    if !force {
        let dirty = b
            .dir_status(&fs.root, "", false)
            .map(|(e, _)| {
                e.iter()
                    .any(|e| e.state.committable() || e.state == FileState::Conflict)
            })
            .unwrap_or(false);
        let modified_buffers = app.editor.buffer_ids().into_iter().any(|id| {
            app.editor.state.get_buffer(id).is_some_and(|b| {
                b.is_modified() && b.path().is_some_and(|p| p.starts_with(&fs.root))
            })
        });
        if modified_buffers {
            return app.error("Save your buffers in this repository before switching branches");
        }
        if dirty {
            let p = format!(
                "The working tree has uncommitted changes; switch to {name} anyway? (y or n) "
            );
            return confirm(app, p, Confirm::SwitchDirty(fs.clone(), name.to_string()));
        }
    }
    match b.switch_branch(&fs.root, name) {
        Ok(cmd) => run_then_refresh(
            app,
            cmd,
            "Switching",
            &fs.root,
            format!("Switched to {name}"),
        ),
        Err(e) => app.error(e),
    }
}

fn delete_branch(app: &mut App, fs: &Fileset, name: &str, force: bool) {
    let Ok(cmd) = fs.backend().delete_branch(&fs.root, name, force) else {
        return app.error(format!("{} can't delete branches", fs.kind.name()));
    };
    let fs2 = fs.clone();
    let name = name.to_string();
    let line = cmd.line();
    let root = fs.root.clone();
    runner::run(app, cmd, "Deleting branch", None, move |app, out| {
        if out.success() {
            app.msg(format!("Deleted branch {name}"));
            branches::refresh_lists(app, &root);
        } else if !force
            && (out.stderr.contains("not fully merged") || out.stderr.contains("is not merged"))
        {
            let p = format!("Branch {name} is not fully merged; delete it anyway? (y or n) ");
            confirm(app, p, Confirm::ForceDeleteBranch(fs2, name));
        } else {
            runner::report_failure(app, &line, &out, true);
        }
    });
}

pub fn stash_op(app: &mut App, fs: &Fileset, name: &str, op: StashOp) {
    let b = fs.backend();
    let r = match op {
        StashOp::Apply => b.stash_apply(&fs.root, name, false),
        StashOp::Pop => b.stash_apply(&fs.root, name, true),
        StashOp::Drop => b.stash_drop(&fs.root, name),
        StashOp::Show => b.stash_show(&fs.root, name),
    };
    let cmd = match r {
        Ok(c) => c,
        Err(e) => return app.error(e),
    };
    if op == StashOp::Show {
        let line = cmd.line();
        let view = diff::DiffView {
            root: fs.root.clone(),
            kind: Some(fs.kind),
            ..Default::default()
        };
        return runner::run(app, cmd, "Showing stash", None, move |app, out| {
            if !out.success() {
                return runner::report_failure(app, &line, &out, true);
            }
            let id = diff::show(app, "*vc-stash*", &out.stdout, Some(view));
            app.switch_to(id);
        });
    }
    let what = match op {
        StashOp::Apply => "Applied",
        StashOp::Pop => "Popped",
        StashOp::Drop => "Dropped",
        StashOp::Show => unreachable!(),
    };
    let root = fs.root.clone();
    let line = cmd.line();
    let name = name.to_string();
    runner::run(app, cmd, "Stash", None, move |app, out| {
        refresh_root(app, &root);
        branches::refresh_lists(app, &root);
        if out.success() {
            app.msg(format!("{what} {name}"));
        } else {
            runner::report_failure(app, &line, &out, true);
        }
    });
}

fn pick_stash(app: &mut App, op: StashOp) {
    let Some(fs) = fileset_or_error(app, true) else {
        return;
    };
    if let Some(name) = branches::stash_at_point(app) {
        if op == StashOp::Drop {
            return confirm(
                app,
                format!("Drop {name}? (y or n) "),
                Confirm::DropStash(fs, name),
            );
        }
        return stash_op(app, &fs, &name, op);
    }
    let stashes = fs.backend().stashes(&fs.root);
    if stashes.is_empty() {
        return app.error("There are no stashes");
    }
    let cands: Vec<String> = stashes.iter().map(|(n, d)| format!("{n}: {d}")).collect();
    let verb = match op {
        StashOp::Apply => "Apply",
        StashOp::Pop => "Pop",
        StashOp::Drop => "Drop",
        StashOp::Show => "Show",
    };
    let first = stashes[0].0.clone();
    ask(
        app,
        &format!("{verb} stash (default {first}): "),
        Ask::StashPick(fs, op),
        cands,
        &first,
    );
}

// ------------------------------------------------------------- dispatch

/// Every VC command, with its documentation. Sorted by name.
pub const COMMANDS: &[CommandDoc] = &[
    forge_docs::CommandDoc::line(
        "auto-fill-mode",
        "Toggle breaking lines at the fill column as you type (on in commit messages).",
    ),
    forge_docs::CommandDoc::new("diff-apply-hunk", "Apply the hunk at point to its file.")
        .body("With C-u, reverse it. A hunk that is already applied is offered for undoing."),
    forge_docs::CommandDoc::new(
        "diff-apply-hunk-undo",
        "Undo the last hunk applied with diff-apply-hunk (if its buffer hasn't changed",
    )
    .body("since)."),
    forge_docs::CommandDoc::new(
        "diff-backup",
        "Show the differences between the file and its latest backup (file~ or the",
    )
    .body("highest file.~N~)."),
    forge_docs::CommandDoc::line("diff-file-kill", "Remove the file at point from the diff."),
    forge_docs::CommandDoc::line("diff-file-next", "Move to the next file in the diff."),
    forge_docs::CommandDoc::line("diff-file-prev", "Move to the previous file in the diff."),
    forge_docs::CommandDoc::new(
        "diff-goto-source",
        "Visit the source line under point in another window.",
    )
    .body("With C-u, the old version of it."),
    forge_docs::CommandDoc::line("diff-hunk-kill", "Remove the hunk at point from the diff."),
    forge_docs::CommandDoc::line("diff-hunk-next", "Move to the next hunk."),
    forge_docs::CommandDoc::line("diff-hunk-prev", "Move to the previous hunk."),
    forge_docs::CommandDoc::line(
        "diff-refine-hunk",
        "Highlight the words that changed inside the hunk at point.",
    ),
    forge_docs::CommandDoc::line("diff-regenerate", "Run the diff again (g)."),
    forge_docs::CommandDoc::line(
        "diff-reverse-direction",
        "Reverse the diff: removed lines become added ones and back.",
    ),
    forge_docs::CommandDoc::line(
        "diff-side-by-side",
        "Show the diff in two columns, old and new.",
    ),
    forge_docs::CommandDoc::line(
        "diff-split-hunk",
        "Split the hunk at point into two, at point's line.",
    ),
    forge_docs::CommandDoc::line(
        "diff-toggle-fold",
        "Fold or unfold the hunk (on its header) or the file at point.",
    ),
    forge_docs::CommandDoc::line(
        "diff-toggle-fold-all",
        "Fold every file to its first line, or unfold them all.",
    ),
    forge_docs::CommandDoc::line(
        "diff-toggle-whitespace",
        "Regenerate the diff ignoring whitespace changes, or stop ignoring them.",
    ),
    forge_docs::CommandDoc::line(
        "log-edit-comment-search-backward",
        "Replace the message with an earlier one matching a regexp.",
    ),
    forge_docs::CommandDoc::line(
        "log-edit-comment-search-forward",
        "Replace the message with a later one matching a regexp.",
    ),
    forge_docs::CommandDoc::line("log-edit-done", "Commit with this message (C-c C-c)."),
    forge_docs::CommandDoc::line(
        "log-edit-kill",
        "Give up the commit (C-c C-k). The message is kept as a draft for next time.",
    ),
    forge_docs::CommandDoc::line(
        "log-edit-next-comment",
        "Replace the message with the next one in the history (M-n).",
    ),
    forge_docs::CommandDoc::line(
        "log-edit-previous-comment",
        "Replace the message with the previous one in the history (M-p).",
    ),
    forge_docs::CommandDoc::line(
        "log-edit-show-diff",
        "Show the changes being committed (C-c C-d).",
    ),
    forge_docs::CommandDoc::line(
        "log-edit-show-files",
        "List the files being committed (C-c C-f).",
    ),
    forge_docs::CommandDoc::line(
        "log-edit-toggle-amend",
        "Amend the last commit instead of making a new one (C-c C-e): the Amend: header.",
    ),
    forge_docs::CommandDoc::line(
        "log-edit-toggle-no-verify",
        "Skip the pre-commit hooks (C-c C-n): the No-Verify: header.",
    ),
    forge_docs::CommandDoc::line(
        "log-edit-toggle-signoff",
        "Add a Signed-off-by line (C-c C-s): the Sign-Off: header.",
    ),
    forge_docs::CommandDoc::line(
        "log-view-annotate-version",
        "Annotate the file as of the revision at point.",
    ),
    forge_docs::CommandDoc::new(
        "log-view-diff",
        "Diff the revision at point with its parent (or the marked revisions,",
    )
    .body("or the region's)."),
    forge_docs::CommandDoc::line(
        "log-view-diff-changeset",
        "Diff the whole changeset of the revision at point.",
    ),
    forge_docs::CommandDoc::line(
        "log-view-diff-working",
        "Diff the revision at point against the working tree.",
    ),
    forge_docs::CommandDoc::line(
        "log-view-find-revision",
        "Visit the file as of the revision at point.",
    ),
    forge_docs::CommandDoc::line("log-view-next-entry", "Move to the next log entry."),
    forge_docs::CommandDoc::line("log-view-previous-entry", "Move to the previous log entry."),
    forge_docs::CommandDoc::line("log-view-refresh", "Read the log again."),
    forge_docs::CommandDoc::line("log-view-show-more", "Show twice as many entries."),
    forge_docs::CommandDoc::line(
        "log-view-toggle-entry-display",
        "Show or hide the whole message of the entry at point.",
    ),
    forge_docs::CommandDoc::line(
        "log-view-toggle-mark-entry",
        "Mark or unmark the entry at point (d then diffs the two marked).",
    ),
    forge_docs::CommandDoc::line(
        "project-vc-dir",
        "Show the version-control status of the current project.",
    ),
    forge_docs::CommandDoc::line(
        "read-only-mode",
        "Make the buffer read-only, or editable again (C-x C-q).",
    ),
    forge_docs::CommandDoc::new(
        "smerge-ediff",
        "Compare the three versions of the conflicted file: upper, base and lower,",
    )
    .body("above the result."),
    forge_docs::CommandDoc::line(
        "smerge-keep-all",
        "Keep all three versions of the conflict at point.",
    ),
    forge_docs::CommandDoc::line(
        "smerge-keep-base",
        "Resolve the conflict at point with the common-ancestor text.",
    ),
    forge_docs::CommandDoc::line(
        "smerge-keep-current",
        "Keep the version of the conflict that point is in.",
    ),
    forge_docs::CommandDoc::line(
        "smerge-keep-lower",
        "Keep the lower (other, theirs) version of the conflict at point.",
    ),
    forge_docs::CommandDoc::line(
        "smerge-keep-upper",
        "Keep the upper (mine) version of the conflict at point.",
    ),
    forge_docs::CommandDoc::line(
        "smerge-mode",
        "Toggle smerge-mode: keys (C-c ^ ...) for resolving conflict markers.",
    ),
    forge_docs::CommandDoc::line("smerge-next", "Move to the next conflict."),
    forge_docs::CommandDoc::line("smerge-prev", "Move to the previous conflict."),
    forge_docs::CommandDoc::line(
        "smerge-refine",
        "Highlight the words that differ between the sides of the conflict at point.",
    ),
    forge_docs::CommandDoc::new(
        "vc-annotate",
        "Show who last changed each line of the file, and when (C-x v g).",
    )
    .body("With C-u, as of a revision."),
    forge_docs::CommandDoc::line(
        "vc-annotate-find-revision-at-line",
        "Visit the file as of the revision of the line at point.",
    ),
    forge_docs::CommandDoc::line(
        "vc-annotate-goto-line",
        "Visit the file at the line at point.",
    ),
    forge_docs::CommandDoc::line(
        "vc-annotate-next-revision",
        "Annotate the file as of the next revision.",
    ),
    forge_docs::CommandDoc::line(
        "vc-annotate-prev-revision",
        "Annotate the file as of the previous revision.",
    ),
    forge_docs::CommandDoc::line(
        "vc-annotate-revision-at-line",
        "Annotate the file as of the revision of the line at point.",
    ),
    forge_docs::CommandDoc::line(
        "vc-annotate-revision-previous-to-line",
        "Annotate the file as of the revision before the line at point's.",
    ),
    forge_docs::CommandDoc::line(
        "vc-annotate-show-changeset-diff-revision-at-line",
        "Show the whole changeset of the line at point's revision.",
    ),
    forge_docs::CommandDoc::line(
        "vc-annotate-show-diff-revision-at-line",
        "Show how the line at point's revision changed the file.",
    ),
    forge_docs::CommandDoc::line(
        "vc-annotate-show-log-revision-at-line",
        "Show the log entry of the line at point's revision.",
    ),
    forge_docs::CommandDoc::line(
        "vc-annotate-toggle-annotation-visibility",
        "Show or hide the annotations.",
    ),
    forge_docs::CommandDoc::line(
        "vc-annotate-working-revision",
        "Annotate the file as of the working revision.",
    ),
    forge_docs::CommandDoc::line("vc-clone", "Clone a repository and open it in Dired."),
    forge_docs::CommandDoc::line(
        "vc-create-branch",
        "Make a new branch and switch to it (C-x v b c).",
    ),
    forge_docs::CommandDoc::line("vc-create-repo", "Make a new repository in a directory."),
    forge_docs::CommandDoc::line("vc-create-tag", "Tag a revision (C-x v s)."),
    forge_docs::CommandDoc::line("vc-delete-branch", "Delete a branch."),
    forge_docs::CommandDoc::line(
        "vc-delete-file",
        "Delete the file and tell version control (C-x v x).",
    ),
    forge_docs::CommandDoc::new(
        "vc-diff",
        "Show the changes to the fileset since the last commit (C-x v =).",
    )
    .body("With C-u, between two revisions."),
    forge_docs::CommandDoc::line(
        "vc-diff-incoming",
        "Show what the upstream has that this branch lacks.",
    ),
    forge_docs::CommandDoc::new(
        "vc-diff-mergebase",
        "Show the changes between a branch and its merge base with the working revision",
    )
    .body("(C-x v M D)."),
    forge_docs::CommandDoc::line(
        "vc-diff-outgoing",
        "Show the local commits the upstream lacks, as one diff.",
    ),
    forge_docs::CommandDoc::line(
        "vc-dir",
        "Show the version-control status of a directory (C-x v d).",
    ),
    forge_docs::CommandDoc::line(
        "vc-dir-clean-files",
        "Delete the marked unregistered files (or the one at point) from disk.",
    ),
    forge_docs::CommandDoc::line(
        "vc-dir-copy-file-names",
        "Copy the names of the marked files (or the one at point) to the kill ring (w).",
    ),
    forge_docs::CommandDoc::line("vc-dir-find-file", "Visit the file at point."),
    forge_docs::CommandDoc::line(
        "vc-dir-find-file-other-window",
        "Open the file at point in a second window.",
    ),
    forge_docs::CommandDoc::line(
        "vc-dir-help",
        "Show vc-dir's keys; the next key runs one of them.",
    ),
    forge_docs::CommandDoc::line(
        "vc-dir-hide-up-to-date",
        "Hide (or show again) the files with nothing to commit.",
    ),
    forge_docs::CommandDoc::line(
        "vc-dir-mark",
        "Mark the file at point (or every file in the region).",
    ),
    forge_docs::CommandDoc::line(
        "vc-dir-mark-all-files",
        "Mark every file in the same state as the one at point. With C-u, every file.",
    ),
    forge_docs::CommandDoc::line("vc-dir-next-group", "Move to the next group of files."),
    forge_docs::CommandDoc::line("vc-dir-next-line", "Move to the next file."),
    forge_docs::CommandDoc::line(
        "vc-dir-previous-group",
        "Move to the previous group of files.",
    ),
    forge_docs::CommandDoc::line("vc-dir-previous-line", "Move to the previous file."),
    forge_docs::CommandDoc::line("vc-dir-refresh", "Read the status again."),
    forge_docs::CommandDoc::line(
        "vc-dir-stage",
        "Stage the marked files (or the one at point) for the next commit.",
    ),
    forge_docs::CommandDoc::line("vc-dir-toggle-ignored", "Show or hide ignored files."),
    forge_docs::CommandDoc::line(
        "vc-dir-unmark",
        "Unmark the file at point (or every file in the region).",
    ),
    forge_docs::CommandDoc::line("vc-dir-unmark-all-files", "Unmark every file."),
    forge_docs::CommandDoc::line(
        "vc-dir-unmark-file-up",
        "Unmark the file above point and move there.",
    ),
    forge_docs::CommandDoc::line(
        "vc-dir-unstage",
        "Unstage the marked files (or the one at point).",
    ),
    forge_docs::CommandDoc::line(
        "vc-ediff",
        "Compare the file with its last committed version (as a diff buffer).",
    ),
    forge_docs::CommandDoc::line(
        "vc-edit-next-command",
        "Edit the command line of the next VC command before it runs (C-x v !).",
    ),
    forge_docs::CommandDoc::line("vc-fetch", "Fetch from the remote without merging."),
    forge_docs::CommandDoc::line(
        "vc-find-conflicted-file",
        "Visit the next file in the repository with unresolved conflicts.",
    ),
    forge_docs::CommandDoc::line(
        "vc-git-grep",
        "Search the tracked files for a regexp (git grep).",
    ),
    forge_docs::CommandDoc::line("vc-git-stash", "Stash the changes in the working tree."),
    forge_docs::CommandDoc::line("vc-git-stash-apply", "Apply a stash, keeping it."),
    forge_docs::CommandDoc::line("vc-git-stash-delete", "Drop a stash."),
    forge_docs::CommandDoc::line("vc-git-stash-pop", "Apply a stash and drop it."),
    forge_docs::CommandDoc::line("vc-git-stash-show", "Show a stash's changes."),
    forge_docs::CommandDoc::new(
        "vc-git-stash-snapshot",
        "Stash the changes and apply them again at once:",
    )
    .body("a snapshot of the work in progress."),
    forge_docs::CommandDoc::line(
        "vc-ignore",
        "Tell version control to ignore a file (C-x v G).",
    ),
    forge_docs::CommandDoc::line(
        "vc-insert-headers",
        "Explain that Git and Mercurial keep no version headers.",
    ),
    forge_docs::CommandDoc::line(
        "vc-kill-running-command",
        "Stop the version-control commands that are running.",
    ),
    forge_docs::CommandDoc::line(
        "vc-list-branches",
        "List the branches, to switch, merge, rename or delete them.",
    ),
    forge_docs::CommandDoc::line(
        "vc-list-item-action",
        "Act on the branch, tag or stash at point (switch, retrieve, show).",
    ),
    forge_docs::CommandDoc::line(
        "vc-list-item-apply",
        "Apply the stash at point (keeping it).",
    ),
    forge_docs::CommandDoc::line(
        "vc-list-item-delete",
        "Delete the branch or drop the stash at point.",
    ),
    forge_docs::CommandDoc::line(
        "vc-list-item-log",
        "Show the log of the branch or tag at point.",
    ),
    forge_docs::CommandDoc::line("vc-list-item-merge", "Merge the branch at point."),
    forge_docs::CommandDoc::line("vc-list-item-pop", "Apply and drop the stash at point."),
    forge_docs::CommandDoc::line("vc-list-item-rename", "Rename the branch at point."),
    forge_docs::CommandDoc::line("vc-list-refresh", "Read the list again."),
    forge_docs::CommandDoc::line(
        "vc-list-stashes",
        "List the stashes, to show, apply, pop or drop them.",
    ),
    forge_docs::CommandDoc::line(
        "vc-list-tags",
        "List the tags, to check one out or see its log.",
    ),
    forge_docs::CommandDoc::line(
        "vc-log-incoming",
        "Show the changes a pull would bring in (C-x v I).",
    ),
    forge_docs::CommandDoc::new(
        "vc-log-mergebase",
        "Show the log between a branch and its merge base with the working revision (C-x",
    )
    .body("v M L)."),
    forge_docs::CommandDoc::line(
        "vc-log-outgoing",
        "Show the changes a push would send (C-x v O).",
    ),
    forge_docs::CommandDoc::line(
        "vc-log-search",
        "Show the commits whose messages match a regexp.",
    ),
    forge_docs::CommandDoc::line(
        "vc-merge",
        "Merge a branch or revision into the working tree (C-x v m).",
    ),
    forge_docs::CommandDoc::new(
        "vc-next-action",
        "Do the next sensible thing for the fileset (C-x v v): register it, commit it,",
    )
    .body("resolve its conflicts, or update it. With C-u, pick the backend or revision."),
    forge_docs::CommandDoc::line(
        "vc-print-branch-log",
        "Show the log of a branch (C-x v b l).",
    ),
    forge_docs::CommandDoc::line(
        "vc-print-log",
        "Show the fileset's log (C-x v l). With C-u, ask how many entries and from where.",
    ),
    forge_docs::CommandDoc::line("vc-print-root-log", "Show the repository's log (C-x v L)."),
    forge_docs::CommandDoc::line("vc-pull", "Pull changes from the remote (C-x v +)."),
    forge_docs::CommandDoc::line(
        "vc-push",
        "Push your commits to the remote (C-x v P). With no upstream, asks where.",
    ),
    forge_docs::CommandDoc::line(
        "vc-refresh-state",
        "Look at the file's version-control state again.",
    ),
    forge_docs::CommandDoc::line(
        "vc-region-history",
        "Show the history of the lines in the region (C-x v h).",
    ),
    forge_docs::CommandDoc::line(
        "vc-register",
        "Put the fileset under version control (C-x v i).",
    ),
    forge_docs::CommandDoc::line("vc-rename-branch", "Rename a branch."),
    forge_docs::CommandDoc::line(
        "vc-rename-file",
        "Rename the file and tell version control.",
    ),
    forge_docs::CommandDoc::line(
        "vc-resolve-conflicts",
        "Turn on smerge-mode in this buffer to resolve its conflict markers.",
    ),
    forge_docs::CommandDoc::line("vc-retrieve-tag", "Check out a tag or branch (C-x v r)."),
    forge_docs::CommandDoc::line(
        "vc-revert",
        "Discard the changes to the fileset, after showing them (C-x v u).",
    ),
    forge_docs::CommandDoc::line(
        "vc-revision-other-window",
        "Visit the file as of another revision, in another window (C-x v ~).",
    ),
    forge_docs::CommandDoc::line("vc-rollback", "Undo the last commit, keeping its changes."),
    forge_docs::CommandDoc::line(
        "vc-root-diff",
        "Show every change in the repository since the last commit (C-x v D).",
    ),
    forge_docs::CommandDoc::line(
        "vc-root-version-diff",
        "Show the changes between two revisions of the whole repository.",
    ),
    forge_docs::CommandDoc::line("vc-switch-backend", "Say which backend handles the file."),
    forge_docs::CommandDoc::line("vc-switch-branch", "Switch to another branch (C-x v b s)."),
    forge_docs::CommandDoc::line("vc-update", "Pull changes from the remote (C-x v +)."),
    forge_docs::CommandDoc::line(
        "vc-version-diff",
        "Show the changes to the fileset between two revisions.",
    ),
    forge_docs::CommandDoc::line("vc-working-revision", "Say which revision the file is at."),
];

/// C-x v and the modes' keys.
pub const GLOBAL_BINDINGS: &[(&str, &str)] = &[
    ("C-x v v", "vc-next-action"),
    ("C-x v i", "vc-register"),
    ("C-x v =", "vc-diff"),
    ("C-x v D", "vc-root-diff"),
    ("C-x v l", "vc-print-log"),
    ("C-x v L", "vc-print-root-log"),
    ("C-x v I", "vc-log-incoming"),
    ("C-x v O", "vc-log-outgoing"),
    ("C-x v g", "vc-annotate"),
    ("C-x v ~", "vc-revision-other-window"),
    ("C-x v u", "vc-revert"),
    ("C-x v h", "vc-region-history"),
    ("C-x v d", "vc-dir"),
    ("C-x v +", "vc-update"),
    ("C-x v P", "vc-push"),
    ("C-x v m", "vc-merge"),
    ("C-x v G", "vc-ignore"),
    ("C-x v s", "vc-create-tag"),
    ("C-x v r", "vc-retrieve-tag"),
    ("C-x v x", "vc-delete-file"),
    ("C-x v !", "vc-edit-next-command"),
    ("C-x v b c", "vc-create-branch"),
    ("C-x v b s", "vc-switch-branch"),
    ("C-x v b l", "vc-print-branch-log"),
    ("C-x v M D", "vc-diff-mergebase"),
    ("C-x v M L", "vc-log-mergebase"),
    ("C-x p v", "project-vc-dir"),
    ("C-x C-q", "read-only-mode"),
];

pub const DIFF_MODE_BINDINGS: &[(&str, &str)] = &[
    ("n", "diff-hunk-next"),
    ("p", "diff-hunk-prev"),
    ("M-n", "diff-hunk-next"),
    ("M-p", "diff-hunk-prev"),
    ("N", "diff-file-next"),
    ("P", "diff-file-prev"),
    ("}", "diff-file-next"),
    ("{", "diff-file-prev"),
    ("RET", "diff-goto-source"),
    ("o", "diff-goto-source"),
    ("C-c C-c", "diff-goto-source"),
    ("k", "diff-hunk-kill"),
    ("M-k", "diff-hunk-kill"),
    ("K", "diff-file-kill"),
    ("M-K", "diff-file-kill"),
    ("C-c C-a", "diff-apply-hunk"),
    ("C-c C-s", "diff-split-hunk"),
    ("C-c C-r", "diff-reverse-direction"),
    ("C-c C-b", "diff-refine-hunk"),
    ("C-c C-w", "diff-toggle-whitespace"),
    ("C-c C-v", "diff-side-by-side"),
    ("TAB", "diff-toggle-fold"),
    ("<backtab>", "diff-toggle-fold-all"),
    ("g", "diff-regenerate"),
];

pub const LOG_VIEW_BINDINGS: &[(&str, &str)] = &[
    ("n", "log-view-next-entry"),
    ("p", "log-view-previous-entry"),
    ("RET", "log-view-toggle-entry-display"),
    ("TAB", "log-view-toggle-entry-display"),
    ("d", "log-view-diff"),
    ("=", "log-view-diff"),
    ("D", "log-view-diff-changeset"),
    ("w", "log-view-diff-working"),
    ("f", "log-view-find-revision"),
    ("o", "log-view-find-revision"),
    ("a", "log-view-annotate-version"),
    ("m", "log-view-toggle-mark-entry"),
    ("+", "log-view-show-more"),
    ("g", "log-view-refresh"),
];

pub const ANNOTATE_BINDINGS: &[(&str, &str)] = &[
    ("n", "vc-annotate-next-revision"),
    ("p", "vc-annotate-prev-revision"),
    ("j", "vc-annotate-revision-at-line"),
    ("a", "vc-annotate-revision-previous-to-line"),
    ("l", "vc-annotate-show-log-revision-at-line"),
    ("d", "vc-annotate-show-diff-revision-at-line"),
    ("=", "vc-annotate-show-diff-revision-at-line"),
    ("D", "vc-annotate-show-changeset-diff-revision-at-line"),
    ("f", "vc-annotate-find-revision-at-line"),
    ("w", "vc-annotate-working-revision"),
    ("v", "vc-annotate-toggle-annotation-visibility"),
    ("RET", "vc-annotate-goto-line"),
];

pub const DIR_BINDINGS: &[(&str, &str)] = &[
    ("w", "vc-dir-copy-file-names"),
    ("n", "vc-dir-next-line"),
    ("p", "vc-dir-previous-line"),
    ("C-n", "vc-dir-next-line"),
    ("C-p", "vc-dir-previous-line"),
    ("TAB", "vc-dir-next-group"),
    ("<backtab>", "vc-dir-previous-group"),
    ("RET", "vc-dir-find-file"),
    ("f", "vc-dir-find-file"),
    ("e", "vc-dir-find-file"),
    ("o", "vc-dir-find-file-other-window"),
    ("m", "vc-dir-mark"),
    ("u", "vc-dir-unmark"),
    ("DEL", "vc-dir-unmark-file-up"),
    ("M", "vc-dir-mark-all-files"),
    ("U", "vc-dir-unmark-all-files"),
    ("g", "vc-dir-refresh"),
    ("x", "vc-dir-hide-up-to-date"),
    ("!", "vc-dir-toggle-ignored"),
    ("d", "vc-dir-clean-files"),
    ("a", "vc-dir-stage"),
    ("A", "vc-dir-unstage"),
    ("?", "vc-dir-help"),
    ("v", "vc-next-action"),
    ("=", "vc-diff"),
    ("D", "vc-root-diff"),
    ("l", "vc-print-log"),
    ("L", "vc-print-root-log"),
    ("I", "vc-log-incoming"),
    ("O", "vc-log-outgoing"),
    ("i", "vc-register"),
    ("+", "vc-update"),
    ("P", "vc-push"),
    ("G", "vc-ignore"),
    ("b c", "vc-create-branch"),
    ("b s", "vc-switch-branch"),
    ("b l", "vc-list-branches"),
    ("z c", "vc-git-stash"),
    ("z s", "vc-git-stash-snapshot"),
    ("z p", "vc-git-stash-pop"),
    ("z a", "vc-git-stash-apply"),
    ("z l", "vc-list-stashes"),
];

pub const LOG_EDIT_BINDINGS: &[(&str, &str)] = &[
    ("C-c C-c", "log-edit-done"),
    ("C-c C-k", "log-edit-kill"),
    ("C-c C-d", "log-edit-show-diff"),
    ("C-c C-f", "log-edit-show-files"),
    ("M-p", "log-edit-previous-comment"),
    ("M-n", "log-edit-next-comment"),
    ("M-r", "log-edit-comment-search-backward"),
    ("M-s", "log-edit-comment-search-forward"),
    ("C-c C-e", "log-edit-toggle-amend"),
    ("C-c C-s", "log-edit-toggle-signoff"),
    ("C-c C-n", "log-edit-toggle-no-verify"),
];

pub const SMERGE_BINDINGS: &[(&str, &str)] = &[
    ("C-c ^ n", "smerge-next"),
    ("C-c ^ p", "smerge-prev"),
    ("C-c ^ u", "smerge-keep-upper"),
    ("C-c ^ l", "smerge-keep-lower"),
    ("C-c ^ m", "smerge-keep-upper"),
    ("C-c ^ o", "smerge-keep-lower"),
    ("C-c ^ b", "smerge-keep-base"),
    ("C-c ^ a", "smerge-keep-all"),
    ("C-c ^ RET", "smerge-keep-current"),
    ("C-c ^ R", "smerge-refine"),
    ("C-c ^ E", "smerge-ediff"),
];

pub const LIST_BINDINGS: &[(&str, &str)] = &[
    ("RET", "vc-list-item-action"),
    ("l", "vc-list-item-log"),
    ("m", "vc-list-item-merge"),
    ("r", "vc-list-item-rename"),
    ("d", "vc-list-item-delete"),
    ("a", "vc-list-item-apply"),
    ("P", "vc-list-item-pop"),
    ("c", "vc-create-branch"),
    ("g", "vc-list-refresh"),
];

/// The modes, their parents, and their keys.
pub fn install_keymaps(keymaps: &mut forge_core::Keymaps) {
    use forge_core::keymap::{Origin, Scope};
    let bind = |keymaps: &mut forge_core::Keymaps, scope: &Scope, table: &[(&str, &str)]| {
        for (k, c) in table {
            keymaps
                .bind(scope, k, c, Origin::Package)
                .expect("valid VC binding");
        }
    };
    bind(keymaps, &Scope::Global, GLOBAL_BINDINGS);
    for (mode, table, special) in [
        (diff::MODE, DIFF_MODE_BINDINGS, false),
        (log::MODE, LOG_VIEW_BINDINGS, true),
        (annotate::MODE, ANNOTATE_BINDINGS, true),
        (dir::MODE, DIR_BINDINGS, true),
        (log_edit::MODE, LOG_EDIT_BINDINGS, false),
        (smerge::MODE, SMERGE_BINDINGS, false),
        (branches::MODE, LIST_BINDINGS, true),
    ] {
        bind(keymaps, &Scope::Mode(mode.into()), table);
        if special {
            keymaps.set_mode_parent(mode, "special-mode");
        }
    }
    // A diff buffer is read-only like the special ones, but stays its own
    // mode so C-x C-q can make it editable: give it q and the paging keys.
    let d = Scope::Mode(diff::MODE.into());
    for (k, c) in [
        ("q", "quit-window"),
        ("SPC", "scroll-up-command"),
        ("DEL", "scroll-down-command"),
    ] {
        keymaps.bind(&d, k, c, Origin::Package).expect("valid");
    }
    keymaps.set_mode_parent("diff-side-by-side-mode", "special-mode");
    keymaps.set_mode_parent("vc-output-mode", "special-mode");
    keymaps.set_mode_parent(log_edit::MODE, "text-mode");
}

/// Run a VC command. False if `cmd` isn't one.
pub fn execute(app: &mut App, cmd: &str) -> bool {
    let n = app.count().unsigned_abs() as usize;
    let arg = app.has_arg();
    match cmd {
        "vc-next-action" => next_action(app),
        "vc-register" => {
            if let Some(fs) = fileset_or_error(app, false) {
                if fs.files.is_empty() {
                    app.error("No file to register");
                } else {
                    register(app, fs);
                }
            }
        }
        "vc-diff" | "vc-root-diff" => {
            let whole = cmd == "vc-root-diff";
            if let Some(fs) = fileset_or_error(app, whole) {
                if arg {
                    let cands = fs
                        .backend()
                        .revision_completions(&fs.root, single(&fs).as_deref());
                    let def = working_label(&fs);
                    ask(
                        app,
                        &format!("Older revision (default {def}): "),
                        Ask::DiffRev1(fs),
                        cands,
                        "",
                    );
                } else {
                    show_diff(app, &fs, None, None, false);
                }
            }
        }
        "vc-version-diff" | "vc-root-version-diff" => {
            let whole = cmd == "vc-root-version-diff";
            if let Some(fs) = fileset_or_error(app, whole) {
                let cands = fs
                    .backend()
                    .revision_completions(&fs.root, single(&fs).as_deref());
                let def = working_label(&fs);
                ask(
                    app,
                    &format!("Older revision (default {def}): "),
                    Ask::DiffRev1(fs),
                    cands,
                    "",
                );
            }
        }
        "vc-diff-incoming" | "vc-diff-outgoing" => {
            if let Some(fs) = fileset_or_error(app, true) {
                if fs.kind != Kind::Git {
                    app.error("Only Git can diff against the upstream here");
                    return true;
                }
                let fs = Fileset {
                    files: Vec::new(),
                    ..fs
                };
                let (a, b) = if cmd == "vc-diff-incoming" {
                    ("HEAD", "@{upstream}")
                } else {
                    ("@{upstream}", "HEAD")
                };
                show_diff(app, &fs, Some(a.into()), Some(b.into()), false);
            }
        }
        "vc-working-revision" => {
            if let Some(fs) = fileset_or_error(app, false) {
                match fs.backend().working_revision(&fs.root) {
                    Some(r) => app.msg(format!("Working revision: {r}")),
                    None => app.error("No working revision"),
                }
            }
        }
        "vc-find-conflicted-file" => {
            if let Some(fs) = fileset_or_error(app, true) {
                let out = std::process::Command::new("git")
                    .args(["diff", "--name-only", "--diff-filter=U"])
                    .current_dir(&fs.root)
                    .output();
                let first = out.ok().and_then(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .lines()
                        .next()
                        .map(str::to_string)
                });
                match first {
                    Some(f) => crate::minibuffer::visit_file(app, &fs.root.join(f), false),
                    None => app.msg("No files with conflicts"),
                }
            }
        }
        "vc-resolve-conflicts" => {
            smerge::execute(app, "smerge-mode");
        }
        "vc-log-search" => {
            if let Some(fs) = fileset_or_error(app, true) {
                ask(
                    app,
                    "Search log for regexp: ",
                    Ask::LogSearch(fs),
                    Vec::new(),
                    "",
                );
            }
        }
        "vc-rollback" => {
            if let Some(fs) = fileset_or_error(app, true) {
                ask(
                    app,
                    "Undo the last commit, keeping its changes? Type yes: ",
                    Ask::Rollback(fs),
                    Vec::new(),
                    "",
                );
            }
        }
        "vc-clone" => ask(
            app,
            "Clone repository (URL): ",
            Ask::CloneUrl,
            Vec::new(),
            "",
        ),
        "vc-switch-backend" => {
            if let Some(fs) = fileset_or_error(app, false) {
                app.msg(format!(
                    "{} is handled by {}; Forge only knows the backend that owns the tree",
                    fs.describe(),
                    fs.kind.name()
                ));
            }
        }
        "vc-insert-headers" => app.msg("Git and Mercurial keep no version headers in files"),
        "vc-ediff" => {
            app.msg("Showing the changes as a diff buffer");
            return execute(app, "vc-diff");
        }
        "vc-print-log" | "vc-print-root-log" => {
            let whole = cmd == "vc-print-root-log";
            if let Some(fs) = fileset_or_error(app, whole) {
                if arg {
                    let def = config().log_limit.to_string();
                    ask(
                        app,
                        "Number of entries to show (0 for all): ",
                        Ask::LogLimit(fs, whole),
                        Vec::new(),
                        &def,
                    );
                } else {
                    let here = (!whole)
                        .then(|| app.vc.files.get(&app.active_id()).map(|_| ()))
                        .flatten();
                    let _ = here;
                    log::print_log(app, &fs, whole, config().log_limit, None, log::Source::Log);
                }
            }
        }
        "vc-print-branch-log" => {
            if let Some(fs) = fileset_or_error(app, true) {
                let cands = fs.backend().branches(&fs.root, true);
                ask(app, "Branch log: ", Ask::BranchLog(fs), cands, "");
            }
        }
        "vc-log-incoming" | "vc-log-outgoing" => {
            if let Some(fs) = fileset_or_error(app, true) {
                let src = if cmd == "vc-log-incoming" {
                    log::Source::Incoming
                } else {
                    log::Source::Outgoing
                };
                log::print_log(app, &fs, true, 0, None, src);
            }
        }
        "vc-region-history" => log::region_history(app),
        "vc-annotate" => {
            if let Some(fs) = fileset_or_error(app, false) {
                let Some(f) = single(&fs) else {
                    app.error("Annotate works on one file");
                    return true;
                };
                if arg {
                    let cands = fs.backend().revision_completions(&fs.root, Some(&f));
                    ask(
                        app,
                        "Annotate from revision (default: the working revision): ",
                        Ask::AnnotateRev(fs),
                        cands,
                        "",
                    );
                } else {
                    let line = app
                        .vc
                        .files
                        .contains_key(&app.active_id())
                        .then(|| app.buf_ref().point().line + 1);
                    annotate::annotate(app, fs.kind, &fs.root, &f, None, line);
                }
            }
        }
        "vc-revision-other-window" => {
            if let Some(fs) = fileset_or_error(app, false) {
                let Some(f) = single(&fs) else {
                    app.error("Pick one file");
                    return true;
                };
                let cands = fs.backend().revision_completions(&fs.root, Some(&f));
                let def = working_label(&fs);
                ask(
                    app,
                    &format!("Revision to visit (default {def}): "),
                    Ask::RevisionOtherWindow(fs),
                    cands,
                    "",
                );
            }
        }
        "vc-revert" => revert(app),
        "vc-dir" => {
            let default = crate::compile::default_directory(app);
            let d = crate::files::abbreviate(&default);
            ask(
                app,
                "VC status for directory: ",
                Ask::Dir,
                Vec::new(),
                &format!("{}/", d.trim_end_matches('/')),
            );
        }
        "project-vc-dir" => {
            let dir = crate::compile::default_directory(app);
            dir::open_dir(app, &dir);
        }
        "vc-update" | "vc-pull" => {
            if let Some(fs) = fileset_or_error(app, true) {
                pull(app, fs);
            }
        }
        "vc-push" => {
            if let Some(fs) = fileset_or_error(app, true) {
                if arg {
                    app.vc.edit_next = true;
                }
                push(app, fs);
            }
        }
        "vc-fetch" => {
            if let Some(fs) = fileset_or_error(app, true) {
                match fs.backend().fetch(&fs.root) {
                    Ok(c) => remote_command(app, &fs, c, "Fetching"),
                    Err(e) => app.error(e),
                }
            }
        }
        "vc-merge" => {
            if let Some(fs) = fileset_or_error(app, true) {
                let cands = fs.backend().branches(&fs.root, true);
                ask(
                    app,
                    "Merge from branch or revision: ",
                    Ask::Merge(fs),
                    cands,
                    "",
                );
            }
        }
        "vc-ignore" => {
            if let Some(fs) = fileset_or_error(app, false) {
                let initial = single(&fs).map(|f| format!("/{f}")).unwrap_or_default();
                ask(
                    app,
                    "Ignore file (a pattern): ",
                    Ask::Ignore(fs),
                    Vec::new(),
                    &initial,
                );
            }
        }
        "vc-create-tag" => {
            if let Some(fs) = fileset_or_error(app, true) {
                ask(app, "New tag name: ", Ask::CreateTag(fs), Vec::new(), "");
            }
        }
        "vc-retrieve-tag" => {
            if let Some(fs) = fileset_or_error(app, true) {
                let mut cands = fs.backend().tags(&fs.root);
                cands.extend(fs.backend().branches(&fs.root, false));
                ask(
                    app,
                    "Tag or branch to check out: ",
                    Ask::RetrieveTag(fs),
                    cands,
                    "",
                );
            }
        }
        "vc-create-branch" => {
            if let Some(fs) = fileset_or_error(app, true) {
                ask(
                    app,
                    "New branch name: ",
                    Ask::CreateBranch(fs),
                    Vec::new(),
                    "",
                );
            }
        }
        "vc-switch-branch" => {
            if let Some(fs) = fileset_or_error(app, true) {
                let cands = fs.backend().branches(&fs.root, true);
                ask(app, "Switch to branch: ", Ask::SwitchBranch(fs), cands, "");
            }
        }
        "vc-rename-branch" => {
            if let Some(fs) = fileset_or_error(app, true) {
                let cands = fs.backend().branches(&fs.root, false);
                let cur = fs.backend().current_branch(&fs.root).unwrap_or_default();
                ask(app, "Rename branch: ", Ask::RenameBranch(fs), cands, &cur);
            }
        }
        "vc-delete-branch" => {
            if let Some(fs) = fileset_or_error(app, true) {
                let cands = fs.backend().branches(&fs.root, false);
                ask(app, "Delete branch: ", Ask::DeleteBranch(fs), cands, "");
            }
        }
        "vc-delete-file" => {
            if let Some(fs) = fileset_or_error(app, false) {
                match single(&fs) {
                    Some(f) => {
                        let p = format!("Really delete {f}? (y or n) ");
                        confirm(app, p, Confirm::DeleteFile(fs));
                    }
                    None => app.error("Delete one file at a time"),
                }
            }
        }
        "vc-rename-file" => {
            if let Some(fs) = fileset_or_error(app, false) {
                match single(&fs) {
                    Some(f) => ask(
                        app,
                        &format!("Rename {f} to: "),
                        Ask::RenameFile(fs),
                        Vec::new(),
                        "",
                    ),
                    None => app.error("Rename one file at a time"),
                }
            }
        }
        "vc-edit-next-command" => {
            app.vc.edit_next = true;
            app.msg("The next VC command's command line will be shown for editing");
        }
        "vc-diff-mergebase" | "vc-log-mergebase" => {
            if let Some(fs) = fileset_or_error(app, true) {
                let cands = fs.backend().branches(&fs.root, true);
                let what = if cmd == "vc-diff-mergebase" {
                    Ask::MergebaseDiff(fs)
                } else {
                    Ask::MergebaseLog(fs)
                };
                ask(
                    app,
                    "Branch to compare with its merge base: ",
                    what,
                    cands,
                    "",
                );
            }
        }
        "vc-create-repo" => {
            let dir = crate::compile::default_directory(app);
            let dir = app
                .buf_ref()
                .path()
                .and_then(|p| p.parent().map(Path::to_path_buf))
                .unwrap_or(dir);
            let names: Vec<String> = config()
                .handled
                .iter()
                .map(|k| k.name().to_string())
                .collect();
            ask(
                app,
                &format!(
                    "Create a repository in {} with (default Git): ",
                    crate::files::abbreviate(&dir)
                ),
                Ask::CreateRepoBackend(dir),
                names,
                "",
            );
        }
        "vc-refresh-state" => {
            let id = app.active_id();
            refresh_buffer(app, id);
            match app.vc.files.get(&id) {
                Some(i) => {
                    let s = format!("{}: {}", i.rel, i.state.label());
                    app.msg(s);
                }
                None => app.msg("Not under version control"),
            }
        }
        "vc-kill-running-command" => {
            let n = runner::kill_all(app);
            app.msg(if n == 0 {
                "No VC command is running".to_string()
            } else {
                format!("Stopped {n} command{}", if n == 1 { "" } else { "s" })
            });
        }
        "vc-git-grep" => {
            if let Some(fs) = fileset_or_error(app, true) {
                let word = crate::compile::word_at_point_pub(app.buf_ref());
                ask(
                    app,
                    "Search for (regexp): ",
                    Ask::GrepRegexp(fs),
                    Vec::new(),
                    &word,
                );
            }
        }
        "vc-git-stash" => {
            if let Some(fs) = fileset_or_error(app, true) {
                ask(
                    app,
                    "Stash message (optional): ",
                    Ask::StashMessage(fs),
                    Vec::new(),
                    "",
                );
            }
        }
        "vc-git-stash-snapshot" => {
            if let Some(fs) = fileset_or_error(app, true) {
                let b = fs.backend();
                match (
                    b.stash(&fs.root, "snapshot"),
                    b.stash_apply(&fs.root, "stash@{0}", false),
                ) {
                    (Ok(a), Ok(c)) => {
                        let root = fs.root.clone();
                        run_sequence(app, vec![a, c], "Snapshot", move |app, ok| {
                            refresh_root(app, &root);
                            if ok {
                                app.msg("Took a snapshot of the working tree (in the stash)");
                            }
                        });
                    }
                    (Err(e), _) | (_, Err(e)) => app.error(e),
                }
            }
        }
        "vc-git-stash-pop" => pick_stash(app, StashOp::Pop),
        "vc-git-stash-apply" => pick_stash(app, StashOp::Apply),
        "vc-git-stash-delete" => pick_stash(app, StashOp::Drop),
        "vc-git-stash-show" => pick_stash(app, StashOp::Show),
        "auto-fill-mode" => {
            let b = app.buf();
            let on = !b.minor_mode_enabled("auto-fill-mode");
            if on {
                b.enable_minor_mode("auto-fill-mode");
            } else {
                b.disable_minor_mode("auto-fill-mode");
            }
            app.msg(if on {
                "Auto-Fill mode enabled in current buffer"
            } else {
                "Auto-Fill mode disabled in current buffer"
            });
        }
        "read-only-mode" => {
            let b = app.buf();
            let now = !b.locked();
            b.set_locked(now);
            app.msg(if now {
                "Read-only mode enabled in current buffer"
            } else {
                "Read-only mode disabled in current buffer"
            });
        }
        "diff-backup" => {
            let Some(path) = app.buf_ref().path().map(Path::to_path_buf) else {
                app.error("This buffer isn't visiting a file");
                return true;
            };
            // From a backup's buffer, compare it with its file instead.
            let (old, new) = match forge_core::backup::file_of_backup(&path) {
                Some(orig) if orig.exists() => (path.clone(), orig),
                _ => match forge_core::backup::latest_backup(&path) {
                    Some(b) => (b, path.clone()),
                    None => {
                        app.error(format!("No backup of {}", crate::files::abbreviate(&path)));
                        return true;
                    }
                },
            };
            let a = std::fs::read_to_string(&old).unwrap_or_default();
            let b = std::fs::read_to_string(&new).unwrap_or_default();
            let text = diff::unified(
                &a,
                &b,
                &crate::files::abbreviate(&old),
                &crate::files::abbreviate(&new),
                3,
            );
            if text.is_empty() {
                app.msg("No differences");
            } else {
                let view = diff::DiffView {
                    root: PathBuf::from("/"),
                    ..Default::default()
                };
                let id = diff::show(app, diff::DIFF_BUFFER, &text, Some(view));
                crate::compile::display_buffer(app, id);
            }
        }
        "diff-hunk-next" => diff::hunk_move(app, true, n),
        "diff-hunk-prev" => diff::hunk_move(app, false, n),
        "diff-file-next" => diff::file_move(app, true, n),
        "diff-file-prev" => diff::file_move(app, false, n),
        "diff-goto-source" => diff::goto_source(app, arg),
        "diff-hunk-kill" => diff::kill_hunk(app),
        "diff-file-kill" => diff::kill_file(app),
        "diff-apply-hunk" => diff::apply_hunk(app, arg),
        "diff-apply-hunk-undo" => diff::undo_apply(app),
        "diff-split-hunk" => diff::split_hunk(app),
        "diff-reverse-direction" => diff::reverse_direction(app),
        "diff-refine-hunk" => diff::refine_at_point(app),
        "diff-toggle-whitespace" => diff::toggle_whitespace(app),
        "diff-side-by-side" => diff::side_by_side(app),
        "diff-toggle-fold" => diff::toggle_fold(app),
        "diff-toggle-fold-all" => diff::toggle_fold_all(app),
        "diff-regenerate" => {
            let id = app.active_id();
            diff::regenerate(app, id);
        }
        _ => {
            return log::execute(app, cmd)
                || annotate::execute(app, cmd)
                || dir::execute(app, cmd)
                || log_edit::execute(app, cmd)
                || smerge::execute(app, cmd)
                || branches::execute(app, cmd);
        }
    }
    true
}

/// After every command: keep an editable diff's headers right, keep the
/// commit buffer's file summary read-only, and know the buffer's state.
pub fn after_command(app: &mut App) {
    let mode = app.buf_ref().major_mode().to_string();
    if mode == diff::MODE {
        diff::after_edit(app);
    } else if mode == log_edit::MODE {
        log_edit::after_command(app);
    }
    if app.buf_ref().minor_mode_enabled(smerge::MODE) {
        smerge::after_command(app);
    }
    ensure_state(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_table_is_sorted_and_documented() {
        let names: Vec<&str> = COMMANDS.iter().map(|cd| cd.name()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(names, sorted);
        for cd in COMMANDS {
            assert!(!cd.summary().is_empty(), "{}", cd.name());
        }
    }

    #[test]
    fn every_binding_names_a_command() {
        let all: Vec<&(&str, &str)> = GLOBAL_BINDINGS
            .iter()
            .chain(DIFF_MODE_BINDINGS)
            .chain(LOG_VIEW_BINDINGS)
            .chain(ANNOTATE_BINDINGS)
            .chain(DIR_BINDINGS)
            .chain(LOG_EDIT_BINDINGS)
            .chain(SMERGE_BINDINGS)
            .chain(LIST_BINDINGS)
            .collect();
        for (k, c) in all {
            assert!(
                COMMANDS.iter().any(|cd| cd.name() == *c) || crate::commands::is_command(c),
                "{k} -> unknown {c}"
            );
        }
    }

    #[test]
    fn a_missing_program_says_so() {
        let c = Cmd::new("forge-no-such-vcs", Path::new("."), &["status"]);
        let e = c.run().unwrap_err();
        assert!(e.contains("isn't installed"), "{e}");
    }

    #[test]
    fn big_statuses_parse_quickly() {
        let mut out = String::from("# branch.head main\0");
        for i in 0..20_000 {
            out.push_str(&format!(
                "1 .M N... 100644 100644 100644 a b dir/file{i}.rs\0"
            ));
        }
        let t = std::time::Instant::now();
        let (e, _) = git::parse_status_v2(&out);
        assert_eq!(e.len(), 20_000);
        assert!(
            t.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            t.elapsed()
        );
        let lines: Vec<String> = (0..50_000)
            .map(|i| {
                if i % 3 == 0 {
                    format!("+x{i}")
                } else {
                    format!(" y{i}")
                }
            })
            .collect();
        let mut d = vec![
            "--- a".to_string(),
            "+++ b".to_string(),
            "@@ -1,33333 +1,50000 @@".to_string(),
        ];
        d.extend(lines);
        let t = std::time::Instant::now();
        assert_eq!(diff::parse(&d).len(), 1);
        assert!(t.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn command_lines_split_like_a_shell() {
        assert_eq!(
            split_command_line(r#"git push -u origin "my branch" a\ b ''"#),
            vec!["git", "push", "-u", "origin", "my branch", "a b", ""]
        );
    }

    #[test]
    fn mode_line_strings_follow_the_rules() {
        let d = std::env::temp_dir().join(format!("forge-vc-ml-{}", std::process::id()));
        std::fs::create_dir_all(d.join(".git")).unwrap();
        std::fs::write(d.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let mk = |state| FileInfo {
            kind: Kind::Git,
            root: d.clone(),
            rel: "f".into(),
            state,
        };
        assert_eq!(mode_line_string(&mk(FileState::UpToDate)), "Git-main");
        assert_eq!(mode_line_string(&mk(FileState::Edited)), "Git:main");
        assert_eq!(mode_line_string(&mk(FileState::Added)), "Git@main");
        assert_eq!(mode_line_string(&mk(FileState::Conflict)), "Git!main");
        assert_eq!(mode_line_string(&mk(FileState::Missing)), "Git?main");
        std::fs::remove_dir_all(d).unwrap();
    }
}
