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
use crate::package_ui::{self, Status as UiStatus, Symbols, Text};
use forge_core::decoration::PackageFace;
use forge_core::package::{self, Index, Installed, InstalledDb, Package};
use forge_core::package_doctor::Finding;
use forge_core::package_log::{Operation, Source, SourceKind, Status, Timer};
use forge_core::package_source::{SourceAllowlist, SourceKind as PackageSourceKind};
use forge_core::{CapabilityChange, CapabilityDiff, Grants, LockFile, Manifest};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

pub const BUFFER: &str = "*Packages*";

/// What to do once the indexes are in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Then {
    Nothing,
    List,
    Manager,
    Install { force: bool },
    UpgradeAll,
    UpgradeDryRun,
    UpgradeOne(String),
}

struct PendingCapability {
    package: Package,
    bytes: Vec<u8>,
    diff: CapabilityDiff,
}

enum Fetch {
    Index {
        location: String,
        file: PathBuf,
        signature: Option<PathBuf>,
        started: Timer,
    },
    Package {
        package: Package,
        file: PathBuf,
        upgrade: bool,
        force: bool,
        started: Timer,
    },
}

#[derive(Default)]
pub(crate) struct UpdateTransaction {
    operation: Option<String>,
    batch: bool,
    journal: Option<forge_core::package_journal::Journal>,
    blocked: Option<String>,
    started: Option<Timer>,
}

fn update_paths(app: &App) -> Result<(PathBuf, PathBuf, PathBuf), String> {
    let data = forge_core::config::data_dir()
        .ok_or_else(|| "no data directory (set HOME or FORGE_DATA_DIR)".to_string())?;
    let config = forge_core::config::config_dir()
        .ok_or_else(|| "no config directory (set HOME or FORGE_CONFIG_DIR)".to_string())?;
    let plugins = app
        .packages
        .dir()
        .ok_or_else(|| "no plugins directory".to_string())?;
    Ok((data, config, plugins))
}

pub(crate) fn recover_startup(app: &mut App) {
    use forge_core::package_journal::{self, Recovery};
    let Some(data) = forge_core::config::data_dir() else {
        return;
    };
    match package_journal::recover(&data) {
        Ok(Some(Recovery::RolledBack { packages })) => app.msg(format!(
            "Rolled back interrupted package update ({packages} package{})",
            if packages == 1 { "" } else { "s" }
        )),
        Ok(Some(Recovery::Finished)) => {}
        Ok(None) => {}
        Err(error) => {
            app.packages.update.blocked = Some(error.clone());
            app.error(format!("Package update recovery failed: {error}"));
        }
    }
}

pub(crate) fn begin_update(app: &mut App, operation: &str, batch: bool) -> Result<(), String> {
    if let Some(error) = &app.packages.update.blocked {
        return Err(format!(
            "package updates are blocked after a failed journal recovery: {error}"
        ));
    }
    if app.packages.update.operation.is_none() {
        app.packages.update.operation = Some(operation.to_string());
    }
    if app.packages.update.started.is_none() {
        app.packages.update.started = Some(Timer::start());
    }
    app.packages.update.batch |= batch;
    Ok(())
}

pub(crate) fn prepare_update(app: &mut App, name: &str) -> Result<(), String> {
    if let Some(error) = &app.packages.update.blocked {
        return Err(format!(
            "package updates are blocked after a failed journal recovery: {error}"
        ));
    }
    if app.packages.update.operation.is_none() {
        app.packages.update.operation = Some("upgrade".to_string());
    }
    if app.packages.update.started.is_none() {
        app.packages.update.started = Some(Timer::start());
    }
    if app.packages.update.journal.is_none() {
        let (data, config, plugins) = update_paths(app)?;
        let operation = app
            .packages
            .update
            .operation
            .clone()
            .unwrap_or_else(|| "upgrade".to_string());
        app.packages.update.journal = Some(forge_core::package_journal::Journal::new(
            &data, &operation, &config, &plugins,
        ));
    }
    app.packages
        .update
        .journal
        .as_mut()
        .ok_or_else(|| "no package update transaction".to_string())?
        .add(name)
        .map(|_| ())
}

pub(crate) fn commit_update(app: &mut App) -> Result<(), String> {
    let names = app
        .packages
        .update
        .journal
        .as_ref()
        .map(forge_core::package_journal::Journal::names)
        .unwrap_or_default();
    if let Some(mut journal) = app.packages.update.journal.take()
        && let Err(error) = journal.commit()
    {
        app.packages.update.journal = Some(journal);
        return Err(error);
    }
    let status = if names.is_empty() {
        Status::Unchanged
    } else {
        Status::Ok
    };
    log_update_outcome(app, status, None, &names);
    app.packages.update.operation = None;
    app.packages.update.batch = false;
    Ok(())
}

pub(crate) fn rollback_update(app: &mut App) -> Result<(), String> {
    let names = app
        .packages
        .update
        .journal
        .as_ref()
        .map(forge_core::package_journal::Journal::names)
        .unwrap_or_default();
    if app.packages.update.journal.is_none() {
        app.packages.update.operation = None;
        app.packages.update.batch = false;
        app.packages.update.started = None;
        return Ok(());
    }
    let timer = Timer::start();
    let result = match app.packages.update.journal.take() {
        Some(mut journal) => match journal.rollback() {
            Ok(()) => Ok(()),
            Err(error) => {
                app.packages.update.journal = Some(journal);
                app.packages.update.blocked = Some(error.clone());
                Err(error)
            }
        },
        None => Ok(()),
    };
    let status = if result.is_ok() {
        Status::Ok
    } else {
        Status::Failed
    };
    let mut event = timer
        .finish(Operation::Rollback, status, None)
        .detail("packages", names.join(", "));
    if let Err(error) = &result {
        event = event.error(error.clone());
    }
    crate::package_log::record(event);
    if result.is_ok() {
        app.packages.update.operation = None;
        app.packages.update.batch = false;
        app.packages.update.started = None;
    }
    result
}

fn log_update_outcome(app: &mut App, status: Status, error: Option<&str>, names: &[String]) {
    let Some(timer) = app.packages.update.started.take() else {
        return;
    };
    let kind = app
        .packages
        .update
        .operation
        .clone()
        .unwrap_or_else(|| "upgrade".to_string());
    let package = if names.len() == 1 {
        Some(names[0].as_str())
    } else {
        None
    };
    let mut event = timer
        .finish(Operation::Upgrade, status, package)
        .detail("kind", kind);
    if names.len() > 1 {
        event = event.detail("packages", names.join(", "));
    }
    if let Some(error) = error {
        event = event.error(error);
    }
    crate::package_log::record(event);
}

pub(crate) fn update_is_batch(app: &App) -> bool {
    app.packages.update.batch
}

pub(crate) fn finish_update(app: &mut App) {
    if let Err(error) = commit_update(app) {
        abort_update(app, format!("Couldn't finish package update: {error}"));
    } else {
        crate::package_manager_ui::update_finish(app);
    }
}

pub(crate) fn abort_update_message(app: &mut App, error: String) -> String {
    crate::package_manager_ui::update_fail_note(app, &error);
    let reload = app
        .packages
        .update
        .journal
        .as_ref()
        .map(forge_core::package_journal::Journal::names)
        .unwrap_or_default();
    log_update_outcome(app, Status::Failed, Some(&error), &reload);
    let rollback = rollback_update(app);
    app.packages.upgrade_queue.clear();
    app.packages.pending_upgrade = None;
    app.packages.pending_capability = None;
    app.packages.upgrade_git_after_index = false;
    app.packages.vc.cancel_queued();
    match rollback {
        Ok(()) => {
            reload_after_rollback(app, &reload);
            error
        }
        Err(rollback) => format!("{error}; rollback failed: {rollback}"),
    }
}

