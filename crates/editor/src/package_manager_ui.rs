// SPDX-License-Identifier: MIT OR Apache-2.0
//! The tabbed package manager: Home, Install, Update, Check, Log, Restore,
//! Profile, Doctor and Help in one buffer.
//!
//! The manager only renders state. Commands in `packages.rs` change it, so a
//! click or key press never starts a network request while drawing.

use crate::app::App;
use crate::package_ui::{self, Status, Symbols};
use forge_core::decoration::PackageFace;
use forge_core::package_doctor::{Finding, Severity};
use forge_core::package_log::{Event, Operation, Status as LogStatus, Timer};
use forge_core::plugin::profile::ProfileReport;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

pub const MODE: &str = "package-manager-mode";
pub const BUFFER: &str = crate::packages::BUFFER;
pub const TAB_REGION_PREFIX: &str = "package-manager-tab-";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum View {
    #[default]
    Home,
    Install,
    Update,
    Sync,
    Clean,
    Check,
    Log,
    Restore,
    Profile,
    Doctor,
    Help,
}

impl View {
    pub const ALL: [View; 11] = [
        View::Home,
        View::Install,
        View::Update,
        View::Sync,
        View::Clean,
        View::Check,
        View::Log,
        View::Restore,
        View::Profile,
        View::Doctor,
        View::Help,
    ];

    pub fn label(self) -> &'static str {
        match self {
            View::Home => "Home",
            View::Install => "Install",
            View::Update => "Update",
            View::Sync => "Sync",
            View::Clean => "Clean",
            View::Check => "Check",
            View::Log => "Log",
            View::Restore => "Restore",
            View::Profile => "Profile",
            View::Doctor => "Doctor",
            View::Help => "Help",
        }
    }

    pub fn key(self) -> char {
        match self {
            View::Home => 'H',
            View::Install => 'I',
            View::Update => 'U',
            View::Sync => 'S',
            View::Clean => 'X',
            View::Check => 'C',
            View::Log => 'L',
            View::Restore => 'R',
            View::Profile => 'P',
            View::Doctor => 'D',
            View::Help => '?',
        }
    }

    pub fn command(self) -> &'static str {
        match self {
            View::Home => "package-manager-show-home",
            View::Install => "package-manager-show-install",
            View::Update => "package-manager-show-update",
            View::Sync => "package-manager-show-sync",
            View::Clean => "package-manager-show-clean",
            View::Check => "package-manager-show-check",
            View::Log => "package-manager-show-log",
            View::Restore => "package-manager-show-restore",
            View::Profile => "package-manager-show-profile",
            View::Doctor => "package-manager-show-doctor",
            View::Help => "package-manager-show-help",
        }
    }

    pub(crate) fn slug(self) -> &'static str {
        match self {
            View::Home => "home",
            View::Install => "install",
            View::Update => "update",
            View::Sync => "sync",
            View::Clean => "clean",
            View::Check => "check",
            View::Log => "log",
            View::Restore => "restore",
            View::Profile => "profile",
            View::Doctor => "doctor",
            View::Help => "help",
        }
    }

    fn region(self) -> String {
        format!("{TAB_REGION_PREFIX}{}", self.slug())
    }

    fn selectable(self) -> bool {
        matches!(self, View::Home | View::Install | View::Update)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Updates,
    Problems,
    Loaded,
    NotLoaded,
}

