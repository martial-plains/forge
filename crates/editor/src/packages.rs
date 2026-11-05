// SPDX-License-Identifier: MIT OR Apache-2.0
//! Installing plugins from package indexes (forge_core::package):
//! M-x list-packages, package-install, package-delete,
//! package-upgrade-all, package-refresh-contents.
//!
//! Indexes come from `forge.opt.package_index` (one location or a list).
//! Local files are read at once; https:// ones, and the packages they
//! list, are downloaded with reqwest in the background. Downloads are checked
//! against the index's sha256, then the plugin loads as if installed by
//! hand (Forge asks for its permissions).

use crate::app::App;
use crate::minibuffer::{Choice, Purpose};
use forge_core::LockFile;
use forge_core::package::{self, Index, InstalledDb, Package};
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

pub const BUFFER: &str = "*Packages*";

/// What to do once the indexes are in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Then {
    Nothing,
    List,
    Manager,
    Install,
    UpgradeAll,
    UpgradeDryRun,
    UpgradeOne(String),
}

enum Fetch {
    Index {
        location: String,
        file: PathBuf,
    },
    Package {
        package: Package,
        file: PathBuf,
        upgrade: bool,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct PackageBackup {
    artifact: Option<Vec<u8>>,
    installed: Option<forge_core::package::Installed>,
    locked: Option<forge_core::LockEntry>,
}

pub(crate) fn capture_package_backup(app: &App, name: &str) -> Result<PackageBackup, String> {
    let dir = app
        .packages
        .plugin_dir
        .clone()
        .or_else(forge_core::config::plugin_dir)
        .ok_or_else(|| "no plugins directory".to_string())?;
    let installed = InstalledDb::load(&dir);
    let artifact = match std::fs::read(installed.plugin_file(name)) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("{name}: reading current artifact: {error}")),
    };
    let locked = forge_core::config::config_dir()
        .map(|config| LockFile::load(&config))
        .and_then(|lock| lock.get(name).cloned());
    Ok(PackageBackup {
        artifact,
        installed: installed.packages.get(name).cloned(),
        locked,
    })
}

pub(crate) fn restore_package_backup(
    app: &App,
    name: &str,
    backup: PackageBackup,
) -> Result<(), String> {
    let dir = app
        .packages
        .plugin_dir
        .clone()
        .or_else(forge_core::config::plugin_dir)
        .ok_or_else(|| "no plugins directory".to_string())?;
    let mut installed = InstalledDb::load(&dir);
    let path = installed.plugin_file(name);
    match backup.artifact {
        Some(bytes) => forge_core::fsutil::write_atomic(&path, bytes)
            .map_err(|error| format!("{name}: restoring artifact: {error}"))?,
        None => match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("{name}: removing failed artifact: {error}")),
        },
    }
    if let Some(record) = backup.installed {
        installed.packages.insert(name.to_string(), record);
    } else {
        installed.packages.remove(name);
    }
    installed
        .save()
        .map_err(|error| format!("{name}: {error}"))?;

    if let Some(config) = forge_core::config::config_dir() {
        let mut lock = LockFile::load(&config);
        if let Some(entry) = backup.locked {
            lock.set(name, entry);
        } else {
            lock.remove(name);
        }
        lock.save()
            .map_err(|error| format!("{name}: restoring lock entry: {error}"))?;
    }
    Ok(())
}

#[derive(Default)]
pub struct Packages {
    pub indexes: Vec<String>,
    /// Where plugins are installed (None: the default plugins directory).
    pub plugin_dir: Option<PathBuf>,
    pub index: Option<Index>,
    manager_filter: String,
    manager_selected: Option<String>,
    /// Downloads under way: each on its own thread, answering here.
    downloads: Vec<(u64, Receiver<Result<(), String>>)>,
    next_download: u64,
    fetches: BTreeMap<u64, Fetch>,
    upgrade_queue: VecDeque<Package>,
    pending_upgrade: Option<Package>,
    upgrade_git_after_index: bool,
    /// Indexes read so far in a refresh, and what to do after.
    refreshing: Option<(Index, Vec<String>, Then)>,
    /// Packages from version control (package_vc.rs).
    pub vc: crate::package_vc::Vc,
    /// Tree-sitter grammar installs (grammar_install.rs).
    pub grammar: crate::grammar_install::GrammarInstall,
}

impl Packages {
    pub fn busy(&self) -> bool {
        !self.downloads.is_empty() || self.vc.busy() || self.grammar.busy()
    }

    /// Something to poll for when idle (downloads, builds, a question).
    pub fn needs_poll(&self) -> bool {
        self.busy() || self.vc.has_question()
    }

    fn dir(&self) -> Option<PathBuf> {
        self.plugin_dir
            .clone()
            .or_else(forge_core::config::plugin_dir)
    }
}

fn db(app: &App) -> Option<InstalledDb> {
    app.packages.dir().map(|d| InstalledDb::load(&d))
}