/// A rolled-back package may already be running the new artifact in memory.
/// Load the restored file again, but only for plugins that were loaded.
fn reload_after_rollback(app: &mut App, names: &[String]) {
    if names.is_empty() {
        return;
    }
    let loaded: BTreeSet<String> = crate::plugins::loaded_names(app).into_iter().collect();
    let Some(plugins) = app.packages.dir() else {
        return;
    };
    for name in names {
        if loaded.contains(name) {
            crate::plugins::load(app, &plugins.join(format!("{name}.wasm")));
        }
    }
}

pub(crate) fn abort_update(app: &mut App, error: String) {
    let error = abort_update_message(app, error);
    app.error(error);
}

pub(crate) fn check_source(app: &App, kind: PackageSourceKind, source: &str) -> Result<(), String> {
    match &app.packages.allowed_sources {
        Some(list) => list.check(kind, source),
        None => Ok(()),
    }
}

/// What a headless command may do without asking. Interactive use leaves
/// these false and keeps the prompts.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Approvals {
    pub build: bool,
    pub capabilities: bool,
    pub grants: bool,
}

impl Approvals {
    pub const BUILD_FLAG: &'static str = "--allow-build";
    pub const BUILD_CONFIG: &'static str = "forge.opt[\"package_allow_build\"]";
    pub const CAPABILITIES_FLAG: &'static str = "--allow-capabilities";
    pub const CAPABILITIES_CONFIG: &'static str = "forge.opt[\"package_allow_capabilities\"]";
    pub const GRANTS_FLAG: &'static str = "--allow-grants";
    pub const GRANTS_CONFIG: &'static str = "forge.opt[\"package_allow_grants\"]";

    pub fn build_hint() -> String {
        format!(
            "Use {} or set {} = true to allow it.",
            Self::BUILD_FLAG,
            Self::BUILD_CONFIG
        )
    }

    pub fn capabilities_hint() -> String {
        format!(
            "Use {} or set {} = true to allow it.",
            Self::CAPABILITIES_FLAG,
            Self::CAPABILITIES_CONFIG
        )
    }

    pub fn grants_hint() -> String {
        format!(
            "Use {} or set {} = true to allow it.",
            Self::GRANTS_FLAG,
            Self::GRANTS_CONFIG
        )
    }
}

