// SPDX-License-Identifier: MIT OR Apache-2.0
//! Package doctor: read-only checks for declared and installed plugins.
//!
//! Every check takes the same [`DoctorContext`] and returns [`Finding`]s.
//! The editor and the headless command both collect the context and render
//! those findings, so text output, JSON, and exit codes can't disagree.

use crate::declare::PluginUse;
use crate::lockfile::LockFile;
use crate::package::{self, InstalledDb};
use crate::permissions::{Grants, Manifest};
use crate::plugin::runtime::{WIT_VERSION, is_supported_import};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// How much attention a finding needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// The check that produced a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckId {
    UsePackage,
    DeclaredNotInstalled,
    InstalledNotDeclared,
    LockHash,
    Compatibility,
    BuildTools,
    Grants,
    OrphanedVc,
    DependencyCycle,
    Cache,
    Size,
}

impl CheckId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UsePackage => "use-package",
            Self::DeclaredNotInstalled => "declared-not-installed",
            Self::InstalledNotDeclared => "installed-not-declared",
            Self::LockHash => "lock-hash",
            Self::Compatibility => "compatibility",
            Self::BuildTools => "build-tools",
            Self::Grants => "grants",
            Self::OrphanedVc => "orphaned-vc",
            Self::DependencyCycle => "dependency-cycle",
            Self::Cache => "cache",
            Self::Size => "size",
        }
    }
}

/// One problem, warning, or report line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Finding {
    pub severity: Severity,
    pub check: CheckId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl Finding {
    pub fn new(
        severity: Severity,
        check: CheckId,
        plugin: Option<String>,
        line: Option<usize>,
        message: impl Into<String>,
        fix: Option<String>,
    ) -> Self {
        Self {
            severity,
            check,
            plugin,
            line,
            message: message.into(),
            fix,
        }
    }

    pub fn error(
        check: CheckId,
        plugin: Option<String>,
        line: Option<usize>,
        message: impl Into<String>,
        fix: impl Into<String>,
    ) -> Self {
        Self::new(
            Severity::Error,
            check,
            plugin,
            line,
            message,
            Some(fix.into()),
        )
    }

    pub fn warning(
        check: CheckId,
        plugin: Option<String>,
        line: Option<usize>,
        message: impl Into<String>,
        fix: impl Into<String>,
    ) -> Self {
        Self::new(
            Severity::Warning,
            check,
            plugin,
            line,
            message,
            Some(fix.into()),
        )
    }

    pub fn info(check: CheckId, plugin: Option<String>, message: impl Into<String>) -> Self {
        Self::new(Severity::Info, check, plugin, None, message, None)
    }
}

/// What a check needs to know about a plugin component on disk.
#[derive(Debug, Clone, Default)]
pub struct PluginInfo {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub sha256: String,
    pub manifest: Option<Manifest>,
    pub imports: Vec<String>,
}

/// Everything the checks read, collected once.
#[derive(Debug, Default)]
pub struct DoctorContext {
    pub uses: Vec<PluginUse>,
    pub report: Vec<String>,
    pub init_errors: Vec<String>,
    pub lock: LockFile,
    pub installed: InstalledDb,
    pub grants: BTreeMap<String, Grants>,
    pub plugins: BTreeMap<String, PluginInfo>,
    pub config_dir: Option<PathBuf>,
    pub data_dir: Option<PathBuf>,
    pub cache_dir: Option<PathBuf>,
}

type Check = fn(&DoctorContext) -> Vec<Finding>;

/// The checks, in report order. Adding a check means adding it here and
/// giving it a test.
pub const CHECKS: &[(&str, Check)] = &[
    ("use-package", check_use_package),
    ("declared-not-installed", check_declared_not_installed),
    ("installed-not-declared", check_installed_not_declared),
    ("lock-hash", check_lock_hash),
    ("compatibility", check_compatibility),
    ("build-tools", check_build_tools),
    ("grants", check_grants),
    ("orphaned-vc", check_orphaned_vc),
    ("dependency-cycle", check_dependency_cycle),
    ("cache", check_cache),
    ("size", check_size),
];

