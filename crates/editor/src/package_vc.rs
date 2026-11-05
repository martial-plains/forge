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
use forge_core::package::{self, BuildPlan, Installed, InstalledDb, VcSource};
use forge_core::process::{ProcessEvent, ProcessId, Processes, Program, shell_quote};
use std::collections::VecDeque;
use std::path::PathBuf;

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
    src: PathBuf,
    /// Upgrading: stop if the commit is the one installed.
    upgrade: bool,
    commit: String,
    plan: Option<BuildPlan>,
    /// The commits an upgrade brings (git log since the locked commit),
    /// shown before building it (see `forge-lock.toml`).
    review: Option<String>,
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
    Submodules,
    Commit,
    /// An upgrade of a declared plugin: list what changed before building.
    Review,
    Build,
}

#[derive(Default)]
pub struct Vc {
    processes: Processes,
    queue: VecDeque<VcJob>,
    /// The job being worked on, its running step and that step's stdout.
    current: Option<(VcJob, Stage, ProcessId, String)>,
    /// A job waiting for "build it? y or n".
    confirming: Option<VcJob>,
    /// That question, until the minibuffer is free to ask it.
    ask: Option<String>,
}

impl Vc {
    pub fn busy(&self) -> bool {
        self.processes.any_running()
    }

    pub fn has_question(&self) -> bool {
        self.ask.is_some()
    }
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
            if upgrade_all(app) == 0 {
                app.msg("No packages installed from version control");
            }
        }
        _ => return false,
    }
    true
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
    let job = VcJob {
        name: name.to_string(),
        repo: repo.to_string(),
        rev,
        dir,
        build: declared.as_ref().and_then(|u| u.build.clone()),
        features: declared.map(|u| u.features).unwrap_or_default(),
        src,
        upgrade,
        commit: String::new(),
        plan: None,
        review: None,
    };
    app.packages.vc.queue.push_back(job);
    next(app);
}

pub fn upgrade(app: &mut App, name: &str) {
    let Some(entry) = db(app).and_then(|d| d.from_vc().into_iter().find(|v| v.0 == name)) else {
        return app.error(format!("{name} wasn't installed from version control"));
    };
    install(app, &entry.0, &entry.1, entry.2, true);
}

/// Upgrade every package from version control; returns how many.
pub fn upgrade_all(app: &mut App) -> usize {
    let all = db(app).map(|d| d.from_vc()).unwrap_or_default();
    for (name, repo, rev) in &all {
        install(app, name, repo, rev.clone(), true);
    }
    all.len()
}

/// Start the next queued job, if nothing is running.
fn next(app: &mut App) {
    let vc = &mut app.packages.vc;
    if vc.current.is_some() || vc.confirming.is_some() {
        return;
    }
    let Some(job) = vc.queue.pop_front() else {
        return;
    };
    clear_output(app);
    let verb = if job.upgrade { "Updating" } else { "Getting" };
    app.msg(format!("{verb} {} from {}...", job.name, job.repo));
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
    step(app, job, Stage::Fetch, r);
}

fn spawn(app: &mut App, program: Program, dir: PathBuf) -> Result<ProcessId, String> {
    spawn_with_env(app, program, dir, &[])
}

fn spawn_with_env(
    app: &mut App,
    program: Program,
    dir: PathBuf,
    env: &[(String, String)],
) -> Result<ProcessId, String> {
    let line = match &program {
        Program::Shell(l) => format!("$ {l}\n"),
        Program::Argv(p, a) => format!("$ {p} {}\n", a.join(" ")),
    };
    append(app, &line);
    app.packages
        .vc
        .processes
        .spawn(program, &dir, false, env)
        .map_err(|e| e.to_string())
}

fn step(app: &mut App, job: VcJob, stage: Stage, r: Result<ProcessId, String>) {
    match r {
        Ok(id) => app.packages.vc.current = Some((job, stage, id, String::new())),
        Err(e) => fail(app, &job, &e),
    }
}

fn fail(app: &mut App, job: &VcJob, why: &str) {
    append(app, &format!("\n{why}\n"));
    if let Some(id) = app.editor.find_buffer(BUFFER) {
        crate::compile::display_buffer(app, id);
    }
    app.error(format!("Can't install {}: {why}", job.name));
    app.packages.vc.current = None;
    next(app);
}

/// Output and finished steps (idle time).
pub fn poll(app: &mut App) {
    ask(app);
    if app.packages.vc.current.is_none() {
        return;
    }
    for ev in app.packages.vc.processes.poll() {
        match ev {
            ProcessEvent::Output(id, text) => {
                if let Some((_, _, cur, out)) =
                    app.packages.vc.current.as_mut().filter(|c| c.2 == id)
                {
                    let _ = cur;
                    out.push_str(&text);
                }
                append(app, &text);
            }
            ProcessEvent::Stderr(_, text) => append(app, &text),
            ProcessEvent::Exited(id, code) => {
                let Some((job, stage, cur, out)) = app.packages.vc.current.take() else {
                    continue;
                };
                if cur != id {
                    app.packages.vc.current = Some((job, stage, cur, out));
                    continue;
                }
                if code != Some(0) {
                    let what = match stage {
                        Stage::Fetch => "git couldn't get the repository",
                        Stage::Checkout => "git couldn't check out that revision",
                        Stage::Submodules => "git couldn't update submodules",
                        Stage::Commit => "git couldn't read the checked-out revision",
                        Stage::Review => "git couldn't list the changes",
                        Stage::Build => "the build failed",
                    };
                    fail(app, &job, &format!("{what} (see {BUFFER})"));
                    continue;
                }
                finished(app, job, stage, out);
            }
        }
    }
}