#[derive(Default)]
pub struct Packages {
    pub indexes: Vec<String>,
    /// Trusted Minisign public keys for package indexes (rotation: several).
    pub index_keys: Vec<String>,
    /// Refuse unsigned package indexes.
    pub index_strict: bool,
    /// Hosts, URLs and paths package sources may come from.
    pub allowed_sources: Option<SourceAllowlist>,
    /// Where plugins are installed (None: the default plugins directory).
    pub plugin_dir: Option<PathBuf>,
    pub index: Option<Index>,
    pub(crate) manager_filter: String,
    pub(crate) manager_view: crate::package_manager_ui::View,
    pub(crate) manager_previous_view: crate::package_manager_ui::View,
    pub(crate) manager_selected_row: usize,
    pub(crate) manager_expanded: BTreeSet<String>,
    pub(crate) manager_tick: u64,
    pub(crate) manager_update: Option<crate::package_manager_ui::UpdateSnapshot>,
    pub(crate) manager_findings: Option<Vec<Finding>>,
    pub(crate) manager_check: Option<Result<String, String>>,
    pub(crate) manager_selected: Option<String>,
    /// Downloads under way: each on its own thread, answering here.
    downloads: Vec<(u64, Receiver<Result<(), String>>)>,
    next_download: u64,
    fetches: BTreeMap<u64, Fetch>,
    upgrade_queue: VecDeque<Package>,
    pending_upgrade: Option<Package>,
    pending_capability: Option<PendingCapability>,
    upgrade_git_after_index: bool,
    /// Indexes read so far in a refresh, and what to do after.
    refreshing: Option<(Index, Vec<String>, Then)>,
    /// Durable rollback state for an in-progress update.
    pub(crate) update: UpdateTransaction,
    /// Approvals a headless command was given.
    pub approvals: Approvals,
    /// Why an unattended package operation stopped for approval.
    pub approval_blocked: Option<String>,
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

pub(crate) fn db(app: &App) -> Option<InstalledDb> {
    app.packages.dir().map(|d| InstalledDb::load(&d))
}

fn index_source(location: &str) -> Source {
    Source::new(SourceKind::Index).location(location)
}

fn package_source(package: &Package) -> Source {
    let source = if package::local_path(&package.url).is_some() {
        Source::new(SourceKind::Path).location(&package.url)
    } else {
        Source::new(SourceKind::Url).location(&package.url)
    };
    match &package.rev {
        Some(revision) => source.revision(revision),
        None => source,
    }
}

fn log_fetch<T>(timer: Timer, package: Option<&str>, source: Source, result: &Result<T, String>) {
    let status = if result.is_ok() {
        Status::Ok
    } else {
        Status::Failed
    };
    let mut event = timer
        .finish(Operation::Fetch, status, package)
        .source(source);
    if let Err(error) = result {
        event = event.error(error.clone());
    }
    crate::package_log::record(event);
}

fn log_package_fetch(timer: Timer, package: &Package, result: &Result<Vec<u8>, String>) {
    let status = if result.is_ok() {
        Status::Ok
    } else {
        Status::Failed
    };
    let mut event = timer
        .finish(Operation::Fetch, status, Some(&package.name))
        .source(package_source(package))
        .detail("version", &package.version);
    if !package.sha256.is_empty() {
        event = event.detail("sha256", &package.sha256);
    }
    if let Err(error) = result {
        event = event.error(error.clone());
    }
    crate::package_log::record(event);
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
        "package-manager-toggle" => crate::package_manager_ui::toggle_selected(app),
        "package-manager-back" => crate::package_manager_ui::back(app),
        "package-manager-show-home" => crate::package_manager_ui::show_home(app),
        "package-manager-show-install" => crate::package_manager_ui::show_install(app),
        "package-manager-show-update" => crate::package_manager_ui::show_update(app),
        "package-manager-show-check" => crate::package_manager_ui::show_check(app),
        "package-manager-show-log" => crate::package_manager_ui::show_log(app),
        "package-manager-show-restore" => crate::package_manager_ui::show_restore(app),
        "package-manager-show-profile" => crate::package_manager_ui::show_profile(app),
        "package-manager-show-doctor" => crate::package_manager_ui::show_doctor(app),
        "package-manager-show-help" => crate::package_manager_ui::show_help(app),
        "package-manager-upgrade-all" => {
            app.packages.manager_previous_view = app.packages.manager_view;
            app.packages.manager_view = crate::package_manager_ui::View::Update;
            with_index(app, Then::UpgradeAll);
        }
        "package-manager-filter" => app.prompt(
            "Filter packages: ",
            crate::minibuffer::Purpose::PackageFilter,
        ),
        "package-manager-clear-filter" => {
            app.packages.manager_filter.clear();
            manager(app);
        }
        "list-packages" | "package-list-packages" => with_index(app, Then::List),
        "package-install" => {
            let force = app.raw_arg();
            with_index(app, Then::Install { force });
        }
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
        "package-doctor" => crate::package_doctor::show(app),
        "package-log" => crate::package_log::show(app),
        "package-log-export" => crate::package_log::export(app),
        "plugin-repro" => {
            let names = crate::plugin_repro::candidates(app);
            if names.is_empty() {
                app.error("No plugins are declared by use_package");
            } else {
                app.prompt(
                    "Write reproduction for plugin: ",
                    Purpose::Choose {
                        what: Choice::ReproPlugin,
                        candidates: names,
                    },
                );
            }
        }
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
            let force = app.raw_arg();
            if let Some(name) = selected_name(app) {
                install_selected(app, &name, force);
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
    let lock = match crate::declare::load_lock() {
        Ok(lock) => lock,
        Err(error) => return app.error(format!("Couldn't save package snapshot {name}: {error}")),
    };
    let saved = lock.save_snapshot_with(name, |plugin| {
        crate::declare::declared_component_path(app, plugin)
            .unwrap_or_else(|| plugins.join(format!("{plugin}.wasm")))
    });
    match saved {
        Ok(()) => app.msg(format!("Saved package snapshot {name}")),
        Err(e) => app.error(format!("Couldn't save package snapshot {name}: {e}")),
    }
}

fn show_snapshots(app: &mut App) {
    use crate::package_ui::Text;
    use forge_core::decoration::PackageFace;

    let Some(config) = config_dir() else {
        return app.error("No config directory for package snapshots");
    };
    match LockFile::snapshots(&config) {
        Ok(names) if names.is_empty() => app.msg("No package snapshots"),
        Ok(names) => {
            let mut text = Text::new();
            text.push_face("Package Snapshots", PackageFace::Title);
            text.newline();
            text.newline();
            for name in names {
                text.push_face("  ", PackageFace::Rule);
                text.push_face(&name, PackageFace::Primary);
                text.newline();
            }
            text.show(app, "*Package Snapshots*", "special-mode", None);
        }
        Err(e) => app.error(format!("Couldn't list package snapshots: {e}")),
    }
}

fn show_snapshot_diff(app: &mut App, before: &str, after: &str) {
    use crate::package_ui::{Status, Symbols, Text};
    use forge_core::decoration::PackageFace;

    let Some(config) = config_dir() else {
        return app.error("No config directory for package snapshots");
    };
    match LockFile::diff_snapshots(&config, before, after) {
        Ok(changes) => {
            let symbols = Symbols::for_app(app);
            let mut text = Text::new();
            let title = "Package Snapshot Diff";
            text.push_face(title, PackageFace::Title);
            let versions = format!("{before}{}{after}", symbols.arrow);
            let width = app.win().width.max(40);
            let gap = width.saturating_sub(
                crate::package_ui::display_width(title)
                    + crate::package_ui::display_width(&versions),
            );
            if gap > 0 {
                text.push(&" ".repeat(gap));
            }
            text.push_face(&versions, PackageFace::Heading);
            text.newline();
            text.newline();
            if changes.is_empty() {
                text.push("No changes\n");
            }
            for change in changes {
                let status = match change.change.as_str() {
                    "added" => Status::Ok,
                    "removed" => Status::Error,
                    "changed" => Status::Warning,
                    _ => Status::Neutral,
                };
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
                text.push("  ");
                text.push_status(status, &change.change, 10, false, symbols);
                text.push("  ");
                text.push_face(&change.name, PackageFace::Primary);
                text.push(": ");
                text.push(&format!("{old}{}{new}", symbols.arrow));
                text.newline();
            }
            text.show(app, "*Package Snapshot Diff*", "special-mode", None);
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
    decorate_package_text(app, id);
    crate::compile::display_buffer(app, id);
}

fn decorate_package_text(app: &mut App, id: forge_core::BufferId) {
    use forge_core::decoration::{DecoFace, DecoSpec, PackageFace};

    let Some(b) = app.editor.state.get_buffer_mut(id) else {
        return;
    };
    let mut decos = Vec::new();
    for line in 0..b.line_count() {
        let text = b.line(line);
        if text.is_empty() {
            continue;
        }
        let trimmed = text.trim_start();
        let start = b.pos_to_offset(forge_core::Pos::new(line, 0));
        if line == 0
            || trimmed.starts_with("Package upgrade plan")
            || trimmed.starts_with("Package upgrade review")
        {
            decos.push(DecoSpec::new(
                start,
                start + text.chars().count(),
                DecoFace::Package(PackageFace::Title),
            ));
            continue;
        }
        let mut marked = false;
        for (word, face) in [
            ("WARNING", PackageFace::Warning),
            ("BREAKING", PackageFace::Error),
        ] {
            if let Some(byte) = text.find(word) {
                let at = start + text[..byte].chars().count();
                decos.push(DecoSpec::new(
                    at,
                    at + word.chars().count(),
                    DecoFace::Package(face),
                ));
                marked = true;
                break;
            }
        }
        if marked {
            continue;
        }
        if trimmed.ends_with(':') || trimmed.starts_with("These additions need") {
            decos.push(DecoSpec::new(
                start,
                start + text.chars().count(),
                DecoFace::Package(PackageFace::Heading),
            ));
        }
    }
    b.set_decorations("package-text", decos);
}

/// One upgrade review screen shared by index and Git upgrades.
pub(crate) struct UpgradeReview<'a> {
    pub name: &'a str,
    pub old_version: &'a str,
    pub new_version: &'a str,
    pub new: &'a Grants,
    pub diff: &'a CapabilityDiff,
    pub build: Option<&'a str>,
    pub recipe_changes: &'a [String],
    pub commits: Option<&'a str>,
    pub notes: Option<&'a str>,
}

pub(crate) fn show_capability_review(app: &mut App, review: UpgradeReview<'_>) {
    let width = app.win().width.max(20);
    let symbols = Symbols::for_app(app);
    let mut text = Text::new();
    package_ui::title(
        &mut text,
        "Upgrade Review",
        &format!(
            "{} {} {} {}",
            review.name, review.old_version, symbols.arrow, review.new_version
        ),
        width,
        symbols,
    );
    text.push_face(&symbols.rule.repeat(width), PackageFace::Rule);
    text.newline();
    text.newline();
    text.push_face(review.name, PackageFace::Heading);
    text.newline();
    text.newline();
    package_ui::band(
        &mut text,
        UiStatus::Warning,
        "unattended upgrades stop and change nothing",
        width,
        symbols,
    );
    text.newline();

    let additions = &review.diff.additions;
    let removals = &review.diff.removals;
    let unchanged: Vec<CapabilityChange> = capability_list(review.new)
        .into_iter()
        .filter(|change| !additions.contains(change))
        .collect();
    package_ui::heading(
        &mut text,
        &format!(
            "Capabilities (+{} -{} ={})",
            additions.len(),
            removals.len(),
            unchanged.len()
        ),
        width,
        symbols,
    );
    text.newline();
    for change in additions {
        push_capability_change(
            &mut text,
            "+",
            PackageFace::Warning,
            change,
            true,
            width,
            symbols,
        );
    }
    for change in removals {
        push_capability_change(
            &mut text,
            "-",
            PackageFace::Inactive,
            change,
            false,
            width,
            symbols,
        );
    }
    for change in &unchanged {
        push_capability_change(
            &mut text,
            "=",
            PackageFace::Ok,
            change,
            false,
            width,
            symbols,
        );
    }

    if let Some(commits) = review.commits.filter(|text| !text.trim().is_empty()) {
        text.newline();
        package_ui::heading(&mut text, "Changes", width, symbols);
        text.newline();
        for line in commits.lines() {
            let upper = line.to_ascii_uppercase();
            let face = if upper.contains("BREAKING") {
                PackageFace::Error
            } else if upper.contains("WARNING") {
                PackageFace::Warning
            } else {
                PackageFace::Neutral
            };
            let line = package_ui::fit(line, width, symbols.ellipsis);
            text.push_face(&line, face);
            text.newline();
        }
    }

    if review.build.is_some() || !review.recipe_changes.is_empty() {
        text.newline();
        package_ui::heading(&mut text, "Build", width, symbols);
        text.newline();
        if let Some(build) = review.build {
            text.push_face("  command: ", PackageFace::Heading);
            text.push_face(build, PackageFace::Primary);
            text.newline();
        }
        for change in review.recipe_changes {
            text.push_face(
                &package_ui::fit(&format!("  {change}"), width, symbols.ellipsis),
                PackageFace::Warning,
            );
            text.newline();
        }
    }

    if let Some(notes) = review.notes.filter(|notes| !notes.trim().is_empty()) {
        text.newline();
        package_ui::heading(&mut text, "Release notes", width, symbols);
        text.newline();
        for line in notes.lines() {
            text.push_face(
                &package_ui::fit(line, width, symbols.ellipsis),
                PackageFace::Inactive,
            );
            text.newline();
        }
    }

    text.newline();
    text.push_face("y", PackageFace::Accent);
    text.push(" apply upgrade   ");
    text.push_face("n", PackageFace::Accent);
    text.push(" skip   ");
    text.push_face("C-g", PackageFace::Accent);
    text.push(" cancel");
    text.newline();
    text.show(app, "*Package Upgrade Review*", "special-mode", None);
}

fn capability_list(grants: &Grants) -> Vec<CapabilityChange> {
    let mut changes = Vec::new();
    if grants.network {
        changes.push(CapabilityChange::Network);
    }
    if grants.http {
        changes.push(CapabilityChange::Http);
    }
    if grants.environment {
        changes.push(CapabilityChange::Environment);
    }
    for dir in &grants.directories {
        changes.push(CapabilityChange::Directory {
            path: dir.path.clone(),
            write: dir.write,
        });
    }
    if let Some(memory) = grants.memory_mb {
        changes.push(CapabilityChange::Memory {
            from: memory,
            to: memory,
        });
    }
    for process in &grants.processes {
        changes.push(CapabilityChange::Process(process.clone()));
    }
    changes
}

fn push_capability_change(
    text: &mut Text,
    mark: &str,
    mark_face: PackageFace,
    change: &CapabilityChange,
    risk: bool,
    width: usize,
    symbols: Symbols,
) {
    let prefix = format!("  {mark} ");
    let risk_label = "higher risk";
    let risk_width = if risk {
        package_ui::display_width(risk_label) + 2
    } else {
        0
    };
    let room =
        width.saturating_sub(package_ui::display_width(&prefix) + risk_width + usize::from(risk));
    let description = package_ui::fit(&change.describe(), room, symbols.ellipsis);
    text.push_face(&prefix, mark_face);
    text.push(&description);
    if risk {
        let gap = width.saturating_sub(
            package_ui::display_width(&prefix)
                + package_ui::display_width(&description)
                + risk_width,
        );
        if gap > 0 {
            text.push(&" ".repeat(gap));
        }
        package_ui::chip(text, risk_label, PackageFace::Risk);
    }
    text.newline();
}

fn restore_snapshot(app: &mut App, name: &str) {
    let (Some(config), Some(plugins)) = (config_dir(), app.packages.dir()) else {
        return app
            .error("Cannot restore a package snapshot without config and plugin directories");
    };
    if let Err(e) = LockFile::restore_snapshot(&config, &plugins, name) {
        return app.error(format!("Couldn't restore package snapshot {name}: {e}"));
    }
    let restored = match LockFile::load(&config) {
        Ok(lock) => lock,
        Err(error) => {
            return app.error(format!(
                "Snapshot {name} restored, but the package lock couldn't be read: {error}"
            ));
        }
    };
    if let Err(e) = sync_installed_metadata(app, &restored.packages) {
        return app.error(format!(
            "Snapshot {name} restored, but packages.toml wasn't updated: {e}"
        ));
    }
    app.msg(format!("Restored package snapshot {name}"));
}

fn restore_locked(app: &mut App) {
    let lock = match crate::declare::load_lock() {
        Ok(lock) => lock,
        Err(error) => return app.error(format!("Can't read the package lock: {error}")),
    };
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
    let locked: BTreeSet<&str> = artifacts.iter().map(|(name, _, _)| name.as_str()).collect();
    for name in current.packages.keys() {
        if !locked.contains(name.as_str()) {
            differences.push(format!("{name}: installed -> removed"));
        }
    }
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
                    recipe: entry
                        .recipe
                        .clone()
                        .or_else(|| previous.and_then(|p| p.recipe.clone())),
                },
            );
            db_changed = true;
        }
    }
    let keep: BTreeSet<String> = artifacts.iter().map(|(name, _, _)| name.clone()).collect();
    let removed = db.prune_except(&keep)?;
    if db_changed && removed.is_empty() {
        db.save().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Restore one content-addressed lock artifact, updating the installed
/// record if the lock describes a package-managed plugin.
pub(crate) fn restore_locked_entry(
    app: &App,
    name: &str,
    entry: &forge_core::LockEntry,
    bytes: &[u8],
) -> Result<PathBuf, String> {
    let Some(plugins) = app.packages.dir() else {
        return Err("no plugins directory".into());
    };
    let mut db = InstalledDb::load(&plugins);
    let installed = installed_from_lock(entry, db.packages.get(name));
    db.restore_locked(name, installed, bytes)
        .map_err(|error| format!("{name}: {error}"))
}

fn installed_from_lock(entry: &forge_core::LockEntry, previous: Option<&Installed>) -> Installed {
    Installed {
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
        recipe: entry
            .recipe
            .clone()
            .or_else(|| previous.and_then(|p| p.recipe.clone())),
    }
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
                    recipe: entry
                        .recipe
                        .clone()
                        .or_else(|| previous.and_then(|p| p.recipe.clone())),
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
    app.packages.manager_findings = None;
    app.packages.manager_check = None;
    let locations = app.packages.indexes.clone();
    app.packages.refreshing = Some((Index::default(), Vec::new(), then));
    let mut remote = 0;
    for loc in locations {
        let started = Timer::start();
        if let Err(error) = check_source(app, PackageSourceKind::Index, &loc) {
            log_fetch(
                started,
                None,
                index_source(&loc),
                &Err::<String, String>(error.clone()),
            );
            index_read(app, &loc, Err(error));
            continue;
        }
        match package::local_path(&loc) {
            Some(path) => {
                let r = read_local_index(app, &loc, &path);
                log_fetch(started, None, index_source(&loc), &r);
                index_read(app, &loc, r);
            }
            None => {
                let file = temp_file("index");
                let signature = index_needs_signature(app).then(|| temp_file("index-signature"));
                match download_index(app, &loc, &file, signature.as_deref()) {
                    Ok(id) => {
                        app.packages.fetches.insert(
                            id,
                            Fetch::Index {
                                location: loc,
                                file,
                                signature,
                                started,
                            },
                        );
                        remote += 1;
                    }
                    Err(e) => {
                        log_fetch(
                            started,
                            None,
                            index_source(&loc),
                            &Err::<String, String>(e.clone()),
                        );
                        index_read(app, &loc, Err(e));
                    }
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

fn index_needs_signature(app: &App) -> bool {
    !app.packages.index_keys.is_empty() || app.packages.index_strict
}

fn read_signature(path: &std::path::Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(signature) => Ok(Some(signature)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

fn verify_index_bytes(
    app: &App,
    location: &str,
    bytes: &[u8],
    signature: Option<String>,
) -> Result<String, String> {
    package::verify_index(
        location,
        bytes,
        signature.as_deref(),
        &app.packages.index_keys,
        app.packages.index_strict,
    )?;
    String::from_utf8(bytes.to_vec()).map_err(|e| format!("{location}: index is not UTF-8: {e}"))
}

fn read_local_index(app: &App, location: &str, path: &std::path::Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{location}: {e}"))?;
    let signature = if index_needs_signature(app) {
        let signature_path = package::local_path(&package::signature_location(location))
            .ok_or_else(|| format!("{location}: can't find the signature path"))?;
        read_signature(&signature_path).map_err(|e| format!("{location}: {e}"))?
    } else {
        None
    };
    verify_index_bytes(app, location, &bytes, signature)
}

fn read_downloaded_index(
    app: &App,
    location: &str,
    file: &std::path::Path,
    signature: Option<&std::path::Path>,
) -> Result<String, String> {
    let bytes = std::fs::read(file).map_err(|e| format!("{location}: {e}"))?;
    let signature = match signature {
        Some(path) => read_signature(path).map_err(|e| format!("{location}: {e}"))?,
        None => None,
    };
    verify_index_bytes(app, location, &bytes, signature)
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
        Then::Install { force } => {
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
                        what: Choice::InstallPackage { force },
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
                    finish_update(app);
                }
            } else {
                app.packages.upgrade_git_after_index = true;
                start_upgrade_queue(app, todo, true);
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

pub(crate) fn upgrade_candidates(app: &App, index: &Index) -> Result<Vec<Package>, String> {
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

pub(crate) fn dry_run(app: &App, index: &Index, include_git: bool) -> Result<String, String> {
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
    start_upgrade_queue(app, vec![selected], false);
}

fn start_upgrade_queue(app: &mut App, packages: Vec<Package>, batch: bool) {
    if let Err(error) = begin_update(app, if batch { "upgrade-all" } else { "upgrade" }, batch) {
        return app.error(format!("Can't start package update: {error}"));
    }
    crate::package_manager_ui::update_start_index(app, &packages, batch);
    app.packages.upgrade_queue = packages.into();
    next_upgrade_prompt(app);
}

fn next_upgrade_prompt(app: &mut App) {
    let Some(package) = app.packages.upgrade_queue.pop_front() else {
        if std::mem::take(&mut app.packages.upgrade_git_after_index) {
            if crate::package_vc::upgrade_all(app) == 0 {
                finish_update(app);
            }
        } else {
            finish_update(app);
        }
        return;
    };
    crate::package_manager_ui::update_running(app, &package.name, "installing");
    if app.unattended {
        app.msg(format!("Applying upgrade for {}", package.name));
        if !install(app, package.clone(), false) && app.packages.pending_capability.is_none() {
            next_upgrade_prompt(app);
        }
        return;
    }
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
        app.msg(format!("Applying upgrade for {}", package.name));
        if !install(app, package.clone(), false) && app.packages.pending_capability.is_none() {
            next_upgrade_prompt(app);
        }
    } else {
        crate::package_manager_ui::update_skipped(app, &package.name, "declined");
        app.msg(format!("Skipped upgrade for {}", package.name));
        next_upgrade_prompt(app);
    }
}

pub fn package_capabilities_confirmed(app: &mut App, yes: bool) {
    let Some(pending) = app.packages.pending_capability.take() else {
        return;
    };
    if yes {
        app.msg(format!("Applying upgrade for {}", pending.package.name));
        install_ready(app, pending.package, pending.bytes, true);
        next_upgrade_prompt(app);
    } else {
        crate::package_manager_ui::update_skipped(
            app,
            &pending.package.name,
            "capabilities not approved",
        );
        app.msg(format!(
            "Skipped upgrade for {}: {} new capabilit{} not approved",
            pending.package.name,
            pending.diff.additions.len(),
            if pending.diff.additions.len() == 1 {
                "y was"
            } else {
                "ies were"
            }
        ));
        next_upgrade_prompt(app);
    }
}

/// Show declared, installed, and indexed packages without requiring an index.
pub fn manager(app: &mut App) {
    crate::package_manager_ui::show(app);
}

fn show_details(app: &mut App, name: &str) {
    let text = details(app, name);
    if text.is_empty() {
        return app.error(format!("No declared or installed package {name}"));
    }
    app.show_special_buffer("*Package Details*", "special-mode", &text);
}

pub(crate) fn details(app: &App, name: &str) -> String {
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
    crate::package_manager_ui::navigate(app, delta);
}

pub(crate) fn manager_matches(
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
        if !crate::package_manager_ui::selectable(app) {
            app.error("No package selected in this view");
            return None;
        }
        return app.packages.manager_selected.clone();
    }
    name_on_line(app)
}

pub(crate) fn capability_additions(app: &App, name: &str) -> Vec<String> {
    let Some(pending) = app.packages.pending_capability.as_ref() else {
        return Vec::new();
    };
    if pending.package.name != name {
        return Vec::new();
    }
    pending
        .diff
        .additions
        .iter()
        .map(short_capability)
        .collect()
}

fn short_capability(change: &forge_core::CapabilityChange) -> String {
    use forge_core::CapabilityChange;
    match change {
        CapabilityChange::Network => "network".into(),
        CapabilityChange::Http => "http".into(),
        CapabilityChange::Environment => "environment".into(),
        CapabilityChange::Directory { path, write } => {
            format!("files:{}{path}", if *write { "rw:" } else { "r:" })
        }
        CapabilityChange::Process(program) => format!("process:{program}"),
        CapabilityChange::Memory { to, .. } => format!("memory:{to}mb"),
    }
}

fn install_selected(app: &mut App, name: &str, force: bool) {
    if let Some(u) = crate::declare::declared(app, name)
        && let Some(index) = u.index.clone()
    {
        return install_named_version(app, &index, u.version.as_deref(), force);
    }
    if app
        .packages
        .index
        .as_ref()
        .is_some_and(|i| i.packages.contains_key(name))
    {
        return install_named_with_force(app, name, force);
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
        install_named_with_force(app, &index, force);
    } else {
        app.error(format!("{name} has no install source"));
    }
}

pub fn filter_manager(app: &mut App, filter: &str) {
    app.packages.manager_filter = filter.to_string();
    manager(app);
}

/// A candidate component's `forge:editor` compatibility, if its bytes are
/// available without downloading (a local index entry, or a cached artifact).
fn package_compatibility(app: &App, package: &Package) -> Option<String> {
    let bytes = package_bytes(package)?;
    component_compatibility(app, &package.name, &bytes)
}

fn package_bytes(package: &Package) -> Option<Vec<u8>> {
    if let Some(path) = package::local_path(&package.url)
        && let Ok(bytes) = std::fs::read(path)
    {
        return Some(bytes);
    }
    let config = forge_core::config::config_dir()?;
    if package.sha256.is_empty() {
        return None;
    }
    forge_core::LockFile::artifact(&config, &package.sha256).ok()
}

/// Check a component's imports against what this Forge links, without
/// running any of its code.
fn component_compatibility(app: &App, name: &str, bytes: &[u8]) -> Option<String> {
    if let Some(host) = app.plugin_host.as_ref() {
        return host.compatibility_problem(name, bytes).ok().flatten();
    }
    let cache = app.plugin_cache.as_ref().map(|dir| dir.join("wasmtime"));
    let runtime = forge_core::PluginRuntime::with_cache(cache.as_deref()).ok()?;
    let inspection = runtime.inspect(name, bytes).ok()?;
    runtime.compatibility_problem(&inspection)
}

/// The interfaces a component's code actually imports, without running it.
pub(crate) fn component_imports(name: &str, bytes: &[u8]) -> Result<Vec<String>, String> {
    let runtime = forge_core::PluginRuntime::new()
        .map_err(|error| format!("starting component inspection: {error:#}"))?;
    runtime
        .inspect(name, bytes)
        .map(|inspection| inspection.used_imports)
        .map_err(|error| format!("inspecting component imports: {error:#}"))
}

/// Refuse a component that imports a permission its manifest doesn't
/// declare: an import is not an implicit grant.
pub(crate) fn import_mismatch(
    name: &str,
    manifest: Option<&Manifest>,
    imports: &[String],
) -> Option<String> {
    let mismatches = forge_core::uncovered_imports(manifest, imports);
    if mismatches.is_empty() {
        return None;
    }
    let details = mismatches
        .iter()
        .map(forge_core::ImportMismatch::describe)
        .collect::<Vec<_>>()
        .join("; ");
    Some(format!(
        "{name}: the component imports permissions its manifest doesn't declare: {details}. \
         Add them to [permissions] or fix the plugin."
    ))
}

/// The *Packages* buffer.
pub fn list(app: &mut App) {
    let Some(index) = app.packages.index.clone() else {
        return;
    };
    let installed = db(app).map(|d| d.packages).unwrap_or_default();
    let symbols = crate::package_ui::Symbols::for_app(app);

    struct Row {
        name: String,
        version: String,
        status: String,
        kind: crate::package_ui::Status,
        description: String,
        problem: Option<String>,
    }

    let mut names: Vec<&String> = index.packages.keys().chain(installed.keys()).collect();
    names.sort();
    names.dedup();
    let mut rows = Vec::with_capacity(names.len());
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
                format!("git {}", from_git.clone().unwrap_or_default()),
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
        let problem = avail.and_then(|a| package_compatibility(app, a));
        rows.push(Row {
            name: name.clone(),
            version,
            kind: crate::package_ui::Status::of(&status),
            status,
            description: avail.map(|a| a.description.clone()).unwrap_or_default(),
            problem,
        });
    }

    let width = app.win().width.max(40);
    let name_w = rows
        .iter()
        .map(|r| crate::package_ui::display_width(&r.name))
        .max()
        .unwrap_or(4)
        .clamp(4, 20);
    let version_w = rows
        .iter()
        .map(|r| crate::package_ui::display_width(&r.version))
        .max()
        .unwrap_or(7)
        .clamp(7, 14);
    let status_w = rows
        .iter()
        .map(|r| crate::package_ui::display_width(&r.status) + 2)
        .max()
        .unwrap_or(9)
        .clamp(9, 22);
    let desc_w = width
        .saturating_sub(2 + name_w + 2 + version_w + 2 + status_w + 2)
        .max(8);
    let table_w = 2 + name_w + 2 + version_w + 2 + status_w + 2 + desc_w;

    let mut text = crate::package_ui::Text::new();
    let title = "Packages";
    let hint = "RET install or upgrade  d delete  g refresh";
    text.push_face(title, forge_core::decoration::PackageFace::Title);
    let gap = width.saturating_sub(
        crate::package_ui::display_width(title) + crate::package_ui::display_width(hint),
    );
    if gap > 0 {
        text.push(&" ".repeat(gap));
    }
    text.push_face(hint, forge_core::decoration::PackageFace::Heading);
    text.newline();
    text.newline();

    let header = format!(
        "  {}  {}  {}  {}",
        crate::package_ui::pad("Name", name_w, symbols.ellipsis),
        crate::package_ui::pad("Version", version_w, symbols.ellipsis),
        crate::package_ui::pad("Status", status_w, symbols.ellipsis),
        crate::package_ui::pad("Description", desc_w, symbols.ellipsis),
    );
    text.push_face(&header, forge_core::decoration::PackageFace::Heading);
    text.newline();
    text.push_face(
        &symbols.rule.repeat(table_w.min(width)),
        forge_core::decoration::PackageFace::Rule,
    );
    text.newline();
    for row in &rows {
        text.push("  ");
        text.push(&crate::package_ui::pad(&row.name, name_w, symbols.ellipsis));
        text.push("  ");
        text.push(&crate::package_ui::pad(
            &row.version,
            version_w,
            symbols.ellipsis,
        ));
        text.push("  ");
        text.push_status(row.kind, &row.status, status_w, false, symbols);
        text.push("  ");
        let mut description = row.description.clone();
        if let Some(problem) = &row.problem {
            if !description.is_empty() {
                description.push_str("  ");
            }
            description.push('[');
            description.push_str(problem);
            description.push(']');
        }
        text.push(&crate::package_ui::pad(
            &description,
            desc_w,
            symbols.ellipsis,
        ));
        text.newline();
    }
    if rows.is_empty() {
        text.push("No packages in the index.\n");
    }
    text.newline();
    text.push_face(
        "RET install or upgrade  d delete  g refresh  q quit",
        forge_core::decoration::PackageFace::Heading,
    );
    text.show(
        app,
        BUFFER,
        "package-menu-mode",
        Some(forge_core::Pos::new(if rows.is_empty() { 2 } else { 4 }, 0)),
    );
}

fn name_on_line(app: &mut App) -> Option<String> {
    let b = app.buf_ref();
    let line = b.line(b.point().line);
    let name = line
        .split_whitespace()
        .find(|word| !matches!(*word, "▌" | "|"))
        .map(String::from);
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
        false,
    );
}

/// RET in project-find-file style prompts: see minibuffer.rs.
pub fn install_named(app: &mut App, name: &str) {
    install_named_with_force(app, name, false);
}

pub(crate) fn install_named_with_force(app: &mut App, name: &str, force: bool) {
    install_named_version(app, name, None, force);
}

fn install_named_version(app: &mut App, name: &str, requirement: Option<&str>, force: bool) {
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
        start_upgrade_queue(app, vec![package], false);
    } else {
        let _ = install(app, package, force);
    }
}

/// Start installing a package; returns true when a download will finish later.
fn install(app: &mut App, p: Package, force: bool) -> bool {
    let upgrading = db(app).is_some_and(|db| db.packages.contains_key(&p.name));
    let (kind, source) = match &p.git {
        Some(git) => (PackageSourceKind::Git, git.as_str()),
        None => (PackageSourceKind::Package, p.url.as_str()),
    };
    if let Err(error) = check_source(app, kind, source) {
        let message = format!("Can't install {}: {error}", p.name);
        if upgrading {
            crate::package_manager_ui::update_failed(app, &p.name, &message);
            abort_update(app, message);
        } else {
            app.error(message);
        }
        return false;
    }
    if let Some(git) = &p.git {
        crate::package_vc::install(app, &p.name, git, p.rev.clone(), upgrading);
        return false;
    }
    let started = Timer::start();
    match package::local_path(&p.url) {
        Some(path) => {
            let r = std::fs::read(&path).map_err(|e| format!("{}: {e}", p.url));
            log_package_fetch(started, &p, &r);
            installed(app, p, r, upgrading, force);
            false
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
                            force,
                            started,
                        },
                    );
                    true
                }
                Err(error) => {
                    log_package_fetch(started, &p, &Err(error.clone()));
                    let message = format!("Can't install {}: {error}", p.name);
                    if upgrading {
                        crate::package_manager_ui::update_failed(app, &p.name, &message);
                        abort_update(app, message);
                    } else {
                        app.error(message);
                    }
                    false
                }
            }
        }
    }
}

fn candidate_manifest(package: &Package, bytes: &[u8]) -> Result<Option<Manifest>, String> {
    match package::local_path(&package.url) {
        Some(path) => Manifest::for_component(&path, bytes),
        None => Manifest::embedded(bytes),
    }
}

fn candidate_capability_diff(
    app: &App,
    package: &Package,
    bytes: &[u8],
) -> Result<(Grants, Grants, CapabilityDiff), String> {
    let (_, config, _) = update_paths(app)?;
    let old = LockFile::load(&config)
        .map_err(|e| format!("can't read the package lock: {e}"))?
        .get(&package.name)
        .map(|entry| entry.capability_grants().clone())
        .unwrap_or_default();
    let new = candidate_manifest(package, bytes)?
        .map(|manifest| manifest.permissions)
        .unwrap_or_default();
    let diff = CapabilityDiff::between(&old, &new);
    Ok((old, new, diff))
}

fn unattended_capability_error(diff: &CapabilityDiff) -> String {
    format!(
        "it asks for new capabilities that need confirmation: {}. No changes were made.",
        diff.describe_additions().join("; ")
    )
}

/// Ask about new capabilities (or stop an unattended upgrade). Returns
/// true when installation may proceed now.
fn capability_gate(app: &mut App, package: &Package, bytes: &[u8]) -> Result<bool, String> {
    let (_old, new, diff) = candidate_capability_diff(app, package, bytes)?;
    if !diff.has_additions() {
        return Ok(true);
    }
    if app.unattended {
        if app.packages.approvals.capabilities {
            return Ok(true);
        }
        let error = format!(
            "{} {}",
            unattended_capability_error(&diff),
            Approvals::capabilities_hint()
        );
        app.packages.approval_blocked = Some(error.clone());
        return Err(error);
    }
    let installed = db(app).and_then(|db| db.packages.get(&package.name).cloned());
    let old_version = installed
        .as_ref()
        .map(|installed| installed.version.as_str())
        .unwrap_or("not installed");
    let build = installed
        .as_ref()
        .and_then(|installed| installed.build.as_deref());
    show_capability_review(
        app,
        UpgradeReview {
            name: &package.name,
            old_version,
            new_version: &package.version,
            new: &new,
            diff: &diff,
            build,
            recipe_changes: &[],
            commits: None,
            notes: package.release_notes.as_deref(),
        },
    );
    app.packages.pending_capability = Some(PendingCapability {
        package: package.clone(),
        bytes: bytes.to_vec(),
        diff,
    });
    app.prompt(
        format!(
            "Upgrade {} and allow new capabilities? (y or n) ",
            package.name
        ),
        Purpose::Confirm(crate::minibuffer::ConfirmAction::IndexPackageCapabilities),
    );
    Ok(false)
}

/// A download finished (or a local file was read): check, install, load.
fn installed(
    app: &mut App,
    p: Package,
    bytes: Result<Vec<u8>, String>,
    upgrade: bool,
    force: bool,
) {
    let bytes = match bytes {
        Ok(bytes) => bytes,
        Err(error) => {
            let message = format!("Can't install {}: {error}", p.name);
            if upgrade {
                return abort_update(app, message);
            }
            return app.error(message);
        }
    };
    if !force && let Some(problem) = component_compatibility(app, &p.name, &bytes) {
        let message = if upgrade {
            format!("Can't upgrade {}: {problem}", p.name)
        } else {
            format!(
                "Can't install {}: {problem}; use C-u M-x package-install to force",
                p.name
            )
        };
        if upgrade {
            return abort_update(app, message);
        }
        return app.error(message);
    }
    let manifest = candidate_manifest(&p, &bytes).ok().flatten();
    if let Ok(imports) = component_imports(&p.name, &bytes)
        && let Some(problem) = import_mismatch(&p.name, manifest.as_ref(), &imports)
    {
        let message = if upgrade {
            format!("Can't upgrade {}: {problem}", p.name)
        } else {
            format!("Can't install {}: {problem}", p.name)
        };
        if upgrade {
            return abort_update(app, message);
        }
        return app.error(message);
    }
    if upgrade {
        match capability_gate(app, &p, &bytes) {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => return abort_update(app, format!("Can't upgrade {}: {error}", p.name)),
        }
    }
    install_ready(app, p, bytes, upgrade);
}

fn install_ready(app: &mut App, p: Package, bytes: Vec<u8>, upgrade: bool) {
    let Some(mut db) = db(app) else {
        return app.error("No plugins directory (set HOME or FORGE_CONFIG_DIR)");
    };
    if upgrade {
        if let Err(error) = smoke_test_component(app, &p.name, &bytes) {
            let message = format!("Can't upgrade {}: {error}", p.name);
            crate::package_manager_ui::update_failed(app, &p.name, &message);
            return abort_update(app, message);
        }
        if let Err(error) = prepare_update(app, &p.name) {
            let message = format!("Can't upgrade {}: {error}", p.name);
            crate::package_manager_ui::update_failed(app, &p.name, &message);
            return abort_update(app, message);
        }
    }
    let started = Timer::start();
    match db.install(&p, &bytes) {
        Ok(path) => {
            let mut event = started
                .finish(Operation::Install, Status::Ok, Some(&p.name))
                .source(package_source(&p))
                .detail("version", &p.version);
            if upgrade {
                event = event.detail("upgrade", "true");
            }
            crate::package_log::record(event);
            if let Err(error) = crate::declare::record_checked(app, &p.name, &path) {
                return abort_update(app, format!("Can't upgrade {}: {error}", p.name));
            }
            if upgrade
                && !app.packages.update.batch
                && let Err(error) = commit_update(app)
            {
                return abort_update(app, format!("Can't finish upgrading {}: {error}", p.name));
            }
            crate::package_manager_ui::update_done(app, &p.name, &p.version);
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
            let event = started
                .finish(Operation::Install, Status::Failed, Some(&p.name))
                .source(package_source(&p))
                .detail("version", &p.version)
                .error(&e);
            crate::package_log::record(event);
            let message = format!("Can't install {}: {e}", p.name);
            if upgrade {
                crate::package_manager_ui::update_failed(app, &p.name, &message);
                abort_update(app, message);
            } else {
                app.error(message);
            }
        }
    }
}

/// Extra checks run after a candidate plugin loads and registers commands.
/// Phase 3's `package-doctor` will provide the real implementation; until
/// then the default is deliberately a no-op.
pub trait SmokeTestCheck {
    fn check(&self, app: &App, editor: &forge_core::Editor, name: &str) -> Result<(), String>;
}

/// The default smoke test: no diagnostics beyond the isolated load itself.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoSmokeTestCheck;

impl SmokeTestCheck for NoSmokeTestCheck {
    fn check(&self, _app: &App, _editor: &forge_core::Editor, _name: &str) -> Result<(), String> {
        Ok(())
    }
}

pub(crate) fn smoke_test_component(app: &App, name: &str, bytes: &[u8]) -> Result<(), String> {
    smoke_test_component_with(app, name, bytes, &NoSmokeTestCheck)
}

pub fn smoke_test_component_with(
    app: &App,
    name: &str,
    bytes: &[u8],
    check: &dyn SmokeTestCheck,
) -> Result<(), String> {
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
        check.check(app, &editor, name)
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
    let allowed = app.packages.allowed_sources.clone();
    let rx = download_with(url, to, None, allowed)?;
    let id = app.packages.next_download;
    app.packages.next_download += 1;
    app.packages.downloads.push((id, rx));
    Ok(id)
}

fn download_index(
    app: &mut App,
    url: &str,
    to: &std::path::Path,
    signature: Option<&std::path::Path>,
) -> Result<u64, String> {
    let allowed = app.packages.allowed_sources.clone();
    let rx = download_with(url, to, signature, allowed)?;
    let id = app.packages.next_download;
    app.packages.next_download += 1;
    app.packages.downloads.push((id, rx));
    Ok(id)
}

fn download_with(
    url: &str,
    to: &std::path::Path,
    signature: Option<&std::path::Path>,
    allowed: Option<SourceAllowlist>,
) -> Result<Receiver<Result<(), String>>, String> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err(format!(
            "{url}: only local files and http(s) URLs are supported"
        ));
    }
    let (tx, rx) = mpsc::channel();
    let (url_owned, to_owned) = (url.to_string(), to.to_path_buf());
    let signature = signature.map(std::path::Path::to_path_buf);
    std::thread::Builder::new()
        .name("package-download".into())
        .spawn(move || {
            let result = (|| {
                http_get_to_file(&url_owned, &to_owned, allowed.clone())?;
                if let Some(signature) = signature {
                    http_get_optional_to_file(
                        &package::signature_location(&url_owned),
                        &signature,
                        allowed,
                    )?;
                }
                Ok(())
            })();
            let _ = tx.send(result);
        })
        .map_err(|e| format!("can't start the download of {url}: {e}"))?;
    Ok(rx)
}