/// Run every check and return one deterministic list.
pub fn run(ctx: &DoctorContext) -> Vec<Finding> {
    let mut findings: Vec<Finding> = CHECKS.iter().flat_map(|(_, check)| check(ctx)).collect();
    findings.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then_with(|| a.check.cmp(&b.check))
            .then_with(|| a.plugin.cmp(&b.plugin))
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| a.message.cmp(&b.message))
    });
    findings
}

/// True when any finding should make a headless run fail.
pub fn has_errors(findings: &[Finding]) -> bool {
    findings
        .iter()
        .any(|finding| finding.severity == Severity::Error)
}

fn split_report(line: &str) -> Option<(&str, &str)> {
    line.split_once(": ")
}

fn report_line(rest: &str) -> Option<usize> {
    rest.rsplit_once(" (line ")
        .and_then(|(_, n)| n.strip_suffix(')'))
        .and_then(|n| n.parse().ok())
}

fn without_line_suffix(message: &str) -> &str {
    match message.rsplit_once(" (line ") {
        Some((head, tail))
            if tail
                .strip_suffix(')')
                .is_some_and(|n| n.parse::<usize>().is_ok()) =>
        {
            head
        }
        _ => message,
    }
}

fn use_fix() -> &'static str {
    "Fix the declaration in init.rhai; see docs/init.example.rhai for the keys use_package accepts."
}

pub fn check_use_package(ctx: &DoctorContext) -> Vec<Finding> {
    let mut out = Vec::new();
    for error in &ctx.init_errors {
        out.push(Finding::error(
            CheckId::UsePackage,
            None,
            None,
            format!("init.rhai did not load: {error}"),
            use_fix(),
        ));
    }
    for u in &ctx.uses {
        if let Err(message) = validate_use(u) {
            out.push(Finding::error(
                CheckId::UsePackage,
                Some(u.name.clone()),
                u.line,
                format!("{}: {message}", u.name),
                use_fix(),
            ));
        }
    }
    for line in &ctx.report {
        let Some((name, rest)) = split_report(line) else {
            continue;
        };
        let plugin = Some(name.to_string());
        let message = without_line_suffix(line);
        if rest.contains("unknown key") {
            out.push(Finding::error(
                CheckId::UsePackage,
                plugin,
                report_line(rest),
                message,
                use_fix(),
            ));
        } else if rest.contains("requires ") && rest.contains("isn't declared") {
            out.push(Finding::error(
                CheckId::UsePackage,
                plugin,
                report_line(rest),
                message,
                "Declare the required plugin before this one, or remove it from requires.",
            ));
        } else if rest.contains(" failed: ") {
            out.push(Finding::warning(
                CheckId::UsePackage,
                plugin,
                report_line(rest),
                message,
                "Fix the Rhai code in this declaration, or remove catch to see the error at startup.",
            ));
        }
    }
    out
}

