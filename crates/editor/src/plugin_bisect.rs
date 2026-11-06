// SPDX-License-Identifier: MIT OR Apache-2.0
//! M-x plugin-bisect: find the plugin that causes a problem by disabling
//! halves of the loaded set, asking after each step whether it still
//! happens. `forge --headless ... --test CMD` runs the same search with a
//! shell command as the answer.
//!
//! The user's original set is journaled before anything is unloaded and
//! restored on every exit path: finishing, a "no" answer at the end, C-g,
//! or an error. A restarted editor finds the journal and cleans it up;
//! normal startup already loads the original set.

use crate::app::App;
use crate::minibuffer::{ConfirmAction, Purpose};
use crate::plugins;
use forge_core::plugin_bisect::{
    Bisect, BisectJournal, BisectOutcome, BisectReport, DependencyGraph, PluginState, ProblemOracle,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct PluginRecord {
    pub path: Option<PathBuf>,
    pub dormant: bool,
    lazy: Option<(Vec<String>, Vec<String>, Vec<String>)>,
}

pub struct Session {
    search: Bisect,
    journal: Option<BisectJournal>,
    records: BTreeMap<String, PluginRecord>,
    original: BTreeSet<String>,
    current: BTreeSet<String>,
}

/// M-x plugin-bisect.
pub fn start(app: &mut App) {
    if app.plugin_bisect.is_some() {
        return app.error("A plugin bisect is already running");
    }
    if app.minibuffer.is_some() {
        return app.error("Finish the current prompt before starting a plugin bisect");
    }
    let original: BTreeSet<String> = plugins::loaded_names(app).into_iter().collect();
    if original.is_empty() {
        return app.error("No plugins are loaded; there is nothing to bisect");
    }
    let records = collect_records(app, &original);
    let uses = app
        .rhai
        .as_ref()
        .map(|rhai| rhai.plugin_uses())
        .unwrap_or_default();
    let graph = DependencyGraph::from_declarations(&uses, &original);
    let journal = match forge_core::config::data_dir() {
        Some(dir) => {
            let plugins = original
                .iter()
                .filter_map(|name| {
                    let record = records.get(name)?;
                    Some(PluginState::new(
                        name.clone(),
                        record.path.clone(),
                        record.dormant,
                    ))
                })
                .collect();
            match BisectJournal::begin(&dir, plugins) {
                Ok(journal) => Some(journal),
                Err(error) => return app.error(error),
            }
        }
        None => None,
    };
    app.plugin_bisect = Some(Session {
        search: Bisect::new(graph),
        journal,
        records,
        original: original.clone(),
        current: original,
    });
    next(app);
}

/// The answer to the current y/n question.
pub fn answer(app: &mut App, problem: bool) {
    let Some(session) = app.plugin_bisect.as_mut() else {
        return;
    };
    session.search.record(problem);
    next(app);
}

/// C-g: put the original plugins back and stop.
pub fn abort(app: &mut App) {
    let Some(session) = app.plugin_bisect.take() else {
        return;
    };
    let tests = session.search.tests();
    match restore(app, session) {
        Ok(()) => app.msg(format!(
            "Plugin bisect aborted after {tests} test(s); restored the plugins"
        )),
        Err(error) => app.error(format!(
            "Plugin bisect aborted, but restoring the plugins failed: {error}. Restart Forge."
        )),
    }
}

/// At startup: remove a journal left by a crash. Plugins load normally,
/// which is exactly the state the journal recorded.
pub fn recover_startup(app: &mut App) {
    let Some(dir) = forge_core::config::data_dir() else {
        return;
    };
    match forge_core::plugin_bisect::recover(&dir) {
        Ok(Some(original)) => app.msg(format!(
            "Recovered from an interrupted plugin bisect ({} plugin{})",
            original.len(),
            if original.len() == 1 { "" } else { "s" }
        )),
        Ok(None) => {}
        Err(error) => app.error(format!(
            "Could not recover the plugin bisect journal: {error}"
        )),
    }
}

fn next(app: &mut App) {
    let Some(mut session) = app.plugin_bisect.take() else {
        return;
    };
    let Some(test) = session.search.next_test() else {
        return finish(app, session);
    };
    let enabled = test.enabled.clone();
    if let Err(error) = apply(app, &session.records, &enabled) {
        app.plugin_bisect = Some(session);
        app.error(format!("Cannot disable plugins for the bisect: {error}"));
        return abort(app);
    }
    session.current = enabled;
    if let Some(journal) = session.journal.as_mut() {
        let disabled: BTreeSet<String> = session
            .original
            .difference(&session.current)
            .cloned()
            .collect();
        if let Err(error) = journal.set_disabled(&disabled) {
            app.error(error);
        }
    }
    let number = test.number;
    let count = session.current.len();
    let total = session.original.len();
    app.plugin_bisect = Some(session);
    app.prompt(
        format!(
            "Plugin bisect {number}: does the problem happen with {count} of {total} plugins enabled? (y/n) "
        ),
        Purpose::Confirm(ConfirmAction::PluginBisect),
    );
}

fn finish(app: &mut App, session: Session) {
    let outcome = session
        .search
        .outcome()
        .cloned()
        .unwrap_or(BisectOutcome::NoProblem);
    let tests = session.search.tests();
    match restore(app, session) {
        Ok(()) => match outcome {
            BisectOutcome::Culprit(name) => app.msg(format!(
                "Plugin bisect: the culprit is {name} ({tests} test{})",
                if tests == 1 { "" } else { "s" }
            )),
            BisectOutcome::NoSingleCulprit => app.msg(format!(
                "Plugin bisect: no single culprit (the problem needs more than one plugin; {tests} test{})",
                if tests == 1 { "" } else { "s" }
            )),
            BisectOutcome::NoProblem => app.msg(format!(
                "Plugin bisect: the problem did not happen with all plugins enabled ({tests} test{})",
                if tests == 1 { "" } else { "s" }
            )),
        },
        Err(error) => app.error(format!(
            "Plugin bisect finished, but restoring the plugins failed: {error}. Restart Forge."
        )),
    }
}

/// Put the original plugins back and remove the journal only once that
/// worked. If it did not, the journal stays for the next startup.
fn restore(app: &mut App, mut session: Session) -> Result<(), String> {
    apply(app, &session.records, &session.original)?;
    if let Some(journal) = session.journal.take() {
        journal.finish()?;
    }
    Ok(())
}

fn collect_records(app: &App, names: &BTreeSet<String>) -> BTreeMap<String, PluginRecord> {
    let mut records = BTreeMap::new();
    for name in names {
        let path = app
            .plugin_host
            .as_ref()
            .and_then(|host| host.source(name))
            .map(Path::to_path_buf)
            .or_else(|| crate::declare::declared_component_path(app, name));
        let dormant = app
            .plugin_host
            .as_ref()
            .is_some_and(|host| host.is_dormant(name));
        let lazy = dormant
            .then(|| crate::declare::declared(app, name))
            .flatten()
            .filter(|use_| use_.lazy())
            .map(|use_| {
                (
                    use_.on_commands.clone(),
                    use_.on_modes.clone(),
                    use_.on_extensions.clone(),
                )
            });
        records.insert(
            name.clone(),
            PluginRecord {
                path,
                dormant,
                lazy,
            },
        );
    }
    records
}

/// Make exactly `enabled` loaded, unloading the rest and reloading the
/// ones that come back. Dependencies are already included by the search.
pub fn apply(
    app: &mut App,
    records: &BTreeMap<String, PluginRecord>,
    enabled: &BTreeSet<String>,
) -> Result<(), String> {
    let current: BTreeSet<String> = plugins::loaded_names(app).into_iter().collect();
    let disable: Vec<String> = current.difference(enabled).cloned().collect();
    if !disable.is_empty() {
        let Some(mut host) = app.plugin_host.take() else {
            return Err("the plugin runtime is not running".into());
        };
        // Dependents before their dependencies.
        for name in disable.iter().rev() {
            host.unload(&mut app.editor, name);
        }
        app.plugin_host = Some(host);
        app.sync_docs();
    }
    for name in enabled.difference(&current) {
        let record = records
            .get(name)
            .ok_or_else(|| format!("{name} was not loaded when the bisect started"))?;
        let Some(path) = record.path.as_ref() else {
            return Err(format!("{name} has no file to load it from again"));
        };
        let lazy = record.lazy.as_ref().map(|(commands, modes, extensions)| {
            (commands.as_slice(), modes.as_slice(), extensions.as_slice())
        });
        plugins::reload_for_bisect(app, path, record.dormant, lazy)?;
    }
    Ok(())
}

/// `forge --headless ... --test CMD`: the shell command's exit status is
/// the answer (a nonzero status means the problem happens).
pub fn run_headless(app: &mut App, command: &str) -> Result<BisectReport, String> {
    let original: BTreeSet<String> = plugins::loaded_names(app).into_iter().collect();
    if original.is_empty() {
        return Err("no plugins are loaded; there is nothing to bisect".into());
    }
    let records = collect_records(app, &original);
    let uses = app
        .rhai
        .as_ref()
        .map(|rhai| rhai.plugin_uses())
        .unwrap_or_default();
    let graph = DependencyGraph::from_declarations(&uses, &original);
    let journal = match forge_core::config::data_dir() {
        Some(dir) => {
            let plugins = original
                .iter()
                .filter_map(|name| {
                    let record = records.get(name)?;
                    Some(PluginState::new(
                        name.clone(),
                        record.path.clone(),
                        record.dormant,
                    ))
                })
                .collect();
            Some(BisectJournal::begin(&dir, plugins)?)
        }
        None => None,
    };
    let searched = {
        let mut oracle = CommandOracle {
            app,
            records: &records,
            command,
        };
        forge_core::plugin_bisect::bisect(graph, &mut oracle)
    };
    let restored = apply(app, &records, &original);
    if restored.is_ok()
        && let Some(journal) = journal
    {
        journal.finish()?;
    }
    let report = searched?;
    restored?;
    Ok(report)
}

struct CommandOracle<'a> {
    app: &'a mut App,
    records: &'a BTreeMap<String, PluginRecord>,
    command: &'a str,
}

impl ProblemOracle for CommandOracle<'_> {
    fn problem_occurs(&mut self, enabled: &BTreeSet<String>) -> Result<bool, String> {
        apply(self.app, self.records, enabled)?;
        let status = command_status(self.command, enabled)
            .map_err(|error| format!("running the test command: {error}"))?;
        Ok(!status)
    }
}

fn command_status(command: &str, enabled: &BTreeSet<String>) -> std::io::Result<bool> {
    let value = enabled.iter().cloned().collect::<Vec<_>>().join(",");
    #[cfg(windows)]
    let status = Command::new("cmd")
        .args(["/C", command])
        .env("FORGE_BISECT_ENABLED", &value)
        .status()?;
    #[cfg(not(windows))]
    let status = Command::new("sh")
        .args(["-c", command])
        .env("FORGE_BISECT_ENABLED", &value)
        .status()?;
    Ok(status.success())
}