fn finished(app: &mut App, mut job: VcJob, stage: Stage, out: String) {
    match stage {
        Stage::Fetch => {
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
            job.review = Some(out);
            let installed = db(app).and_then(|d| d.packages.get(&job.name).cloned());
            choose_build(app, job, installed);
        }
        Stage::Build => install_output(app, job),
    }
}

fn checkout_complete(app: &mut App, mut job: VcJob) {
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
    let planned = package::build_plan(&root, &job.name);
    let plan = match (planned, &job.build) {
        (Ok(mut p), Some(cmd)) => {
            p.command = Some(cmd.clone());
            p.output = root.join(".forge-build").join(format!("{}.wasm", p.name));
            p.how = "make in the declaration".into();
            p
        }
        (Err(_), Some(cmd)) => package::BuildPlan {
            name: job.name.clone(),
            command: Some(cmd.clone()),
            output: root.join(".forge-build").join(format!("{}.wasm", job.name)),
            how: "make in the declaration".into(),
        },
        (Ok(mut p), None) => {
            if !job.features.is_empty()
                && p.how == "Cargo.toml"
                && let Some(c) = p.command.as_mut()
            {
                c.push_str(&format!(
                    " --features {}",
                    shell_quote(&job.features.join(","))
                ));
            }
            p
        }
        (Err(e), None) => return fail(app, &job, &e),
    };
    job.name = plan.name.clone();
    job.plan = Some(plan);
    // Upgrading a declared plugin: review the commits since the
    // locked one before building (a changed permission set asks
    // again when it loads).
    if job.upgrade
        && let Some(old) = locked_commit(&job.name).filter(|o| *o != job.commit)
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
        .get(name)
        .and_then(|e| e.commit.clone())
}

/// Decide how to build a checked-out job: install it if there's no build
/// command, build it silently if the same command was agreed to before,
/// else ask. An upgrade that was reviewed always asks, showing the
/// commits first.
fn choose_build(app: &mut App, job: VcJob, installed: Option<Installed>) {
    let command = job.plan.as_ref().and_then(|p| p.command.clone());
    match command {
        None => install_output(app, job),
        // Already agreed to this very command for this package, and this
        // isn't a reviewed upgrade.
        Some(c)
            if job.review.is_none()
                && installed.and_then(|i| i.build).as_deref() == Some(c.as_str()) =>
        {
            build(app, job)
        }
        Some(c) => {
            if let Some(log) = &job.review {
                append(
                    app,
                    &format!("\nChanges in this upgrade ({}):\n{log}", short(&job.commit)),
                );
            }
            let prompt = format!(
                "Build {} from {} with `{c}`? It runs code from the repository. (y or n) ",
                job.name, job.repo
            );
            app.packages.vc.confirming = Some(job);
            app.packages.vc.ask = Some(prompt);
            ask(app);
        }
    }
}

/// Ask "build it?" once no other question is open.
fn ask(app: &mut App) {
    if app.minibuffer.is_none()
        && let Some(prompt) = app.packages.vc.ask.take()
    {
        app.prompt(prompt, Purpose::Confirm(ConfirmAction::BuildPackage));
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

/// The user said n (or C-g).
pub fn declined(app: &mut App) {
    if let Some(job) = app.packages.vc.confirming.take() {
        app.msg(format!("Not building {}", job.name));
        next(app);
    }
}

pub fn confirming(app: &App) -> bool {
    app.packages.vc.confirming.is_some()
}

fn build(app: &mut App, job: VcJob) {
    let plan = job.plan.clone().expect("planned");
    let command = plan.command.clone().expect("a build command");
    if let Some(parent) = plan.output.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    app.msg(format!("Building {}...", job.name));
    if let Some(id) = app.editor.find_buffer(BUFFER) {
        crate::compile::display_buffer(app, id);
    }
    #[cfg(windows)]
    let r = spawn_with_env(
        app,
        Program::Shell(&command),
        job.root(),
        &[("OUT".into(), plan.output.display().to_string())],
    );
    #[cfg(not(windows))]
    let r = {
        let script = format!(
            "OUT={}; export OUT; {command}",
            shell_quote(&plan.output.display().to_string())
        );
        spawn(app, Program::Shell(&script), job.root())
    };
    step(app, job, Stage::Build, r);
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
    let manifest = forge_core::permissions::Manifest::for_component(&plan.output, &bytes)
        .ok()
        .flatten();
    let version = manifest
        .and_then(|m| m.version)
        .unwrap_or_else(|| "0.0.0".into());
    let source = VcSource {
        git: job.repo.clone(),
        rev: job.rev.clone(),
        commit: job.commit.clone(),
        build: plan.command.clone(),
    };
    let Some(mut db) = db(app) else {
        return fail(app, &job, "no plugins directory");
    };
    match db.install_built(&job.name, &version, &source, &bytes) {
        Ok(path) => {
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
            crate::declare::record(app, &job.name, &path);
            crate::plugins::load(app, &path);
        }
        Err(e) => return fail(app, &job, &e),
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
