// SPDX-License-Identifier: MIT OR Apache-2.0
//! Installing plugins from version control: M-x package-vc-install,
//! package-vc-upgrade, package-vc-upgrade-all, and index entries with
//! `git = "..."`.
//!
//! One package at a time, in the background:
//!
//! 1. clone the repository into the data directory (or fetch, if it's
//!    already there), check out the rev (a tag, branch or commit; else
//!    the default branch) and its submodules;
//! 2. work out how to build it (forge_core::package::build_plan) and,
//!    the first time or when the command changed, ask before running it
//!    (it's code from the repository, run with your rights);
//! 3. build, then install the component and load it (Forge asks for its
//!    permissions, as for any plugin).
//!
//! Git's and the build's output go to *Package Build*, shown while it
//! builds and when something fails.

use crate::app::App;
use crate::minibuffer::{ConfirmAction, Purpose};
use forge_core::package::{self, BuildPlan, BuildPolicy, Installed, InstalledDb, VcSource};
use forge_core::package_log::{Operation, Source, SourceKind, Status, Timer};
use forge_core::package_source::{
    RepositoryInfo, SourceKind as PackageSourceKind, SystemRepositoryInfo, repository_warnings,
};
use forge_core::process::{ProcessEvent, ProcessId, Processes, Program, shell_quote};
use forge_core::{CapabilityDiff, LockFile};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    mpsc::{self, Receiver, TryRecvError},
};
use std::time::Instant;

pub const BUFFER: &str = "*Package Build*";

#[derive(Debug, Clone)]
pub struct VcJob {
    pub name: String,
    pub repo: String,
    pub rev: Option<String>,
    /// From the declaration: a subdirectory to build in, a build command,
    /// cargo features.
    dir: Option<String>,
    build: Option<String>,
    features: Vec<String>,
    /// What the build may inherit and how long it may run.
    policy: BuildPolicy,
    src: PathBuf,
    /// Upgrading: stop if the commit is the one installed.
    upgrade: bool,
    /// The declaration lets an unattended upgrade cross a history rewrite.
    allow_rewrite: bool,
    commit: String,
    plan: Option<BuildPlan>,
    /// The commits an upgrade brings (git log since the locked commit),
    /// shown before building it (see `forge-lock.toml`).
    review: Option<String>,
    /// Supply-chain warnings about the upgrade's history or pinned tag.
    history_warnings: Vec<String>,
    /// The new commit isn't a descendant of the locked one.
    rewritten: bool,
    /// A locked sync must install exactly this commit.
    expected_commit: Option<String>,
    /// A locked sync must install exactly these artifact bytes.
    expected_artifact: Option<String>,
    fetch_started: Option<Timer>,
    build_started: Option<Timer>,
}

#[derive(Debug, Clone, Copy)]
enum VcQuestion {
    Build,
    ReviewUpgrade,
}

