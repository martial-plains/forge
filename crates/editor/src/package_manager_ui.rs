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
    Check,
    Log,
    Restore,
    Profile,
    Doctor,
    Help,
}

impl View {
    pub const ALL: [View; 9] = [
        View::Home,
        View::Install,
        View::Update,
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
            View::Check => "Check",
            View::Log => "Log",
            View::Restore => "Restore",
            View::Profile => "Profile",
            View::Doctor => "Doctor",
            View::Help => "? Help",
        }
    }

    pub fn key(self) -> char {
        match self {
            View::Home => 'H',
            View::Install => 'I',
            View::Update => 'U',
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
    pub frozen: bool,
    pub section: Option<Section>,
    pub problem: Option<Problem>,
    pub capabilities: Vec<String>,
    pub load_ms: Option<u64>,
    pub has_update: bool,
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
}

impl UpdateItem {
    pub fn state_label(&self, symbols: Symbols, tick: usize) -> String {
        match &self.state {
            ItemState::Queued => "queued".into(),
            ItemState::Running(step) => format!("{} {step}", symbols.spinner(tick)),
            ItemState::Done(result) => format!("{} {result}", symbols.check),
            ItemState::Failed(reason) => format!("{} {reason}", symbols.cross),
            ItemState::Skipped(reason) => format!("- {reason}"),
        }
    }

    fn status(&self) -> Status {
        match &self.state {
            ItemState::Queued => Status::Neutral,
            ItemState::Running(_) => Status::Info,
            ItemState::Done(_) => Status::Ok,
            ItemState::Failed(_) => Status::Error,
            ItemState::Skipped(_) => Status::Neutral,
        }
    }
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
        rows.push(Row {
            name: name.clone(),
            status,
            kind,
            version,
            target,
            source,
            frozen,
            section,
            problem,
            capabilities: crate::packages::capability_additions(app, name),
            load_ms: load_times.get(name).copied(),
            has_update,
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
            out.insert(plugin.plugin.clone(), millis(stat.total()));
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
    width: usize,
    symbols: Symbols,
    tick: usize,
) -> Rendered {
    let mut text = package_ui::Text::new();
    render_title(&mut text, model, width, symbols);
    render_tabs(&mut text, model.view, width, symbols);
    render_filter(&mut text, model, width, symbols);
    text.newline();

    let mut selected_line = 0;
    match model.view {
        View::Home => render_home(
            app,
            &mut text,
            model,
            width,
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
            symbols,
            tick,
            &mut selected_line,
        ),
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
    let title = " Forge Packages ";
    text.push_face(title, PackageFace::TitlePill);
    let counts = format!(
        "{} package{} {} {} update{} {} {} problem{}",
        model.rows.len(),
        plural(model.rows.len()),
        symbols.dot,
        model.updates,
        plural(model.updates),
        symbols.dot,
        model.problems,
        plural(model.problems),
    );
    let used = package_ui::display_width(title) + package_ui::display_width(&counts);
    let gap = width.saturating_sub(used + 1);
    if gap > 0 {
        text.push(&" ".repeat(gap));
    }
    text.push_face(
        &package_ui::fit(&counts, width.saturating_sub(used), symbols.ellipsis),
        PackageFace::Heading,
    );
    text.newline();
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
        let start = text.chars();
        if tab.active {
            let label = format!(" {} ", tab.label);
            text.push_face(&label, PackageFace::TabActive);
        } else {
            text.push_face(&format!("{}", tab.key), PackageFace::Accent);
            if tab.expanded {
                text.push_face(&tab.label[1..], PackageFace::Heading);
            }
        }
        text.push_region(start, text.chars(), &tab.view.region());
    }
    if labels.is_empty() {
        text.push_face(" ? help", PackageFace::Accent);
        let start = text.chars().saturating_sub(7);
        text.push_region(start, text.chars(), &View::Help.region());
    }
    let _ = symbols;
    text.newline();
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
    let full_width: usize = View::ALL
        .iter()
        .map(|tab| tab.label().len() + 3)
        .sum::<usize>()
        + View::ALL.len().saturating_sub(1);
    if width > 100 && full_width <= width {
        return View::ALL
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

fn render_home(
    app: &App,
    text: &mut package_ui::Text,
    model: &ManagerModel,
    width: usize,
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
                symbols,
                tick,
                selected_line,
            );
        }
        text.newline();
    }
}

fn render_update(
    app: &App,
    text: &mut package_ui::Text,
    model: &ManagerModel,
    width: usize,
    symbols: Symbols,
    tick: usize,
    selected_line: &mut usize,
) {
    if let Some(update) = &model.update {
        let total = update.items.len();
        let done = update.finished();
        let label = match update.state {
            UpdateState::Running => {
                let running = update.running();
                if running > 0 {
                    format!("Updating {done}/{total}")
                } else {
                    format!("Starting update {done}/{total}")
                }
            }
            UpdateState::Done => format!("Updated {} of {total}", update.updated()),
            UpdateState::Failed => format!("Update stopped at {done}/{total}"),
        };
        let elapsed = millis(update.started.elapsed());
        let right = format!("{elapsed} ms");
        render_heading(text, &label, None, width, PackageFace::Heading, symbols);
        if update.state == UpdateState::Running {
            let bar_width = width
                .saturating_sub(label.len() + right.len() + 6)
                .clamp(8, 28);
            let filled = done
                .saturating_mul(bar_width)
                .checked_div(total)
                .unwrap_or(0)
                .min(bar_width);
            let mut bar = String::new();
            bar.push_str(&symbols.progress_full.repeat(filled));
            bar.push_str(&symbols.progress_empty.repeat(bar_width - filled));
            let used = bar_width + right.len() + 2;
            text.push_face(&bar, PackageFace::Accent);
            let gap = width.saturating_sub(used);
            if gap > 0 {
                text.push(&" ".repeat(gap));
            }
            text.push_face(&right, PackageFace::Heading);
            text.newline();
        }
        if let Some(note) = &update.note {
            text.push_face(note, PackageFace::Error);
            text.newline();
        }
        text.newline();
        let name_w = update
            .items
            .iter()
            .map(|item| package_ui::display_width(&item.name))
            .max()
            .unwrap_or(4)
            .clamp(4, 24);
        let state_w = width.saturating_sub(name_w + 6).max(12);
        for item in &update.items {
            text.push("  ");
            text.push(&package_ui::pad(&item.name, name_w, symbols.ellipsis));
            text.push("  ");
            let state = item.state_label(symbols, tick);
            text.push_status(
                item.status(),
                &package_ui::fit(&state, state_w, symbols.ellipsis),
                state_w,
                false,
                symbols,
            );
            text.newline();
            if matches!(item.state, ItemState::Failed(_)) {
                let detail = item
                    .started
                    .map(|timer| format!("  after {} ms", millis(timer.elapsed())))
                    .unwrap_or_default();
                if !detail.is_empty() {
                    text.push_face(&detail, PackageFace::Inactive);
                    text.newline();
                }
            }
        }
        if update.state == UpdateState::Running && update.running() == 0 {
            text.newline();
            text.push_face("Waiting for the next package...", PackageFace::Inactive);
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
    render_rows(app, text, model, width, symbols, tick, selected_line);
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
    text.push_face(&heading, face);
    let used = package_ui::display_width(&heading);
    if used < width {
        text.push_face(
            &symbols.rule.repeat(width.saturating_sub(used + 1)),
            PackageFace::Rule,
        );
    }
    text.newline();
}

fn render_rows(
    app: &App,
    text: &mut package_ui::Text,
    model: &ManagerModel,
    width: usize,
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
    symbols: Symbols,
    tick: usize,
    selected_line: &mut usize,
) {
    let selected = model.selected == Some(index);
    if selected {
        *selected_line = text.line_count();
    }
    let base = if selected {
        Some(PackageFace::Selected)
    } else if row.frozen {
        Some(PackageFace::Inactive)
    } else {
        None
    };
    let name_w = width.saturating_sub(50).clamp(8, 26);
    let status_w = 14.min(width.saturating_sub(name_w + 8)).max(9);
    let version_w = 10.min(width.saturating_sub(name_w + status_w + 8)).max(4);
    let show_source = width >= 100;
    let show_target = row.target.is_some() && width >= 54;
    let source_w = width
        .saturating_sub(3 + name_w + 2 + status_w + 2 + version_w + 2 + 12)
        .max(6);

    let mut used = 0;
    let marker = if selected { symbols.selected } else { " " };
    text.push_face(marker, PackageFace::SelectedMarker);
    used += package_ui::display_width(marker);
    push_base(text, &mut used, " ", base);
    push_cell(text, &mut used, &row.name, name_w, base, symbols);
    push_base(text, &mut used, "  ", base);
    let status = package_ui::fit(&row.status, status_w, symbols.ellipsis);
    text.push_status(row.kind, &status, status_w, selected, symbols);
    used += package_ui::display_width(&status) + 2;
    push_base(text, &mut used, "  ", base);
    push_cell(text, &mut used, &row.version, version_w, base, symbols);
    if show_target {
        push_base(text, &mut used, " ", base);
        push_base(text, &mut used, symbols.arrow, Some(PackageFace::Accent));
        push_base(text, &mut used, " ", base);
        let target = row.target.as_deref().unwrap_or("");
        push_cell(
            text,
            &mut used,
            target,
            version_w.saturating_sub(2).max(3),
            base,
            symbols,
        );
    }
    if let Some(ms) = row.load_ms {
        let load = format!(" {ms} ms");
        if used + load.len() <= width {
            push_base(text, &mut used, &load, Some(PackageFace::Inactive));
        }
    }
    if show_source {
        push_base(text, &mut used, "  ", base);
        let room = source_w.min(width.saturating_sub(used));
        push_cell(text, &mut used, &row.source, room, base, symbols);
    }
    if selected && used < width {
        let fill = " ".repeat(width - used);
        push_base(text, &mut used, &fill, base);
    }
    text.newline();

    if app.packages.manager_expanded.contains(&row.name) {
        render_expanded(app, text, row, width, symbols, tick);
    }
}

fn render_expanded(
    app: &App,
    text: &mut package_ui::Text,
    row: &Row,
    width: usize,
    symbols: Symbols,
    tick: usize,
) {
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
    if !row.capabilities.is_empty() {
        push_wrapped(
            text,
            "    ",
            &format!("New capabilities: {}", row.capabilities.join(", ")),
            width,
            PackageFace::BadgeWarning,
            symbols,
        );
    }
    let details = crate::packages::details(app, &row.name);
    for line in details.lines() {
        if line.trim().is_empty() {
            continue;
        }
        push_wrapped(
            text,
            "      ",
            line.trim_start(),
            width,
            PackageFace::Inactive,
            symbols,
        );
    }
    let status = crate::package_vc::status(app, &row.name);
    if let Some(status) = status {
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
        View::Home => "RET expand  i install  U update  f filter  g refresh  q quit",
        View::Install => "RET expand  i install  f filter  g refresh  q quit",
        View::Update => "U start update  j/k select  RET expand  g refresh  q quit",
        View::Check => "g run the check again  U start update  q quit",
        View::Log => "g refresh  l full log  q quit",
        View::Restore => "M-x package-snapshot-restore  g refresh  q quit",
        View::Profile => "M-x plugin-profile  g refresh  q quit",
        View::Doctor => "g run the doctor again  q quit",
        View::Help => "q quit",
    };
    text.push_face(
        &package_ui::fit(hint, width, symbols.ellipsis),
        PackageFace::Heading,
    );
}

fn empty(text: &mut package_ui::Text, message: &str) {
    text.push_face(message, PackageFace::Inactive);
    text.newline();
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
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
        let medium = tab_labels(View::Home, 80);
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
    fn update_labels_use_static_ascii() {
        let item = UpdateItem {
            name: "demo".into(),
            old: "1.0".into(),
            new: "2.0".into(),
            state: ItemState::Running("fetching".into()),
            started: None,
        };
        assert_eq!(item.state_label(Symbols::ASCII, 0), "... fetching");
    }
}
