// SPDX-License-Identifier: MIT OR Apache-2.0
//! `forge package sync`: install exactly what `init.rhai` declares, using
//! the commits and artifact hashes in `forge-lock.toml`.
//!
//! This is the headless counterpart of startup's `declare::resolve`: no
//! prompts, no interactive review. A locked package is fetched at its
//! locked commit (or url) when it isn't in the artifact cache, and the
//! result must match the locked sha256 before anything is installed.
//! Operations that would normally ask need an explicit approval flag or
//! `forge.opt` setting; otherwise sync stops before changing state.

use crate::app::App;
use forge_core::{LockEntry, LockFile, PluginUse};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub json: bool,
    pub allow_build: bool,
    pub allow_capabilities: bool,
    pub allow_grants: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Failed,
    Blocked,
}

#[derive(Serialize)]
struct Report {
    command: &'static str,
    status: &'static str,
    lockfile: Option<String>,
    packages: Vec<PackageReport>,
    errors: Vec<SyncError>,
}

#[derive(Serialize)]
struct PackageReport {
    name: String,
    action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
}

#[derive(Serialize)]
struct SyncError {
    message: String,
    blocked: bool,
}

impl SyncError {
    fn failed(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            blocked: false,
        }
    }

    fn blocked(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            blocked: true,
        }
    }
}

pub fn run(options: Options) -> anyhow::Result<Status> {
    let mut app = crate::package_doctor::headless_app()?;
    app.unattended = true;
    app.packages.approvals.build |= options.allow_build;
    app.packages.approvals.capabilities |= options.allow_capabilities;
    app.packages.approvals.grants |= options.allow_grants;
    app.packages.approval_blocked = None;

    let config = forge_core::config::config_dir();
    let lockfile = config
        .as_ref()
        .map(|dir| LockFile::path(dir).display().to_string());
    let mut errors = Vec::new();
    for error in &app.init_errors {
        errors.push(SyncError::failed(error.clone()));
    }
    if app.bell
        && let Some(message) = &app.message
    {
        errors.push(SyncError::failed(message.clone()));
    }

    let uses = declared_uses(&app);
    if uses.is_empty() {
        return finish(app, options.json, lockfile, errors, BTreeSet::new());
    }
    if let Some(dir) = &config
        && let Err(error) = LockFile::load(dir)
    {
        errors.push(SyncError::failed(format!(
            "cannot read forge-lock.toml: {error}"
        )));
    }

    let original = load_lock(&config)
        .map(|lock| lock.shared_packages().clone())
        .unwrap_or_default();
    let mut lock = match load_lock(&config) {
        Ok(lock) => lock,
        Err(error) => {
            errors.push(SyncError::failed(format!(
                "cannot read forge-lock.toml: {error}"
            )));
            return finish(app, options.json, lockfile, errors, BTreeSet::new());
        }
    };

    let needs_index = uses.iter().any(|u| {
        lock.get(&u.name).is_none()
            && u.index.is_some()
            && declared_path(&app, &u.name).is_none_or(|path| !path.exists())
    });
    if needs_index && app.packages.index.is_none() {
        crate::packages::refresh(&mut app, crate::packages::Then::Nothing);
        if let Some(prompt) = drive(&mut app, &mut errors) {
            hold(&mut errors, prompt);
        }
    }

    let mut changed = BTreeSet::new();
    for u in &uses {
        match start(&mut app, u, lock.get(&u.name), &config) {
            Ok(true) => {
                changed.insert(u.name.clone());
            }
            Ok(false) => {}
            Err(error) => errors.push(SyncError::failed(error)),
        }
    }

    if let Some(prompt) = drive(&mut app, &mut errors) {
        hold(&mut errors, prompt);
    }
    if let Some(error) = app.packages.approval_blocked.clone() {
        hold(&mut errors, error);
    }

    lock = match load_lock(&config) {
        Ok(lock) => lock,
        Err(error) => {
            errors.push(SyncError::failed(format!(
                "cannot read forge-lock.toml after syncing: {error}"
            )));
            return finish(app, options.json, lockfile, errors, changed);
        }
    };

    for u in &uses {
        let Some(path) = declared_path(&app, &u.name) else {
            errors.push(SyncError::failed(format!(
                "{}: no plugin directory (set HOME, FORGE_CONFIG_DIR or load_path)",
                u.name
            )));
            continue;
        };
        if !path.exists() {
            errors.push(SyncError::failed(format!(
                "{} wasn't installed at {}",
                u.name,
                path.display()
            )));
            continue;
        }
        let got = match hash_file(&path) {
            Ok(hash) => hash,
            Err(error) => {
                errors.push(SyncError::failed(format!("{}: {error}", u.name)));
                continue;
            }
        };
        let Some(entry) = lock.get(&u.name) else {
            errors.push(SyncError::failed(format!(
                "{}: no forge-lock.toml entry after syncing",
                u.name
            )));
            continue;
        };
        let expected = artifact_hash(entry);
        if got != expected {
            errors.push(SyncError::failed(format!(
                "{}: installed sha256 {got}, forge-lock.toml says {expected}",
                u.name
            )));
        }
    }

    // Installs record themselves in the lock. Sync must not move a locked
    // package's branch/tag or recipe just because it ran: put the original
    // shared entries back after the artifact cache has been written.
    if !original.is_empty() {
        for (name, entry) in &original {
            if lock.get(name) != Some(entry) {
                lock.set(name, entry.clone());
            }
        }
        if let Err(error) = lock.save() {
            errors.push(SyncError::failed(format!(
                "cannot restore forge-lock.toml entries: {error}"
            )));
        }
    }

    finish(app, options.json, lockfile, errors, changed)
}