impl VcJob {
    /// Where the build runs: the repository, or `lisp_dir` inside it.
    fn root(&self) -> PathBuf {
        match &self.dir {
            Some(d) => self.src.join(d),
            None => self.src.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Fetch,
    Checkout,
    #[cfg_attr(not(windows), allow(dead_code))]
    Submodules,
    Commit,
    /// An upgrade of a declared plugin: list what changed before building.
    Review,
    Build,
}

/// Git operations the package manager needs without mutating installed
/// state. Tests replace this with a fixture that never touches the network.
pub trait GitRemote: Send + Sync {
    /// Resolve a tag, branch, commit or HEAD at the remote.
    fn remote_commit(&self, repository: &str, rev: Option<&str>) -> Result<String, String>;

    /// Detect the build command in the remote revision's tree.
    fn build_plan_at(
        &self,
        repository: &str,
        commit: &str,
        name: &str,
        dir: Option<&str>,
        build: Option<&str>,
        features: &[String],
    ) -> Result<BuildPlan, String>;

    /// Is `old` an ancestor of `new`? A false answer means the upstream
    /// history was rewritten (or force-pushed). `checkout` may hold objects
    /// the remote no longer advertises. Fixtures that don't model history
    /// default to true, so they never produce a false warning.
    fn is_ancestor(
        &self,
        _repository: &str,
        _checkout: Option<&Path>,
        _old: &str,
        _new: &str,
    ) -> Result<bool, String> {
        Ok(true)
    }

    /// The commit a tag now points to, or None when `rev` is a branch or
    /// isn't a tag. Fixtures that don't model tags default to None.
    fn tag_commit(&self, _repository: &str, _rev: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemGit;

pub struct Vc {
    processes: Processes,
    queue: VecDeque<VcJob>,
    /// The job being worked on, its running step and that step's stdout.
    current: Option<Current>,
    /// A job waiting for "build it? y or n".
    confirming: Option<VcJob>,
    /// That question, until the minibuffer is free to ask it.
    ask: Option<(String, VcQuestion)>,
    reviewing: Option<VcJob>,
    capability: Option<PendingCapability>,
    git: Box<dyn GitRemote>,
    repository_info: Arc<dyn RepositoryInfo>,
    repository_checks: bool,
    repository_checks_pending: Vec<(String, Receiver<Vec<String>>)>,
    /// The default build policy; declarations extend it per job.
    policy: BuildPolicy,
}

struct Current {
    job: VcJob,
    stage: Stage,
    id: ProcessId,
    out: String,
    /// When a build must be killed; only build stages have one.
    deadline: Option<Instant>,
    timed_out: bool,
}

struct PendingCapability {
    job: VcJob,
    bytes: Vec<u8>,
    diff: CapabilityDiff,
}

impl Default for Vc {
    fn default() -> Self {
        Self {
            processes: Processes::default(),
            queue: VecDeque::new(),
            current: None,
            confirming: None,
            ask: None,
            reviewing: None,
            capability: None,
            git: Box::new(SystemGit),
            repository_info: Arc::new(SystemRepositoryInfo),
            repository_checks: false,
            repository_checks_pending: Vec::new(),
            policy: BuildPolicy::default(),
        }
    }
}

impl Vc {
    pub fn busy(&self) -> bool {
        self.processes.any_running() || !self.repository_checks_pending.is_empty()
    }

    pub fn has_question(&self) -> bool {
        self.ask.is_some()
    }

    /// Drop jobs that have not started yet (a failed batch update).
    pub(crate) fn cancel_queued(&mut self) {
        self.queue.clear();
        self.capability = None;
    }

    /// Replace remote Git access (tests use a fixture implementation).
    pub fn set_git_remote(&mut self, remote: Box<dyn GitRemote>) {
        self.git = remote;
    }

    /// Replace repository metadata lookups (tests use a fixture).
    pub fn set_repository_info(&mut self, info: Arc<dyn RepositoryInfo>) {
        self.repository_info = info;
    }

    /// Turn optional archived/owner-changed checks on or off.
    pub fn set_repository_checks(&mut self, on: bool) {
        self.repository_checks = on;
    }

    /// Replace the default build policy (declarations still extend it).
    pub fn set_build_policy(&mut self, policy: BuildPolicy) {
        self.policy = policy;
    }
}

/// What the package manager should show for an in-flight Git install.
pub fn status(app: &App, name: &str) -> Option<&'static str> {
    let vc = &app.packages.vc;
    if let Some(current) = &vc.current
        && current.job.name == name
    {
        return Some(match current.stage {
            Stage::Fetch => "fetching Git",
            Stage::Checkout => "checking out",
            Stage::Submodules => "getting submodules",
            Stage::Commit => "reading commit",
            Stage::Review => "reviewing changes",
            Stage::Build => "building",
        });
    }
    if vc.confirming.as_ref().is_some_and(|job| job.name == name) {
        return Some("awaiting build approval");
    }
    if vc.reviewing.as_ref().is_some_and(|job| job.name == name) {
        return Some("awaiting upgrade approval");
    }
    if vc
        .capability
        .as_ref()
        .is_some_and(|pending| pending.job.name == name)
    {
        return Some("awaiting capability approval");
    }
    vc.queue
        .iter()
        .any(|job| job.name == name)
        .then_some("queued")
}

fn db(app: &App) -> Option<InstalledDb> {
    app.packages
        .plugin_dir
        .clone()
        .or_else(forge_core::config::plugin_dir)
        .map(|d| InstalledDb::load(&d))
}

pub fn execute(app: &mut App, cmd: &str) -> bool {
    match cmd {
        "package-vc-install" => {
            app.prompt("Install from repository (URL[@REV]): ", Purpose::PackageVc)
        }
        "package-vc-upgrade" => {
            if app.raw_arg() {
                show_dry_run(app);
                return true;
            }
            let names: Vec<String> = db(app)
                .map(|d| d.from_vc().into_iter().map(|v| v.0).collect())
                .unwrap_or_default();
            if names.is_empty() {
                app.error("No packages installed from version control");
            } else {
                app.prompt(
                    "Upgrade package: ",
                    Purpose::Choose {
                        what: crate::minibuffer::Choice::UpgradeVcPackage,
                        candidates: names,
                    },
                );
            }
        }
        "package-vc-upgrade-all" => {
            if app.raw_arg() {
                show_dry_run(app);
            } else if upgrade_all(app) == 0 {
                app.msg("No packages installed from version control");
            }
        }
        _ => return false,
    }
    true
}

fn show_dry_run(app: &mut App) {
    match dry_run_lines(app) {
        Ok(lines) => {
            let mut text = String::from("Git package upgrade plan (no files changed)\n\n");
            if lines.is_empty() {
                text.push_str("No Git packages need upgrading.\n");
            } else {
                text.push_str(&lines.join("\n"));
                text.push('\n');
            }
            app.show_special_buffer("*Package Upgrade Plan*", "special-mode", &text);
        }
        Err(error) => app.error(format!("Couldn't plan Git package upgrades: {error}")),
    }
}

/// Answer to package-vc-install: `URL[@REV]`.
pub fn install_spec(app: &mut App, spec: &str) {
    let (repo, rev) = package::parse_vc_spec(spec);
    if repo.is_empty() {
        return;
    }
    let name = package::vc_name(&repo);
    install(app, &name, &repo, rev, false);
}

/// Queue a package to fetch, build and install.
pub fn install(app: &mut App, name: &str, repo: &str, rev: Option<String>, upgrade: bool) {
    install_expected(app, name, repo, rev, upgrade, None, None);
}

/// Queue a package at an exact commit and artifact, as recorded in
/// `forge-lock.toml`. A moved branch or rebuilt artifact is an error, not
/// something to silently replace.
pub fn install_locked(app: &mut App, name: &str, repo: &str, commit: &str, artifact_sha256: &str) {
    install_expected(
        app,
        name,
        repo,
        Some(commit.to_string()),
        false,
        Some(commit.to_string()),
        Some(artifact_sha256.to_string()),
    );
}

fn install_expected(
    app: &mut App,
    name: &str,
    repo: &str,
    rev: Option<String>,
    upgrade: bool,
    expected_commit: Option<String>,
    expected_artifact: Option<String>,
) {
    if let Err(error) = crate::packages::check_source(app, PackageSourceKind::Git, repo) {
        if upgrade {
            crate::packages::abort_update(app, error);
        } else {
            app.error(error);
        }
        return;
    }
    start_repository_check(app, name, repo);
    let Some(src) = package::vc_dir(name) else {
        return app.error("No data directory (set HOME or FORGE_DATA_DIR)");
    };
    let declared = crate::declare::declared(app, name);
    let dir = declared.as_ref().and_then(|u| u.git_dir.clone());
    if let Some(d) = &dir {
        let p = std::path::Path::new(d);
        if p.is_absolute()
            || p.components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return app.error(format!("{name}: lisp_dir must stay inside the repository"));
        }
    }
    let (build, features, policy) = match &declared {
        Some(u) => (u.build.clone(), u.features.clone(), u.build_policy()),
        None => (None, Vec::new(), app.packages.vc.policy.clone()),
    };
    let job = VcJob {
        name: name.to_string(),
        repo: repo.to_string(),
        rev,
        dir,
        build,
        features,
        policy,
        src,
        upgrade,
        allow_rewrite: declared
            .as_ref()
            .is_some_and(|use_spec| use_spec.allow_rewrite),
        commit: String::new(),
        plan: None,
        review: None,
        history_warnings: Vec::new(),
        rewritten: false,
        expected_commit,
        expected_artifact,
        fetch_started: None,
        build_started: None,
    };
    app.packages.vc.queue.push_back(job);
    next(app);
}

pub fn upgrade(app: &mut App, name: &str) {
    if crate::declare::declared(app, name).is_some_and(|use_spec| use_spec.frozen) {
        return app.msg(format!("{name} is pinned or frozen; not upgrading"));
    }
    let Some(entry) = db(app).and_then(|d| d.from_vc().into_iter().find(|v| v.0 == name)) else {
        return app.error(format!("{name} wasn't installed from version control"));
    };
    if let Err(error) = crate::packages::begin_update(app, "upgrade", false) {
        return app.error(format!("Can't start package update: {error}"));
    }
    install(app, &entry.0, &entry.1, entry.2, true);
}

/// Upgrade every package from version control; returns how many.
pub fn upgrade_all(app: &mut App) -> usize {
    if let Err(error) = crate::packages::begin_update(app, "upgrade-all", true) {
        app.error(format!("Can't start package upgrade: {error}"));
        return 0;
    }
    let all = db(app).map(|d| d.from_vc()).unwrap_or_default();
    let frozen: std::collections::BTreeSet<String> = app
        .rhai
        .as_ref()
        .map(|r| {
            r.plugin_uses()
                .into_iter()
                .filter(|u| u.frozen)
                .map(|u| u.name)
                .collect()
        })
        .unwrap_or_default();
    let all: Vec<_> = all
        .into_iter()
        .filter(|(name, _, _)| !frozen.contains(name))
        .collect();
    for (name, repo, rev) in &all {
        install(app, name, repo, rev.clone(), true);
    }
    all.len()
}

pub fn dry_run_lines(app: &App) -> Result<Vec<String>, String> {
    let installed = db(app).map(|d| d.from_vc()).unwrap_or_default();
    let frozen: std::collections::BTreeSet<String> = app
        .rhai
        .as_ref()
        .map(|r| {
            r.plugin_uses()
                .into_iter()
                .filter(|u| u.frozen)
                .map(|u| u.name)
                .collect()
        })
        .unwrap_or_default();
    let records = db(app).unwrap_or_default();
    let mut lines = Vec::new();
    for (name, repository, rev) in installed {
        if frozen.contains(&name) {
            continue;
        }
        crate::packages::check_source(app, PackageSourceKind::Git, &repository)?;
        let package = records
            .packages
            .get(&name)
            .ok_or_else(|| format!("{name}: installed record disappeared"))?;
        let old_commit = package.commit.as_deref();
        let old = old_commit.unwrap_or("(unknown commit)");
        let new = app
            .packages
            .vc
            .git
            .remote_commit(&repository, rev.as_deref())?;
        if new == old {
            continue;
        }
        let history = history_warnings(
            app.packages.vc.git.as_ref(),
            &repository,
            None,
            rev.as_deref(),
            old_commit,
            &new,
        );
        let declared = crate::declare::declared(app, &name);
        let dir = declared.as_ref().and_then(|u| u.git_dir.as_deref());
        let build = declared.as_ref().and_then(|u| u.build.as_deref());
        let features = declared
            .as_ref()
            .map(|u| u.features.as_slice())
            .unwrap_or(&[]);
        let build =
            match app
                .packages
                .vc
                .git
                .build_plan_at(&repository, &new, &name, dir, build, features)
            {
                Ok(plan) => plan
                    .command
                    .unwrap_or_else(|| "(no build command)".to_string()),
                Err(error) => format!("(no build command: {error})"),
            };
        lines.push(format!(
            "{name}: {} -> {}; build: {build}",
            short(old),
            short(&new)
        ));
        lines.extend(
            history
                .warnings
                .into_iter()
                .map(|warning| format!("  {warning}")),
        );
    }
    Ok(lines)
}

impl GitRemote for SystemGit {
    fn remote_commit(&self, repository: &str, rev: Option<&str>) -> Result<String, String> {
        let args = match rev {
            Some(rev) => vec![
                "ls-remote".to_string(),
                "--heads".into(),
                "--tags".into(),
                repository.to_string(),
                format!("refs/heads/{rev}"),
                format!("refs/tags/{rev}"),
                format!("refs/tags/{rev}^{{}}"),
            ],
            None => vec![
                "ls-remote".to_string(),
                "--symref".into(),
                repository.to_string(),
                "HEAD".into(),
            ],
        };
        let current = std::env::current_dir().map_err(|e| e.to_string())?;
        let stop = std::sync::atomic::AtomicBool::new(false);
        let result = forge_core::process::run_sync(
            Program::Argv("git", &args),
            &current,
            None,
            std::time::Duration::from_secs(15),
            &stop,
        )
        .map_err(|e| format!("{repository}: couldn't query remote revision: {e}"))?;
        if result.code != Some(0) {
            let error = String::from_utf8_lossy(&result.stderr).trim().to_string();
            return Err(format!("{repository}: git ls-remote failed: {error}"));
        }
        let mut hashes = std::collections::BTreeMap::new();
        for line in String::from_utf8_lossy(&result.stdout).lines() {
            let mut fields = line.split_whitespace();
            let Some(first) = fields.next() else { continue };
            let Some(reference) = fields.next() else {
                continue;
            };
            if reference == "HEAD" || reference.starts_with("refs/") {
                hashes.insert(reference.to_string(), first.to_string());
            }
        }
        let commit = if let Some(rev) = rev {
            hashes
                .get(&format!("refs/heads/{rev}"))
                .or_else(|| hashes.get(&format!("refs/tags/{rev}^{{}}")))
                .or_else(|| hashes.get(&format!("refs/tags/{rev}")))
        } else {
            hashes.get("HEAD")
        };
        commit.cloned().ok_or_else(|| {
            format!(
                "{repository}: remote revision {} was not found",
                rev.unwrap_or("HEAD")
            )
        })
    }

    fn is_ancestor(
        &self,
        repository: &str,
        checkout: Option<&Path>,
        old: &str,
        new: &str,
    ) -> Result<bool, String> {
        if old == new {
            return Ok(true);
        }
        let temp = temp_checkout_dir();
        let _ = std::fs::remove_dir_all(&temp);
        let result = (|| {
            let parent = temp
                .parent()
                .ok_or_else(|| "temporary ancestry path has no parent".to_string())?;
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("creating temporary ancestry directory: {error}"))?;
            run_git(
                &[
                    "init".to_string(),
                    "--bare".into(),
                    "--quiet".into(),
                    temp.display().to_string(),
                ],
                parent,
            )?;
            let fetch = |source: &str, commit: &str| {
                git_output(
                    &[
                        "fetch".to_string(),
                        "--quiet".into(),
                        "--no-tags".into(),
                        source.to_string(),
                        commit.to_string(),
                    ],
                    &temp,
                )
                .is_ok_and(|result| result.code == Some(0))
            };
            let fetch_refs = |source: &str| {
                git_output(
                    &[
                        "fetch".to_string(),
                        "--quiet".into(),
                        "--force".into(),
                        source.to_string(),
                        "+refs/heads/*:refs/heads/*".into(),
                        "+refs/tags/*:refs/tags/*".into(),
                    ],
                    &temp,
                )
                .is_ok_and(|result| result.code == Some(0))
            };
            let mut have_old = fetch(repository, old);
            let mut have_new = fetch(repository, new);
            if !have_old || !have_new {
                fetch_refs(repository);
            }
            if let Some(checkout) = checkout {
                let checkout = checkout.display().to_string();
                if !have_old {
                    have_old = fetch(&checkout, old) || fetch_refs(&checkout);
                }
                if !have_new {
                    have_new = fetch(&checkout, new) || fetch_refs(&checkout);
                }
            }
            let has = |commit: &str| {
                git_output(
                    &[
                        "cat-file".to_string(),
                        "-e".into(),
                        format!("{commit}^{{commit}}"),
                    ],
                    &temp,
                )
                .is_ok_and(|result| result.code == Some(0))
            };
            if !have_old && !has(old) {
                return Err(format!(
                    "couldn't find locked commit {old} to check whether the upstream history was rewritten"
                ));
            }
            if !have_new && !has(new) {
                return Err(format!("couldn't find upstream commit {new}"));
            }
            let result = git_output(
                &[
                    "merge-base".to_string(),
                    "--is-ancestor".into(),
                    old.to_string(),
                    new.to_string(),
                ],
                &temp,
            )?;
            match result.code {
                Some(0) => Ok(true),
                Some(1) => Ok(false),
                _ => {
                    let error = String::from_utf8_lossy(&result.stderr).trim().to_string();
                    Err(format!("git merge-base --is-ancestor failed: {error}"))
                }
            }
        })();
        let _ = std::fs::remove_dir_all(&temp);
        result
    }

    fn tag_commit(&self, repository: &str, rev: &str) -> Result<Option<String>, String> {
        let rev = rev.strip_prefix("refs/tags/").unwrap_or(rev);
        if rev.is_empty() || rev.starts_with("refs/") {
            return Ok(None);
        }
        let args = vec![
            "ls-remote".to_string(),
            "--heads".into(),
            "--tags".into(),
            repository.to_string(),
            format!("refs/heads/{rev}"),
            format!("refs/tags/{rev}"),
            format!("refs/tags/{rev}^{{}}"),
        ];
        let current = std::env::current_dir().map_err(|e| e.to_string())?;
        let stop = std::sync::atomic::AtomicBool::new(false);
        let result = forge_core::process::run_sync(
            Program::Argv("git", &args),
            &current,
            None,
            std::time::Duration::from_secs(15),
            &stop,
        )
        .map_err(|e| format!("{repository}: couldn't query remote tag: {e}"))?;
        if result.code != Some(0) {
            let error = String::from_utf8_lossy(&result.stderr).trim().to_string();
            return Err(format!("{repository}: git ls-remote failed: {error}"));
        }
        let mut hashes = std::collections::BTreeMap::new();
        for line in String::from_utf8_lossy(&result.stdout).lines() {
            let mut fields = line.split_whitespace();
            let Some(first) = fields.next() else { continue };
            let Some(reference) = fields.next() else {
                continue;
            };
            if reference.starts_with("refs/") {
                hashes.insert(reference.to_string(), first.to_string());
            }
        }
        if hashes.contains_key(&format!("refs/heads/{rev}")) {
            return Ok(None);
        }
        Ok(hashes
            .get(&format!("refs/tags/{rev}^{{}}"))
            .or_else(|| hashes.get(&format!("refs/tags/{rev}")))
            .cloned())
    }

    fn build_plan_at(
        &self,
        repository: &str,
        commit: &str,
        name: &str,
        dir: Option<&str>,
        build: Option<&str>,
        features: &[String],
    ) -> Result<BuildPlan, String> {
        let temp = temp_checkout_dir();
        let _ = std::fs::remove_dir_all(&temp);
        let result = (|| {
            let parent = temp
                .parent()
                .ok_or_else(|| "temporary checkout path has no parent".to_string())?;
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("creating temporary checkout: {error}"))?;
            let args = vec![
                "clone".to_string(),
                "--quiet".into(),
                "--no-checkout".into(),
                repository.to_string(),
                temp.display().to_string(),
            ];
            run_git(&args, parent)?;
            let args = vec![
                "checkout".to_string(),
                "--quiet".into(),
                "--detach".into(),
                commit.to_string(),
            ];
            run_git(&args, &temp)?;
            let root = match dir {
                Some(dir)
                    if !std::path::Path::new(dir).is_absolute()
                        && !std::path::Path::new(dir)
                            .components()
                            .any(|part| matches!(part, std::path::Component::ParentDir)) =>
                {
                    temp.join(dir)
                }
                Some(_) => {
                    return Err("git directory must stay inside the repository".into());
                }
                None => temp.clone(),
            };
            resolve_build_plan(&root, name, build, features)
        })();
        let _ = std::fs::remove_dir_all(&temp);
        result
    }
}