fn validate_use(u: &PluginUse) -> Result<(), String> {
    if u.name.is_empty()
        || u.name.starts_with('.')
        || u.name.contains(['/', '\\'])
        || u.name.bytes().any(|b| b.is_ascii_control())
    {
        return Err(format!("{:?} is not a safe plugin name", u.name));
    }
    let sources = [
        u.git.as_ref().map(|_| "git"),
        u.index.as_ref().map(|_| "index"),
        u.url.as_ref().map(|_| "url"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if sources.len() > 1 {
        return Err(format!(
            "declares more than one source ({}); use just one",
            sources.join(", ")
        ));
    }
    if u.url.is_some() && u.sha256.as_deref().unwrap_or_default().is_empty() {
        return Err("a url source needs a sha256".into());
    }
    if u.sha256.is_some() && u.url.is_none() {
        return Err("sha256 is only used with a url source".into());
    }
    if let Some(git) = &u.git
        && git.trim().is_empty()
    {
        return Err("git source is empty".into());
    }
    Ok(())
}

pub fn check_declared_not_installed(ctx: &DoctorContext) -> Vec<Finding> {
    let mut out = Vec::new();
    for u in &ctx.uses {
        if !ctx.plugins.contains_key(&u.name) {
            out.push(Finding::error(
                CheckId::DeclaredNotInstalled,
                Some(u.name.clone()),
                u.line,
                format!("{}: declared in init.rhai but no component is installed", u.name),
                "Install it with M-x package-restore, or restart Forge so use_package can install it.",
            ));
        }
    }
    out
}

pub fn check_installed_not_declared(ctx: &DoctorContext) -> Vec<Finding> {
    let declared: BTreeSet<&str> = ctx.uses.iter().map(|u| u.name.as_str()).collect();
    let mut installed: BTreeSet<&str> = ctx.installed.packages.keys().map(String::as_str).collect();
    installed.extend(ctx.plugins.keys().map(String::as_str));
    let mut out = Vec::new();
    for name in installed {
        if !declared.contains(name) {
            out.push(Finding::warning(
                CheckId::InstalledNotDeclared,
                Some(name.to_string()),
                None,
                format!("{name}: installed but not declared in init.rhai"),
                "Declare it with use_package, or remove it if you no longer use it.",
            ));
        }
    }
    out
}

pub fn check_lock_hash(ctx: &DoctorContext) -> Vec<Finding> {
    let mut out = Vec::new();
    for (name, entry) in &ctx.lock.packages {
        let Some(info) = ctx.plugins.get(name) else {
            continue;
        };
        let expected = entry
            .artifact_sha256
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&entry.sha256);
        if !expected.is_empty() && !expected.eq_ignore_ascii_case(&info.sha256) {
            out.push(Finding::error(
                CheckId::LockHash,
                Some(name.clone()),
                None,
                format!(
                    "{name}: the installed component's sha256 is {}, but forge-lock.toml expects {expected}",
                    info.sha256
                ),
                "Run M-x package-restore, or verify the artifact and record the lock again.",
            ));
        }
        if let Some(installed) = ctx.installed.packages.get(name)
            && !installed.sha256.is_empty()
            && !installed.sha256.eq_ignore_ascii_case(expected)
        {
            out.push(Finding::warning(
                CheckId::LockHash,
                Some(name.clone()),
                None,
                format!(
                    "{name}: packages.toml records sha256 {}, but forge-lock.toml expects {expected}",
                    installed.sha256
                ),
                "Reinstall the plugin so packages.toml and forge-lock.toml agree.",
            ));
        }
    }
    out
}

const FORGE_INTERFACES: &[&str] = &[
    "types",
    "log",
    "buffers",
    "decorations",
    "menus",
    "commands",
    "keymap",
    "events",
    "minibuffer",
    "modes",
    "processes",
    "files",
    "config",
    "environment",
    "kill-ring",
    "session",
    "registers",
    "kmacro",
    "marks",
    "xref",
    "theme",
    "org",
    "images",
    "canvas",
    "http",
    "http-handler",
    "process-handler",
    "completion",
    "completion-handler",
    "handler",
];

fn import_problem(import: &str) -> Option<String> {
    if !is_supported_import(import) {
        return Some(format!(
            "imports {import}, which this Forge does not provide"
        ));
    }
    let rest = import.strip_prefix("forge:editor/")?;
    let (interface, version) = match rest.split_once('@') {
        Some((interface, version)) => (interface, Some(version)),
        None => (rest, None),
    };
    if !FORGE_INTERFACES.contains(&interface) {
        return Some(format!(
            "imports forge:editor/{interface}, which this Forge does not provide"
        ));
    }
    if let Some(version) = version {
        let parts: Vec<u32> = version
            .split('.')
            .take(3)
            .map_while(|part| part.parse().ok())
            .collect();
        if let [major, minor, patch] = parts.as_slice()
            && (*major != 0 || *minor != 1 || *patch > WIT_VERSION)
        {
            return Some(format!(
                "needs plugin API {major}.{minor}.{patch}, but this Forge supports 0.1.{WIT_VERSION}"
            ));
        }
    }
    None
}

pub fn check_compatibility(ctx: &DoctorContext) -> Vec<Finding> {
    let mut out = Vec::new();
    for info in ctx.plugins.values() {
        let mut problems = Vec::new();
        for import in &info.imports {
            if let Some(problem) = import_problem(import)
                && !problems.contains(&problem)
            {
                problems.push(problem);
            }
        }
        if !problems.is_empty() {
            out.push(Finding::error(
                CheckId::Compatibility,
                Some(info.name.clone()),
                None,
                format!("{}: {}", info.name, problems.join("; ")),
                "Update Forge, or use a version of the plugin built for this plugin API.",
            ));
        }
    }
    out
}

pub fn check_build_tools(ctx: &DoctorContext) -> Vec<Finding> {
    let mut out = Vec::new();
    for u in &ctx.uses {
        if u.git.is_none() {
            continue;
        }
        let Some(src) = ctx
            .data_dir
            .as_ref()
            .map(|dir| dir.join("vc").join(&u.name))
            .or_else(|| package::vc_dir(&u.name))
        else {
            continue;
        };
        if !src.is_dir() {
            continue;
        }
        let root = match &u.git_dir {
            Some(dir) if !Path::new(dir).is_absolute() => src.join(dir),
            Some(dir) => {
                out.push(Finding::error(
                    CheckId::BuildTools,
                    Some(u.name.clone()),
                    u.line,
                    format!(
                        "{}: git directory {dir:?} must be relative to the repository",
                        u.name
                    ),
                    "Use a relative lisp_dir/git_dir, or remove it.",
                ));
                continue;
            }
            None => src,
        };
        if !root.is_dir() {
            out.push(Finding::error(
                CheckId::BuildTools,
                Some(u.name.clone()),
                u.line,
                format!(
                    "{}: build directory {} does not exist in the repository",
                    u.name,
                    root.display()
                ),
                "Check lisp_dir/git_dir in init.rhai, or fetch the repository again.",
            ));
            continue;
        }
        let command = match build_command(u, &root) {
            Ok(command) => command,
            Err(error) => {
                out.push(Finding::error(
                    CheckId::BuildTools,
                    Some(u.name.clone()),
                    u.line,
                    format!("{}: can't work out how to build it: {error}", u.name),
                    "Add forge-package.toml, build.sh, Cargo.toml, package.json, or a build command.",
                ));
                continue;
            }
        };
        let Some(command) = command else {
            continue;
        };
        let mut missing = Vec::new();
        for tool in command_tools(&command) {
            if tool.starts_with("./") || tool.starts_with('/') || tool.contains('\\') {
                let path = if Path::new(&tool).is_absolute() {
                    PathBuf::from(&tool)
                } else {
                    root.join(&tool)
                };
                if !path.exists() {
                    missing.push(tool);
                }
            } else if !tool_available(&tool) {
                missing.push(tool);
            }
        }
        missing.sort();
        missing.dedup();
        if !missing.is_empty() {
            out.push(Finding::error(
                CheckId::BuildTools,
                Some(u.name.clone()),
                u.line,
                format!(
                    "{}: missing build tool{}: {}",
                    u.name,
                    if missing.len() == 1 { "" } else { "s" },
                    missing.join(", ")
                ),
                "Install the missing tool (or put it on PATH), then rebuild the package.",
            ));
        }
    }
    out
}

fn build_command(u: &PluginUse, root: &Path) -> Result<Option<String>, String> {
    match (package::build_plan(root, &u.name), u.build.as_deref()) {
        (_, Some(command)) => Ok(Some(command.to_string())),
        (Ok(mut plan), None) => {
            if !u.features.is_empty()
                && plan.how == "Cargo.toml"
                && let Some(command) = plan.command.as_mut()
            {
                command.push_str(&format!(" --features {}", u.features.join(",")));
            }
            Ok(plan.command)
        }
        (Err(error), None) => Err(error),
    }
}

fn command_tools(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    for segment in command.split(['&', '|', ';']) {
        let mut words = segment.split_whitespace();
        let mut tool = loop {
            let Some(word) = words.next() else { break None };
            let word = word.trim_matches(['"', '\'']);
            if word.is_empty() || word.contains('=') || word == "env" {
                continue;
            }
            break Some(word.to_string());
        };
        if let Some(tool) = tool.take() {
            out.push(tool);
        }
    }
    out
}

fn tool_available(tool: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| has_tool(&dir, tool))
}