pub fn execute(app: &mut App, cmd: &str) -> bool {
    match cmd {
        "package-manager" => {
            if app.packages.index.is_none() && !app.packages.indexes.is_empty() {
                with_index(app, Then::Manager);
            } else {
                manager(app);
            }
        }
        "package-manager-next" => manager_move(app, 1),
        "package-manager-prev" => manager_move(app, -1),
        "package-manager-filter" => app.prompt(
            "Filter packages: ",
            crate::minibuffer::Purpose::PackageFilter,
        ),
        "package-manager-clear-filter" => {
            app.packages.manager_filter.clear();
            manager(app);
        }
        "list-packages" | "package-list-packages" => with_index(app, Then::List),
        "package-install" => with_index(app, Then::Install),
        "package-upgrade-all" if app.raw_arg() => with_index(app, Then::UpgradeDryRun),
        "package-upgrade-all" => with_index(app, Then::UpgradeAll),
        "package-upgrade" => {
            if app.raw_arg() {
                with_index(app, Then::UpgradeDryRun);
                return true;
            }
            let names: Vec<String> = db(app)
                .map(|d| d.packages.keys().cloned().collect())
                .unwrap_or_default();
            if names.is_empty() {
                app.msg("No packages are installed");
            } else {
                app.prompt(
                    "Upgrade package: ",
                    Purpose::Choose {
                        what: Choice::UpgradePackage,
                        candidates: names,
                    },
                );
            }
        }
        "package-upgrade-all-dry-run" => with_index(app, Then::UpgradeDryRun),
        "package-refresh-contents" => refresh(app, Then::Nothing),
        "package-snapshot-save" => app.prompt(
            "Save package snapshot: ",
            Purpose::PackageSnapshot(crate::minibuffer::PackageSnapshotAction::Save),
        ),
        "package-snapshot-list" => show_snapshots(app),
        "package-snapshot-diff" => app.prompt(
            "Diff from snapshot: ",
            Purpose::PackageSnapshot(crate::minibuffer::PackageSnapshotAction::DiffFrom),
        ),
        "package-snapshot-restore" => app.prompt(
            "Restore package snapshot: ",
            Purpose::PackageSnapshot(crate::minibuffer::PackageSnapshotAction::Restore),
        ),
        "package-restore" => restore_locked(app),
        "package-delete" => {
            let names: Vec<String> = db(app)
                .map(|d| d.packages.keys().cloned().collect())
                .unwrap_or_default();
            if names.is_empty() {
                app.error("No packages installed from an index");
            } else {
                app.prompt(
                    "Delete package: ",
                    Purpose::Choose {
                        what: Choice::DeletePackage,
                        candidates: names,
                    },
                );
            }
        }
        "package-menu-install" => {
            if let Some(name) = selected_name(app) {
                install_selected(app, &name);
            }
        }
        "package-menu-upgrade" => {
            if let Some(name) = selected_name(app) {
                upgrade_named(app, &name);
            }
        }
        "package-menu-view" => {
            if let Some(name) = selected_name(app) {
                show_details(app, &name);
            }
        }
        "package-menu-build-log" => {
            if let Some(id) = app.editor.find_buffer(crate::package_vc::BUFFER) {
                app.switch_to(id);
            } else {
                app.msg("No package build log yet");
            }
        }
        "package-menu-delete" => {
            if let Some(name) = selected_name(app) {
                delete(app, &name);
            }
        }
        "package-menu-refresh" => {
            let then = if app.buf_ref().major_mode() == "package-manager-mode" {
                Then::Manager
            } else {
                Then::List
            };
            refresh(app, then);
        }
        _ if crate::grammar_install::execute(app, cmd) => {}
        _ => return crate::package_vc::execute(app, cmd),
    }
    true
}

pub fn answer_snapshot_prompt(
    app: &mut App,
    action: crate::minibuffer::PackageSnapshotAction,
    input: &str,
) {
    let name = input.trim();
    if name.is_empty() {
        return;
    }
    match action {
        crate::minibuffer::PackageSnapshotAction::Save => save_snapshot(app, name),
        crate::minibuffer::PackageSnapshotAction::DiffFrom => app.prompt(
            "Diff to snapshot: ",
            Purpose::PackageSnapshot(crate::minibuffer::PackageSnapshotAction::DiffTo(
                name.to_string(),
            )),
        ),
        crate::minibuffer::PackageSnapshotAction::DiffTo(before) => {
            show_snapshot_diff(app, &before, name)
        }
        crate::minibuffer::PackageSnapshotAction::Restore => restore_snapshot(app, name),
    }
}

fn config_dir() -> Option<PathBuf> {
    forge_core::config::config_dir()
}

fn save_snapshot(app: &mut App, name: &str) {
    let (Some(_config), Some(plugins)) = (config_dir(), app.packages.dir()) else {
        return app.error("Cannot save a package snapshot without config and plugin directories");
    };
    match crate::declare::load_lock().save_snapshot(name, &plugins) {
        Ok(()) => app.msg(format!("Saved package snapshot {name}")),
        Err(e) => app.error(format!("Couldn't save package snapshot {name}: {e}")),
    }
}

fn show_snapshots(app: &mut App) {
    let Some(config) = config_dir() else {
        return app.error("No config directory for package snapshots");
    };
    match LockFile::snapshots(&config) {
        Ok(names) if names.is_empty() => app.msg("No package snapshots"),
        Ok(names) => {
            let body = format!("Package snapshots\n\n{}\n", names.join("\n"));
            show_package_text(app, "*Package Snapshots*", &body);
        }
        Err(e) => app.error(format!("Couldn't list package snapshots: {e}")),
    }
}

fn show_snapshot_diff(app: &mut App, before: &str, after: &str) {
    let Some(config) = config_dir() else {
        return app.error("No config directory for package snapshots");
    };
    match LockFile::diff_snapshots(&config, before, after) {
        Ok(changes) => {
            let mut body = format!("Package snapshot diff: {before} -> {after}\n\n");
            if changes.is_empty() {
                body.push_str("No changes\n");
            }
            for change in changes {
                let old = change
                    .before
                    .as_ref()
                    .map(lock_revision)
                    .unwrap_or_else(|| "-".into());
                let new = change
                    .after
                    .as_ref()
                    .map(lock_revision)
                    .unwrap_or_else(|| "-".into());
                body.push_str(&format!(
                    "{} {}: {} -> {}\n",
                    change.change, change.name, old, new
                ));
            }
            show_package_text(app, "*Package Snapshot Diff*", &body);
        }
        Err(e) => app.error(format!("Couldn't compare package snapshots: {e}")),
    }
}

fn lock_revision(entry: &forge_core::LockEntry) -> String {
    entry
        .commit
        .as_deref()
        .or(entry.artifact_sha256.as_deref())
        .unwrap_or(&entry.sha256)
        .chars()
        .take(12)
        .collect()
}

fn show_package_text(app: &mut App, name: &str, text: &str) {
    let id = app.editor.create_buffer(name);
    if let Some(buffer) = app.editor.state.get_buffer_mut(id) {
        buffer.set_contents(text);
    }
    crate::compile::display_buffer(app, id);
}

fn restore_snapshot(app: &mut App, name: &str) {
    let (Some(config), Some(plugins)) = (config_dir(), app.packages.dir()) else {
        return app
            .error("Cannot restore a package snapshot without config and plugin directories");
    };
    if let Err(e) = LockFile::restore_snapshot(&config, &plugins, name) {
        return app.error(format!("Couldn't restore package snapshot {name}: {e}"));
    }
    let restored = LockFile::load(&config);
    if let Err(e) = sync_installed_metadata(app, &restored.packages) {
        return app.error(format!(
            "Snapshot {name} restored, but packages.toml wasn't updated: {e}"
        ));
    }
    app.msg(format!("Restored package snapshot {name}"));
}