fn git_output(
    args: &[String],
    dir: &std::path::Path,
) -> Result<forge_core::process::SyncOutput, String> {
    let stop = std::sync::atomic::AtomicBool::new(false);
    forge_core::process::run_sync(
        Program::Argv("git", args),
        dir,
        None,
        std::time::Duration::from_secs(60),
        &stop,
    )
    .map_err(|error| format!("running git: {error}"))
}

fn run_git(args: &[String], dir: &std::path::Path) -> Result<(), String> {
    let result = git_output(args, dir)?;
    if result.code == Some(0) {
        return Ok(());
    }
    let error = String::from_utf8_lossy(&result.stderr).trim().to_string();
    Err(format!("git failed: {error}"))
}

fn temp_checkout_dir() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "forge-git-inspect-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn start_repository_check(app: &mut App, name: &str, repo: &str) {
    let vc = &app.packages.vc;
    if !vc.repository_checks {
        return;
    }
    let info = Arc::clone(&vc.repository_info);
    let repository = repo.to_string();
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("package-repository-check".into())
        .spawn(move || {
            let warnings = info
                .inspect(&repository)
                .map(|metadata| repository_warnings(&repository, &metadata))
                .unwrap_or_default();
            let _ = tx.send(warnings);
        });
    if spawned.is_ok() {
        app.packages
            .vc
            .repository_checks_pending
            .push((name.to_string(), rx));
    }
}

