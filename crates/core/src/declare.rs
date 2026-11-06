// SPDX-License-Identifier: MIT OR Apache-2.0
//! `use_package(name, spec)`: plugins declared in init.rhai.
//!
//! Declaring a plugin in `init.rhai` records where it comes from (a git
//! repository, or an index name / url+sha256), how it's configured and
//! granted, when it may load lazily (`on`), and what it must come after
//! (`after`). At startup Forge installs any that are missing and loads
//! the rest, so a fresh machine rebuilds itself from `init.rhai` and
//! `forge-lock.toml`.
//!
//! This module is just the data the editor collects from init.rhai and
//! the ordering; the installing and loading live in the editor crate.

use crate::permissions::Grants;

/// One `use_package(name, {...})`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginUse {
    pub name: String,
    /// A git repository to clone and build.
    pub git: Option<String>,
    /// With `git`: the tag, branch or commit.
    pub rev: Option<String>,
    /// An index package name to resolve from `forge.opt.package_index`.
    pub index: Option<String>,
    /// Semver requirement used when resolving an index package.
    pub version: Option<String>,
    /// Explicit `pin` or `freeze: true` prevents automatic upgrades.
    pub frozen: bool,
    /// A direct url plus its sha256.
    pub url: Option<String>,
    pub sha256: Option<String>,
    /// The settings, as JSON (also stored in `plugin.config`).
    pub config: Option<String>,
    /// Permissions pre-granted, as `forge.plugin.grant`.
    pub grants: Grants,
    /// `hook = { ["org-mode"] = "org-modern-mode" }`, as `forge.mode.hook`.
    pub hooks: Vec<(String, String)>,
    /// Lazy loading: load when one of these is first used.
    pub on_commands: Vec<String>,
    pub on_modes: Vec<String>,
    pub on_extensions: Vec<String>,
    /// Plugins to load after (a name, or a built-in that must be ready).
    pub after: Vec<String>,
    /// Plugins that must be declared before this one (`requires`).
    pub requires: Vec<String>,
    /// The line in init.rhai that declared it, when known.
    pub line: Option<usize>,
    /// Directories to look for `NAME.wasm` in before the plugins directory
    /// (`load_path`).
    pub load_path: Vec<String>,
    /// Install and register it but don't start it unless something lazy
    /// triggers it (`no_require`).
    pub no_require: bool,
    /// With `git`: build in this subdirectory of the repository (`lisp_dir`).
    pub git_dir: Option<String>,
    /// With `git`: the shell command that builds it, given `$OUT` (`make`).
    pub build: Option<String>,
    /// With `git` and a Cargo build: cargo features to turn on.
    pub features: Vec<String>,
    /// Extra environment names or `PREFIX_*` patterns this build may inherit.
    pub build_env: Vec<String>,
    /// With `git`: how long the build may run, in seconds.
    pub build_timeout_secs: Option<u64>,
}

impl PluginUse {
    /// Is the plugin loaded lazily rather than at startup?
    pub fn lazy(&self) -> bool {
        !self.on_commands.is_empty() || !self.on_modes.is_empty() || !self.on_extensions.is_empty()
    }

    /// What the build may inherit and how long it may run.
    pub fn build_policy(&self) -> crate::package::BuildPolicy {
        let mut policy = crate::package::BuildPolicy::default();
        policy.allow(self.build_env.iter().cloned());
        if let Some(secs) = self.build_timeout_secs {
            policy.timeout = std::time::Duration::from_secs(secs);
        }
        policy
    }
}

/// Declarations in an order that puts each plugin after the ones it names
/// in `after`. Ties keep the order they were declared in, and a name in
/// `after` that isn't declared (a built-in, or a plugin loaded another
/// way) is ignored. A cycle is broken where it closes, so nothing is lost.
pub fn order(uses: &[PluginUse]) -> Vec<String> {
    let index: std::collections::BTreeMap<&str, usize> = uses
        .iter()
        .enumerate()
        .map(|(i, u)| (u.name.as_str(), i))
        .collect();
    let mut indegree = vec![0usize; uses.len()];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); uses.len()];
    for (i, u) in uses.iter().enumerate() {
        for after in &u.after {
            if let Some(&j) = index.get(after.as_str())
                && j != i
            {
                dependents[j].push(i);
                indegree[i] += 1;
            }
        }
    }
    // Ready nodes in declaration order.
    let mut ready: std::collections::VecDeque<usize> =
        (0..uses.len()).filter(|&i| indegree[i] == 0).collect();
    let mut out = Vec::with_capacity(uses.len());
    let mut done = vec![false; uses.len()];
    while let Some(i) = ready.pop_front() {
        if std::mem::replace(&mut done[i], true) {
            continue;
        }
        out.push(uses[i].name.clone());
        for &d in &dependents[i] {
            indegree[d] = indegree[d].saturating_sub(1);
            if indegree[d] == 0 {
                ready.push_back(d);
            }
        }
    }
    // A cycle: append whatever is left, in declaration order.
    for (i, u) in uses.iter().enumerate() {
        if !done[i] {
            out.push(u.name.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn use_of(name: &str, after: &[&str]) -> PluginUse {
        PluginUse {
            name: name.into(),
            after: after.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn orders_after_dependencies() {
        let uses = vec![
            use_of("magit", &["transient"]),
            use_of("transient", &[]),
            use_of("corfu", &[]),
        ];
        assert_eq!(order(&uses), ["transient", "corfu", "magit"]);
    }

    #[test]
    fn ties_keep_declaration_order_and_unknown_afters_are_ignored() {
        let uses = vec![
            use_of("b", &["built-in"]),
            use_of("a", &[]),
            use_of("c", &["a"]),
        ];
        assert_eq!(order(&uses), ["b", "a", "c"]);
    }

    #[test]
    fn a_cycle_is_broken_not_dropped() {
        let uses = vec![use_of("a", &["b"]), use_of("b", &["a"])];
        let out = order(&uses);
        assert_eq!(out.len(), 2, "{out:?}");
        assert!(out.contains(&"a".to_string()) && out.contains(&"b".to_string()));
    }
}