fn declared_uses(app: &App) -> Vec<PluginUse> {
    app.rhai
        .as_ref()
        .map(|rhai| {
            let uses = rhai.plugin_uses();
            let by_name: BTreeMap<&str, &PluginUse> =
                uses.iter().map(|u| (u.name.as_str(), u)).collect();
            forge_core::declare::order(&uses)
                .into_iter()
                .filter_map(|name| by_name.get(name.as_str()).map(|u| (*u).clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn declared_path(app: &App, name: &str) -> Option<std::path::PathBuf> {
    crate::declare::declared_component_path(app, name)
}

fn load_lock(config: &Option<std::path::PathBuf>) -> Result<LockFile, String> {
    let Some(dir) = config else {
        return Ok(LockFile::in_memory());
    };
    LockFile::load(dir).map_err(|error| error.to_string())
}

/// Start one declaration. True means bytes were installed or restored.
fn start(
    app: &mut App,
    use_spec: &PluginUse,
    entry: Option<&LockEntry>,
    config: &Option<std::path::PathBuf>,
) -> Result<bool, String> {
    let name = &use_spec.name;
    let Some(path) = declared_path(app, name) else {
        return Err(format!(
            "{name}: no plugin directory (set HOME, FORGE_CONFIG_DIR or load_path)"
        ));
    };
    let existing = path.exists().then(|| hash_file(&path)).transpose()?;
    let Some(entry) = entry else {
        if existing.is_some() {
            crate::declare::record(app, name, &path);
            return Ok(false);
        }
        crate::declare::install(app, use_spec);
        return Ok(true);
    };
    let expected = artifact_hash(entry);
    if existing.as_deref() == Some(expected) {
        return Ok(false);
    }
    if let Some(dir) = config
        && let Ok(bytes) = LockFile::artifact(dir, expected)
    {
        crate::packages::restore_locked_entry(app, name, entry, &bytes)?;
        return Ok(true);
    }
    if let Some(git) = entry.git.as_deref().or(use_spec.git.as_deref()) {
        let Some(commit) = entry.commit.as_deref() else {
            return Err(format!(
                "{name}: forge-lock.toml has no commit for the locked git source"
            ));
        };
        crate::package_vc::install_locked(app, name, git, commit, expected);
        return Ok(true);
    }
    if let Some(url) = entry.url.as_deref().or(use_spec.url.as_deref()) {
        crate::packages::install_url(app, name, url, expected);
        return Ok(true);
    }
    if let Some(index_name) = use_spec.index.as_deref() {
        let Some(index) = app.packages.index.as_ref() else {
            return Err(format!(
                "{name}: no package index is loaded for {index_name}"
            ));
        };
        let package = index
            .resolve(index_name, use_spec.version.as_deref())
            .map_err(|error| format!("{name}: {error}"))?
            .ok_or_else(|| format!("{name}: no package {index_name} in the index"))?
            .clone();
        if let Some(git) = package.git.as_deref() {
            let Some(commit) = entry.commit.as_deref() else {
                return Err(format!(
                    "{name}: forge-lock.toml has no commit for the locked git source"
                ));
            };
            crate::package_vc::install_locked(app, name, git, commit, expected);
        } else {
            crate::packages::install_url(app, name, &package.url, expected);
        }
        return Ok(true);
    }
    Err(format!(
        "{name}: forge-lock.toml entry has no git or url source"
    ))
}

fn drive(app: &mut App, errors: &mut Vec<SyncError>) -> Option<String> {
    let mut seen = app.message.clone();
    loop {
        if let Some(prompt) = waiting_prompt(app) {
            return Some(prompt);
        }
        if app.packages.needs_poll() {
            app.on_idle();
            collect(app, errors, &mut seen);
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        app.on_idle();
        collect(app, errors, &mut seen);
        if let Some(prompt) = waiting_prompt(app) {
            return Some(prompt);
        }
        if !app.packages.needs_poll() {
            return None;
        }
    }
}

fn waiting_prompt(app: &App) -> Option<String> {
    if let Some(prompt) = &app.minibuffer {
        return Some(format!(
            "a prompt is waiting: {}. No changes beyond what just finished were made.",
            prompt.prompt.trim()
        ));
    }
    if app.pending_grant.is_some() {
        return Some("a plugin permission prompt is waiting; no changes were made".into());
    }
    None
}

fn hold(errors: &mut Vec<SyncError>, message: String) {
    if let Some(error) = errors.iter_mut().find(|error| error.message == message) {
        error.blocked = true;
    } else {
        errors.push(SyncError::blocked(message));
    }
}

fn collect(app: &App, errors: &mut Vec<SyncError>, seen: &mut Option<String>) {
    let Some(message) = &app.message else {
        return;
    };
    if seen.as_deref() == Some(message.as_str()) {
        return;
    }
    *seen = Some(message.clone());
    if looks_like_error(message) && !errors.iter().any(|e| e.message == *message) {
        errors.push(SyncError::failed(message.clone()));
    }
}

fn looks_like_error(message: &str) -> bool {
    message.starts_with("Can't ")
        || message.starts_with("Cannot ")
        || message.starts_with("Failed ")
        || message.starts_with("error:")
        || message.starts_with("No package index")
        || (message.starts_with("Plugin ") && message.contains("not loaded"))
}

fn artifact_hash(entry: &LockEntry) -> &str {
    entry
        .artifact_sha256
        .as_deref()
        .unwrap_or(entry.sha256.as_str())
}

fn hash_file(path: &Path) -> Result<String, String> {
    let bytes =
        std::fs::read(path).map_err(|error| format!("reading {}: {error}", path.display()))?;
    Ok(forge_core::package::sha256_hex(&bytes))
}

fn finish(
    app: App,
    json: bool,
    lockfile: Option<String>,
    errors: Vec<SyncError>,
    changed: BTreeSet<String>,
) -> anyhow::Result<Status> {
    let lock = load_lock(&forge_core::config::config_dir()).unwrap_or_default();
    let packages = app
        .rhai
        .as_ref()
        .map(|rhai| {
            let uses = rhai.plugin_uses();
            let by_name: BTreeMap<&str, &PluginUse> =
                uses.iter().map(|u| (u.name.as_str(), u)).collect();
            forge_core::declare::order(&uses)
                .into_iter()
                .filter_map(|name| by_name.get(name.as_str()).map(|u| (*u).clone()))
                .map(|u| {
                    let path = declared_path(&app, &u.name);
                    let sha256 = path
                        .as_deref()
                        .filter(|path| path.exists())
                        .and_then(|path| hash_file(path).ok());
                    let commit = lock.get(&u.name).and_then(|e| e.commit.clone());
                    PackageReport {
                        action: if changed.contains(&u.name) {
                            "installed"
                        } else {
                            "unchanged"
                        },
                        name: u.name,
                        sha256,
                        commit,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let blocked =
        app.packages.approval_blocked.is_some() || errors.iter().any(|error| error.blocked);
    let status = if blocked {
        Status::Blocked
    } else if errors.is_empty() {
        Status::Ok
    } else {
        Status::Failed
    };
    if json {
        let report = Report {
            command: "sync",
            status: match status {
                Status::Ok => "ok",
                Status::Failed => "failed",
                Status::Blocked => "blocked",
            },
            lockfile,
            packages,
            errors,
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if status == Status::Ok {
        let installed = changed.len();
        let total = packages.len();
        println!(
            "Synced {total} package{} from forge-lock.toml ({installed} installed).",
            if total == 1 { "" } else { "s" }
        );
    } else {
        for error in &errors {
            eprintln!("forge package sync: {}", error.message);
        }
    }
    Ok(status)
}
