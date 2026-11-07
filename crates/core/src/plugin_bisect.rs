// SPDX-License-Identifier: MIT OR Apache-2.0
//! Find the plugin that causes a problem by disabling halves of the
//! loaded set.
//!
//! The search is deliberately independent of the editor: it takes a
//! [`ProblemOracle`], which is told which plugins are enabled and says
//! whether the problem still happens. The editor implements that oracle by
//! unloading plugins; tests implement it with a closure.
//!
//! `requires` dependencies stay with the plugins that need them: a test
//! always enables the enabled set's transitive dependencies. The set that
//! is split is an order-preserving half of the candidates, so a plugin in
//! the other half is never dragged back in by a dependency edge.
//!
//! A bisect can end three ways: a single culprit, no problem at all, or
//! no single culprit (the problem needs more than one plugin, such as two
//! plugins that only misbehave together).

use crate::declare::PluginUse;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const JOURNAL_FILE: &str = "plugin-bisect-journal.toml";
const JOURNAL_VERSION: u32 = 1;

/// The plugin dependency graph used for splitting and for keeping
/// `requires` dependencies enabled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DependencyGraph {
    nodes: BTreeSet<String>,
    requires: BTreeMap<String, BTreeSet<String>>,
}

impl DependencyGraph {
    pub fn new<I>(nodes: I) -> Self
    where
        I: IntoIterator<Item = String>,
    {
        Self {
            nodes: nodes.into_iter().collect(),
            requires: BTreeMap::new(),
        }
    }

    /// Build the graph for the loaded plugins, taking `requires` edges
    /// only from declarations whose both ends are loaded.
    pub fn from_declarations(uses: &[PluginUse], loaded: &BTreeSet<String>) -> Self {
        let mut graph = Self::new(loaded.iter().cloned());
        for use_ in uses {
            if !loaded.contains(&use_.name) {
                continue;
            }
            for dependency in &use_.requires {
                if loaded.contains(dependency) {
                    graph.add_requires(&use_.name, dependency);
                }
            }
        }
        graph
    }

    pub fn add_requires(&mut self, plugin: &str, requires: &str) {
        self.nodes.insert(plugin.to_string());
        self.nodes.insert(requires.to_string());
        self.requires
            .entry(plugin.to_string())
            .or_default()
            .insert(requires.to_string());
    }

    pub fn nodes(&self) -> &BTreeSet<String> {
        &self.nodes
    }

    pub fn requires(&self, plugin: &str) -> impl Iterator<Item = &str> {
        self.requires
            .get(plugin)
            .into_iter()
            .flatten()
            .map(String::as_str)
    }

    /// `enabled` plus everything it requires, transitively.
    pub fn dependencies(&self, enabled: &BTreeSet<String>) -> BTreeSet<String> {
        let mut out = enabled.clone();
        let mut pending: Vec<String> = enabled.iter().cloned().collect();
        while let Some(plugin) = pending.pop() {
            if let Some(requires) = self.requires.get(&plugin) {
                for dependency in requires {
                    if out.insert(dependency.clone()) {
                        pending.push(dependency.clone());
                    }
                }
            }
        }
        out
    }

    /// `enabled` plus everything that requires it, transitively.
    pub fn dependents(&self, enabled: &BTreeSet<String>) -> BTreeSet<String> {
        let mut out = enabled.clone();
        let mut pending: Vec<String> = enabled.iter().cloned().collect();
        while let Some(plugin) = pending.pop() {
            for (dependent, requires) in &self.requires {
                if requires.contains(&plugin) && out.insert(dependent.clone()) {
                    pending.push(dependent.clone());
                }
            }
        }
        out
    }