/// GET `url` into `to` (an error for a failing status, as curl -f was).
fn http_get_to_file(
    url: &str,
    to: &std::path::Path,
    allowed: Option<SourceAllowlist>,
) -> Result<(), String> {
    http_get(url, to, false, allowed).map(|_| ())
}

/// GET an optional file; a 404 means it isn't published.
fn http_get_optional_to_file(
    url: &str,
    to: &std::path::Path,
    allowed: Option<SourceAllowlist>,
) -> Result<bool, String> {
    http_get(url, to, true, allowed)
}

fn http_get(
    url: &str,
    to: &std::path::Path,
    allow_missing: bool,
    allowed: Option<SourceAllowlist>,
) -> Result<bool, String> {
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
    if let Some(allowed) = allowed {
        builder = builder.redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 10 {
                return attempt.error("too many redirects");
            }
            match allowed.check_redirect(attempt.url().as_str()) {
                Ok(()) => attempt.follow(),
                Err(error) => attempt.error(error),
            }
        }));
    }
    let client = builder
        .build()
        .map_err(|e| format!("download failed ({})", describe(&e)))?;
    let mut resp = client
        .get(url)
        .send()
        .map_err(|e| format!("download failed ({})", describe(&e)))?;
    if allow_missing && resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }
    if !resp.status().is_success() {
        return Err(format!(
            "download failed (the server answered {})",
            resp.status()
        ));
    }
    let mut file = std::fs::File::create(to).map_err(|e| format!("download failed ({e})"))?;
    resp.copy_to(&mut file)
        .map_err(|e| format!("download failed ({})", describe(&e)))?;
    Ok(true)
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
                    Fetch::Index {
                        location,
                        file,
                        signature,
                        started,
                    } => {
                        let r = match failed {
                            Some(e) => Err(format!("{location}: {e}")),
                            None => {
                                read_downloaded_index(app, &location, &file, signature.as_deref())
                            }
                        };
                        log_fetch(started, None, index_source(&location), &r);
                        let _ = std::fs::remove_file(&file);
                        if let Some(signature) = &signature {
                            let _ = std::fs::remove_file(signature);
                        }
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
                        force,
                        started,
                    } => {
                        let r = match failed {
                            Some(e) => Err(e),
                            None => std::fs::read(&file).map_err(|e| e.to_string()),
                        };
                        log_package_fetch(started, &package, &r);
                        let _ = std::fs::remove_file(&file);
                        installed(app, package, r, upgrade, force);
                        if upgrade && app.packages.pending_capability.is_none() {
                            next_upgrade_prompt(app);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod review_snapshots {
    use super::*;
    use forge_core::permissions::{CapabilityChange, DirGrant};
    use forge_core::{Buffer, Editor, Grants};

    fn app() -> App {
        let mut app = App::new(Editor::default());
        let id = app.editor.add_buffer(Buffer::new("*scratch*"));
        app.editor.show_buffer(id);
        app.win_mut().width = 80;
        app
    }

    fn text(app: &App) -> String {
        app.buf_ref()
            .get_contents()
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn check(actual: &str, expected: &str) {
        assert_eq!(actual, expected, "\n--- actual ---\n{actual}\n--- end ---");
    }

    #[test]
    fn upgrade_with_an_addition_snapshot() {
        let new = Grants {
            network: true,
            directories: vec![DirGrant {
                path: "~/.config/forge".into(),
                write: false,
                why: None,
            }],
            memory_mb: Some(256),
            ..Default::default()
        };
        let diff = CapabilityDiff {
            additions: vec![CapabilityChange::Network],
            removals: vec![CapabilityChange::Process("git".into())],
        };
        let mut app = app();
        show_capability_review(
            &mut app,
            UpgradeReview {
                name: "org-modern",
                old_version: "0.2.0",
                new_version: "0.3.1",
                new: &new,
                diff: &diff,
                build: Some("cargo build --release --target wasm32-wasip2"),
                recipe_changes: &[],
                commits: None,
                notes: None,
            },
        );
        check(
            &text(&app),
            r#" Upgrade Review                                       org-modern 0.2.0 -> 0.3.1
--------------------------------------------------------------------------------

org-modern

 ! unattended upgrades stop and change nothing

Capabilities (+1 -1 =2) --------------------------------------------------------
  + network: open connections and look up host names                higher risk
  - programs: run git (outside the sandbox, with your rights)
  = files: read ~/.config/forge
  = memory: up to 256 MB

Build --------------------------------------------------------------------------
  command: cargo build --release --target wasm32-wasip2

y apply upgrade   n skip   C-g cancel"#,
        );
    }

    #[test]
    fn upgrade_with_only_removals_snapshot() {
        let new = Grants::default();
        let diff = CapabilityDiff {
            additions: Vec::new(),
            removals: vec![
                CapabilityChange::Network,
                CapabilityChange::Directory {
                    path: "~/.cache/old".into(),
                    write: false,
                },
            ],
        };
        let mut app = app();
        show_capability_review(
            &mut app,
            UpgradeReview {
                name: "quiet",
                old_version: "1.0.0",
                new_version: "1.1.0",
                new: &new,
                diff: &diff,
                build: None,
                recipe_changes: &[],
                commits: None,
                notes: Some("Removed network access."),
            },
        );
        check(
            &text(&app),
            r#" Upgrade Review                                            quiet 1.0.0 -> 1.1.0
--------------------------------------------------------------------------------

quiet

 ! unattended upgrades stop and change nothing

Capabilities (+0 -2 =0) --------------------------------------------------------
  - network: open connections and look up host names
  - files: read ~/.cache/old

Release notes ------------------------------------------------------------------
Removed network access.

y apply upgrade   n skip   C-g cancel"#,
        );
    }
}