#[cfg(windows)]
fn has_tool(dir: &Path, tool: &str) -> bool {
    dir.join(tool).is_file()
        || ["exe", "cmd", "bat"]
            .iter()
            .any(|ext| dir.join(format!("{tool}.{ext}")).is_file())
}

#[cfg(not(windows))]
fn has_tool(dir: &Path, tool: &str) -> bool {
    dir.join(tool).is_file()
}

pub fn check_grants(ctx: &DoctorContext) -> Vec<Finding> {
    let mut out = Vec::new();
    for info in ctx.plugins.values() {
        let Some(manifest) = &info.manifest else {
            continue;
        };
        let granted = ctx.grants.get(&info.name).cloned().unwrap_or_default();
        let missing = granted.missing(&manifest.permissions);
        if !missing.is_empty() {
            out.push(Finding::warning(
                CheckId::Grants,
                Some(info.name.clone()),
                None,
                format!(
                    "{}: asks for permissions that are not granted: {}",
                    info.name,
                    missing.describe().join("; ")
                ),
                "Approve them when Forge asks, or add forge.plugin.grant in init.rhai.",
            ));
        }
        let stale = manifest.permissions.missing(&granted);
        if !stale.is_empty() {
            out.push(Finding::warning(
                CheckId::Grants,
                Some(info.name.clone()),
                None,
                format!(
                    "{}: granted permissions the manifest no longer asks for: {}",
                    info.name,
                    stale.describe().join("; ")
                ),
                "Revoke them with M-x revoke-plugin-permissions, or edit plugin-grants.toml.",
            ));
        }
    }
    out
}