    /// A dependency-first order of the candidates. A plugin's `requires`
    /// edges within the candidate set come before it, so any prefix is
    /// closed under `requires` within the set.
    fn order(&self, candidates: &BTreeSet<String>) -> Vec<String> {
        let mut indegree: BTreeMap<&str, usize> = candidates
            .iter()
            .map(|name| (name.as_str(), 0usize))
            .collect();
        let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for name in candidates {
            for dependency in self.requires(name) {
                if candidates.contains(dependency) && dependency != name.as_str() {
                    *indegree.get_mut(name.as_str()).expect("a candidate") += 1;
                    dependents
                        .entry(dependency)
                        .or_default()
                        .push(name.as_str());
                }
            }
        }
        let mut ready: BTreeSet<&str> = indegree
            .iter()
            .filter(|(_, degree)| **degree == 0)
            .map(|(name, _)| *name)
            .collect();
        let mut out = Vec::with_capacity(candidates.len());
        let mut done: BTreeSet<&str> = BTreeSet::new();
        while let Some(name) = ready.pop_first() {
            if !done.insert(name) {
                continue;
            }
            out.push(name.to_string());
            if let Some(children) = dependents.get(name) {
                for &child in children {
                    if let Some(degree) = indegree.get_mut(child) {
                        *degree = degree.saturating_sub(1);
                        if *degree == 0 {
                            ready.insert(child);
                        }
                    }
                }
            }
        }
        // A cycle: append whatever is left in name order.
        for name in candidates {
            if !done.contains(name.as_str()) {
                out.push(name.clone());
            }
        }
        out
    }

    fn split(&self, candidates: &BTreeSet<String>) -> Option<(BTreeSet<String>, BTreeSet<String>)> {
        if candidates.len() < 2 {
            return None;
        }
        let order = self.order(candidates);
        let middle = order.len() / 2;
        let first: BTreeSet<String> = order[..middle].iter().cloned().collect();
        let second: BTreeSet<String> = order[middle..].iter().cloned().collect();
        Some((first, second))
    }
}

/// Answers "does the problem happen with exactly these plugins enabled?"
pub trait ProblemOracle {
    fn problem_occurs(&mut self, enabled: &BTreeSet<String>) -> Result<bool, String>;
}