impl Section {
    pub fn label(self) -> &'static str {
        match self {
            Section::Updates => "Updates",
            Section::Problems => "Problems",
            Section::Loaded => "Loaded",
            Section::NotLoaded => "Not loaded",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Row {
    pub name: String,
    pub status: String,
    pub kind: Status,
    pub version: String,
    pub target: Option<String>,
    pub source: String,
    pub triggers: String,
    pub frozen: bool,
    pub section: Option<Section>,
    pub problem: Option<Problem>,
    pub capabilities: Vec<String>,
    /// Start-up time in microseconds.
    pub load_us: Option<u64>,
    pub has_update: bool,
    pub build: Option<String>,
    pub about: String,
    pub last_op: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub severity: Severity,
    pub message: String,
    pub fix: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ManagerModel {
    pub view: View,
    pub rows: Vec<Row>,
    pub selected: Option<usize>,
    pub filter: String,
    pub updates: usize,
    pub problems: usize,
    pub update: Option<UpdateSnapshot>,
    pub findings: Vec<Finding>,
    pub check: Option<Result<String, String>>,
    pub log: Vec<Event>,
    pub profile: Option<ProfileReport>,
    pub restore: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateState {
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone)]
pub enum ItemState {
    Queued,
    Running(String),
    Done(String),
    Failed(String),
    Skipped(String),
}

#[derive(Debug, Clone)]
pub struct UpdateItem {
    pub name: String,
    pub old: String,
    pub new: String,
    pub state: ItemState,
    pub started: Option<Timer>,
    /// How long the package took, once it finished.
    pub took: Option<Duration>,
}

#[derive(Debug, Clone)]
pub struct UpdateSnapshot {
    pub items: Vec<UpdateItem>,
    pub started: Timer,
    pub state: UpdateState,
    pub note: Option<String>,
}

impl UpdateSnapshot {
    pub fn new(items: Vec<UpdateItem>) -> Self {
        Self {
            items,
            started: Timer::start(),
            state: UpdateState::Running,
            note: None,
        }
    }

    pub fn running(&self) -> usize {
        self.items
            .iter()
            .filter(|item| matches!(item.state, ItemState::Running(_)))
            .count()
    }

    pub fn finished(&self) -> usize {
        self.items
            .iter()
            .filter(|item| {
                matches!(
                    item.state,
                    ItemState::Done(_) | ItemState::Failed(_) | ItemState::Skipped(_)
                )
            })
            .count()
    }

    pub fn updated(&self) -> usize {
        self.items
            .iter()
            .filter(|item| matches!(item.state, ItemState::Done(_)))
            .count()
    }

    /// The short status shown at the right of the title.
    pub fn title(&self) -> String {
        let total = self.items.len();
        let done = self.finished();
        match self.state {
            UpdateState::Running if self.running() > 0 => format!("Updating {done}/{total}"),
            UpdateState::Running => format!("Starting update {done}/{total}"),
            UpdateState::Done => format!("Updated {} of {total}", self.updated()),
            UpdateState::Failed => format!("Update stopped at {done}/{total}"),
        }
    }
}

/// The widths of the name and version columns, shared by every row on a
/// screen so the columns line up across sections.
#[derive(Debug, Clone, Copy)]
struct Columns {
    name: usize,
    version: usize,
}

impl Columns {
    fn of(model: &ManagerModel, width: usize) -> Self {
        let names = model
            .rows
            .iter()
            .map(|row| package_ui::display_width(&row.name));
        let versions = model
            .rows
            .iter()
            .map(|row| package_ui::display_width(&version_text(row)));
        let update = model.update.iter().flat_map(|update| &update.items);
        let names = names.chain(update.clone().map(|i| package_ui::display_width(&i.name)));
        let versions =
            versions.chain(update.map(|i| package_ui::display_width(&update_versions(i))));
        let mut name = names.max().unwrap_or(4).clamp(18, 24);
        let mut version = (versions.max().unwrap_or(4) + 3).clamp(8, 24);
        // Leave room for the mark, the gaps and a little detail.
        let room = width.saturating_sub(6 + 12);
        if name + version > room {
            version = version.min(room / 2).max(6);
            name = room.saturating_sub(version).max(6);
        }
        Self { name, version }
    }
}

fn version_text(row: &Row) -> String {
    match &row.target {
        Some(target) => format!("{} → {target}", row.version),
        None => row.version.clone(),
    }
}

fn update_versions(item: &UpdateItem) -> String {
    format!("{} → {}", item.old, item.new)
}

fn seconds(duration: Duration) -> String {
    format!("{:.1}s", duration.as_secs_f64())
}

#[derive(Debug, Default)]
pub struct Rendered {
    pub text: package_ui::Text,
    pub selected_line: usize,
}

pub fn selectable(app: &App) -> bool {
    app.packages.manager_view.selectable()
}

pub fn show_install(app: &mut App) {
    show_view(app, View::Install);
}

pub fn show_update(app: &mut App) {
    show_view(app, View::Update);
}

pub fn show_sync(app: &mut App) {
    show_view(app, View::Sync);
}

pub fn show_clean(app: &mut App) {
    show_view(app, View::Clean);
}

pub fn show_check(app: &mut App) {
    app.packages.manager_check = None;
    show_view(app, View::Check);
}

pub fn show_log(app: &mut App) {
    show_view(app, View::Log);
}

pub fn show_restore(app: &mut App) {
    show_view(app, View::Restore);
}

pub fn show_profile(app: &mut App) {
    show_view(app, View::Profile);
}

pub fn show_doctor(app: &mut App) {
    app.packages.manager_findings = None;
    show_view(app, View::Doctor);
}

pub fn show_help(app: &mut App) {
    show_view(app, View::Help);
}

pub fn show_home(app: &mut App) {
    show_view(app, View::Home);
}

fn show_view(app: &mut App, view: View) {
    if app.packages.manager_view != view {
        app.packages.manager_previous_view = app.packages.manager_view;
    }
    app.packages.manager_view = view;
    crate::packages::manager(app);
}

pub fn back(app: &mut App) {
    let view = app.packages.manager_previous_view;
    app.packages.manager_view = view;
    crate::packages::manager(app);
}

pub fn toggle_selected(app: &mut App) {
    if !selectable(app) {
        return app.msg("No package row to expand in this view");
    }
    let Some(name) = app.packages.manager_selected.clone() else {
        return app.msg("No package selected");
    };
    if !app.packages.manager_expanded.remove(&name) {
        app.packages.manager_expanded.insert(name);
    }
    crate::packages::manager(app);
}

pub fn navigate(app: &mut App, delta: isize) {
    if !selectable(app) {
        return app.msg("No package row to select in this view");
    }
    let model = collect(app);
    if model.rows.is_empty() {
        return app.msg("No packages match this filter");
    }
    let current = model.selected.unwrap_or(0);
    let next = (current as isize + delta).rem_euclid(model.rows.len() as isize) as usize;
    app.packages.manager_selected = Some(model.rows[next].name.clone());
    app.packages.manager_selected_row = next;
    crate::packages::manager(app);
}

pub fn show(app: &mut App) {
    let model = collect(app);
    if let Some(index) = model.selected
        && let Some(row) = model.rows.get(index)
    {
        app.packages.manager_selected = Some(row.name.clone());
        app.packages.manager_selected_row = index;
    }
    let width = app.win().width.max(40);
    let symbols = Symbols::for_app(app);
    let tick = app.packages.manager_tick as usize;
    app.packages.manager_tick = app.packages.manager_tick.wrapping_add(1);
    let rendered = render(app, &model, width, symbols, tick);
    rendered.text.show(
        app,
        BUFFER,
        MODE,
        Some(forge_core::Pos::new(rendered.selected_line, 0)),
    );
}

fn collect(app: &mut App) -> ManagerModel {
    if app.packages.manager_findings.is_none() {
        app.packages.manager_findings = Some(crate::package_doctor::findings(app));
    }
    let findings = app.packages.manager_findings.clone().unwrap_or_default();
    let installed = crate::packages::db(app)
        .map(|db| db.packages)
        .unwrap_or_default();
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
    let loaded: BTreeSet<String> = crate::plugins::loaded_names(app).into_iter().collect();
    let index = app.packages.index.as_ref();
    let filter = app.packages.manager_filter.to_lowercase();
    let events = forge_core::package_log::read_events().unwrap_or_default();
    let latest = latest_events(&events);
    let problems = problem_map(&findings, &latest);
    let profile = if app.packages.manager_view == View::Profile {
        Some(crate::startup_profile::report(app))
    } else {
        None
    };
    let load_times = load_times(profile.as_ref());
    let updates: BTreeMap<String, String> = index
        .and_then(|index| crate::packages::upgrade_candidates(app, index).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|package| (package.name, package.version))
        .collect();

    let mut names: Vec<String> = installed
        .keys()
        .chain(declared.keys())
        .chain(index.iter().flat_map(|i| i.packages.keys()))
        .cloned()
        .collect();
    names.sort();
    names.dedup();
    names.retain(|name| {
        crate::packages::manager_matches(
            name,
            &filter,
            &installed,
            &declared,
            app.packages.index.as_ref(),
        )
    });

    let mut rows = Vec::new();
    for name in &names {
        let declaration = declared.get(name);
        let package = installed.get(name);
        let indexed = index.and_then(|i| i.packages.get(name));
        let frozen = declaration.is_some_and(|u| u.frozen);
        let problem = problems.get(name).cloned();
        let has_update = updates.contains_key(name);
        let task = crate::package_vc::status(app, name);
        let (status, kind) = if let Some(task) = task {
            (task.to_string(), Status::of(task))
        } else if frozen {
            ("frozen".into(), Status::Neutral)
        } else if let Some(problem) = &problem {
            let label = match problem.severity {
                Severity::Error => "error".to_string(),
                Severity::Warning => "warning".to_string(),
                Severity::Info => "info".to_string(),
            };
            let kind = Status::of(&label);
            (label, kind)
        } else if has_update {
            ("upgrade".into(), Status::Warning)
        } else if package.is_some() {
            let label = if loaded.contains(name) {
                "loaded"
            } else if declaration.is_some_and(|u| u.lazy()) {
                "installed (lazy)"
            } else if declaration.is_some_and(|u| u.no_require) {
                "installed (deferred)"
            } else {
                "installed"
            };
            (label.into(), Status::of(label))
        } else if declaration.is_some() {
            ("not installed".into(), Status::Neutral)
        } else {
            ("available".into(), Status::Neutral)
        };
        let version = package
            .map(|p| p.version.as_str())
            .or_else(|| indexed.map(|p| p.version.as_str()))
            .unwrap_or("-")
            .to_string();
        let target = has_update
            .then(|| updates.get(name).cloned())
            .flatten()
            .filter(|new| *new != version);
        let source = declaration
            .and_then(|u| u.git.as_deref().or(u.url.as_deref()).or(u.index.as_deref()))
            .or_else(|| {
                package
                    .and_then(|p| p.git.as_deref().or(Some(p.url.as_str())))
                    .filter(|s| !s.is_empty())
            })
            .or_else(|| {
                indexed
                    .map(|p| p.git.as_deref().or(Some(p.url.as_str())).unwrap_or(""))
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or("-")
            .to_string();
        let triggers = declaration
            .filter(|u| u.lazy())
            .map(|u| {
                let mut parts = Vec::new();
                if !u.on_commands.is_empty() {
                    parts.push(format!("cmd: {}", u.on_commands.join(", ")));
                }
                if !u.on_modes.is_empty() {
                    parts.push(format!("mode: {}", u.on_modes.join(", ")));
                }
                if !u.on_extensions.is_empty() {
                    parts.push(format!("ext: {}", u.on_extensions.join(", ")));
                }
                parts.join("  ")
            })
            .unwrap_or_default();

        let section = if app.packages.manager_view == View::Home {
            Some(if problem.is_some() {
                Section::Problems
            } else if has_update {
                Section::Updates
            } else if package.is_some() {
                Section::Loaded
            } else {
                Section::NotLoaded
            })
        } else {
            None
        };
        let build = package
            .and_then(|p| p.build.clone())
            .or_else(|| declaration.and_then(|u| u.build.clone()));
        let about = indexed.map(|p| p.description.clone()).unwrap_or_default();
        let last_op = latest.get(name).map(|event| {
            format!(
                "{} {} {} {:.1}s",
                operation_label(event.operation),
                status_label(event.status),
                "·",
                event.duration_us as f64 / 1_000_000.0
            )
        });
        rows.push(Row {
            name: name.clone(),
            status,
            kind,
            version,
            target,
            source,
            triggers,
            frozen,
            section,
            problem,
            capabilities: crate::packages::capability_additions(app, name),
            load_us: load_times.get(name).copied(),
            has_update,
            build,
            about,
            last_op,
        });
    }

    if app.packages.manager_view == View::Home {
        rows.sort_by(|a, b| {
            let ra = a.section.map(|x| x as usize).unwrap_or(4);
            let rb = b.section.map(|x| x as usize).unwrap_or(4);
            ra.cmp(&rb).then_with(|| a.name.cmp(&b.name))
        });
    }

    match app.packages.manager_view {
        View::Install => rows.retain(|row| !installed.contains_key(&row.name)),
        View::Update => rows.retain(|row| row.has_update || row.status.starts_with("upgrade")),
        _ => {}
    }

    let selected = app
        .packages
        .manager_selected
        .as_ref()
        .and_then(|name| rows.iter().position(|row| &row.name == name))
        .or_else(|| {
            (!rows.is_empty()).then(|| app.packages.manager_selected_row.min(rows.len() - 1))
        });

    let updates = rows
        .iter()
        .filter(|row| row.section == Some(Section::Updates))
        .count();
    let problems = rows
        .iter()
        .filter(|row| row.section == Some(Section::Problems))
        .count();

    let check = if app.packages.manager_view == View::Check {
        if app.packages.manager_check.is_none() {
            app.packages.manager_check = Some(match app.packages.index.as_ref() {
                Some(index) => crate::packages::dry_run(app, index, true),
                None => Err("No package index loaded; press g to refresh.".into()),
            });
        }
        app.packages.manager_check.clone()
    } else {
        None
    };

    let restore = if app.packages.manager_view == View::Restore {
        restore_text()
    } else {
        String::new()
    };

    ManagerModel {
        view: app.packages.manager_view,
        rows,
        selected,
        filter: app.packages.manager_filter.clone(),
        updates,
        problems,
        update: app.packages.manager_update.clone(),
        findings,
        check,
        log: events,
        profile,
        restore,
    }
}

fn latest_events(events: &[Event]) -> BTreeMap<String, Event> {
    let mut latest = BTreeMap::new();
    for event in events {
        if let Some(name) = &event.package {
            latest.insert(name.clone(), event.clone());
        }
    }
    latest
}

fn problem_map(
    findings: &[Finding],
    latest: &BTreeMap<String, Event>,
) -> BTreeMap<String, Problem> {
    let mut out: BTreeMap<String, Problem> = BTreeMap::new();
    for finding in findings {
        let Some(name) = &finding.plugin else {
            continue;
        };
        if finding.severity == Severity::Info {
            continue;
        }
        let problem = Problem {
            severity: finding.severity,
            message: finding.message.clone(),
            fix: finding.fix.clone(),
        };
        if let Some(old) = out.get(name)
            && old.severity >= problem.severity
        {
            continue;
        }
        out.insert(name.clone(), problem);
    }
    for (name, event) in latest {
        if out.contains_key(name) || event.status != LogStatus::Failed {
            continue;
        }
        let message = event
            .error
            .clone()
            .unwrap_or_else(|| format!("{} failed", operation_label(event.operation)));
        out.insert(
            name.clone(),
            Problem {
                severity: Severity::Error,
                message,
                fix: None,
            },
        );
    }
    out
}

fn load_times(profile: Option<&ProfileReport>) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    let Some(profile) = profile else {
        return out;
    };
    for plugin in &profile.plugins {
        let stat = if plugin.lazy_start.count > 0 {
            plugin.lazy_start
        } else if plugin.init.count > 0 {
            plugin.init
        } else if plugin.compile.count > 0 {
            plugin.compile
        } else {
            plugin.cache_lookup
        };
        if stat.count > 0 {
            out.insert(
                plugin.plugin.clone(),
                u64::try_from(stat.total().as_micros()).unwrap_or(u64::MAX),
            );
        }
    }
    out
}

fn restore_text() -> String {
    let Some(config) = forge_core::config::config_dir() else {
        return "No config directory; snapshots are unavailable.\n".into();
    };
    let mut out = String::from("Package snapshots\n\n");
    match forge_core::LockFile::snapshots(&config) {
        Ok(names) if names.is_empty() => out.push_str("No package snapshots.\n"),
        Ok(names) => {
            for name in names {
                out.push_str(&format!("  {name}\n"));
            }
            out.push_str("\nM-x package-snapshot-restore restores a named snapshot.\n");
            out.push_str("M-x package-restore restores forge-lock.toml.\n");
        }
        Err(error) => out.push_str(&format!("Couldn't list snapshots: {error}\n")),
    }
    out
}

fn render(
    app: &App,
    model: &ManagerModel,
    outer: usize,
    symbols: Symbols,
    tick: usize,
) -> Rendered {
    let width = package_ui::inner(outer);
    let mut text = package_ui::Text::for_width(outer);
    render_title(&mut text, model, width, symbols);
    render_tabs(&mut text, model.view, width, symbols);

    if !model.filter.is_empty() {
        render_filter(&mut text, model, width, symbols);
        text.newline();
    }

    let cols = Columns::of(model, width);
    let mut selected_line = 0;
    match model.view {
        View::Home => render_home(
            app,
            &mut text,
            model,
            width,
            cols,
            symbols,
            tick,
            &mut selected_line,
        ),
        View::Install => {
            render_heading(
                &mut text,
                "Installable packages",
                None,
                width,
                PackageFace::Heading,
                symbols,
            );
            if model.rows.is_empty() {
                empty(
                    &mut text,
                    if model.filter.is_empty() {
                        "Everything in the index is installed."
                    } else {
                        "No installable packages match this filter."
                    },
                );
            } else {
                render_rows(
                    app,
                    &mut text,
                    model,
                    width,
                    cols,
                    symbols,
                    tick,
                    &mut selected_line,
                );
            }
        }
        View::Update => render_update(
            app,
            &mut text,
            model,
            width,
            cols,
            symbols,
            tick,
            &mut selected_line,
        ),
        View::Sync => {
            render_heading(
                &mut text,
                "Sync",
                None,
                width,
                PackageFace::Heading,
                symbols,
            );
            text.push_face(
                "Sync declared packages to forge-lock.toml.",
                PackageFace::Inactive,
            );
            text.newline();
            text.newline();
            render_rows(
                app,
                &mut text,
                model,
                width,
                cols,
                symbols,
                tick,
                &mut selected_line,
            );
        }
        View::Clean => {
            render_heading(
                &mut text,
                "Installed but undeclared",
                None,
                width,
                PackageFace::Heading,
                symbols,
            );
            text.push_face(
                "Packages installed locally but not declared in init.rhai.",
                PackageFace::Inactive,
            );
            text.newline();
            text.newline();
            render_rows(
                app,
                &mut text,
                model,
                width,
                cols,
                symbols,
                tick,
                &mut selected_line,
            );
        }
        View::Check => render_report(&mut text, model.check.as_ref(), width),
        View::Log => render_log(&mut text, model, width, symbols),
        View::Restore => {
            for line in model.restore.lines() {
                text.push(&package_ui::fit(line, width, symbols.ellipsis));
                text.newline();
            }
        }
        View::Profile => {
            if let Some(profile) = &model.profile {
                for line in profile.render_text().lines() {
                    text.push(&package_ui::fit(line, width, symbols.ellipsis));
                    text.newline();
                }
            } else {
                empty(&mut text, "No profile report.");
            }
        }
        View::Doctor => render_doctor(&mut text, model, width, symbols),
        View::Help => render_help(&mut text, width, symbols),
    }

    text.newline();
    render_footer(&mut text, model.view, width, symbols);
    Rendered {
        text,
        selected_line,
    }
}

fn render_title(text: &mut package_ui::Text, model: &ManagerModel, width: usize, symbols: Symbols) {
    let right = match (&model.update, model.view) {
        (Some(update), View::Update) => update.title(),
        _ => format!(
            "{} package{} {} {} update{}",
            model.rows.len(),
            plural(model.rows.len()),
            symbols.dot,
            model.updates,
            plural(model.updates),
        ),
    };
    package_ui::title(text, "Forge Packages", &right, width, symbols);
}

fn render_filter(
    text: &mut package_ui::Text,
    model: &ManagerModel,
    width: usize,
    symbols: Symbols,
) {
    let filter = if model.filter.is_empty() {
        "(none)"
    } else {
        model.filter.as_str()
    };
    let left = format!("Filter: {filter}");
    text.push_face("Filter: ", PackageFace::Heading);
    text.push(filter);
    let hint = "f filter";
    let gap =
        width.saturating_sub(package_ui::display_width(&left) + package_ui::display_width(hint));
    if gap > 0 {
        text.push(&" ".repeat(gap));
    }
    text.push_face(hint, PackageFace::Heading);
    let _ = symbols;
    text.newline();
}

fn render_tabs(text: &mut package_ui::Text, view: View, width: usize, symbols: Symbols) {
    let labels = tab_labels(view, width);
    for (index, tab) in labels.iter().enumerate() {
        if index > 0 {
            text.push(" ");
        }
        let start = text.mark();
        if tab.active {
            let label = format!(" {} {} ", tab.key, tab.label);
            text.push_face(&label, PackageFace::TabActive);
        } else {
            text.push_face(&format!("{}", tab.key), PackageFace::Accent);
            if tab.expanded {
                text.push_face(&format!(" {}", tab.label), PackageFace::Inactive);
            }
        }
        text.push_region(start, text.chars(), &tab.view.region());
    }
    if labels.is_empty() {
        let start = text.mark();
        text.push_face("? help", PackageFace::Accent);
        text.push_region(start, text.chars(), &View::Help.region());
    }
    text.newline();
    package_ui::rule(text, width, symbols);
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TabLabel {
    view: View,
    label: String,
    key: char,
    active: bool,
    expanded: bool,
}

fn tab_labels(view: View, width: usize) -> Vec<TabLabel> {
    // The tabs the bar shows when it is short on room. Restore and Profile
    // are one-off reports, so they appear when open or when there is space.
    let reduced: Vec<View> = View::ALL
        .iter()
        .copied()
        .filter(|tab| !matches!(tab, View::Restore | View::Profile) || *tab == view)
        .collect();
    let fits = |tabs: &[View]| {
        let labels: usize = tabs
            .iter()
            .map(|tab| tab.label().len() + 2 + usize::from(*tab == view) * 2)
            .sum();
        labels + tabs.len().saturating_sub(1) <= width
    };
    let all = View::ALL.to_vec();
    let expanded = if fits(&all) {
        Some(all)
    } else if fits(&reduced) {
        Some(reduced)
    } else {
        None
    };
    if let Some(tabs) = expanded {
        return tabs
            .iter()
            .map(|tab| TabLabel {
                view: *tab,
                label: tab.label().to_string(),
                key: tab.key(),
                active: *tab == view,
                expanded: true,
            })
            .collect();
    }
    if width < 60 {
        let mut labels = vec![TabLabel {
            view,
            label: view.label().to_string(),
            key: view.key(),
            active: true,
            expanded: true,
        }];
        if view != View::Help {
            labels.push(TabLabel {
                view: View::Help,
                label: View::Help.label().to_string(),
                key: View::Help.key(),
                active: false,
                expanded: true,
            });
        }
        return labels;
    }
    View::ALL
        .iter()
        .map(|tab| TabLabel {
            view: *tab,
            label: tab.label().to_string(),
            key: tab.key(),
            active: *tab == view,
            expanded: *tab == view,
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn render_home(
    app: &App,
    text: &mut package_ui::Text,
    model: &ManagerModel,
    width: usize,
    cols: Columns,
    symbols: Symbols,
    tick: usize,
    selected_line: &mut usize,
) {
    if model.rows.is_empty() {
        empty(
            text,
            if model.filter.is_empty() {
                "No packages are declared or installed."
            } else {
                "No packages match this filter."
            },
        );
        return;
    }
    let sections = [
        Section::Updates,
        Section::Problems,
        Section::Loaded,
        Section::NotLoaded,
    ];
    let mut first = true;
    for section in sections {
        let rows: Vec<(usize, &Row)> = model
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.section == Some(section))
            .collect();
        if rows.is_empty() {
            continue;
        }
        if !first {
            text.newline();
        }
        first = false;
        render_heading(
            text,
            section.label(),
            Some(rows.len()),
            width,
            PackageFace::Heading,
            symbols,
        );
        for (index, row) in rows {
            render_row(
                app,
                text,
                model,
                row,
                index,
                width,
                cols,
                symbols,
                tick,
                selected_line,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_update(
    app: &App,
    text: &mut package_ui::Text,
    model: &ManagerModel,
    width: usize,
    cols: Columns,
    symbols: Symbols,
    tick: usize,
    selected_line: &mut usize,
) {
    if let Some(update) = &model.update {
        let total = update.items.len();
        let done = update.finished();
        if update.state == UpdateState::Running {
            let bar_width = 27.min(width.saturating_sub(24)).max(8);
            let filled = done
                .saturating_mul(bar_width)
                .checked_div(total)
                .unwrap_or(0)
                .min(bar_width);
            text.push_face(&symbols.progress_full.repeat(filled), PackageFace::Accent);
            text.push_face(
                &symbols.progress_empty.repeat(bar_width - filled),
                PackageFace::Inactive,
            );
            text.push(&format!("  {done}/{total}  "));
            text.push_face(&seconds(update.started.elapsed()), PackageFace::Inactive);
            text.newline();
            text.newline();
        }
        let label = match update.state {
            UpdateState::Running => "Updating",
            UpdateState::Done => "Updated",
            UpdateState::Failed => "Update stopped",
        };
        render_heading(
            text,
            label,
            Some(total),
            width,
            PackageFace::Heading,
            symbols,
        );
        let current = update
            .items
            .iter()
            .position(|item| matches!(item.state, ItemState::Running(_)));
        for (index, item) in update.items.iter().enumerate() {
            let selected = current == Some(index);
            if selected {
                *selected_line = text.line_count();
            }
            render_update_item(text, item, selected, width, cols, symbols, tick);
        }
        if let Some(note) = &update.note {
            text.newline();
            text.push_face(
                &package_ui::fit(note, width, symbols.ellipsis),
                PackageFace::Error,
            );
            text.newline();
        }
        if update.state == UpdateState::Running {
            text.newline();
            render_heading(
                text,
                "On failure",
                None,
                width,
                PackageFace::Heading,
                symbols,
            );
            text.push_face(
                &package_ui::fit(
                    "A failed smoke test restores the previous version and names the plugin.",
                    width,
                    symbols.ellipsis,
                ),
                PackageFace::Inactive,
            );
            text.newline();
        }
        return;
    }

    render_heading(
        text,
        "Available updates",
        None,
        width,
        PackageFace::Heading,
        symbols,
    );
    if model.rows.is_empty() {
        empty(text, "All packages are up to date.");
        return;
    }
    text.push_face(
        "U starts the update. No files change until then.",
        PackageFace::Inactive,
    );
    text.newline();
    text.newline();
    render_rows(app, text, model, width, cols, symbols, tick, selected_line);
}

fn render_update_item(
    text: &mut package_ui::Text,
    item: &UpdateItem,
    selected: bool,
    width: usize,
    cols: Columns,
    symbols: Symbols,
    tick: usize,
) {
    let base = selected.then_some(PackageFace::Selected);
    text.begin_line(base);
    let mut used = 0;
    let mark_w = symbols.spinner_width().max(1);
    let (mark, mark_face) = match &item.state {
        ItemState::Queued => (symbols.dot, PackageFace::Inactive),
        ItemState::Running(_) => (symbols.spinner(tick), PackageFace::Info),
        ItemState::Done(_) => (symbols.check, PackageFace::Ok),
        ItemState::Failed(_) => (symbols.cross, PackageFace::Error),
        ItemState::Skipped(_) => (symbols.dot, PackageFace::Inactive),
    };
    text.push_face(mark, mark_face);
    used += package_ui::display_width(mark);
    let gap = " ".repeat(mark_w.saturating_sub(used) + 1);
    push_base(text, &mut used, &gap, base);
    push_cell(text, &mut used, &item.name, cols.name, base, symbols);
    push_base(text, &mut used, " ", base);
    let versions_face = match item.state {
        ItemState::Queued if !selected => Some(PackageFace::Inactive),
        _ => base,
    };
    push_cell(
        text,
        &mut used,
        &update_versions(item),
        cols.version,
        versions_face,
        symbols,
    );
    let took = |item: &UpdateItem| {
        item.took
            .or_else(|| item.started.map(|timer| timer.elapsed()))
            .map(seconds)
    };
    let (detail, face) = match &item.state {
        ItemState::Queued => ("queued".to_string(), PackageFace::Inactive),
        ItemState::Running(step) => (join_dot(step, took(item), symbols), PackageFace::Info),
        ItemState::Done(result) => (join_dot(result, took(item), symbols), PackageFace::Inactive),
        ItemState::Failed(reason) => (join_dot(reason, took(item), symbols), PackageFace::Error),
        ItemState::Skipped(reason) => (reason.clone(), PackageFace::Inactive),
    };
    let room = width.saturating_sub(used);
    let detail = package_ui::fit(&detail, room, symbols.ellipsis);
    push_base(text, &mut used, &detail, Some(face));
    finish_row(text, &mut used, width, base);
}

fn join_dot(left: &str, right: Option<String>, symbols: Symbols) -> String {
    match right {
        Some(right) => format!("{left} {} {right}", symbols.dot),
        None => left.to_string(),
    }
}

/// End a row. A selected row's surface runs to the window edge.
fn finish_row(
    text: &mut package_ui::Text,
    used: &mut usize,
    width: usize,
    base: Option<PackageFace>,
) {
    if let Some(face) = base {
        if *used < width {
            let fill = " ".repeat(width - *used);
            push_base(text, used, &fill, Some(face));
        }
        text.end_line_fill(face);
    }
    text.newline();
}

fn render_report(
    text: &mut package_ui::Text,
    report: Option<&Result<String, String>>,
    width: usize,
) {
    match report {
        Some(Ok(report)) => {
            for line in report.lines() {
                text.push(&package_ui::fit(line, width, "..."));
                text.newline();
            }
        }
        Some(Err(error)) => empty(text, error),
        None => empty(text, "No check has run."),
    }
}

fn render_log(text: &mut package_ui::Text, model: &ManagerModel, width: usize, symbols: Symbols) {
    if model.log.is_empty() {
        empty(text, "No package operations have been logged.");
        return;
    }
    let header = format!(
        "  {}  {}  {}  {}",
        package_ui::pad("Operation", 10, symbols.ellipsis),
        package_ui::pad("Status", 9, symbols.ellipsis),
        package_ui::pad("Package", 24, symbols.ellipsis),
        "Duration",
    );
    text.push_face(&header, PackageFace::Heading);
    text.newline();
    text.push_face(&symbols.rule.repeat(width.min(70)), PackageFace::Rule);
    text.newline();
    for event in model.log.iter().rev().take(200) {
        text.push("  ");
        text.push(&package_ui::pad(
            operation_label(event.operation),
            10,
            symbols.ellipsis,
        ));
        text.push("  ");
        let status = status_label(event.status);
        let kind = match event.status {
            LogStatus::Ok | LogStatus::Unchanged => Status::Ok,
            LogStatus::Failed => Status::Error,
            LogStatus::Declined | LogStatus::Cancelled => Status::Neutral,
        };
        text.push_status(
            kind,
            &package_ui::pad(status, 9, symbols.ellipsis),
            9,
            false,
            symbols,
        );
        text.push("  ");
        text.push(&package_ui::pad(
            event.package.as_deref().unwrap_or("-"),
            24,
            symbols.ellipsis,
        ));
        text.push("  ");
        text.push(&format!("{} ms", event.duration_us / 1000));
        text.newline();
        if let Some(error) = &event.error {
            text.push_face(
                &format!(
                    "    {}",
                    package_ui::fit(error, width.saturating_sub(4), symbols.ellipsis)
                ),
                PackageFace::Error,
            );
            text.newline();
        }
    }
}

fn render_doctor(
    text: &mut package_ui::Text,
    model: &ManagerModel,
    width: usize,
    symbols: Symbols,
) {
    if model.findings.is_empty() {
        render_heading(
            text,
            "Doctor",
            Some(0),
            width,
            PackageFace::Heading,
            symbols,
        );
        empty(text, "No problems found.");
        return;
    }
    let errors = model
        .findings
        .iter()
        .filter(|finding| finding.severity == Severity::Error)
        .count();
    let warnings = model
        .findings
        .iter()
        .filter(|finding| finding.severity == Severity::Warning)
        .count();
    let summary = format!(
        "{} error{} {} {} warning{}",
        errors,
        plural(errors),
        symbols.dot,
        warnings,
        plural(warnings),
    );
    render_heading(text, "Doctor", None, width, PackageFace::Heading, symbols);
    text.push_face(&summary, PackageFace::Heading);
    text.newline();
    text.newline();
    for finding in &model.findings {
        let kind = match finding.severity {
            Severity::Error => Status::Error,
            Severity::Warning => Status::Warning,
            Severity::Info => Status::Info,
        };
        text.push("  ");
        text.push_status(kind, finding.severity.as_str(), 9, false, symbols);
        text.push("  ");
        let name = finding.plugin.as_deref().unwrap_or("(general)");
        text.push(&package_ui::fit(name, 24, symbols.ellipsis));
        text.push("  ");
        text.push(&package_ui::fit(
            &finding.message,
            width.saturating_sub(40),
            symbols.ellipsis,
        ));
        text.newline();
        if let Some(fix) = &finding.fix {
            text.push_face(
                &format!(
                    "    fix: {}",
                    package_ui::fit(fix, width.saturating_sub(9), symbols.ellipsis)
                ),
                PackageFace::Inactive,
            );
            text.newline();
        }
    }
}

fn render_help(text: &mut package_ui::Text, width: usize, symbols: Symbols) {
    let lines = [
        "Package manager keys",
        "",
        "  j / k, arrows     next / previous package",
        "  RET, TAB          expand or collapse the selected package",
        "  i                 install the selected package",
        "  u                 upgrade the selected package",
        "  U                 start all updates and show the Update view",
        "  v                 show details in its own buffer",
        "  d                 delete the selected package",
        "  f / F             filter / clear filter",
        "  g                 refresh indexes and checks",
        "  H I C L R P D ?   switch views",
        "  q                 quit",
        "",
        "Update runs in batches. A package that needs a build or new",
        "capabilities asks before anything is written. Failed updates",
        "roll back the batch.",
    ];
    for line in lines {
        text.push(&package_ui::fit(line, width, symbols.ellipsis));
        text.newline();
    }
}

fn render_heading(
    text: &mut package_ui::Text,
    label: &str,
    count: Option<usize>,
    width: usize,
    face: PackageFace,
    symbols: Symbols,
) {
    let mut heading = label.to_string();
    if let Some(count) = count {
        heading.push_str(&format!(" ({count})"));
    }
    let _ = (width, symbols);
    text.push_face(&heading, face);
    text.newline();
}

#[allow(clippy::too_many_arguments)]
fn render_rows(
    app: &App,
    text: &mut package_ui::Text,
    model: &ManagerModel,
    width: usize,
    cols: Columns,
    symbols: Symbols,
    tick: usize,
    selected_line: &mut usize,
) {
    for (index, row) in model.rows.iter().enumerate() {
        render_row(
            app,
            text,
            model,
            row,
            index,
            width,
            cols,
            symbols,
            tick,
            selected_line,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn render_row(
    app: &App,
    text: &mut package_ui::Text,
    model: &ManagerModel,
    row: &Row,
    index: usize,
    width: usize,
    cols: Columns,
    symbols: Symbols,
    tick: usize,
    selected_line: &mut usize,
) {
    let selected = model.selected == Some(index);
    if selected {
        *selected_line = text.line_count();
    }
    let base = selected.then_some(PackageFace::Selected);
    let quiet = row.kind == Status::Neutral;

    text.begin_line(base);
    let mut used = 0;
    // The mark carries the status; a quiet row's mark is dim, not boxed.
    let mark_face = if quiet {
        PackageFace::Inactive
    } else {
        row.kind.face()
    };
    text.push_face(symbols.mark(row.kind), mark_face);
    used += package_ui::display_width(symbols.mark(row.kind));
    push_base(text, &mut used, " ", base);
    push_cell(text, &mut used, &row.name, cols.name, base, symbols);
    push_base(text, &mut used, " ", base);
    let version_face = if selected {
        base
    } else if quiet {
        Some(PackageFace::Inactive)
    } else {
        None
    };
    push_cell(
        text,
        &mut used,
        &version_text(row),
        cols.version,
        version_face,
        symbols,
    );

    if let Some(problem) = &row.problem {
        let room = width.saturating_sub(used);
        push_cell(
            text,
            &mut used,
            &problem.message,
            room,
            Some(PackageFace::Error),
            symbols,
        );
    } else {
        let mut chipped = false;
        if row.has_update && !row.capabilities.is_empty() {
            let chip = format!(" adds {} ", row.capabilities.join(", "));
            push_chip(
                text,
                &mut used,
                &chip,
                PackageFace::BadgeWarning,
                width,
                symbols,
            );
            chipped = true;
        } else if row.frozen {
            push_chip(
                text,
                &mut used,
                " frozen ",
                PackageFace::BadgeInactive,
                width,
                symbols,
            );
            chipped = true;
        }
        // Keep a cell between the chip and the detail, but only if the
        // detail has something to say.
        let gap = usize::from(chipped && used < width);
        let room = width.saturating_sub(used + gap);
        let detail = row_detail(row, model.view, symbols, room);
        if !detail.is_empty() {
            if gap > 0 {
                push_base(text, &mut used, " ", base);
            }
            let face = if selected {
                base
            } else {
                Some(PackageFace::Inactive)
            };
            push_base(text, &mut used, &detail, face);
        }
    }
    finish_row(text, &mut used, width, base);

    if app.packages.manager_expanded.contains(&row.name) {
        render_expanded(app, text, row, width, symbols, tick);
    }
}

/// What a row says after its version: when it loads and how long that took.
///
/// The timing is reserved before the description is cut, so a long trigger
/// list shortens itself with an ellipsis instead of hiding the start-up
/// time at the end of the row.
fn row_detail(row: &Row, view: View, symbols: Symbols, room: usize) -> String {
    let timing = row
        .load_us
        .map(|us| format!("{:.1} ms", us as f64 / 1000.0));
    let mut parts = Vec::new();
    if !row.triggers.is_empty() {
        parts.push(row.triggers.clone());
    } else if row.load_us.is_some() {
        parts.push("startup".to_string());
    } else if view == View::Install && row.source != "-" {
        parts.push(row.source.clone());
    }
    let description = parts.join(&format!(" {} ", symbols.dot));
    let Some(timing) = timing else {
        return package_ui::fit(&description, room, symbols.ellipsis);
    };
    if description.is_empty() {
        return package_ui::fit(&timing, room, symbols.ellipsis);
    }
    let sep = format!(" {} ", symbols.dot);
    let reserved = package_ui::display_width(&timing) + package_ui::display_width(&sep);
    if room <= reserved {
        return package_ui::fit(&timing, room, symbols.ellipsis);
    }
    let description = package_ui::fit(&description, room - reserved, symbols.ellipsis);
    format!("{description}{sep}{timing}")
}

fn push_chip(
    text: &mut package_ui::Text,
    used: &mut usize,
    label: &str,
    face: PackageFace,
    width: usize,
    symbols: Symbols,
) {
    let room = width.saturating_sub(*used);
    if room < 4 {
        return;
    }
    let label = package_ui::fit(label, room, symbols.ellipsis);
    text.push_face(&label, face);
    *used += package_ui::display_width(&label);
}

fn render_expanded(
    app: &App,
    text: &mut package_ui::Text,
    row: &Row,
    width: usize,
    symbols: Symbols,
    tick: usize,
) {
    const INDENT: usize = 4;
    if let Some(problem) = &row.problem {
        let label = match problem.severity {
            Severity::Error => "Error",
            Severity::Warning => "Warning",
            Severity::Info => "Info",
        };
        push_wrapped(
            text,
            "    ",
            &format!("{label}: {}", problem.message),
            width,
            PackageFace::Error,
            symbols,
        );
        if let Some(fix) = &problem.fix {
            push_wrapped(
                text,
                "    ",
                &format!("Fix: {fix}"),
                width,
                PackageFace::Inactive,
                symbols,
            );
        }
    }
    if row.source != "-" {
        let value = match &row.target {
            Some(target) => format!(
                "{}  v{} {} v{target}",
                row.source, row.version, symbols.arrow
            ),
            None => row.source.clone(),
        };
        package_ui::field(text, INDENT, "source", &value, None, width, symbols);
    }
    if let Some(build) = &row.build {
        package_ui::field(text, INDENT, "build", build, None, width, symbols);
    }
    if !row.about.is_empty() {
        package_ui::field(text, INDENT, "about", &row.about, None, width, symbols);
    }
    if row.has_update && !row.capabilities.is_empty() {
        let value = format!(
            "{}  (new capability, needs confirmation)",
            row.capabilities.join(", ")
        );
        package_ui::field(
            text,
            INDENT,
            "adds",
            &value,
            Some(PackageFace::Warning),
            width,
            symbols,
        );
    }
    if let Some(last) = &row.last_op {
        package_ui::field(text, INDENT, "last op", last, None, width, symbols);
    }
    if let Some(status) = crate::package_vc::status(app, &row.name) {
        push_wrapped(
            text,
            "    ",
            &format!("{} {status}", symbols.spinner(tick)),
            width,
            PackageFace::Info,
            symbols,
        );
    }
}

fn push_base(
    text: &mut package_ui::Text,
    used: &mut usize,
    value: &str,
    face: Option<PackageFace>,
) {
    match face {
        Some(face) => text.push_face(value, face),
        None => text.push(value),
    }
    *used += package_ui::display_width(value);
}

fn push_cell(
    text: &mut package_ui::Text,
    used: &mut usize,
    value: &str,
    width: usize,
    base: Option<PackageFace>,
    symbols: Symbols,
) {
    let value = package_ui::pad(value, width, symbols.ellipsis);
    push_base(text, used, &value, base);
}

fn push_wrapped(
    text: &mut package_ui::Text,
    indent: &str,
    line: &str,
    width: usize,
    face: PackageFace,
    _symbols: Symbols,
) {
    let room = width
        .saturating_sub(package_ui::display_width(indent))
        .max(8);
    for part in wrap(line, room) {
        text.push_face(indent, face);
        text.push_face(&part, face);
        text.newline();
    }
}

fn wrap(line: &str, width: usize) -> Vec<String> {
    if package_ui::display_width(line) <= width {
        return vec![line.to_string()];
    }
    let mut out = Vec::new();
    let mut current = String::new();
    for word in line.split_whitespace() {
        let next = if current.is_empty() {
            package_ui::display_width(word)
        } else {
            package_ui::display_width(&current) + 1 + package_ui::display_width(word)
        };
        if next > width && !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn render_footer(text: &mut package_ui::Text, view: View, width: usize, symbols: Symbols) {
    let hint = match view {
        View::Home => "RET expand  i install  u update  d delete  f filter  g refresh  ? help",
        View::Install => "RET expand  i install  u update  d delete  f filter  g refresh  ? help",
        View::Update => "C-g cancel  L log  H home",
        View::Sync => "g refresh  S sync  q quit",
        View::Clean => "d delete selected  g refresh  q quit",
        View::Check => "g run the check again  U start update  q quit",
        View::Log => "g refresh  l full log  q quit",
        View::Restore => "M-x package-snapshot-restore  g refresh  q quit",
        View::Profile => "M-x plugin-profile  g refresh  q quit",
        View::Doctor => "g run the doctor again  q quit",
        View::Help => "q quit",
    };
    text.push_face(
        &package_ui::fit(hint, width, symbols.ellipsis),
        PackageFace::Inactive,
    );
}

fn empty(text: &mut package_ui::Text, message: &str) {
    text.push_face(message, PackageFace::Inactive);
    text.newline();
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn operation_label(operation: Operation) -> &'static str {
    match operation {
        Operation::Fetch => "fetch",
        Operation::Build => "build",
        Operation::Install => "install",
        Operation::Upgrade => "upgrade",
        Operation::Rollback => "rollback",
    }
}

fn status_label(status: LogStatus) -> &'static str {
    match status {
        LogStatus::Ok => "ok",
        LogStatus::Failed => "failed",
        LogStatus::Declined => "declined",
        LogStatus::Cancelled => "cancelled",
        LogStatus::Unchanged => "unchanged",
    }
}

pub(crate) fn update_start_index(
    app: &mut App,
    packages: &[forge_core::package::Package],
    batch: bool,
) {
    if !batch {
        return;
    }
    let installed = crate::packages::db(app)
        .map(|db| db.packages)
        .unwrap_or_default();
    let items = packages
        .iter()
        .map(|package| UpdateItem {
            name: package.name.clone(),
            old: installed
                .get(&package.name)
                .map(|p| p.version.clone())
                .unwrap_or_else(|| "not installed".into()),
            new: package.version.clone(),
            state: ItemState::Queued,
            started: None,
            took: None,
        })
        .collect();
    app.packages.manager_update = Some(UpdateSnapshot::new(items));
}

pub(crate) fn update_start_git(app: &mut App, packages: &[(String, String, Option<String>)]) {
    if packages.is_empty() {
        return;
    }
    let installed = crate::packages::db(app)
        .map(|db| db.packages)
        .unwrap_or_default();
    let items: Vec<UpdateItem> = packages
        .iter()
        .map(|(name, _, rev)| UpdateItem {
            name: name.clone(),
            old: installed
                .get(name)
                .map(|p| p.version.clone())
                .unwrap_or_else(|| "not installed".into()),
            new: rev.clone().unwrap_or_else(|| "latest".into()),
            state: ItemState::Queued,
            started: None,
            took: None,
        })
        .collect();
    match app.packages.manager_update.as_mut() {
        Some(snapshot) if snapshot.state == UpdateState::Running => {
            let names: BTreeSet<String> = snapshot
                .items
                .iter()
                .map(|item| item.name.clone())
                .collect();
            snapshot
                .items
                .extend(items.into_iter().filter(|item| !names.contains(&item.name)));
        }
        _ => app.packages.manager_update = Some(UpdateSnapshot::new(items)),
    }
}

pub(crate) fn update_running(app: &mut App, name: &str, step: &str) {
    if let Some(snapshot) = app.packages.manager_update.as_mut() {
        if snapshot.state != UpdateState::Running {
            snapshot.state = UpdateState::Running;
        }
        if let Some(item) = snapshot.items.iter_mut().find(|item| item.name == name) {
            if item.started.is_none() {
                item.started = Some(Timer::start());
            }
            item.state = ItemState::Running(step.to_string());
        }
    }
}

pub(crate) fn update_done(app: &mut App, name: &str, result: &str) {
    if let Some(snapshot) = app.packages.manager_update.as_mut()
        && let Some(item) = snapshot.items.iter_mut().find(|item| item.name == name)
    {
        item.took = item.started.map(|timer| timer.elapsed());
        item.state = ItemState::Done(result.to_string());
    }
}

pub(crate) fn update_skipped(app: &mut App, name: &str, reason: &str) {
    if let Some(snapshot) = app.packages.manager_update.as_mut()
        && let Some(item) = snapshot.items.iter_mut().find(|item| item.name == name)
    {
        item.state = ItemState::Skipped(reason.to_string());
    }
}

pub(crate) fn update_failed(app: &mut App, name: &str, reason: &str) {
    if let Some(snapshot) = app.packages.manager_update.as_mut() {
        snapshot.state = UpdateState::Failed;
        snapshot.note = Some(format!("{name}: {reason}"));
        if let Some(item) = snapshot.items.iter_mut().find(|item| item.name == name) {
            item.state = ItemState::Failed(reason.to_string());
            if item.started.is_none() {
                item.started = Some(Timer::start());
            }
            item.took = item.started.map(|timer| timer.elapsed());
        }
    }
}

pub(crate) fn update_fail_note(app: &mut App, reason: &str) {
    if let Some(snapshot) = app.packages.manager_update.as_mut()
        && snapshot.state == UpdateState::Running
    {
        snapshot.state = UpdateState::Failed;
        snapshot.note = Some(reason.to_string());
    }
}

pub(crate) fn update_finish(app: &mut App) {
    if let Some(snapshot) = app.packages.manager_update.as_mut()
        && snapshot.state == UpdateState::Running
    {
        snapshot.state = UpdateState::Done;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_collapse_before_overflowing() {
        let wide = tab_labels(View::Home, 120);
        assert!(wide.iter().all(|tab| tab.expanded));
        let roomy = tab_labels(View::Home, 76);
        assert_eq!(roomy.len(), 9, "Restore and Profile wait for more room");
        assert!(roomy.iter().all(|tab| tab.expanded));
        let medium = tab_labels(View::Home, 64);
        assert!(
            medium
                .iter()
                .filter(|tab| tab.active)
                .all(|tab| tab.expanded)
        );
        assert!(
            medium
                .iter()
                .filter(|tab| !tab.active)
                .all(|tab| !tab.expanded)
        );
        let narrow = tab_labels(View::Home, 50);
        assert_eq!(narrow.len(), 2);
        assert!(narrow[0].active);
        assert_eq!(narrow[1].view, View::Help);
    }

    #[test]
    fn spinner_is_static_in_ascii() {
        assert_eq!(Symbols::ASCII.spinner(0), "...");
        assert_eq!(Symbols::ASCII.spinner(3), "...");
        assert_ne!(Symbols::UNICODE.spinner(0), Symbols::UNICODE.spinner(1));
    }

    #[test]
    fn startup_time_survives_a_long_trigger_list() {
        let row = Row {
            name: "demo".into(),
            status: String::new(),
            kind: Status::Ok,
            version: "1.0.0".into(),
            target: None,
            source: "-".into(),
            triggers: "cmd: a, b, c, d, e, f, g, h, i, j".into(),
            frozen: false,
            section: Some(Section::Loaded),
            problem: None,
            capabilities: Vec::new(),
            load_us: Some(1800),
            has_update: false,
            build: None,
            about: String::new(),
            last_op: None,
        };
        let detail = row_detail(&row, View::Home, Symbols::UNICODE, 24);
        assert!(detail.ends_with("1.8 ms"), "{detail}");
        assert!(
            package_ui::display_width(&detail) <= 24,
            "detail fits: {detail}"
        );
        assert!(detail.contains('…'), "triggers are cut: {detail}");
        let tight = row_detail(&row, View::Home, Symbols::UNICODE, 8);
        assert_eq!(tight, "1.8 ms");
    }
}

#[cfg(test)]
mod dump {
    use super::*;
    use forge_core::{Buffer, Editor};

    fn app() -> App {
        let mut app = App::new(Editor::default());
        let id = app.editor.add_buffer(Buffer::new("*scratch*"));
        app.editor.show_buffer(id);
        app.win_mut().width = 80;
        app
    }

    fn row(name: &str, version: &str, kind: Status, section: Section) -> Row {
        Row {
            name: name.into(),
            status: String::new(),
            kind,
            version: version.into(),
            target: None,
            source: "-".into(),
            triggers: String::new(),
            frozen: false,
            section: Some(section),
            problem: None,
            capabilities: Vec::new(),
            load_us: None,
            has_update: false,
            build: None,
            about: String::new(),
            last_op: None,
        }
    }

    fn update(name: &str, old: &str, new: &str, triggers: &str) -> Row {
        Row {
            target: Some(new.into()),
            triggers: triggers.into(),
            has_update: true,
            ..row(name, old, Status::Warning, Section::Updates)
        }
    }

    pub(super) fn home_model() -> ManagerModel {
        let mut org = update("org-modern", "0.2.0", "0.3.1", "");
        org.capabilities = vec!["network".into()];
        org.source = "github.com/you/forge-org-modern".into();
        org.build = Some("cargo build --release --target wasm32-wasip2".into());
        org.last_op = Some("upgrade ok · 4.2s".into());
        let mut spell = row("spellcheck", "0.3.0", Status::Error, Section::Problems);
        spell.problem = Some(Problem {
            severity: Severity::Error,
            message: "needs newer Forge (missing set-regions)".into(),
            fix: None,
        });
        let mut theme = row("theme", "1.2.0", Status::Ok, Section::Loaded);
        theme.frozen = true;
        let mut rainbow = row("rainbow-delimiters", "0.4.1", Status::Ok, Section::Loaded);
        rainbow.load_us = Some(1800);
        let mut snippets = row("snippets", "2.0.0", Status::Ok, Section::Loaded);
        snippets.triggers = "mode: org-mode".into();
        snippets.load_us = Some(900);
        let mut lsp = row("lsp-helper", "0.7.2", Status::Neutral, Section::NotLoaded);
        lsp.triggers = "cmd: lsp-start, lsp-restart".into();
        let mut gutter = row("git-gutter", "1.1.0", Status::Neutral, Section::NotLoaded);
        gutter.triggers = "event: buffer-opened".into();
        ManagerModel {
            view: View::Home,
            rows: vec![
                org,
                update("magit", "1.4.0", "1.5.2", "cmd: magit-status"),
                update("few", "0.9.1", "0.9.4", "ext: few"),
                spell,
                theme,
                rainbow,
                snippets,
                lsp,
                gutter,
            ],
            selected: Some(0),
            updates: 3,
            ..ManagerModel::default()
        }
    }

    #[test]
    fn dump_home() {
        let mut app = app();
        app.packages.manager_expanded.insert("org-modern".into());
        let rendered = render(&app, &home_model(), 80, Symbols::UNICODE, 0);
        std::fs::create_dir_all("/tmp/actual").unwrap();
        std::fs::write("/tmp/actual/home_dark.txt", rendered.text.text()).unwrap();
    }

    #[test]
    fn dump_update() {
        let app = app();
        let item = |name: &str, old: &str, new: &str, state, took| UpdateItem {
            name: name.into(),
            old: old.into(),
            new: new.into(),
            state,
            started: None,
            took,
        };
        let mut running = item(
            "org-modern",
            "0.2.0",
            "0.3.1",
            ItemState::Running("building · cargo".into()),
            None,
        );
        running.started = Some(Timer::start());
        let snapshot = UpdateSnapshot::new(vec![
            item(
                "magit",
                "1.4.0",
                "1.5.2",
                ItemState::Done("smoke test passed".into()),
                Some(Duration::from_millis(6100)),
            ),
            running,
            item("few", "0.9.1", "0.9.4", ItemState::Queued, None),
        ]);
        let model = ManagerModel {
            view: View::Update,
            update: Some(snapshot),
            ..ManagerModel::default()
        };
        let rendered = render(&app, &model, 80, Symbols::UNICODE, 0);
        std::fs::create_dir_all("/tmp/actual").unwrap();
        std::fs::write(
            "/tmp/actual/update_in_progress_dark.txt",
            rendered.text.text(),
        )
        .unwrap();
    }
}