pub fn check_orphaned_vc(ctx: &DoctorContext) -> Vec<Finding> {
    let Some(data) = &ctx.data_dir else {
        return Vec::new();
    };
    let dir = data.join("vc");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut active: BTreeSet<&str> = ctx
        .uses
        .iter()
        .filter(|u| u.git.is_some())
        .map(|u| u.name.as_str())
        .collect();
    active.extend(
        ctx.installed
            .packages
            .iter()
            .filter(|(_, installed)| installed.git.is_some())
            .map(|(name, _)| name.as_str()),
    );
    active.extend(
        ctx.lock
            .packages
            .iter()
            .filter(|(_, entry)| entry.git.is_some())
            .map(|(name, _)| name.as_str()),
    );
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.') || active.contains(name) {
            continue;
        }
        out.push(Finding::warning(
            CheckId::OrphanedVc,
            Some(name.to_string()),
            None,
            format!(
                "{name}: Git clone is no longer used by any package: {}",
                path.display()
            ),
            format!(
                "Delete {} (or reinstall the package from version control).",
                path.display()
            ),
        ));
    }
    out
}

pub fn check_dependency_cycle(ctx: &DoctorContext) -> Vec<Finding> {
    let names: BTreeSet<String> = ctx.uses.iter().map(|u| u.name.clone()).collect();
    let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for u in &ctx.uses {
        let deps = edges.entry(u.name.clone()).or_default();
        for dep in u.after.iter().chain(u.requires.iter()) {
            if names.contains(dep) {
                deps.push(dep.clone());
            }
        }
        deps.sort();
        deps.dedup();
    }
    let mut state: BTreeMap<String, u8> = BTreeMap::new();
    let mut stack = Vec::new();
    let mut cycles: BTreeSet<Vec<String>> = BTreeSet::new();
    for name in &names {
        if state.get(name).copied().unwrap_or(0) == 0 {
            find_cycles(name, &edges, &mut state, &mut stack, &mut cycles);
        }
    }
    let mut out = Vec::new();
    for cycle in cycles {
        let line = ctx
            .uses
            .iter()
            .find(|u| u.name == cycle[0])
            .and_then(|u| u.line);
        let path = cycle.join(" -> ");
        out.push(Finding::error(
            CheckId::DependencyCycle,
            Some(cycle[0].clone()),
            line,
            format!("dependency cycle: {path}"),
            "Remove one of the requires/after links in the cycle.",
        ));
    }
    out
}