impl<F> ProblemOracle for F
where
    F: FnMut(&BTreeSet<String>) -> Result<bool, String>,
{
    fn problem_occurs(&mut self, enabled: &BTreeSet<String>) -> Result<bool, String> {
        self(enabled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BisectOutcome {
    /// The problem did not happen even with everything enabled.
    NoProblem,
    /// One plugin is necessary and sufficient.
    Culprit(String),
    /// Disabling halves found no single plugin that explains it.
    NoSingleCulprit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BisectReport {
    pub outcome: BisectOutcome,
    /// How many times the oracle was asked.
    pub tests: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BisectTest {
    /// The plugins to enable for this test, including their dependencies.
    pub enabled: BTreeSet<String>,
    /// Tests asked so far, including this one.
    pub number: usize,
}

#[derive(Debug, Clone)]
enum Phase {
    Start,
    AwaitInitial,
    Search(BTreeSet<String>),
    AwaitSplit {
        first: BTreeSet<String>,
        second: BTreeSet<String>,
    },
    VerifyNecessary(String),
    VerifySufficient(String),
    Done(BisectOutcome),
}

/// One bisect in progress. Call [`Self::next_test`], ask the oracle about
/// the returned enabled set, then tell the search with [`Self::record`].
pub struct Bisect {
    graph: DependencyGraph,
    original: BTreeSet<String>,
    phase: Phase,
    tests: usize,
}

impl Bisect {
    pub fn new(graph: DependencyGraph) -> Self {
        let original = graph.nodes().clone();
        Self {
            graph,
            original,
            phase: Phase::Start,
            tests: 0,
        }
    }

    /// The plug-ins that were loaded when the bisect started.
    pub fn original(&self) -> &BTreeSet<String> {
        &self.original
    }

    pub fn tests(&self) -> usize {
        self.tests
    }

    pub fn outcome(&self) -> Option<&BisectOutcome> {
        match &self.phase {
            Phase::Done(outcome) => Some(outcome),
            _ => None,
        }
    }

    /// The next question for the oracle, or `None` when the search is done.
    pub fn next_test(&mut self) -> Option<BisectTest> {
        loop {
            match std::mem::replace(&mut self.phase, Phase::Done(BisectOutcome::NoProblem)) {
                Phase::Start => {
                    self.phase = Phase::AwaitInitial;
                    self.tests += 1;
                    return Some(BisectTest {
                        enabled: self.original.clone(),
                        number: self.tests,
                    });
                }
                Phase::Search(candidates) if candidates.len() > 1 => {
                    let (first, second) = self.graph.split(&candidates).expect("more than one");
                    self.phase = Phase::AwaitSplit {
                        first: first.clone(),
                        second,
                    };
                    self.tests += 1;
                    return Some(BisectTest {
                        enabled: self.graph.dependencies(&first),
                        number: self.tests,
                    });
                }
                Phase::Search(mut candidates) => {
                    let Some(culprit) = candidates.pop_first() else {
                        self.phase = Phase::Done(BisectOutcome::NoSingleCulprit);
                        return None;
                    };
                    self.phase = Phase::VerifyNecessary(culprit);
                }
                Phase::VerifyNecessary(culprit) => {
                    let mut one = BTreeSet::new();
                    one.insert(culprit.clone());
                    let removed = self.graph.dependents(&one);
                    let enabled: BTreeSet<String> =
                        self.original.difference(&removed).cloned().collect();
                    self.phase = Phase::VerifyNecessary(culprit);
                    self.tests += 1;
                    return Some(BisectTest {
                        enabled,
                        number: self.tests,
                    });
                }
                Phase::VerifySufficient(culprit) => {
                    let mut one = BTreeSet::new();
                    one.insert(culprit.clone());
                    self.phase = Phase::VerifySufficient(culprit);
                    self.tests += 1;
                    return Some(BisectTest {
                        enabled: self.graph.dependencies(&one),
                        number: self.tests,
                    });
                }
                other => {
                    self.phase = other;
                    return None;
                }
            }
        }
    }

    /// Record what the oracle said about the last [`BisectTest`].
    pub fn record(&mut self, problem: bool) {
        match std::mem::replace(&mut self.phase, Phase::Done(BisectOutcome::NoProblem)) {
            Phase::AwaitInitial => {
                self.phase = if problem {
                    Phase::Search(self.original.clone())
                } else {
                    Phase::Done(BisectOutcome::NoProblem)
                };
            }
            Phase::AwaitSplit { first, second } => {
                self.phase = Phase::Search(if problem { first } else { second });
            }
            Phase::VerifyNecessary(culprit) => {
                // If the problem survives removing the culprit and its
                // dependents, something else also causes it.
                if problem {
                    self.phase = Phase::Done(BisectOutcome::NoSingleCulprit);
                } else {
                    self.phase = Phase::VerifySufficient(culprit);
                }
            }
            Phase::VerifySufficient(culprit) => {
                self.phase = if problem {
                    Phase::Done(BisectOutcome::Culprit(culprit))
                } else {
                    Phase::Done(BisectOutcome::NoSingleCulprit)
                };
            }
            // An answer after the search ended, or for a test that was
            // never handed out: ignore it.
            done @ Phase::Done(_) => self.phase = done,
            search @ Phase::Search(_) => self.phase = search,
            start @ Phase::Start => self.phase = start,
        }
    }
}

pub fn bisect<O: ProblemOracle>(
    graph: DependencyGraph,
    oracle: &mut O,
) -> Result<BisectReport, String> {
    let mut search = Bisect::new(graph);
    while let Some(test) = search.next_test() {
        let problem = oracle.problem_occurs(&test.enabled)?;
        search.record(problem);
    }
    Ok(BisectReport {
        outcome: search
            .outcome()
            .cloned()
            .ok_or_else(|| "plugin bisect ended without an outcome".to_string())?,
        tests: search.tests(),
    })
}

/// The state of one plugin, kept so an interrupted bisect can be noticed
/// (and the editor's normal startup can restore it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct PluginState {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub dormant: bool,
    #[serde(flatten)]
    extra: BTreeMap<String, toml::Value>,
}

impl PluginState {
    pub fn new(name: impl Into<String>, path: Option<PathBuf>, dormant: bool) -> Self {
        Self {
            name: name.into(),
            path,
            dormant,
            extra: BTreeMap::new(),
        }
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct JournalData {
    #[serde(rename = "journal-version", default = "journal_version")]
    journal_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    operation: Option<String>,
    #[serde(default)]
    original: Vec<PluginState>,
    #[serde(default)]
    disabled: Vec<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, toml::Value>,
}

fn journal_version() -> u32 {
    JOURNAL_VERSION
}

/// A durable record of the plugins that were loaded before a bisect.
pub struct BisectJournal {
    data_dir: PathBuf,
    original: BTreeMap<String, PluginState>,
    disabled: BTreeSet<String>,
    extra: BTreeMap<String, toml::Value>,
}

impl BisectJournal {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join(JOURNAL_FILE)
    }

    pub fn begin(data_dir: &Path, original: Vec<PluginState>) -> Result<Self, String> {
        let journal = Self {
            data_dir: data_dir.to_path_buf(),
            original: original
                .into_iter()
                .map(|plugin| (plugin.name.clone(), plugin))
                .collect(),
            disabled: BTreeSet::new(),
            extra: BTreeMap::new(),
        };
        journal
            .save()
            .map_err(|error| format!("writing plugin bisect journal: {error}"))?;
        Ok(journal)
    }

    pub fn load(data_dir: &Path) -> Result<Option<Self>, String> {
        let path = Self::path(data_dir);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(format!("could not read {}: {error}", path.display()));
            }
        };
        let data: JournalData = toml::from_str(&text)
            .map_err(|error| format!("could not parse {}: {error}", path.display()))?;
        if data.journal_version != JOURNAL_VERSION {
            return Err(format!(
                "{}: unsupported plugin bisect journal version {}",
                path.display(),
                data.journal_version
            ));
        }
        Ok(Some(Self {
            data_dir: data_dir.to_path_buf(),
            original: data
                .original
                .into_iter()
                .map(|plugin| (plugin.name.clone(), plugin))
                .collect(),
            disabled: data.disabled.into_iter().collect(),
            extra: data.extra,
        }))
    }

    pub fn original(&self) -> Vec<PluginState> {
        self.original.values().cloned().collect()
    }

    pub fn original_names(&self) -> BTreeSet<String> {
        self.original.keys().cloned().collect()
    }

    pub fn disabled(&self) -> &BTreeSet<String> {
        &self.disabled
    }

    pub fn set_disabled(&mut self, disabled: &BTreeSet<String>) -> Result<(), String> {
        if self.disabled == *disabled {
            return Ok(());
        }
        self.disabled = disabled.clone();
        self.save()
            .map_err(|error| format!("writing plugin bisect journal: {error}"))
    }

    /// Finish normally: remove the journal.
    pub fn finish(self) -> Result<(), String> {
        match std::fs::remove_file(Self::path(&self.data_dir)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!(
                "removing {}: {error}",
                Self::path(&self.data_dir).display()
            )),
        }
    }

    fn save(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        let data = JournalData {
            journal_version: JOURNAL_VERSION,
            operation: Some("plugin-bisect".to_string()),
            original: self.original.values().cloned().collect(),
            disabled: self.disabled.iter().cloned().collect(),
            extra: self.extra.clone(),
        };
        let text = toml::to_string_pretty(&data).map_err(std::io::Error::other)?;
        let body =
            format!("# Plugin bisect journal (Forge removes this when the bisect ends).\n\n{text}");
        crate::fsutil::write_atomic(&Self::path(&self.data_dir), body)
    }
}

/// If a bisect was interrupted, return the state it started from and
/// remove the journal. Startup then loads plugins normally, which is the
/// original state.
pub fn recover(data_dir: &Path) -> Result<Option<Vec<PluginState>>, String> {
    let Some(journal) = BisectJournal::load(data_dir)? else {
        return Ok(None);
    };
    let original = journal.original();
    journal.finish()?;
    Ok(Some(original))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn graph(names: &[&str]) -> DependencyGraph {
        DependencyGraph::new(names.iter().map(|name| name.to_string()))
    }

    fn temp_dir(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!(
            "forge-plugin-bisect-{name}-{}-{unique}",
            std::process::id()
        ))
    }

    #[test]
    fn one_culprit_is_found_in_logarithmic_tests() {
        let all: Vec<String> = (0..32).map(|i| format!("p{i:02}")).collect();
        let graph = DependencyGraph::new(all.clone());
        let culprit = "p17".to_string();
        let mut answer = |enabled: &BTreeSet<String>| Ok(enabled.contains(&culprit));
        let report = bisect(graph, &mut answer).unwrap();
        assert_eq!(report.outcome, BisectOutcome::Culprit(culprit));
        assert!(report.tests <= 8, "{} tests", report.tests);
    }

    #[test]
    fn dependencies_stay_enabled_with_their_dependents() {
        let graph = {
            let mut graph = graph(&["base", "middle", "app", "other"]);
            graph.add_requires("middle", "base");
            graph.add_requires("app", "middle");
            graph
        };
        let culprit = "middle".to_string();
        let mut saw_app = false;
        let mut answer = |enabled: &BTreeSet<String>| {
            saw_app |= enabled.contains("app");
            Ok(enabled.contains(&culprit))
        };
        let report = bisect(graph, &mut answer).unwrap();
        assert_eq!(report.outcome, BisectOutcome::Culprit(culprit));
        assert!(saw_app, "app and middle were never tested together");
    }

    #[test]
    fn two_interacting_culprits_are_not_one_culprit() {
        let graph = graph(&["a", "b", "c", "d", "e", "f", "g", "h"]);
        let mut answer =
            |enabled: &BTreeSet<String>| Ok(enabled.contains("a") && enabled.contains("b"));
        let report = bisect(graph, &mut answer).unwrap();
        assert_eq!(report.outcome, BisectOutcome::NoSingleCulprit);
    }

    #[test]
    fn two_independent_culprits_are_not_one_culprit() {
        let graph = graph(&["a", "b", "c", "d", "e", "f", "g", "h"]);
        let mut answer =
            |enabled: &BTreeSet<String>| Ok(enabled.contains("a") || enabled.contains("b"));
        let report = bisect(graph, &mut answer).unwrap();
        assert_eq!(report.outcome, BisectOutcome::NoSingleCulprit);
    }

    #[test]
    fn no_problem_when_everything_works() {
        let graph = graph(&["a", "b", "c"]);
        let mut answer = |_: &BTreeSet<String>| Ok(false);
        let report = bisect(graph, &mut answer).unwrap();
        assert_eq!(report.outcome, BisectOutcome::NoProblem);
        assert_eq!(report.tests, 1);
    }

    #[test]
    fn journal_remembers_the_original_and_is_removed() {
        let dir = temp_dir("journal");
        let original = vec![
            PluginState::new("a", Some(PathBuf::from("/tmp/a.wasm")), false),
            PluginState::new("b", None, true),
        ];
        let mut journal = BisectJournal::begin(&dir, original.clone()).unwrap();
        journal.set_disabled(&names(&["b"])).unwrap();
        assert!(BisectJournal::path(&dir).exists());
        let loaded = BisectJournal::load(&dir).unwrap().unwrap();
        assert_eq!(loaded.original(), original);
        assert_eq!(loaded.disabled(), &names(&["b"]));
        assert_eq!(recover(&dir).unwrap(), Some(original));
        assert!(!BisectJournal::path(&dir).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