fn poll_repository_checks(app: &mut App) {
    let mut finished = Vec::new();
    app.packages
        .vc
        .repository_checks_pending
        .retain_mut(|(name, rx)| match rx.try_recv() {
            Ok(warnings) => {
                finished.push((name.clone(), warnings));
                false
            }
            Err(TryRecvError::Empty) => true,
            Err(TryRecvError::Disconnected) => false,
        });
    for (name, warnings) in finished {
        for warning in &warnings {
            append(app, &format!("\n{warning}\n"));
        }
        if !warnings.is_empty() {
            app.msg(format!(
                "Repository check for {name} found a warning (see {BUFFER})"
            ));
        }
    }
}

/// Start the next queued job, if nothing is running.
fn next(app: &mut App) {
    let vc = &mut app.packages.vc;
    if vc.current.is_some() || vc.confirming.is_some() {
        return;
    }
    let Some(mut job) = vc.queue.pop_front() else {
        crate::packages::finish_update(app);
        return;
    };
    clear_output(app);
    app.show_special_buffer(BUFFER, "compilation-mode", "");
    append(app, &format!("{} from {}\n\n", job.name, job.repo));
    let verb = if job.upgrade { "Updating" } else { "Getting" };
    app.msg(format!("{verb} {} from {}...", job.name, job.repo));
    let fetch_started = Timer::start();
    let fresh = !job.src.join(".git").exists();
    let r = if fresh {
        let _ = job.src.parent().map(std::fs::create_dir_all);
        let _ = std::fs::remove_dir_all(&job.src);
        let args = vec![
            "clone".to_string(),
            "--quiet".into(),
            job.repo.clone(),
            job.src.display().to_string(),
        ];
        spawn(
            app,
            Program::Argv("git", &args),
            job.src.parent().unwrap_or(&job.src).to_path_buf(),
        )
    } else {
        let args: Vec<String> = ["fetch", "--quiet", "--tags", "--force", "origin"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        spawn(app, Program::Argv("git", &args), job.src.clone())
    };
    job.fetch_started = Some(fetch_started);
    step(app, job, Stage::Fetch, r);
}

fn spawn(app: &mut App, program: Program, dir: PathBuf) -> Result<ProcessId, String> {
    spawn_process(app, program, dir, false, &[])
}

fn spawn_with_env(
    app: &mut App,
    program: Program,
    dir: PathBuf,
    env: &[(String, String)],
) -> Result<ProcessId, String> {
    spawn_process(app, program, dir, true, env)
}

fn spawn_process(
    app: &mut App,
    program: Program,
    dir: PathBuf,
    clear: bool,
    env: &[(String, String)],
) -> Result<ProcessId, String> {
    let line = match &program {
        Program::Shell(l) => format!("$ {l}\n"),
        Program::Argv(p, a) => format!("$ {p} {}\n", a.join(" ")),
    };
    append(app, &line);
    let processes = &mut app.packages.vc.processes;
    let r = if clear {
        processes.spawn_env(program, &dir, false, env)
    } else {
        processes.spawn(program, &dir, false, env)
    };
    r.map_err(|e| e.to_string())
}

fn step(app: &mut App, job: VcJob, stage: Stage, r: Result<ProcessId, String>) {
    match r {
        Ok(id) => {
            let deadline = if stage == Stage::Build {
                Instant::now().checked_add(job.policy.timeout)
            } else {
                None
            };
            app.packages.vc.current = Some(Current {
                job,
                stage,
                id,
                out: String::new(),
                deadline,
                timed_out: false,
            });
        }
        Err(e) => {
            log_stage(&job, stage, Status::Failed, Some(&e));
            fail(app, &job, &e)
        }
    }
}

fn log_stage(job: &VcJob, stage: Stage, status: Status, error: Option<&str>) {
    let (operation, timer) = match stage {
        Stage::Fetch => (Operation::Fetch, job.fetch_started),
        Stage::Build => (Operation::Build, job.build_started),
        _ => return,
    };
    let Some(timer) = timer else {
        return;
    };
    let mut event = timer
        .finish(operation, status, Some(&job.name))
        .source(job_source(job));
    if !job.commit.is_empty() {
        event = event.detail("commit", &job.commit);
    }
    if operation == Operation::Build
        && let Some(command) = job.plan.as_ref().and_then(|plan| plan.command.as_ref())
    {
        event = event.detail("command", command);
    }
    if let Some(error) = error {
        event = event.error(error);
    }
    crate::package_log::record(event);
}

fn job_source(job: &VcJob) -> Source {
    match &job.rev {
        Some(revision) => Source::new(SourceKind::Git)
            .location(&job.repo)
            .revision(revision),
        None => Source::new(SourceKind::Git).location(&job.repo),
    }
}

fn log_install(job: &VcJob, version: &str, timer: Timer, status: Status, error: Option<&str>) {
    let mut event = timer
        .finish(Operation::Install, status, Some(&job.name))
        .source(job_source(job))
        .detail("version", version);
    if !job.commit.is_empty() {
        event = event.detail("commit", &job.commit);
    }
    if job.upgrade {
        event = event.detail("upgrade", "true");
    }
    if let Some(error) = error {
        event = event.error(error);
    }
    crate::package_log::record(event);
}

fn fail(app: &mut App, job: &VcJob, why: &str) {
    append(app, &format!("\n{why}\n"));
    if let Some(id) = app.editor.find_buffer(BUFFER) {
        crate::compile::display_buffer(app, id);
    }
    let message = format!("Can't install {}: {why}", job.name);
    let message = if job.upgrade {
        crate::packages::abort_update_message(app, message)
    } else {
        message
    };
    app.error(message);
    app.packages.vc.current = None;
    next(app);
}

/// Output and finished steps (idle time).
pub fn poll(app: &mut App) {
    poll_repository_checks(app);
    ask(app);
    if app.packages.vc.current.is_none() {
        return;
    }
    for ev in app.packages.vc.processes.poll() {
        match ev {
            ProcessEvent::Output(id, text) => {
                if let Some(current) = app.packages.vc.current.as_mut().filter(|c| c.id == id) {
                    current.out.push_str(&text);
                }
                append(app, &text);
            }
            ProcessEvent::Stderr(_, text) => append(app, &text),
            ProcessEvent::Exited(id, code) => {
                let Some(current) = app.packages.vc.current.take() else {
                    continue;
                };
                if current.id != id {
                    app.packages.vc.current = Some(current);
                    continue;
                }
                if current.timed_out || code != Some(0) {
                    let message = if current.timed_out {
                        format!(
                            "the build timed out after {} s (see {BUFFER})",
                            current.job.policy.timeout.as_secs()
                        )
                    } else {
                        let what = match current.stage {
                            Stage::Fetch => "git couldn't get the repository",
                            Stage::Checkout => "git couldn't check out that revision",
                            Stage::Submodules => "git couldn't update submodules",
                            Stage::Commit => "git couldn't read the checked-out revision",
                            Stage::Review => "git couldn't list the changes",
                            Stage::Build => "the build failed",
                        };
                        format!("{what} (see {BUFFER})")
                    };
                    log_stage(&current.job, current.stage, Status::Failed, Some(&message));
                    fail(app, &current.job, &message);
                    continue;
                }
                finished(app, current.job, current.stage, current.out);
            }
        }
    }
    // Kill a build that outlived its deadline; its exit is reported by
    // the next poll.
    let expired = app.packages.vc.current.as_ref().and_then(|current| {
        current
            .deadline
            .filter(|deadline| !current.timed_out && Instant::now() >= *deadline)
            .map(|_| current.id)
    });
    if let Some(id) = expired {
        if let Some(current) = app.packages.vc.current.as_mut() {
            current.timed_out = true;
        }
        app.packages.vc.processes.kill(id);
    }
}

fn finished(app: &mut App, mut job: VcJob, stage: Stage, out: String) {
    match stage {
        Stage::Fetch => {
            log_stage(&job, Stage::Fetch, Status::Ok, None);
            #[cfg(windows)]
            {
                let target = job.rev.clone().unwrap_or_else(|| "origin/HEAD".into());
                let args = vec![
                    "checkout".to_string(),
                    "--quiet".into(),
                    "--detach".into(),
                    target,
                ];
                let r = spawn(app, Program::Argv("git", &args), job.src.clone());
                step(app, job, Stage::Checkout, r)
            }
            #[cfg(not(windows))]
            {
                // The rev as a remote branch first (fresh after a fetch), else a
                // tag or commit; no rev: the remote's default branch.
                let target = match &job.rev {
                Some(r) => {
                    let q = shell_quote(r);
                    format!("$(git rev-parse --verify -q origin/{q}^{{commit}} || git rev-parse --verify {q}^{{commit}})")
                }
                None => "$(git rev-parse --verify -q origin/HEAD^{commit} || git rev-parse --verify HEAD^{commit})".into(),
            };
                let script = format!(
                    "set -e; T={target}; git checkout --quiet --detach \"$T\"; \
                 git submodule --quiet update --init --recursive; git rev-parse HEAD"
                );
                let r = spawn(app, Program::Shell(&script), job.src.clone());
                step(app, job, Stage::Checkout, r);
            }
        }
        Stage::Checkout => {
            #[cfg(windows)]
            {
                let args = vec![
                    "submodule".to_string(),
                    "--quiet".into(),
                    "update".into(),
                    "--init".into(),
                    "--recursive".into(),
                ];
                let r = spawn(app, Program::Argv("git", &args), job.src.clone());
                step(app, job, Stage::Submodules, r)
            }
            #[cfg(not(windows))]
            {
                job.commit = out.lines().last().unwrap_or("").trim().to_string();
                checkout_complete(app, job);
            }
        }
        Stage::Submodules => {
            let args = vec!["rev-parse".to_string(), "HEAD".into()];
            let r = spawn(app, Program::Argv("git", &args), job.src.clone());
            step(app, job, Stage::Commit, r);
        }
        Stage::Commit => {
            job.commit = out.trim().to_string();
            checkout_complete(app, job);
        }
        Stage::Review => {
            request_upgrade_review(app, job, out);
        }
        Stage::Build => {
            log_stage(&job, Stage::Build, Status::Ok, None);
            install_output(app, job)
        }
    }
}

fn request_upgrade_review(app: &mut App, mut job: VcJob, output: String) {
    let breaking: Vec<_> = output
        .lines()
        .filter(|line| line.to_ascii_uppercase().contains("BREAKING"))
        .collect();
    let mut review = String::new();
    if !breaking.is_empty() {
        review.push_str("WARNING: breaking-change marker found in commit subjects.\n");
    }
    for warning in &job.history_warnings {
        review.push_str(warning);
        review.push('\n');
    }
    review.push_str(&output);
    append(
        app,
        &format!(
            "\nChanges in this upgrade ({}):\n{}",
            short(&job.commit),
            review
        ),
    );
    if let Some(id) = app.editor.find_buffer(BUFFER) {
        crate::compile::display_buffer(app, id);
    }
    job.review = Some(review);
    app.packages.vc.reviewing = Some(job);
    app.packages.vc.ask = Some((
        "Apply this Git package upgrade after reviewing the changelog? (y or n) ".into(),
        VcQuestion::ReviewUpgrade,
    ));
    ask(app);
}

#[derive(Debug, Default, PartialEq, Eq)]
struct HistoryWarnings {
    /// The new commit is not a descendant of the locked one.
    rewritten: bool,
    warnings: Vec<String>,
}

/// Check an upgrade's commit graph and pinned tag without changing any
/// checkout. Ancestry needs commit objects, so the Git implementation may
/// fetch them into a scratch repository.
fn history_warnings(
    git: &dyn GitRemote,
    repository: &str,
    checkout: Option<&Path>,
    rev: Option<&str>,
    old: Option<&str>,
    new: &str,
) -> HistoryWarnings {
    let mut history = HistoryWarnings::default();
    let Some(old) = old.filter(|old| !old.is_empty() && *old != new) else {
        return history;
    };
    match git.is_ancestor(repository, checkout, old, new) {
        Ok(true) => {}
        Ok(false) => {
            history.rewritten = true;
            history.warnings.push(format!(
                "WARNING: {} is not a descendant of the locked commit {}; upstream history was rewritten or force-pushed.",
                short(new),
                short(old)
            ));
        }
        Err(error) => history.warnings.push(format!(
            "WARNING: couldn't verify that {} descends from the locked commit {}: {error}",
            short(new),
            short(old)
        )),
    }
    if let Some(rev) = rev {
        let tag = rev.strip_prefix("refs/tags/").unwrap_or(rev);
        match git.tag_commit(repository, tag) {
            Ok(Some(target)) if target != old => history.warnings.push(format!(
                "WARNING: tag {tag} now points to {} instead of the locked commit {}; the tag was moved.",
                short(&target),
                short(old)
            )),
            Ok(_) => {}
            Err(error) => history.warnings.push(format!(
                "WARNING: couldn't check whether tag {tag} moved: {error}"
            )),
        }
    }
    history
}

fn resolve_build_plan(
    root: &std::path::Path,
    name: &str,
    build: Option<&str>,
    features: &[String],
) -> Result<BuildPlan, String> {
    let planned = package::build_plan(root, name);
    match (planned, build) {
        (Ok(mut plan), Some(command)) => {
            plan.command = Some(command.to_string());
            plan.output = root
                .join(".forge-build")
                .join(format!("{}.wasm", plan.name));
            plan.how = "make in the declaration".into();
            Ok(plan)
        }
        (Err(_), Some(command)) => Ok(package::BuildPlan {
            name: name.to_string(),
            command: Some(command.to_string()),
            output: root.join(".forge-build").join(format!("{name}.wasm")),
            how: "make in the declaration".into(),
        }),
        (Ok(mut plan), None) => {
            if !features.is_empty()
                && plan.how == "Cargo.toml"
                && let Some(command) = plan.command.as_mut()
            {
                command.push_str(&format!(" --features {}", shell_quote(&features.join(","))));
            }
            Ok(plan)
        }
        (Err(error), None) => Err(error),
    }
}

fn checkout_complete(app: &mut App, mut job: VcJob) {
    if let Some(expected) = &job.expected_commit
        && *expected != job.commit
    {
        return fail(
            app,
            &job,
            &format!(
                "forge-lock.toml pins {}, but the checkout is {}; no changes were made",
                short(expected),
                short(&job.commit)
            ),
        );
    }
    let installed = db(app).and_then(|d| d.packages.get(&job.name).cloned());
    if job.upgrade
        && installed.as_ref().and_then(|i| i.commit.as_deref()) == Some(job.commit.as_str())
    {
        app.msg(format!(
            "{} is up to date ({})",
            job.name,
            short(&job.commit)
        ));
        app.packages.vc.current = None;
        return next(app);
    }
    let root = job.root();
    let plan = match resolve_build_plan(&root, &job.name, job.build.as_deref(), &job.features) {
        Ok(plan) => plan,
        Err(error) => return fail(app, &job, &error),
    };
    job.name = plan.name.clone();
    job.plan = Some(plan);
    // Upgrading a declared plugin: review the commits since the
    // locked one before building (a changed permission set asks
    // again when it loads). Check the commit graph first: it may need a
    // fetch, but only into a scratch repository.
    let old = installed
        .as_ref()
        .and_then(|record| record.commit.clone())
        .or_else(|| locked_commit(&job.name))
        .filter(|old| *old != job.commit);
    if job.upgrade
        && let Some(old) = old.as_deref()
    {
        let history = history_warnings(
            app.packages.vc.git.as_ref(),
            &job.repo,
            Some(&job.src),
            job.rev.as_deref(),
            Some(old),
            &job.commit,
        );
        job.rewritten = history.rewritten;
        job.history_warnings = history.warnings;
        if app.unattended && !job.history_warnings.is_empty() {
            append(app, &format!("\n{}\n", job.history_warnings.join("\n")));
        }
        if app.unattended && job.rewritten && !job.allow_rewrite {
            let why = format!(
                "{}: upstream history was rewritten ({} is not a descendant of the locked commit {}); no changes were made. Set allow_history_rewrite: true in the declaration to allow this unattended.",
                job.name,
                short(&job.commit),
                short(old)
            );
            return fail(app, &job, &why);
        }
    }
    if !app.unattended
        && job.upgrade
        && let Some(old) = old
    {
        let args = vec![
            "log".to_string(),
            "--oneline".to_string(),
            "--no-decorate".to_string(),
            format!("{old}..{}", job.commit),
        ];
        let r = spawn(app, Program::Argv("git", &args), job.src.clone());
        return step(app, job, Stage::Review, r);
    }
    choose_build(app, job, installed);
}

/// The locked commit of a declared plugin, if any (forge-lock.toml).
fn locked_commit(name: &str) -> Option<String> {
    let dir = forge_core::config::config_dir()?;
    forge_core::LockFile::load(&dir)
        .ok()?
        .get(name)
        .and_then(|e| e.commit.clone())
}

/// Decide how to build a checked-out job: install it if there's no build
/// command, build it silently if the same command was agreed to before,
/// else ask. An upgrade that was reviewed always asks, showing the
/// commits first.
fn choose_build(app: &mut App, job: VcJob, installed: Option<Installed>) {
    let command = job.plan.as_ref().and_then(|p| p.command.clone());
    let new_recipe = command.as_deref().map(|command| {
        forge_core::package::BuildRecipe::for_checkout(command, &job.features, &job.root())
    });
    let old_recipe = installed
        .as_ref()
        .and_then(|installed| installed.recipe.clone())
        .or_else(|| locked_recipe(&job.name));
    let mut recipe_changes = match (&old_recipe, &new_recipe) {
        (Some(old), Some(new)) => old.diff(new),
        _ => Vec::new(),
    };
    let hash_changed = new_recipe
        .as_ref()
        .is_some_and(|new| locked_recipe_hash(&job.name).is_some_and(|old| old != new.hash()));
    if recipe_changes.is_empty() && hash_changed {
        recipe_changes
            .push("build recipe changed (command, toolchain, features or target)".to_string());
    }
    let same_command = installed
        .as_ref()
        .and_then(|installed| installed.build.as_deref())
        == command.as_deref();
    match command {
        None => {
            if !recipe_changes.is_empty() {
                append(
                    app,
                    &format!(
                        "\nBuild recipe changed (no build command now):\n  {}\n",
                        recipe_changes.join("\n  ")
                    ),
                );
            }
            install_output(app, job);
        }
        // Already agreed to this very recipe for this package, and this
        // isn't a reviewed upgrade.
        Some(_) if job.review.is_none() && same_command && recipe_changes.is_empty() => {
            build(app, job)
        }
        Some(c) => {
            if app.unattended {
                if app.packages.approvals.build {
                    app.msg(format!("Building {} as approved...", job.name));
                    return build(app, job);
                }
                let why = format!(
                    "{} needs to run `{c}` from {} to build; no changes were made. {}",
                    job.name,
                    job.repo,
                    crate::packages::Approvals::build_hint()
                );
                app.packages.approval_blocked = Some(why.clone());
                return fail(app, &job, &why);
            }
            if let Some(log) = &job.review {
                append(
                    app,
                    &format!("\nChanges in this upgrade ({}):\n{log}", short(&job.commit)),
                );
            }
            if !recipe_changes.is_empty() {
                append(
                    app,
                    &format!(
                        "\nBuild recipe changed:\n  {}\n",
                        recipe_changes.join("\n  ")
                    ),
                );
            }
            append(
                app,
                "\nBuild requires approval; answer y or n at the minibuffer prompt.\n",
            );
            let prompt = format!(
                "Build {} from {} with `{c}`? It runs code from the repository. (y or n) ",
                job.name, job.repo
            );
            app.packages.vc.confirming = Some(job);
            app.packages.vc.ask = Some((prompt, VcQuestion::Build));
            ask(app);
        }
    }
}

fn locked_recipe(name: &str) -> Option<forge_core::package::BuildRecipe> {
    let dir = forge_core::config::config_dir()?;
    forge_core::LockFile::load(&dir)
        .ok()?
        .get(name)?
        .recipe
        .clone()
}

fn locked_recipe_hash(name: &str) -> Option<String> {
    let dir = forge_core::config::config_dir()?;
    forge_core::LockFile::load(&dir)
        .ok()?
        .get(name)?
        .recipe_hash
        .clone()
}

/// Ask "build it?" once no other question is open.
fn ask(app: &mut App) {
    if app.minibuffer.is_none()
        && let Some((prompt, question)) = app.packages.vc.ask.take()
    {
        let action = match question {
            VcQuestion::Build => ConfirmAction::BuildPackage,
            VcQuestion::ReviewUpgrade => ConfirmAction::GitPackageUpgrade,
        };
        app.prompt(prompt, Purpose::Confirm(action));
    }
}

/// Is a "build it?" question waiting to be asked?
pub fn waiting_to_ask(app: &App) -> bool {
    app.packages.vc.ask.is_some()
}

/// The user said y to building.
pub fn confirmed(app: &mut App) {
    if let Some(job) = app.packages.vc.confirming.take() {
        build(app, job);
    }
}

pub fn reviewed_upgrade(app: &mut App) {
    if let Some(job) = app.packages.vc.reviewing.take() {
        let installed = db(app).and_then(|d| d.packages.get(&job.name).cloned());
        choose_build(app, job, installed);
    }
}

pub fn upgrade_review_declined(app: &mut App) {
    if let Some(job) = app.packages.vc.reviewing.take() {
        append(app, &format!("\nNot upgrading {} (declined).\n", job.name));
        app.msg(format!("Not upgrading {}", job.name));
        next(app);
    }
}

/// The user said n (or C-g).
pub fn declined(app: &mut App) {
    if let Some(job) = app.packages.vc.confirming.take() {
        append(app, &format!("\nNot building {} (declined).\n", job.name));
        app.msg(format!("Not building {}", job.name));
        next(app);
    }
}

pub fn confirming(app: &App) -> bool {
    app.packages.vc.confirming.is_some()
}

fn build(app: &mut App, mut job: VcJob) {
    let plan = job.plan.clone().expect("planned");
    let command = plan.command.clone().expect("a build command");
    if let Some(parent) = plan.output.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    app.msg(format!("Building {}...", job.name));
    if let Some(id) = app.editor.find_buffer(BUFFER) {
        crate::compile::display_buffer(app, id);
    }
    let build_started = Timer::start();
    let mut env = package::build_env(&job.policy, &package::process_env());
    env.push(("OUT".into(), plan.output.display().to_string()));
    let names: Vec<&str> = env.iter().map(|(name, _)| name.as_str()).collect();
    append(app, &format!("\nBuild environment: {}\n", names.join(", ")));
    #[cfg(windows)]
    let r = spawn_with_env(app, Program::Shell(&command), job.root(), &env);
    #[cfg(not(windows))]
    let r = {
        let script = format!(
            "OUT={}; export OUT; {command}",
            shell_quote(&plan.output.display().to_string())
        );
        spawn_with_env(app, Program::Shell(&script), job.root(), &env)
    };
    job.build_started = Some(build_started);
    step(app, job, Stage::Build, r);
}

fn capability_diff(
    name: &str,
    manifest: Option<&forge_core::Manifest>,
) -> Result<CapabilityDiff, String> {
    let Some(dir) = forge_core::config::config_dir() else {
        return Err("no config directory (set HOME or FORGE_CONFIG_DIR)".into());
    };
    let old = LockFile::load(&dir)
        .map_err(|e| format!("can't read the package lock: {e}"))?
        .get(name)
        .map(|entry| entry.capability_grants().clone())
        .unwrap_or_default();
    let new = manifest
        .map(|manifest| manifest.permissions.clone())
        .unwrap_or_default();
    Ok(CapabilityDiff::between(&old, &new))
}

fn capability_gate(
    app: &mut App,
    job: &VcJob,
    bytes: &[u8],
    manifest: Option<&forge_core::Manifest>,
) -> Result<bool, String> {
    if job.expected_artifact.is_some() {
        return Ok(true);
    }
    let diff = capability_diff(&job.name, manifest)?;
    if !diff.has_additions() {
        return Ok(true);
    }
    if app.unattended {
        if app.packages.approvals.capabilities {
            return Ok(true);
        }
        let error = format!(
            "it asks for new capabilities that need confirmation: {}. No changes were made. {}",
            diff.describe_additions().join("; "),
            crate::packages::Approvals::capabilities_hint()
        );
        app.packages.approval_blocked = Some(error.clone());
        return Err(error);
    }
    let mut text = format!("\nNew capabilities in {}:\n", job.name);
    for line in diff.describe_additions() {
        text.push_str(&format!("  {line}\n"));
    }
    if !diff.removals.is_empty() {
        text.push_str("\nCapabilities it no longer asks for:\n");
        for line in diff
            .removals
            .iter()
            .map(forge_core::CapabilityChange::describe)
        {
            text.push_str(&format!("  {line}\n"));
        }
    }
    text.push_str("\nThese additions need your approval before the upgrade is applied.\n");
    append(app, &text);
    app.packages.vc.capability = Some(PendingCapability {
        job: job.clone(),
        bytes: bytes.to_vec(),
        diff,
    });
    app.prompt(
        format!("Upgrade {} and allow new capabilities? (y or n) ", job.name),
        Purpose::Confirm(ConfirmAction::GitPackageCapabilities),
    );
    Ok(false)
}

pub fn capabilities_confirmed(app: &mut App, yes: bool) {
    let Some(pending) = app.packages.vc.capability.take() else {
        return;
    };
    if yes {
        app.msg(format!("Applying upgrade for {}", pending.job.name));
        install_ready_output(app, pending.job, pending.bytes);
    } else {
        append(
            app,
            &format!(
                "\nNot upgrading {} ({} new capabilit{} not approved).\n",
                pending.job.name,
                pending.diff.additions.len(),
                if pending.diff.additions.len() == 1 {
                    "y was"
                } else {
                    "ies were"
                }
            ),
        );
        app.msg(format!("Not upgrading {}", pending.job.name));
        next(app);
    }
}

fn install_output(app: &mut App, job: VcJob) {
    app.packages.vc.current = None;
    let plan = job.plan.clone().expect("planned");
    let bytes = match std::fs::read(&plan.output) {
        Ok(b) => b,
        Err(e) => {
            return fail(
                app,
                &job,
                &format!("no component at {}: {e}", plan.output.display()),
            );
        }
    };
    if let Some(expected) = &job.expected_artifact {
        let got = forge_core::package::sha256_hex(&bytes);
        if got != *expected {
            return fail(
                app,
                &job,
                &format!(
                    "forge-lock.toml pins artifact sha256 {expected}, but this build produced {got}; no changes were made"
                ),
            );
        }
    }
    let manifest = forge_core::permissions::Manifest::for_component(&plan.output, &bytes)
        .ok()
        .flatten();
    if let Ok(imports) = crate::packages::component_imports(&job.name, &bytes)
        && let Some(problem) =
            crate::packages::import_mismatch(&job.name, manifest.as_ref(), &imports)
    {
        return fail(app, &job, &problem);
    }
    if job.upgrade {
        match capability_gate(app, &job, &bytes, manifest.as_ref()) {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => return fail(app, &job, &error),
        }
    }
    install_ready_output(app, job, bytes);
}

fn install_ready_output(app: &mut App, job: VcJob, bytes: Vec<u8>) {
    let plan = job.plan.clone().expect("planned");
    if job.upgrade
        && let Err(error) = crate::packages::smoke_test_component(app, &job.name, &bytes)
    {
        return fail(app, &job, &error);
    }
    if job.upgrade
        && let Err(error) = crate::packages::prepare_update(app, &job.name)
    {
        return fail(app, &job, &error);
    }
    let manifest = forge_core::permissions::Manifest::for_component(&plan.output, &bytes)
        .ok()
        .flatten();
    let version = manifest
        .and_then(|m| m.version)
        .unwrap_or_else(|| "0.0.0".into());
    let recipe = plan.command.as_deref().map(|command| {
        forge_core::package::BuildRecipe::for_checkout(command, &job.features, &job.root())
    });
    let source = VcSource {
        git: job.repo.clone(),
        rev: job.rev.clone(),
        commit: job.commit.clone(),
        build: plan.command.clone(),
        recipe,
    };
    let started = Timer::start();
    let Some(mut db) = db(app) else {
        log_install(
            &job,
            &version,
            started,
            Status::Failed,
            Some("no plugins directory"),
        );
        return fail(app, &job, "no plugins directory");
    };
    match db.install_built(&job.name, &version, &source, &bytes) {
        Ok(path) => {
            log_install(&job, &version, started, Status::Ok, None);
            if let Err(error) = crate::declare::record_checked(app, &job.name, &path) {
                return fail(app, &job, &error);
            }
            if job.upgrade
                && !crate::packages::update_is_batch(app)
                && let Err(error) = crate::packages::commit_update(app)
            {
                return fail(app, &job, &format!("finishing update: {error}"));
            }
            append(
                app,
                &format!(
                    "\nInstalled {} {version} ({})\n",
                    job.name,
                    short(&job.commit)
                ),
            );
            app.msg(format!(
                "Installed {} {version} ({}) from {}",
                job.name,
                short(&job.commit),
                job.repo
            ));
            crate::plugins::load(app, &path);
        }
        Err(e) => {
            log_install(&job, &version, started, Status::Failed, Some(&e));
            return fail(app, &job, &e);
        }
    }
    next(app);
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(7)]
}