fn find_cycles(
    name: &str,
    edges: &BTreeMap<String, Vec<String>>,
    state: &mut BTreeMap<String, u8>,
    stack: &mut Vec<String>,
    cycles: &mut BTreeSet<Vec<String>>,
) {
    state.insert(name.to_string(), 1);
    stack.push(name.to_string());
    if let Some(deps) = edges.get(name) {
        for dep in deps {
            match state.get(dep).copied().unwrap_or(0) {
                0 => find_cycles(dep, edges, state, stack, cycles),
                1 => {
                    if let Some(start) = stack.iter().position(|n| n == dep) {
                        let cycle = canonical_cycle(&stack[start..]);
                        cycles.insert(cycle);
                    }
                }
                _ => {}
            }
        }
    }
    stack.pop();
    state.insert(name.to_string(), 2);
}

fn canonical_cycle(nodes: &[String]) -> Vec<String> {
    let Some(min) = nodes.iter().enumerate().min_by_key(|(_, n)| *n) else {
        return Vec::new();
    };
    let mut cycle = Vec::with_capacity(nodes.len() + 1);
    for i in 0..nodes.len() {
        cycle.push(nodes[(min.0 + i) % nodes.len()].clone());
    }
    cycle.push(cycle[0].clone());
    cycle
}

pub fn check_cache(ctx: &DoctorContext) -> Vec<Finding> {
    let Some(cache) = &ctx.cache_dir else {
        return Vec::new();
    };
    if !cache.exists() {
        return Vec::new();
    }
    if !cache.is_dir() {
        return vec![Finding::error(
            CheckId::Cache,
            None,
            None,
            format!("cache path {} is not a directory", cache.display()),
            format!(
                "Remove {} or point FORGE_CACHE_DIR somewhere else.",
                cache.display()
            ),
        )];
    }
    let mut out = Vec::new();
    if std::fs::metadata(cache)
        .map(|m| m.permissions().readonly())
        .unwrap_or(false)
    {
        out.push(Finding::warning(
            CheckId::Cache,
            None,
            None,
            format!("cache directory {} is read-only", cache.display()),
            "Fix its permissions, or point FORGE_CACHE_DIR somewhere else.".to_string(),
        ));
    }
    for part in ["wasmtime", "plugins"] {
        let path = cache.join(part);
        if path.exists() && !path.is_dir() {
            out.push(Finding::error(
                CheckId::Cache,
                None,
                None,
                format!("cache entry {} is not a directory", path.display()),
                format!("Delete {}; Forge rebuilds the cache.", path.display()),
            ));
        }
    }
    let summaries = cache.join("plugins");
    if let Ok(entries) = std::fs::read_dir(&summaries) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || path.extension().is_none_or(|e| e != "toml") {
                continue;
            }
            let broken = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| toml::from_str::<toml::Value>(&text).err());
            if let Some(error) = broken {
                out.push(Finding::warning(
                    CheckId::Cache,
                    None,
                    None,
                    format!("cache entry {} is unreadable: {error}", path.display()),
                    format!("Delete {}; Forge rebuilds the cache.", path.display()),
                ));
            }
        }
    }
    out
}