fn restore_locked(app: &mut App) {
    let lock = crate::declare::load_lock();
    if lock.packages.is_empty() {
        return app.msg("forge-lock.toml has no locked plugins");
    }
    let Some(config) = config_dir() else {
        return app.error("No config directory for locked artifacts");
    };
    let Some(plugins) = app.packages.dir() else {
        return app.error("No plugins directory");
    };
    let artifacts = match read_locked_artifacts(&config, &lock.packages) {
        Ok(artifacts) => artifacts,
        Err(e) => return app.error(format!("Can't restore packages from forge-lock.toml: {e}")),
    };
    let before = InstalledDb::load(&plugins);
    let differences = match locked_differences(&before, &artifacts) {
        Ok(differences) => differences,
        Err(e) => return app.error(format!("Can't inspect installed plugins: {e}")),
    };
    if let Err(e) = write_locked_artifacts(&plugins, &artifacts) {
        return app.error(format!("Can't restore packages from forge-lock.toml: {e}"));
    }
    let mut message = format!("Restored {} locked plugin(s)", artifacts.len());
    if differences.is_empty() {
        message.push_str("; all resolved commits and artifact hashes already matched");
    } else {
        message.push_str(":\n");
        message.push_str(&differences.join("\n"));
    }
    app.msg(message);
}

fn locked_differences(
    current: &InstalledDb,
    artifacts: &[(String, forge_core::LockEntry, Vec<u8>)],
) -> Result<Vec<String>, String> {
    let mut differences = Vec::new();
    for (name, entry, bytes) in artifacts {
        let current_bytes = match std::fs::read(current.plugin_file(name)) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("{name}: {error}")),
        };
        let current_hash = current_bytes
            .as_deref()
            .map(forge_core::package::sha256_hex);
        let locked_hash = entry
            .artifact_sha256
            .as_deref()
            .unwrap_or(entry.sha256.as_str());
        let current_commit = current
            .packages
            .get(name)
            .and_then(|installed| installed.commit.as_deref());
        let expected_commit = entry.commit.as_deref();
        if current_hash.as_deref() != Some(locked_hash) || current_commit != expected_commit {
            let old = current_commit
                .map(str::to_string)
                .or_else(|| current_hash.clone())
                .unwrap_or_else(|| "not installed".into());
            let new = expected_commit
                .map(str::to_string)
                .unwrap_or_else(|| locked_hash.to_string());
            differences.push(format!(
                "{name}: {old} -> {new} (artifact {})",
                &locked_hash[..locked_hash.len().min(12)]
            ));
        }
        if forge_core::package::sha256_hex(bytes) != locked_hash {
            return Err(format!(
                "{name}: stored artifact hash differs from the lock"
            ));
        }
    }
    Ok(differences)
}

fn read_locked_artifacts(
    config: &std::path::Path,
    packages: &BTreeMap<String, forge_core::LockEntry>,
) -> Result<Vec<(String, forge_core::LockEntry, Vec<u8>)>, String> {
    packages
        .iter()
        .map(|(name, entry)| {
            if name.starts_with('.')
                || name.contains(['/', '\\'])
                || name.bytes().any(|b| b.is_ascii_control())
            {
                return Err(format!("invalid plugin name {name:?} in lockfile"));
            }
            let hash = entry
                .artifact_sha256
                .as_deref()
                .unwrap_or(entry.sha256.as_str());
            LockFile::artifact(config, hash)
                .and_then(|bytes| {
                    if bytes.len() < 8 || &bytes[..4] != b"\0asm" {
                        return Err(std::io::Error::other(format!(
                            "{name}: locked artifact is not a WebAssembly component"
                        )));
                    }
                    Ok((name.clone(), entry.clone(), bytes))
                })
                .map_err(|e| format!("{name}: {e}"))
        })
        .collect()
}