fn clear_output(app: &mut App) {
    let id = match app.editor.find_buffer(BUFFER) {
        Some(id) => id,
        None => app.editor.add_buffer(forge_core::Buffer::new(BUFFER)),
    };
    if let Some(b) = app.editor.state.get_buffer_mut(id) {
        b.set_contents("");
        b.set_major_mode("compilation-mode");
    }
}

fn append(app: &mut App, text: &str) {
    let Some(id) = app.editor.find_buffer(BUFFER) else {
        return;
    };
    crate::compile::append(app, id, text);
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::Editor;
    use std::time::Duration;

    fn job(timeout: Duration) -> VcJob {
        VcJob {
            name: "slow".into(),
            repo: "https://example.com/slow".into(),
            rev: None,
            dir: None,
            build: Some("sleep 30".into()),
            features: Vec::new(),
            policy: BuildPolicy {
                timeout,
                ..Default::default()
            },
            src: std::env::temp_dir(),
            upgrade: false,
            allow_rewrite: false,
            commit: String::new(),
            plan: None,
            review: None,
            history_warnings: Vec::new(),
            rewritten: false,
            expected_commit: None,
            expected_artifact: None,
            fetch_started: None,
            build_started: None,
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_build_past_its_deadline_is_killed_and_reported_as_timed_out() {
        let mut app = App::new(Editor::default());
        clear_output(&mut app);
        let dir = std::env::temp_dir();
        let id = app
            .packages
            .vc
            .processes
            .spawn_env(Program::Shell("sleep 30"), &dir, false, &[])
            .unwrap();
        let timeout = Duration::from_secs(1);
        app.packages.vc.current = Some(Current {
            job: job(timeout),
            stage: Stage::Build,
            id,
            out: String::new(),
            deadline: Some(Instant::now() + Duration::from_millis(50)),
            timed_out: false,
        });
        let started = Instant::now();
        while app.packages.vc.current.is_some() && started.elapsed() < Duration::from_secs(10) {
            poll(&mut app);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(app.packages.vc.current.is_none(), "still running");
        let id = app.editor.find_buffer(BUFFER).expect("build buffer");
        let text = app
            .editor
            .state
            .get_buffer(id)
            .map(|buffer| buffer.get_contents())
            .unwrap_or_default();
        assert!(text.contains("timed out"), "{text:?}");
    }

    #[derive(Default)]
    struct FakeRepositoryInfo {
        calls: std::sync::atomic::AtomicUsize,
        metadata: forge_core::package_source::RepositoryMetadata,
    }

    impl RepositoryInfo for FakeRepositoryInfo {
        fn inspect(
            &self,
            _repository: &str,
        ) -> Result<forge_core::package_source::RepositoryMetadata, String> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(self.metadata.clone())
        }
    }

    #[test]
    fn repository_checks_are_off_by_default_and_advisory() {
        let mut app = App::new(Editor::default());
        let info = Arc::new(FakeRepositoryInfo::default());
        app.packages.vc.set_repository_info(info.clone());
        start_repository_check(&mut app, "demo", "https://github.com/old/repo");
        assert!(app.packages.vc.repository_checks_pending.is_empty());
        assert_eq!(
            info.calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the default is off"
        );

        app.packages.vc.set_repository_checks(true);
        clear_output(&mut app);
        let info = Arc::new(FakeRepositoryInfo {
            calls: Default::default(),
            metadata: forge_core::package_source::RepositoryMetadata {
                archived: Some(true),
                canonical: Some("new/name".into()),
            },
        });
        app.packages.vc.set_repository_info(info.clone());
        start_repository_check(&mut app, "demo", "https://github.com/old/repo");
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        while !app.packages.vc.repository_checks_pending.is_empty() && Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
            poll_repository_checks(&mut app);
        }
        assert!(app.packages.vc.repository_checks_pending.is_empty());
        assert_eq!(info.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        let text = app
            .editor
            .state
            .get_buffer(app.editor.find_buffer(BUFFER).unwrap())
            .unwrap()
            .get_contents();
        assert!(text.contains("archived"), "{text}");
        assert!(text.contains("owner or name changed"), "{text}");
    }

    #[derive(Default)]
    struct FakeGit {
        ancestor: Option<bool>,
        tag: Option<String>,
    }

    impl GitRemote for FakeGit {
        fn remote_commit(&self, _repository: &str, _rev: Option<&str>) -> Result<String, String> {
            Ok(String::new())
        }

        fn build_plan_at(
            &self,
            _repository: &str,
            _commit: &str,
            _name: &str,
            _dir: Option<&str>,
            _build: Option<&str>,
            _features: &[String],
        ) -> Result<BuildPlan, String> {
            Err("not used".into())
        }

        fn is_ancestor(
            &self,
            _repository: &str,
            _checkout: Option<&Path>,
            _old: &str,
            _new: &str,
        ) -> Result<bool, String> {
            self.ancestor.ok_or_else(|| "unknown history".into())
        }

        fn tag_commit(&self, _repository: &str, _rev: &str) -> Result<Option<String>, String> {
            Ok(self.tag.clone())
        }
    }

    #[test]
    fn a_fast_forward_has_no_history_warnings() {
        let git = FakeGit {
            ancestor: Some(true),
            tag: None,
        };
        let history = history_warnings(
            &git,
            "https://example.test/repo",
            None,
            Some("main"),
            Some("aaaaaaa"),
            "bbbbbbb",
        );
        assert_eq!(history, HistoryWarnings::default());
    }

    #[test]
    fn a_rewritten_history_is_flagged() {
        let git = FakeGit {
            ancestor: Some(false),
            tag: None,
        };
        let history = history_warnings(
            &git,
            "https://example.test/repo",
            None,
            Some("main"),
            Some("aaaaaaa"),
            "bbbbbbb",
        );
        assert!(history.rewritten);
        assert!(
            history.warnings[0].contains("not a descendant")
                && history.warnings[0].contains("rewritten"),
            "{:?}",
            history.warnings
        );
    }

    #[test]
    fn a_moved_tag_is_flagged_even_for_a_fast_forward() {
        let git = FakeGit {
            ancestor: Some(true),
            tag: Some("bbbbbbb".into()),
        };
        let history = history_warnings(
            &git,
            "https://example.test/repo",
            None,
            Some("v1"),
            Some("aaaaaaa"),
            "bbbbbbb",
        );
        assert!(!history.rewritten);
        assert!(
            history.warnings[0].contains("tag v1") && history.warnings[0].contains("was moved"),
            "{:?}",
            history.warnings
        );
    }

    #[test]
    fn an_ancestry_error_is_a_warning_not_a_rewrite() {
        let git = FakeGit::default();
        let history = history_warnings(
            &git,
            "https://example.test/repo",
            None,
            None,
            Some("aaaaaaa"),
            "bbbbbbb",
        );
        assert!(!history.rewritten);
        assert!(
            history.warnings[0].contains("couldn't verify"),
            "{:?}",
            history.warnings
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_build_stages_keep_the_inherited_environment() {
        let mut app = App::new(Editor::default());
        clear_output(&mut app);
        let id = spawn(
            &mut app,
            Program::Shell("printf '%s' \"$PATH\""),
            std::env::temp_dir(),
        )
        .unwrap();
        let events = app.packages.vc.processes.wait(id, Duration::from_secs(10));
        let output: String = events
            .iter()
            .filter_map(|event| match event {
                ProcessEvent::Output(_, text) | ProcessEvent::Stderr(_, text) => {
                    Some(text.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(output, std::env::var("PATH").unwrap());
    }
}