pub fn check_size(ctx: &DoctorContext) -> Vec<Finding> {
    let mut out = Vec::new();
    for info in ctx.plugins.values() {
        out.push(Finding::info(
            CheckId::Size,
            Some(info.name.clone()),
            format!(
                "{}: {} ({} bytes, {})",
                info.name,
                human_size(info.size),
                info.size,
                info.path.display()
            ),
        ));
    }
    out
}

fn human_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    let n = bytes as f64;
    if n >= MIB {
        format!("{:.1} MiB", n / MIB)
    } else if n >= KIB {
        format!("{:.1} KiB", n / KIB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lockfile::LockEntry;
    use crate::permissions::Manifest;
    use std::fs;

    fn use_of(name: &str) -> PluginUse {
        PluginUse {
            name: name.into(),
            ..Default::default()
        }
    }

    fn info(name: &str, imports: &[&str]) -> PluginInfo {
        PluginInfo {
            name: name.into(),
            path: PathBuf::from(format!("/plugins/{name}.wasm")),
            size: 2048,
            sha256: "a".repeat(64),
            imports: imports.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("forge-doctor-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn use_package_unknown_key_and_broken_sources() {
        let mut ctx = DoctorContext {
            uses: vec![PluginUse {
                name: "bad".into(),
                git: Some("https://example.test/bad".into()),
                url: Some("https://example.test/bad.wasm".into()),
                line: Some(7),
                ..Default::default()
            }],
            ..Default::default()
        };
        ctx.report
            .push("other: skipped, unknown key \"bogus\" (line 12)".into());
        let found = check_use_package(&ctx);
        assert!(found.iter().any(|f| f.plugin.as_deref() == Some("bad")
            && f.line == Some(7)
            && f.message.contains("more than one source")));
        assert!(found.iter().any(|f| f.plugin.as_deref() == Some("other")
            && f.line == Some(12)
            && f.message.contains("unknown key")));
    }

    #[test]
    fn declared_not_installed_reports_missing_component() {
        let ctx = DoctorContext {
            uses: vec![PluginUse {
                name: "missing".into(),
                line: Some(3),
                ..Default::default()
            }],
            ..Default::default()
        };
        let found = check_declared_not_installed(&ctx);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].line, Some(3));
        assert!(found[0].message.contains("no component is installed"));
    }

    #[test]
    fn installed_not_declared_reports_extra_plugin() {
        let mut ctx = DoctorContext::default();
        ctx.plugins.insert("extra".into(), info("extra", &[]));
        let found = check_installed_not_declared(&ctx);
        assert!(found.iter().any(|f| f.plugin.as_deref() == Some("extra")));
    }

    #[test]
    fn lock_hash_reports_mismatch() {
        let mut ctx = DoctorContext::default();
        ctx.plugins.insert("p".into(), info("p", &[]));
        let mut lock = LockFile::in_memory();
        lock.set(
            "p",
            LockEntry {
                sha256: "b".repeat(64),
                ..Default::default()
            },
        );
        ctx.lock = lock;
        let found = check_lock_hash(&ctx);
        assert_eq!(found.len(), 1);
        assert!(found[0].message.contains("expects"));
    }

    #[test]
    fn compatibility_reports_missing_interface() {
        let mut ctx = DoctorContext::default();
        ctx.plugins.insert(
            "p".into(),
            info(
                "p",
                &[
                    "forge:editor/not-a-real-interface@0.1.31",
                    "forge:editor/types@0.1.99",
                ],
            ),
        );
        let found = check_compatibility(&ctx);
        assert_eq!(found.len(), 1);
        assert!(found[0].message.contains("does not provide"));
        assert!(found[0].message.contains("0.1.99"));
    }

    #[test]
    fn build_tools_reports_missing_tool() {
        let dir = temp("build-tools");
        let src = dir.join("vc").join("p");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            src.join("forge-package.toml"),
            "build = \"doctor-missing-tool\"\n",
        )
        .unwrap();
        let ctx = DoctorContext {
            uses: vec![PluginUse {
                name: "p".into(),
                git: Some("https://example.test/p".into()),
                ..Default::default()
            }],
            data_dir: Some(dir.clone()),
            ..Default::default()
        };
        let found = check_build_tools(&ctx);
        let _ = fs::remove_dir_all(&dir);
        assert!(
            found
                .iter()
                .any(|f| f.message.contains("doctor-missing-tool"))
        );
    }

    #[test]
    fn grants_report_missing_and_stale() {
        let mut ctx = DoctorContext::default();
        let manifest = Manifest::parse("[permissions]\nnetwork = true\nhttp = true\n").unwrap();
        ctx.plugins.insert(
            "p".into(),
            PluginInfo {
                manifest: Some(manifest),
                ..info("p", &[])
            },
        );
        ctx.grants.insert(
            "p".into(),
            Grants {
                environment: true,
                ..Default::default()
            },
        );
        let found = check_grants(&ctx);
        assert!(found.iter().any(|f| f.message.contains("not granted")));
        assert!(
            found
                .iter()
                .any(|f| f.message.contains("no longer asks for"))
        );
    }

    #[test]
    fn orphaned_vc_reports_unused_clone() {
        let dir = temp("orphaned-vc");
        fs::create_dir_all(dir.join("vc").join("old")).unwrap();
        let ctx = DoctorContext {
            data_dir: Some(dir.clone()),
            ..Default::default()
        };
        let found = check_orphaned_vc(&ctx);
        let _ = fs::remove_dir_all(&dir);
        assert!(found.iter().any(|f| f.plugin.as_deref() == Some("old")));
    }

    #[test]
    fn dependency_cycle_reports_requires_and_after() {
        let mut a = use_of("a");
        a.requires = vec!["b".into()];
        a.line = Some(2);
        let mut b = use_of("b");
        b.after = vec!["a".into()];
        let ctx = DoctorContext {
            uses: vec![a, b],
            ..Default::default()
        };
        let found = check_dependency_cycle(&ctx);
        assert_eq!(found.len(), 1);
        assert!(found[0].message.contains("a -> b -> a"));
        assert_eq!(found[0].line, Some(2));
    }

    #[test]
    fn cache_reports_file_where_directory_belongs() {
        let dir = temp("cache");
        fs::write(dir.join("wasmtime"), "not a directory").unwrap();
        let ctx = DoctorContext {
            cache_dir: Some(dir.clone()),
            ..Default::default()
        };
        let found = check_cache(&ctx);
        let _ = fs::remove_dir_all(&dir);
        assert!(found.iter().any(|f| f.message.contains("not a directory")));
    }

    #[test]
    fn size_reports_each_plugin() {
        let mut ctx = DoctorContext::default();
        ctx.plugins.insert("p".into(), info("p", &[]));
        let found = check_size(&ctx);
        assert_eq!(found.len(), 1);
        assert!(found[0].message.contains("2.0 KiB"));
    }

    #[test]
    fn run_is_deterministic_and_errors_fail_headless() {
        let mut ctx = DoctorContext::default();
        ctx.uses.push(use_of("missing"));
        let found = run(&ctx);
        assert!(has_errors(&found));
        assert_eq!(found, run(&ctx));
    }
}