fn write_locked_artifacts(
    plugins: &std::path::Path,
    artifacts: &[(String, forge_core::LockEntry, Vec<u8>)],
) -> Result<(), String> {
    std::fs::create_dir_all(plugins).map_err(|e| e.to_string())?;
    let mut db = InstalledDb::load(plugins);
    let mut db_changed = false;
    for (name, entry, bytes) in artifacts {
        forge_core::fsutil::write_atomic(&db.plugin_file(name), bytes)
            .map_err(|e| format!("{name}: {e}"))?;
        if entry.git.is_some() || entry.url.is_some() || db.packages.contains_key(name) {
            let previous = db.packages.get(name);
            db.packages.insert(
                name.clone(),
                forge_core::package::Installed {
                    version: entry
                        .version
                        .clone()
                        .or_else(|| previous.map(|p| p.version.clone()))
                        .unwrap_or_else(|| "0.0.0".into()),
                    sha256: entry
                        .artifact_sha256
                        .clone()
                        .unwrap_or_else(|| entry.sha256.clone()),
                    url: entry
                        .url
                        .clone()
                        .unwrap_or_else(|| previous.map(|p| p.url.clone()).unwrap_or_default()),
                    git: entry
                        .git
                        .clone()
                        .or_else(|| previous.and_then(|p| p.git.clone())),
                    rev: entry
                        .rev
                        .clone()
                        .or_else(|| previous.and_then(|p| p.rev.clone())),
                    commit: entry
                        .commit
                        .clone()
                        .or_else(|| previous.and_then(|p| p.commit.clone())),
                    build: entry
                        .build
                        .clone()
                        .or_else(|| previous.and_then(|p| p.build.clone())),
                },
            );
            db_changed = true;
        }
    }
    if db_changed {
        db.save().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn sync_installed_metadata(
    app: &App,
    packages: &BTreeMap<String, forge_core::LockEntry>,
) -> Result<(), String> {
    let Some(plugins) = app.packages.dir() else {
        return Err("no plugins directory".into());
    };
    let mut db = InstalledDb::load(&plugins);
    let mut changed = false;
    for (name, entry) in packages {
        if entry.git.is_some() || entry.url.is_some() || db.packages.contains_key(name) {
            let previous = db.packages.get(name);
            db.packages.insert(
                name.clone(),
                forge_core::package::Installed {
                    version: entry
                        .version
                        .clone()
                        .or_else(|| previous.map(|p| p.version.clone()))
                        .unwrap_or_else(|| "0.0.0".into()),
                    sha256: entry
                        .artifact_sha256
                        .clone()
                        .unwrap_or_else(|| entry.sha256.clone()),
                    url: entry
                        .url
                        .clone()
                        .unwrap_or_else(|| previous.map(|p| p.url.clone()).unwrap_or_default()),
                    git: entry
                        .git
                        .clone()
                        .or_else(|| previous.and_then(|p| p.git.clone())),
                    rev: entry
                        .rev
                        .clone()
                        .or_else(|| previous.and_then(|p| p.rev.clone())),
                    commit: entry
                        .commit
                        .clone()
                        .or_else(|| previous.and_then(|p| p.commit.clone())),
                    build: entry
                        .build
                        .clone()
                        .or_else(|| previous.and_then(|p| p.build.clone())),
                },
            );
            changed = true;
        }
    }
    if changed {
        db.save().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn with_index(app: &mut App, then: Then) {
    if app.packages.index.is_some() {
        after_refresh(app, then);
    } else {
        refresh(app, then);
    }
}

/// Read every index again, then do `then`.
pub fn refresh(app: &mut App, then: Then) {
    if app.packages.indexes.is_empty() {
        return app.error("No package index: set forge.opt.package_index in init.rhai");
    }
    if app.packages.refreshing.is_some() {
        return app.msg("Still reading the package indexes...");
    }
    let locations = app.packages.indexes.clone();
    app.packages.refreshing = Some((Index::default(), Vec::new(), then));
    let mut remote = 0;
    for loc in locations {
        match package::local_path(&loc) {
            Some(path) => {
                let r = std::fs::read_to_string(&path).map_err(|e| format!("{loc}: {e}"));
                index_read(app, &loc, r);
            }
            None => {
                let file = temp_file("index");
                match download(app, &loc, &file) {
                    Ok(id) => {
                        app.packages.fetches.insert(
                            id,
                            Fetch::Index {
                                location: loc,
                                file,
                            },
                        );
                        remote += 1;
                    }
                    Err(e) => index_read(app, &loc, Err(e)),
                }
            }
        }
    }
    if remote > 0 {
        app.msg("Reading the package indexes...");
    } else {
        finish_refresh(app);
    }
}

fn index_read(app: &mut App, location: &str, text: Result<String, String>) {
    let parsed = text.and_then(|t| Index::parse(&t, location));
    let Some((index, errors, _)) = app.packages.refreshing.as_mut() else {
        return;
    };
    match parsed {
        Ok(i) => index.merge(i),
        Err(e) => errors.push(e),
    }
}

fn finish_refresh(app: &mut App) {
    let Some((index, errors, then)) = app.packages.refreshing.take() else {
        return;
    };
    let n = index.packages.len();
    app.packages.index = Some(index);
    if !errors.is_empty() {
        app.error(errors.join("; "));
    } else if then == Then::Nothing {
        app.msg(format!(
            "{n} package{} in the index",
            if n == 1 { "" } else { "s" }
        ));
    }
    after_refresh(app, then);
}

fn after_refresh(app: &mut App, then: Then) {
    match then {
        Then::Nothing => {}
        Then::List => list(app),
        Then::Manager => manager(app),
        Then::Install => {
            let installed = db(app).map(|d| d.packages).unwrap_or_default();
            let names: Vec<String> = app
                .packages
                .index
                .as_ref()
                .map(|i| {
                    i.packages
                        .keys()
                        .filter(|n| !installed.contains_key(*n))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            if names.is_empty() {
                app.msg("Everything in the index is installed");
            } else {
                app.prompt(
                    "Install package: ",
                    Purpose::Choose {
                        what: Choice::InstallPackage,
                        candidates: names,
                    },
                );
            }
        }
        Then::UpgradeAll => {
            let Some(index) = app.packages.index.clone() else {
                return;
            };
            let todo = match upgrade_candidates(app, &index) {
                Ok(todo) => todo,
                Err(error) => return app.error(error),
            };
            if todo.is_empty() {
                let vc = crate::package_vc::upgrade_all(app);
                if vc == 0 {
                    app.msg("All packages are up to date");
                }
            } else {
                app.packages.upgrade_git_after_index = true;
                start_upgrade_queue(app, todo);
            }
        }
        Then::UpgradeDryRun => {
            let Some(index) = app.packages.index.clone() else {
                return;
            };
            match dry_run(app, &index, true) {
                Ok(report) => show_package_text(app, "*Package Upgrade Plan*", &report),
                Err(error) => app.error(format!("Couldn't plan package upgrades: {error}")),
            }
        }
        Then::UpgradeOne(name) => upgrade_named(app, &name),
    }
}

fn upgrade_candidates(app: &App, index: &Index) -> Result<Vec<Package>, String> {
    let Some(installed) = db(app) else {
        return Ok(Vec::new());
    };
    let declarations = app
        .rhai
        .as_ref()
        .map(|r| r.plugin_uses())
        .unwrap_or_default();
    let mut requirements = BTreeMap::new();
    let mut frozen = std::collections::BTreeSet::new();
    for declaration in declarations {
        let package_name = declaration
            .index
            .clone()
            .unwrap_or_else(|| declaration.name.clone());
        if let Some(version) = declaration.version {
            requirements.insert(package_name.clone(), version);
        }
        if declaration.frozen {
            frozen.insert(declaration.name);
            frozen.insert(package_name);
        }
    }
    installed
        .upgradable_for(index, &requirements, &frozen)
        .map(|packages| packages.into_iter().cloned().collect())
}

fn dry_run(app: &App, index: &Index, include_git: bool) -> Result<String, String> {
    let mut lines = vec![
        "Package upgrade plan (no files changed)".to_string(),
        String::new(),
    ];
    let packages = upgrade_candidates(app, index)?;
    let installed = db(app).unwrap_or_default();
    for package in packages {
        let old = installed
            .packages
            .get(&package.name)
            .map(|p| p.version.as_str())
            .unwrap_or("not installed");
        let recipe = installed
            .packages
            .get(&package.name)
            .and_then(|p| p.build.as_deref())
            .unwrap_or("no build command (prebuilt artifact)");
        lines.push(format!(
            "{}: {} -> {}; build: {}",
            package.name, old, package.version, recipe
        ));
    }
    if include_git {
        lines.extend(crate::package_vc::dry_run_lines(app)?);
    }
    if lines.len() == 2 {
        lines.push("No packages need upgrading.".into());
    }
    Ok(lines.join("\n") + "\n")
}

pub fn upgrade_named(app: &mut App, name: &str) {
    let Some(installed) = db(app).and_then(|d| d.packages.get(name).cloned()) else {
        return app.error(format!("{name} isn't installed"));
    };
    if installed.git.is_some() {
        return crate::package_vc::upgrade(app, name);
    }
    if app.packages.index.is_none() {
        return with_index(app, Then::UpgradeOne(name.to_string()));
    }
    let declaration = crate::declare::declared(app, name);
    let package_name = declaration
        .as_ref()
        .and_then(|u| u.index.as_deref())
        .unwrap_or(name);
    let requirement = declaration.as_ref().and_then(|u| u.version.as_deref());
    let selected = match app
        .packages
        .index
        .as_ref()
        .expect("index loaded")
        .resolve(package_name, requirement)
    {
        Ok(Some(package)) => package.clone(),
        Ok(None) => return app.error(format!("No indexed version of {package_name} matches")),
        Err(error) => return app.error(error),
    };
    if declaration.as_ref().is_some_and(|u| u.frozen) {
        return app.msg(format!("{name} is pinned or frozen; not upgrading"));
    }
    start_upgrade_queue(app, vec![selected]);
}

fn start_upgrade_queue(app: &mut App, packages: Vec<Package>) {
    app.packages.upgrade_queue = packages.into();
    next_upgrade_prompt(app);
}

fn next_upgrade_prompt(app: &mut App) {
    let Some(package) = app.packages.upgrade_queue.pop_front() else {
        if std::mem::take(&mut app.packages.upgrade_git_after_index)
            && crate::package_vc::upgrade_all(app) == 0
        {
            app.msg("No Git packages need upgrading");
        }
        return;
    };
    let old = db(app)
        .and_then(|db| db.packages.get(&package.name).cloned())
        .map(|installed| installed.version)
        .unwrap_or_else(|| "not installed".into());
    let build = db(app)
        .and_then(|db| db.packages.get(&package.name).and_then(|p| p.build.clone()))
        .unwrap_or_else(|| "no build command (prebuilt artifact)".into());
    let mut review = format!(
        "Package upgrade review: {}\nVersion: {} -> {}\nBuild: {}\n\n",
        package.name, old, package.version, build
    );
    if let Some(notes) = &package.release_notes {
        review.push_str(notes);
        review.push('\n');
    } else {
        review.push_str("No release notes were provided by the index.\n");
    }
    show_package_text(app, "*Package Upgrade Review*", &review);
    app.packages.pending_upgrade = Some(package.clone());
    app.prompt(
        format!(
            "Apply {} {} -> {}? (y or n) ",
            package.name, old, package.version
        ),
        Purpose::Confirm(crate::minibuffer::ConfirmAction::IndexPackageUpgrade),
    );
}

pub fn package_upgrade_confirmed(app: &mut App, yes: bool) {
    let Some(package) = app.packages.pending_upgrade.take() else {
        return;
    };
    if yes {
        install(app, package.clone());
        app.msg(format!("Applying upgrade for {}", package.name));
    } else {
        app.msg(format!("Skipped upgrade for {}", package.name));
    }
    next_upgrade_prompt(app);
}

/// Show declared, installed, and indexed packages without requiring an index.
pub fn manager(app: &mut App) {
    let installed = db(app).map(|d| d.packages).unwrap_or_default();
    let declared: BTreeMap<String, forge_core::PluginUse> = app
        .rhai
        .as_ref()
        .map(|r| {
            r.plugin_uses()
                .into_iter()
                .map(|u| (u.name.clone(), u))
                .collect()
        })
        .unwrap_or_default();
    let loaded = crate::plugins::loaded_names(app);
    let mut names: Vec<String> = installed
        .keys()
        .chain(declared.keys())
        .chain(app.packages.index.iter().flat_map(|i| i.packages.keys()))
        .cloned()
        .collect();
    names.sort();
    names.dedup();
    let filter = app.packages.manager_filter.to_lowercase();
    names.retain(|name| {
        manager_matches(
            name,
            &filter,
            &installed,
            &declared,
            app.packages.index.as_ref(),
        )
    });
    if !names
        .iter()
        .any(|n| Some(n) == app.packages.manager_selected.as_ref())
    {
        app.packages.manager_selected = names.first().cloned();
    }
    let selected = app.packages.manager_selected.clone();
    let width = names.iter().map(String::len).max().unwrap_or(4).max(4);
    let mut text = String::from(
        "Forge Packages\nj/k or arrows move  RET/i install  u upgrade  d delete  f filter  F clear\nv details  b build log  g refresh  U upgrade all  q close\n",
    );
    text.push_str(&format!(
        "Filter: {}  |  {} package{}\n\n",
        if app.packages.manager_filter.is_empty() {
            "(none)"
        } else {
            &app.packages.manager_filter
        },
        names.len(),
        if names.len() == 1 { "" } else { "s" }
    ));
    text.push_str(&format!(
        "  {:width$}  {:22}  {:10}  Source\n",
        "Name", "Status", "Version"
    ));
    let mut selected_line = None;
    for (i, name) in names.iter().enumerate() {
        let declaration = declared.get(name);
        let package = installed.get(name);
        let status = crate::package_vc::status(app, name).unwrap_or_else(|| {
            if package.is_some() {
                if loaded.iter().any(|n| n == name) {
                    "loaded"
                } else if declaration.is_some_and(|u| u.lazy()) {
                    "installed (lazy)"
                } else if declaration.is_some_and(|u| u.no_require) {
                    "installed (deferred)"
                } else {
                    "installed"
                }
            } else if declaration.is_some() {
                "not installed"
            } else {
                "available"
            }
        });
        let version = package
            .map(|p| p.version.as_str())
            .or_else(|| {
                app.packages
                    .index
                    .as_ref()?
                    .packages
                    .get(name)
                    .map(|p| p.version.as_str())
            })
            .unwrap_or("-");
        let source = declaration
            .and_then(|u| u.git.as_deref().or(u.url.as_deref()).or(u.index.as_deref()))
            .or_else(|| {
                package
                    .and_then(|p| p.git.as_deref().or(Some(p.url.as_str())))
                    .filter(|s| !s.is_empty())
            })
            .or_else(|| {
                app.packages
                    .index
                    .as_ref()?
                    .packages
                    .get(name)
                    .map(|p| p.git.as_deref().or(Some(p.url.as_str())).unwrap_or(""))
            })
            .unwrap_or("-");
        let marker = if selected.as_deref() == Some(name.as_str()) {
            selected_line = Some(i + 6);
            ">"
        } else {
            " "
        };
        text.push_str(&format!(
            "{name:width$} {marker}  {status:22}  {version:10}  {source}\n"
        ));
    }
    text.push_str("\nDetails\n-------\n");
    if let Some(name) = selected.as_deref() {
        text.push_str(&details(app, name));
    } else {
        text.push_str("No packages match this filter.\n");
    }
    app.show_special_buffer(BUFFER, "package-manager-mode", &text);
    app.buf()
        .set_point(forge_core::Pos::new(selected_line.unwrap_or(3), 0));
}

fn show_details(app: &mut App, name: &str) {
    let text = details(app, name);
    if text.is_empty() {
        return app.error(format!("No declared or installed package {name}"));
    }
    app.show_special_buffer("*Package Details*", "special-mode", &text);
}

fn details(app: &App, name: &str) -> String {
    let declaration = app
        .rhai
        .as_ref()
        .and_then(|r| r.plugin_uses().into_iter().find(|u| u.name == name));
    let package = db(app).and_then(|d| d.packages.get(name).cloned());
    if declaration.is_none() && package.is_none() {
        return String::new();
    }
    let mut text = format!("{name}\n");
    if let Some(p) = package {
        text.push_str(&format!("  version: {}\n", p.version));
        if let Some(git) = p.git {
            text.push_str(&format!("  git: {git}\n"));
            if let Some(rev) = p.rev {
                text.push_str(&format!("  revision: {rev}\n"));
            }
            if let Some(commit) = p.commit {
                text.push_str(&format!(
                    "  commit: {}\n",
                    commit.chars().take(12).collect::<String>()
                ));
            }
            if let Some(build) = p.build {
                text.push_str(&format!("  build: {build}\n"));
            }
        } else if !p.url.is_empty() {
            text.push_str(&format!("  url: {}\n", p.url));
        }
    }
    if let Some(p) = app
        .packages
        .index
        .as_ref()
        .and_then(|i| i.packages.get(name))
    {
        text.push_str(&format!("  available version: {}\n", p.version));
        if !p.description.is_empty() {
            text.push_str(&format!("  {}\n", p.description));
        }
    }
    if let Some(u) = declaration {
        text.push_str("\nDeclaration:\n");
        if let Some(git) = u.git.as_deref() {
            text.push_str(&format!("  git: {git}\n"));
        }
        if let Some(rev) = u.rev.as_deref() {
            text.push_str(&format!("  revision: {rev}\n"));
        }
        if let Some(url) = u.url.as_deref() {
            text.push_str(&format!("  url: {url}\n"));
            if let Some(sha256) = u.sha256.as_deref() {
                text.push_str(&format!("  sha256: {sha256}\n"));
            }
        }
        if let Some(index) = u.index.as_deref() {
            text.push_str(&format!("  index package: {index}\n"));
        }
        if let Some(build) = u.build.as_deref() {
            text.push_str(&format!("  build: {build}\n"));
        }
        if !u.features.is_empty() {
            text.push_str(&format!("  features: {}\n", u.features.join(", ")));
        }
        if u.lazy() {
            text.push_str("  load: lazy\n");
            if !u.on_commands.is_empty() {
                text.push_str(&format!("  commands: {}\n", u.on_commands.join(", ")));
            }
            if !u.on_modes.is_empty() {
                text.push_str(&format!("  modes: {}\n", u.on_modes.join(", ")));
            }
            if !u.on_extensions.is_empty() {
                text.push_str(&format!("  extensions: {}\n", u.on_extensions.join(", ")));
            }
        }
        if !u.after.is_empty() {
            text.push_str(&format!("  after: {}\n", u.after.join(", ")));
        }
    }
    if let Some(status) = crate::package_vc::status(app, name) {
        text.push_str(&format!("\n  task: {status}\n"));
    }
    text
}

fn manager_move(app: &mut App, delta: isize) {
    if app.buf_ref().major_mode() != "package-manager-mode" {
        return app.error("Not in the package manager");
    }
    let installed = db(app).map(|d| d.packages).unwrap_or_default();
    let declared: BTreeMap<String, forge_core::PluginUse> = app
        .rhai
        .as_ref()
        .map(|r| {
            r.plugin_uses()
                .into_iter()
                .map(|u| (u.name.clone(), u))
                .collect()
        })
        .unwrap_or_default();
    let filter = app.packages.manager_filter.to_lowercase();
    let mut names: Vec<String> = installed
        .keys()
        .chain(declared.keys())
        .chain(app.packages.index.iter().flat_map(|i| i.packages.keys()))
        .filter(|n| {
            manager_matches(
                n,
                &filter,
                &installed,
                &declared,
                app.packages.index.as_ref(),
            )
        })
        .cloned()
        .collect();
    names.sort();
    names.dedup();
    if names.is_empty() {
        return app.msg("No packages match this filter");
    }
    let old = app.packages.manager_selected.as_ref();
    let i = old
        .and_then(|n| names.iter().position(|candidate| candidate == n))
        .unwrap_or(0);
    let next = (i as isize + delta).rem_euclid(names.len() as isize) as usize;
    app.packages.manager_selected = Some(names[next].clone());
    manager(app);
}

fn manager_matches(
    name: &str,
    filter: &str,
    installed: &BTreeMap<String, forge_core::package::Installed>,
    declared: &BTreeMap<String, forge_core::PluginUse>,
    index: Option<&Index>,
) -> bool {
    let contains = |s: &str| s.to_lowercase().contains(filter);
    filter.is_empty()
        || contains(name)
        || declared
            .get(name)
            .and_then(|u| u.git.as_deref().or(u.url.as_deref()).or(u.index.as_deref()))
            .is_some_and(contains)
        || installed
            .get(name)
            .and_then(|p| p.git.as_deref().or(Some(p.url.as_str())))
            .filter(|s| !s.is_empty())
            .is_some_and(contains)
        || index.and_then(|i| i.packages.get(name)).is_some_and(|p| {
            p.description.to_lowercase().contains(filter)
                || p.git
                    .as_deref()
                    .or(Some(p.url.as_str()))
                    .is_some_and(contains)
        })
}

fn selected_name(app: &mut App) -> Option<String> {
    if app.buf_ref().major_mode() == "package-manager-mode" {
        return app.packages.manager_selected.clone();
    }
    name_on_line(app)
}

fn install_selected(app: &mut App, name: &str) {
    if let Some(u) = crate::declare::declared(app, name)
        && let Some(index) = u.index.clone()
    {
        return install_named_version(app, &index, u.version.as_deref());
    }
    if app
        .packages
        .index
        .as_ref()
        .is_some_and(|i| i.packages.contains_key(name))
    {
        return install_named(app, name);
    }
    let Some(u) = crate::declare::declared(app, name) else {
        return app.error(format!("No source available for {name}"));
    };
    if let Some(git) = u.git {
        crate::package_vc::install(app, name, &git, u.rev, false);
    } else if let Some(url) = u.url {
        let Some(sha256) = u.sha256 else {
            return app.error(format!("{name}: declaring a url needs sha256"));
        };
        install_url(app, name, &url, &sha256);
    } else if let Some(index) = u.index {
        install_named(app, &index);
    } else {
        app.error(format!("{name} has no install source"));
    }
}

pub fn filter_manager(app: &mut App, filter: &str) {
    app.packages.manager_filter = filter.to_string();
    manager(app);
}

/// The *Packages* buffer.
pub fn list(app: &mut App) {
    let Some(index) = app.packages.index.clone() else {
        return;
    };
    let installed = db(app).map(|d| d.packages).unwrap_or_default();
    let mut text = String::from("Packages (RET install or upgrade, d delete, g refresh)\n\n");
    let width = index
        .packages
        .keys()
        .chain(installed.keys())
        .map(String::len)
        .max()
        .unwrap_or(4)
        .max(4);
    let mut names: Vec<&String> = index.packages.keys().chain(installed.keys()).collect();
    names.sort();
    names.dedup();
    for name in names {
        let avail = index.packages.get(name);
        let have = installed.get(name);
        let from_git = have.and_then(|h| {
            h.git.as_ref().map(|_| {
                h.commit
                    .as_deref()
                    .unwrap_or("")
                    .chars()
                    .take(7)
                    .collect::<String>()
            })
        });
        let (version, status) = match (avail, have) {
            (_, Some(h)) if from_git.is_some() => (
                h.version.clone(),
                format!("git {}", from_git.unwrap_or_default()),
            ),
            (Some(a), None) if a.git.is_some() => (a.version.clone(), "available (git)".into()),
            (Some(a), Some(h)) if package::compare_versions(&a.version, &h.version).is_gt() => {
                (a.version.clone(), format!("upgrade from {}", h.version))
            }
            (Some(a), Some(_)) => (a.version.clone(), "installed".into()),
            (Some(a), None) => (a.version.clone(), "available".into()),
            (None, Some(h)) => (h.version.clone(), "installed (not in the index)".into()),
            (None, None) => continue,
        };
        let desc = avail.map(|a| a.description.as_str()).unwrap_or("");
        text.push_str(&format!(
            "  {name:width$}  {version:10}  {status:22}  {desc}\n"
        ));
    }
    app.show_special_buffer(BUFFER, "package-menu-mode", &text);
    app.buf().set_point(forge_core::Pos::new(2, 0));
}

fn name_on_line(app: &mut App) -> Option<String> {
    let b = app.buf_ref();
    let line = b.line(b.point().line);
    let name = line.split_whitespace().next().map(String::from);
    if name.is_none() || b.point().line < 2 {
        app.error("No package on this line");
        return None;
    }
    name
}

/// Install a plugin straight from a url (use_package's url + sha256).
pub fn install_url(app: &mut App, name: &str, url: &str, sha256: &str) {
    install(
        app,
        Package {
            name: name.to_string(),
            version: "0.0.0".to_string(),
            description: String::new(),
            url: url.to_string(),
            sha256: sha256.to_string(),
            git: None,
            rev: None,
            release_notes: None,
        },
    );
}

/// RET in project-find-file style prompts: see minibuffer.rs.
pub fn install_named(app: &mut App, name: &str) {
    install_named_version(app, name, None);
}

fn install_named_version(app: &mut App, name: &str, requirement: Option<&str>) {
    let selected = app
        .packages
        .index
        .as_ref()
        .map(|index| index.resolve(name, requirement));
    let package = match selected {
        Some(Ok(Some(package))) => package.clone(),
        Some(Ok(None)) => return app.error(format!("No package {name} in the requested version")),
        Some(Err(error)) => return app.error(error),
        None => return app.error("No package index is loaded"),
    };
    if db(app).is_some_and(|db| db.packages.contains_key(name)) {
        start_upgrade_queue(app, vec![package]);
    } else {
        install(app, package);
    }
}

fn install(app: &mut App, p: Package) {
    let upgrading = db(app).is_some_and(|db| db.packages.contains_key(&p.name));
    if let Some(git) = &p.git {
        return crate::package_vc::install(app, &p.name, git, p.rev.clone(), upgrading);
    }
    match package::local_path(&p.url) {
        Some(path) => {
            let r = std::fs::read(&path).map_err(|e| format!("{}: {e}", p.url));
            installed(app, p, r, upgrading);
        }
        None => {
            let file = temp_file(&p.name);
            match download(app, &p.url, &file) {
                Ok(id) => {
                    app.msg(format!("Downloading {} {}...", p.name, p.version));
                    app.packages.fetches.insert(
                        id,
                        Fetch::Package {
                            package: p,
                            file,
                            upgrade: upgrading,
                        },
                    );
                }
                Err(e) => app.error(e),
            }
        }
    }
}

/// A download finished (or a local file was read): check, install, load.
fn installed(app: &mut App, p: Package, bytes: Result<Vec<u8>, String>, upgrade: bool) {
    let backup = if upgrade {
        match capture_package_backup(app, &p.name) {
            Ok(backup) => Some(backup),
            Err(error) => return app.error(error),
        }
    } else {
        None
    };
    let Some(mut db) = db(app) else {
        return app.error("No plugins directory (set HOME or FORGE_CONFIG_DIR)");
    };
    let bytes = bytes.and_then(|b| {
        if upgrade {
            smoke_test_component(app, &p.name, &b)?;
        }
        Ok(b)
    });
    let r = bytes.and_then(|b| db.install(&p, &b));
    match r {
        Ok(path) => {
            if let Err(error) = crate::declare::record_checked(app, &p.name, &path) {
                let rollback = backup
                    .map(|backup| restore_package_backup(app, &p.name, backup))
                    .transpose();
                return app.error(match rollback {
                    Ok(_) => format!("Can't upgrade {}: {error}", p.name),
                    Err(rollback) => {
                        format!(
                            "Can't upgrade {}: {error}; rollback failed: {rollback}",
                            p.name
                        )
                    }
                });
            }
            app.msg(format!("Installed {} {}", p.name, p.version));
            crate::plugins::load(app, &path);
            let manager_open = app.buf_ref().major_mode() == "package-manager-mode";
            let is_package_buffer = app
                .editor
                .find_buffer(BUFFER)
                .is_some_and(|id| id == app.active_id());
            if is_package_buffer && manager_open {
                manager(app);
            } else if is_package_buffer && app.packages.index.is_some() {
                list(app);
            }
        }
        Err(e) => {
            let rollback = backup
                .map(|backup| restore_package_backup(app, &p.name, backup))
                .transpose();
            app.error(match rollback {
                Ok(_) => format!("Can't install {}: {e}", p.name),
                Err(rollback) => {
                    format!("Can't install {}: {e}; rollback failed: {rollback}", p.name)
                }
            });
        }
    }
}

pub(crate) fn smoke_test_component(app: &App, name: &str, bytes: &[u8]) -> Result<(), String> {
    let path = temp_file(&format!("{name}-smoke")).with_extension("wasm");
    let Some(parent) = path.parent() else {
        return Err("smoke-test path has no parent".into());
    };
    std::fs::create_dir_all(parent).map_err(|e| format!("creating smoke-test directory: {e}"))?;
    std::fs::write(&path, bytes).map_err(|e| format!("staging smoke-test component: {e}"))?;
    let result = (|| {
        let manifest = forge_core::Manifest::for_component(&path, bytes)
            .map_err(|e| format!("{name}: reading manifest for smoke test: {e}"))?;
        if let (Some(manifest), Some(config)) = (&manifest, app.editor.plugin_config.get(name)) {
            manifest
                .check_config(config)
                .map_err(|e| format!("{name}: smoke test configuration failed: {e}"))?;
        }
        let mut host = forge_core::PluginHost::new()
            .map_err(|e| format!("{name}: creating smoke-test runtime: {e:#}"))?;
        let mut editor = forge_core::Editor::new();
        let (loaded, _) = host.load_file(
            &mut editor,
            &path,
            &crate::plugins::effective_grants(app, name),
        );
        loaded.map_err(|e| format!("{name}: smoke test failed: {e:#}"))?;
        if let Some(manifest) = manifest {
            for command in manifest.command_names() {
                if editor.commands.plugin_for(&command) != Some(name) {
                    return Err(format!(
                        "{name}: smoke test did not register declared command {command}"
                    ));
                }
            }
        }
        Ok(())
    })();
    let _ = std::fs::remove_file(&path);
    result
}

pub fn delete(app: &mut App, name: &str) {
    let Some(mut db) = db(app) else { return };
    let from_vc = db.packages.get(name).is_some_and(|i| i.git.is_some());
    match db.delete(name) {
        Ok(()) => {
            if from_vc && let Some(src) = package::vc_dir(name) {
                let _ = std::fs::remove_dir_all(src);
            }
            if crate::plugins::loaded_names(app).iter().any(|n| n == name) {
                crate::plugins::unload(app, name);
            }
            app.msg(format!("Deleted {name}"));
            let manager_open = app.buf_ref().major_mode() == "package-manager-mode";
            let is_package_buffer = app
                .editor
                .find_buffer(BUFFER)
                .is_some_and(|id| id == app.active_id());
            if is_package_buffer && manager_open {
                manager(app);
            } else if is_package_buffer && app.packages.index.is_some() {
                list(app);
            }
        }
        Err(e) => app.error(e),
    }
}

fn temp_file(tag: &str) -> PathBuf {
    let dir = forge_core::config::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("downloads");
    let _ = std::fs::create_dir_all(&dir);
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    dir.join(format!("{tag}-{}-{n}", std::process::id()))
}

fn download(app: &mut App, url: &str, to: &std::path::Path) -> Result<u64, String> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err(format!(
            "{url}: only local files and http(s) URLs are supported"
        ));
    }
    let (tx, rx) = mpsc::channel();
    let (url_owned, to_owned) = (url.to_string(), to.to_path_buf());
    std::thread::Builder::new()
        .name("package-download".into())
        .spawn(move || {
            let _ = tx.send(http_get_to_file(&url_owned, &to_owned));
        })
        .map_err(|e| format!("can't start the download of {url}: {e}"))?;
    let id = app.packages.next_download;
    app.packages.next_download += 1;
    app.packages.downloads.push((id, rx));
    Ok(id)
}

/// GET `url` into `to` (an error for a failing status, as curl -f was).
fn http_get_to_file(url: &str, to: &std::path::Path) -> Result<(), String> {
    use std::error::Error;
    let describe = |e: &reqwest::Error| {
        let mut msg = e.to_string();
        let mut src = e.source();
        while let Some(s) = src {
            msg = format!("{msg}: {s}");
            src = s.source();
        }
        msg
    };
    let local = reqwest::Url::parse(url)
        .ok()
        .and_then(|u| {
            u.host_str()
                .map(|h| matches!(h, "localhost" | "127.0.0.1" | "::1" | "[::1]"))
        })
        .unwrap_or(false);
    let mut builder =
        reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(300));
    if local {
        builder = builder.no_proxy();
    }
    let client = builder
        .build()
        .map_err(|e| format!("download failed ({})", describe(&e)))?;
    let mut resp = client
        .get(url)
        .send()
        .map_err(|e| format!("download failed ({})", describe(&e)))?;
    if !resp.status().is_success() {
        return Err(format!(
            "download failed (the server answered {})",
            resp.status()
        ));
    }
    let mut file = std::fs::File::create(to).map_err(|e| format!("download failed ({e})"))?;
    resp.copy_to(&mut file)
        .map_err(|e| format!("download failed ({})", describe(&e)))?;
    Ok(())
}

/// Finished downloads (called when idle).
pub fn poll(app: &mut App) {
    if !app.packages.needs_poll() {
        return;
    }
    crate::package_vc::poll(app);
    crate::grammar_install::poll(app);
    let mut finished: Vec<(u64, Result<(), String>)> = Vec::new();
    app.packages
        .downloads
        .retain(|(id, rx)| match rx.try_recv() {
            Ok(r) => {
                finished.push((*id, r));
                false
            }
            Err(TryRecvError::Empty) => true,
            Err(TryRecvError::Disconnected) => {
                finished.push((
                    *id,
                    Err("download failed (the download stopped unexpectedly)".to_string()),
                ));
                false
            }
        });
    {
        for (id, result) in finished {
            {
                let Some(fetch) = app.packages.fetches.remove(&id) else {
                    continue;
                };
                let failed = result.err();
                match fetch {
                    Fetch::Index { location, file } => {
                        let r = match failed {
                            Some(e) => Err(format!("{location}: {e}")),
                            None => std::fs::read_to_string(&file)
                                .map_err(|e| format!("{location}: {e}")),
                        };
                        let _ = std::fs::remove_file(&file);
                        index_read(app, &location, r);
                        if !app
                            .packages
                            .fetches
                            .values()
                            .any(|f| matches!(f, Fetch::Index { .. }))
                        {
                            finish_refresh(app);
                        }
                    }
                    Fetch::Package {
                        package,
                        file,
                        upgrade,
                    } => {
                        let r = match failed {
                            Some(e) => Err(e),
                            None => std::fs::read(&file).map_err(|e| e.to_string()),
                        };
                        let _ = std::fs::remove_file(&file);
                        installed(app, package, r, upgrade);
                    }
                }
            }
        }
    }
}
