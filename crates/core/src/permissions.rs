// SPDX-License-Identifier: MIT OR Apache-2.0
//! What plugins may do beyond editing: files, network, memory.
//!
//! A plugin asks in a manifest next to its component (`spell.wasm` ->
//! `spell.toml`):
//!
//! ```toml
//! description = "Spell checking"
//! [permissions]
//! network = true
//! http = true                 # make web requests (the http interface)
//! environment = true          # read the process environment
//! memory-mb = 256
//! [[permissions.directories]]
//! path = "~/.local/share/dict"
//! why = "word lists"          # shown when Forge asks you
//! # write = true              # read-only unless asked
//! ```
//!
//! A plugin that runs programs (a language server) names them:
//! `processes = ["pyright-langserver", "rust-analyzer"]` (`"*"`: any).
//! That's trusting it a lot: a program runs with your rights, outside the
//! sandbox.
//!
//! The user's answers are kept in `plugin-grants.toml` in the state
//! directory (`~/.local/state/forge`, following the platform convention),
//! so Forge only asks again when a plugin wants something new. An older
//! file next to `init.rhai` is read once as a migration source, then new
//! answers go only to the state directory. Without a grant a plugin gets
//! no directories, no network, no environment and the default memory
//! limit. Using a denied capability fails the way WASI reports it (access
//! denied, no such directory); a denied environment read returns an error
//! naming the permission; neither crashes the plugin.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Memory a plugin gets unless it asks (and is granted) more.
pub const DEFAULT_MEMORY_MB: u32 = 128;
/// The most a plugin can be granted.
pub const MAX_MEMORY_MB: u32 = 4096;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct DirGrant {
    pub path: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub write: bool,
    /// Why the plugin wants it (manifests only; shown in the prompt).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// A set of capabilities: what a manifest asks for, or what the user granted.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Grants {
    /// TCP/UDP sockets and name lookup (wasi:sockets).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub network: bool,
    /// Make HTTP requests (the `http` interface): Forge fetches a URL for
    /// the plugin. Asked on its own, as a request can carry data out.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub http: bool,
    /// Read the process environment (environment.get / variables).
    /// Variables often hold secrets, so this is asked for on its own.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub environment: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub directories: Vec<DirGrant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u32>,
    /// Programs it may start (processes.spawn), by name; "*" for any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub processes: Vec<String>,
}

/// What the user has granted a *project*: not a set of capabilities, but
/// whether Forge may run code from the directory itself (its
/// `.forge/init.rhai`). A project is a directory the user may have just
/// cloned, so this is asked separately from anything a plugin wants, and
/// it is the only thing that can be granted for one.
///
/// Kept out of [`Grants`] on purpose: a plugin's manifest can't ask for
/// this (it already runs), and a project's trust isn't a capability, so
/// the two live in their own sections of the file rather than sharing a
/// shape.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ProjectGrants {
    /// Run this project's `.forge/init.rhai`, which is code from the
    /// directory itself.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub code: bool,
}

impl Grants {
    pub fn is_empty(&self) -> bool {
        !self.network
            && !self.http
            && !self.environment
            && self.directories.is_empty()
            && self.memory_mb.is_none()
            && self.processes.is_empty()
    }

    pub fn memory_mb(&self) -> u32 {
        self.memory_mb
            .unwrap_or(DEFAULT_MEMORY_MB)
            .min(MAX_MEMORY_MB)
    }

    fn covers_dir(&self, want: &DirGrant) -> bool {
        let want_path = expand_home(&want.path);
        self.directories
            .iter()
            .any(|d| expand_home(&d.path) == want_path && (d.write || !want.write))
    }

    /// What `requested` asks for that these grants don't already give.
    pub fn missing(&self, requested: &Grants) -> Grants {
        Grants {
            network: requested.network && !self.network,
            http: requested.http && !self.http,
            environment: requested.environment && !self.environment,
            directories: requested
                .directories
                .iter()
                .filter(|d| !self.covers_dir(d))
                .cloned()
                .collect(),
            memory_mb: requested.memory_mb.filter(|m| *m > self.memory_mb()),
            processes: requested
                .processes
                .iter()
                .filter(|p| !self.may_run(p))
                .cloned()
                .collect(),
        }
    }

    /// Add `other` to these grants.
    pub fn merge(&mut self, other: &Grants) {
        self.network |= other.network;
        self.http |= other.http;
        self.environment |= other.environment;
        for d in &other.directories {
            if !self.covers_dir(d) {
                self.directories
                    .retain(|g| expand_home(&g.path) != expand_home(&d.path));
                self.directories.push(DirGrant {
                    path: d.path.clone(),
                    write: d.write,
                    why: None,
                });
            }
        }
        if let Some(m) = other.memory_mb {
            self.memory_mb = Some(m.max(self.memory_mb()).min(MAX_MEMORY_MB));
        }
        for p in &other.processes {
            if !self.may_run(p) {
                self.processes.push(p.clone());
            }
        }
    }

    /// May the plugin start `program` (a name or a path)? A grant of a
    /// name covers that program anywhere in PATH or at any path.
    pub fn may_run(&self, program: &str) -> bool {
        let base = Path::new(program)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.processes
            .iter()
            .any(|g| g == "*" || g == program || (!base.is_empty() && *g == base))
    }

    /// Does a directory grant cover `path` (for reading)?
    pub fn covers_path(&self, path: &Path) -> bool {
        let Ok(path) = path.canonicalize() else {
            return false;
        };
        self.directories.iter().any(|d| {
            expand_home(&d.path)
                .canonicalize()
                .is_ok_and(|dir| path.starts_with(dir))
        })
    }

    /// One line per capability, for prompts and listings.
    pub fn describe(&self) -> Vec<String> {
        self.describe_keyed()
            .into_iter()
            .map(|(_, line)| line)
            .collect()
    }

    /// One line per capability, paired with the stable key used to record
    /// when that particular grant last changed.
    pub fn describe_keyed(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if self.network {
            out.push((
                "network".to_string(),
                "network: open connections and look up host names".to_string(),
            ));
        }
        if self.http {
            out.push((
                "http".to_string(),
                "http: make web requests (a request can send data out)".to_string(),
            ));
        }
        if self.environment {
            out.push((
                "environment".to_string(),
                "environment: read the process environment (variables may hold secrets)"
                    .to_string(),
            ));
        }
        for d in &self.directories {
            let mode = if d.write { "read and write" } else { "read" };
            let why = d
                .why
                .as_deref()
                .map(|w| format!(" ({w})"))
                .unwrap_or_default();
            out.push((
                directory_key(&d.path),
                format!("files: {mode} {}{why}", d.path),
            ));
        }
        if let Some(m) = self.memory_mb {
            out.push((
                "memory".to_string(),
                format!(
                    "memory: up to {} MB (default {DEFAULT_MEMORY_MB} MB)",
                    m.min(MAX_MEMORY_MB)
                ),
            ));
        }
        for p in &self.processes {
            let what = if p == "*" {
                "any program".to_string()
            } else {
                p.clone()
            };
            out.push((
                process_key(p),
                format!("programs: run {what} (outside the sandbox, with your rights)"),
            ));
        }
        out
    }

    /// The stable keys for every capability in this set.
    pub fn capability_keys(&self) -> Vec<String> {
        self.describe_keyed()
            .into_iter()
            .map(|(key, _)| key)
            .collect()
    }
}

fn directory_key(path: &str) -> String {
    format!("files:{path}")
}

fn process_key(program: &str) -> String {
    format!("process:{program}")
}

/// A permission a component import needs, and the manifest field that
/// declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionKind {
    Network,
    Http,
    Environment,
    Directories,
    Processes,
}

impl PermissionKind {
    /// The manifest field that declares this permission.
    pub fn key(self) -> &'static str {
        match self {
            PermissionKind::Network => "network",
            PermissionKind::Http => "http",
            PermissionKind::Environment => "environment",
            PermissionKind::Directories => "directories",
            PermissionKind::Processes => "processes",
        }
    }

    /// How to name the permission in a message.
    pub fn describe(self) -> &'static str {
        match self {
            PermissionKind::Network => "network",
            PermissionKind::Http => "http",
            PermissionKind::Environment => "environment",
            PermissionKind::Directories => "directories",
            PermissionKind::Processes => "processes",
        }
    }
}

/// An import the component uses whose permission its manifest doesn't
/// declare. This is refused rather than treated as an implicit grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportMismatch {
    pub import: String,
    pub permission: PermissionKind,
}

impl ImportMismatch {
    pub fn describe(&self) -> String {
        format!(
            "{} needs the {} permission, but the manifest doesn't declare it",
            self.import,
            self.permission.describe()
        )
    }
}

/// The permission an import needs, if any.
pub fn import_permission(import: &str) -> Option<PermissionKind> {
    if import.starts_with("wasi:sockets/") {
        Some(PermissionKind::Network)
    } else if import.starts_with("wasi:filesystem/") {
        Some(PermissionKind::Directories)
    } else if import.starts_with("wasi:cli/environment")
        || import.starts_with("forge:editor/environment")
    {
        Some(PermissionKind::Environment)
    } else if import.starts_with("forge:editor/processes") {
        Some(PermissionKind::Processes)
    } else if import.starts_with("forge:editor/http") {
        Some(PermissionKind::Http)
    } else {
        None
    }
}

impl Grants {
    /// Does this set declare `kind` at all?
    pub fn declares(&self, kind: PermissionKind) -> bool {
        match kind {
            PermissionKind::Network => self.network,
            PermissionKind::Http => self.http,
            PermissionKind::Environment => self.environment,
            PermissionKind::Directories => !self.directories.is_empty(),
            PermissionKind::Processes => !self.processes.is_empty(),
        }
    }

    /// Imports that need a permission this set doesn't declare.
    pub fn uncovered_imports(&self, imports: &[String]) -> Vec<ImportMismatch> {
        let mut out: Vec<ImportMismatch> = Vec::new();
        for import in imports {
            let Some(permission) = import_permission(import) else {
                continue;
            };
            if !self.declares(permission)
                && !out
                    .iter()
                    .any(|m| m.permission == permission && m.import == *import)
            {
                out.push(ImportMismatch {
                    import: import.clone(),
                    permission,
                });
            }
        }
        out
    }
}

/// Imports a manifest doesn't declare permission for. A missing manifest
/// declares nothing, so an import that needs a permission is a mismatch.
pub fn uncovered_imports(manifest: Option<&Manifest>, imports: &[String]) -> Vec<ImportMismatch> {
    match manifest {
        Some(manifest) => manifest.permissions.uncovered_imports(imports),
        None => Grants::default().uncovered_imports(imports),
    }
}

/// A capability that changed between two manifests, for upgrade review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityChange {
    Network,
    Http,
    Environment,
    Directory { path: String, write: bool },
    Process(String),
    Memory { from: u32, to: u32 },
}

impl CapabilityChange {
    /// One line describing the capability, for prompts and listings.
    pub fn describe(&self) -> String {
        match self {
            CapabilityChange::Network => {
                "network: open connections and look up host names".to_string()
            }
            CapabilityChange::Http => {
                "http: make web requests (a request can send data out)".to_string()
            }
            CapabilityChange::Environment => {
                "environment: read the process environment (variables may hold secrets)".to_string()
            }
            CapabilityChange::Directory { path, write } => {
                let mode = if *write { "read and write" } else { "read" };
                format!("files: {mode} {path}")
            }
            CapabilityChange::Process(program) => {
                let what = if program == "*" {
                    "any program".to_string()
                } else {
                    program.clone()
                };
                format!("programs: run {what} (outside the sandbox, with your rights)")
            }
            CapabilityChange::Memory { from, to } => {
                format!("memory: up to {to} MB (was {from} MB)")
            }
        }
    }
}

/// What an upgrade would add and remove: additions need confirmation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapabilityDiff {
    pub additions: Vec<CapabilityChange>,
    pub removals: Vec<CapabilityChange>,
}

impl CapabilityDiff {
    /// Compare what an old and a new manifest ask for.
    pub fn between(old: &Grants, new: &Grants) -> Self {
        let mut diff = CapabilityDiff::default();
        if new.network && !old.network {
            diff.additions.push(CapabilityChange::Network);
        }
        if !new.network && old.network {
            diff.removals.push(CapabilityChange::Network);
        }
        if new.http && !old.http {
            diff.additions.push(CapabilityChange::Http);
        }
        if !new.http && old.http {
            diff.removals.push(CapabilityChange::Http);
        }
        if new.environment && !old.environment {
            diff.additions.push(CapabilityChange::Environment);
        }
        if !new.environment && old.environment {
            diff.removals.push(CapabilityChange::Environment);
        }
        for d in &new.directories {
            if !old.covers_dir(d) {
                diff.additions.push(CapabilityChange::Directory {
                    path: d.path.clone(),
                    write: d.write,
                });
            }
        }
        for d in &old.directories {
            if !new.covers_dir(d) {
                diff.removals.push(CapabilityChange::Directory {
                    path: d.path.clone(),
                    write: d.write,
                });
            }
        }
        let old_memory = old.memory_mb();
        let new_memory = new.memory_mb();
        if new_memory > old_memory {
            diff.additions.push(CapabilityChange::Memory {
                from: old_memory,
                to: new_memory,
            });
        } else if new_memory < old_memory {
            diff.removals.push(CapabilityChange::Memory {
                from: old_memory,
                to: new_memory,
            });
        }
        for p in &new.processes {
            if !old.may_run(p) {
                diff.additions.push(CapabilityChange::Process(p.clone()));
            }
        }
        for p in &old.processes {
            if !new.may_run(p) {
                diff.removals.push(CapabilityChange::Process(p.clone()));
            }
        }
        diff
    }

    pub fn has_additions(&self) -> bool {
        !self.additions.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        self.additions.is_empty() && self.removals.is_empty()
    }

    /// Lines for the additions, in prompt order.
    pub fn describe_additions(&self) -> Vec<String> {
        self.additions
            .iter()
            .map(CapabilityChange::describe)
            .collect()
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A Unix timestamp as a short UTC date and time, without a date library.
pub fn format_grant_time(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    let hour = day_seconds / 3_600;
    let minute = (day_seconds % 3_600) / 60;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02} UTC")
}

/// `~/x` -> `$HOME/x`.
pub fn expand_home(path: &str) -> PathBuf {
    if (path == "~" || path.starts_with("~/"))
        && let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
    {
        return PathBuf::from(home).join(path.trim_start_matches('~').trim_start_matches('/'));
    }
    PathBuf::from(path)
}

/// The kind of value a setting takes, so the host can type-check
/// `forge.plugin.config` against the plugin's declared settings (and a
/// language server can complete them). `any` accepts anything.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SettingKind {
    Boolean,
    Number,
    String,
    List,
    Table,
    #[default]
    Any,
}

impl SettingKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SettingKind::Boolean => "boolean",
            SettingKind::Number => "number",
            SettingKind::String => "string",
            SettingKind::List => "list",
            SettingKind::Table => "table",
            SettingKind::Any => "any",
        }
    }

    fn accepts(self, v: &serde_json::Value) -> bool {
        match self {
            SettingKind::Boolean => v.is_boolean(),
            SettingKind::Number => v.is_number(),
            SettingKind::String => v.is_string(),
            SettingKind::List => v.is_array(),
            SettingKind::Table => v.is_object(),
            SettingKind::Any => true,
        }
    }
}

/// A setting a plugin reads through `forge.plugin.config`, declared in its
/// manifest so the host can check the values before the plugin loads and
/// `M-x describe-plugin` can show them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Setting {
    pub name: String,
    #[serde(default)]
    pub kind: SettingKind,
    /// The default (a TOML value); shown by describe-plugin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub doc: String,
}

/// What a plugin says about itself, in the manifest's `[package]` table.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Package {
    /// The plugin's name (defaults to the file's stem).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// One line shown by `describe-plugin` and `list-packages`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// A long description: a path to a file (for example `README.org`),
    /// relative to the plugin, shown by `describe-plugin`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

/// A command a plugin provides, documented in its manifest so the host can
/// show it before the plugin runs. Written as `[[commands]]` with a
/// `name`, an optional `summary` and an optional `doc`; the older bare
/// `commands = ["a", "b"]` form still works.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum CommandDoc {
    /// Just the name, as the old form wrote it.
    Name(String),
    /// The documented form.
    Full(CommandDetail),
}

/// A documented command, `[[commands]]`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct CommandDetail {
    pub name: String,
    /// One line: what `C-h f` and the command index show first.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// The full documentation; markup follows the `forge-docs` body subset.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub doc: String,
}

/// An option a plugin provides, `[[options]]`, documented in the manifest.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct OptionDoc {
    pub name: String,
    /// The value's kind: "integer", "boolean", "string", "list", "table".
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub doc: String,
}

impl CommandDoc {
    /// The command's name.
    pub fn name(&self) -> &str {
        match self {
            CommandDoc::Name(n) => n,
            CommandDoc::Full(c) => &c.name,
        }
    }

    /// The one-line summary, if the manifest gave one.
    pub fn summary(&self) -> &str {
        match self {
            CommandDoc::Name(_) => "",
            CommandDoc::Full(c) => &c.summary,
        }
    }

    /// The full documentation, if the manifest gave one.
    pub fn doc(&self) -> &str {
        match self {
            CommandDoc::Name(_) => "",
            CommandDoc::Full(c) => &c.doc,
        }
    }
}

/// What a plugin says about itself.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Manifest {
    /// The `[package]` table: name, version, description and links.
    #[serde(default)]
    pub package: Package,
    /// Legacy top-level name; `package.name` is preferred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Legacy top-level version; `package.version` is preferred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Legacy top-level description; `package.description` is preferred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Run init when Forge starts instead of when the plugin is first
    /// needed (see "Loading" in docs/plugin-api.md).
    #[serde(default)]
    pub startup: bool,
    #[serde(default)]
    pub permissions: Grants,
    /// Other plugins this one needs: they are installed and loaded first.
    #[serde(default)]
    pub requires: Vec<String>,
    /// Settings `forge.plugin.config` may set, checked by the host.
    #[serde(default)]
    pub settings: Vec<Setting>,
    /// Commands this plugin provides and how they are documented, so the
    /// host can set up lazy loading and show their docs without running it
    /// (`use_package`'s `on`). The plugin still registers them itself
    /// when it runs.
    #[serde(default)]
    pub commands: Vec<CommandDoc>,
    /// Options this plugin provides, for the option index.
    #[serde(default)]
    pub options: Vec<OptionDoc>,
    #[serde(default)]
    pub modes: Vec<String>,
    #[serde(default)]
    pub extensions: Vec<String>,
}

/// The custom section a component can carry its manifest in.
pub const MANIFEST_SECTION: &str = "forge-manifest";

impl Manifest {
    pub fn parse(src: &str) -> Result<Self, String> {
        toml::from_str(src).map_err(|e| e.to_string())
    }

    /// The plugin's name: `[package] name`, else a legacy top-level `name`.
    pub fn name(&self) -> Option<&str> {
        self.package.name.as_deref().or(self.name.as_deref())
    }

    /// The plugin's version: `[package] version`, else a legacy top-level
    /// `version`.
    pub fn version(&self) -> Option<&str> {
        self.package.version.as_deref().or(self.version.as_deref())
    }

    /// The plugin's one-line description: `[package] description`, else a
    /// legacy top-level `description`.
    pub fn description(&self) -> Option<&str> {
        self.package
            .description
            .as_deref()
            .or(self.description.as_deref())
    }

    /// A link to the plugin's home page, if it gave one.
    pub fn homepage(&self) -> Option<&str> {
        self.package.homepage.as_deref()
    }

    /// The plugin's license, if it gave one.
    pub fn license(&self) -> Option<&str> {
        self.package.license.as_deref()
    }

    /// The path to the plugin's long description, if it gave one.
    pub fn long_doc(&self) -> Option<&str> {
        self.package.doc.as_deref()
    }

    /// The names of the commands the plugin says it provides.
    pub fn command_names(&self) -> Vec<String> {
        self.commands.iter().map(|c| c.name().to_string()).collect()
    }

    /// Check the JSON object `forge.plugin.config` set for this plugin
    /// against the settings it declared: an unknown name or a value of
    /// the wrong kind is refused before the plugin loads. A manifest with
    /// no settings accepts anything (the plugin validates its own keys).
    pub fn check_config(&self, config_json: &str) -> Result<(), String> {
        if self.settings.is_empty() {
            return Ok(());
        }
        let value: serde_json::Value =
            serde_json::from_str(config_json).map_err(|e| format!("settings aren't valid: {e}"))?;
        let object = value
            .as_object()
            .ok_or_else(|| "settings must be a table".to_string())?;
        for name in object.keys() {
            if !self.settings.iter().any(|s| &s.name == name) {
                let known: Vec<&str> = self.settings.iter().map(|s| s.name.as_str()).collect();
                return Err(format!(
                    "unknown setting {name:?} (known: {})",
                    known.join(", ")
                ));
            }
        }
        for s in &self.settings {
            if let Some(v) = object.get(&s.name)
                && !s.kind.accepts(v)
            {
                return Err(format!(
                    "setting {:?} must be a {}, not {}",
                    s.name,
                    s.kind.as_str(),
                    json_kind(v)
                ));
            }
        }
        Ok(())
    }

    /// A plugin's manifest: the file next to it (`x.wasm` -> `x.toml`) if
    /// there is one, else the one embedded in the component, else none.
    /// The file wins so users can override a plugin without rebuilding it.
    pub fn for_component(wasm: &Path, bytes: &[u8]) -> Result<Option<Self>, String> {
        let path = wasm.with_extension("toml");
        match std::fs::read_to_string(&path) {
            Ok(src) => Self::parse(&src)
                .map(Some)
                .map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::embedded(bytes)
                .map_err(|e| format!("{} (embedded manifest): {e}", wasm.display())),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// The manifest in a `forge-manifest` custom section (TOML text),
    /// wherever it is in the component (the Rust SDK puts it in the core
    /// module, which the component contains).
    pub fn embedded(bytes: &[u8]) -> Result<Option<Self>, String> {
        Self::embedded_text(bytes)?.map(Self::parse).transpose()
    }

    /// The raw TOML of the `forge-manifest` custom section, if it has one.
    pub fn embedded_text(bytes: &[u8]) -> Result<Option<&str>, String> {
        for payload in wasmparser::Parser::new(0).parse_all(bytes) {
            let Ok(payload) = payload else {
                return Ok(None);
            };
            if let wasmparser::Payload::CustomSection(c) = payload
                && c.name() == MANIFEST_SECTION
            {
                return std::str::from_utf8(c.data())
                    .map(Some)
                    .map_err(|_| "the manifest isn't UTF-8".to_string());
            }
        }
        Ok(None)
    }
}

fn json_kind(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "a list",
        serde_json::Value::Object(_) => "a table",
    }
}

/// The binary an interactive grant was approved for.
///
/// A grant belongs to a plugin *name and binary*, not just a name: the
/// same name can point at a rebuilt or replaced component. The hash is
/// what decisions are enforced against; the source is kept so a person
/// can see where the approved artifact came from. A binding with no hash
/// (an old hand-written file) falls back to the source.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct GrantBinding {
    /// The path the component was loaded from, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// SHA-256 of the component's bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Fields introduced by later Forge versions survive a read/write cycle.
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

impl GrantBinding {
    /// The binding for a component read from `path`.
    pub fn for_file(path: &Path, bytes: &[u8]) -> Self {
        Self {
            source: Some(path.display().to_string()),
            sha256: Some(sha256_hex(bytes)),
            extra: BTreeMap::new(),
        }
    }

    /// Is `self` (the saved approval) for the same binary as `other` (the
    /// one about to load)? A hash wins over a path; a binding with
    /// neither is an old, unbound approval and matches until it is bound.
    pub fn matches(&self, other: &Self) -> bool {
        match (&self.sha256, &other.sha256) {
            (Some(saved), Some(now)) => saved == now,
            _ => match (&self.source, &other.source) {
                (Some(saved), Some(now)) => saved == now,
                _ => self.sha256.is_none() && self.source.is_none(),
            },
        }
    }

    pub fn is_empty(&self) -> bool {
        self.source.is_none() && self.sha256.is_none() && self.extra.is_empty()
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// What the user has granted, per plugin name and per project, saved in
/// one TOML file.
///
/// A plugin's grant is a table of its own (`[spell]`), as it has always
/// been. A project's is in one section of its own, keyed by the directory
/// (`[projects."/home/me/src/forge"]`), so it can't be asked for by a
/// manifest and doesn't have to pretend to be a capability. Bindings are
/// additive metadata in `[plugin-bindings.spell]`, so files written by
/// older Forge versions still load.
#[derive(Debug, Default)]
pub struct GrantStore {
    path: Option<PathBuf>,
    grants: BTreeMap<String, Grants>,
    projects: BTreeMap<String, ProjectGrants>,
    bindings: BTreeMap<String, GrantBinding>,
    /// When each capability was last added or widened, by plugin and
    /// capability key. Additive metadata like `plugin-bindings`.
    grant_times: BTreeMap<String, BTreeMap<String, i64>>,
    /// Root values this version doesn't understand, kept for a rewrite.
    extra: toml::Table,
}

/// The one table name the file reserves for itself, so a plugin can't
/// take it over. A plugin with that name can be granted nothing.
const PROJECTS: &str = "projects";
/// The other reserved section: which binary each interactive grant belongs to.
const BINDINGS: &str = "plugin-bindings";
/// When each grant last changed, by plugin and capability key.
const GRANT_TIMES: &str = "plugin-grant-times";

/// The name a project is revoked and talked about by: the directory,
/// tagged so it can't be confused with a plugin's name.
pub fn project_key(root: &Path) -> String {
    format!("project:{}", root.display())
}

impl GrantStore {
    /// Grants kept only in memory (tests, `forge -q`).
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Read the grants file; a missing file means nothing is granted yet.
    pub fn load(path: PathBuf) -> Result<Self, String> {
        Self::read(&path.clone(), path)
    }

    /// Load the state-directory grants, migrating an old config-directory
    /// file the first time. Once the state file exists it is authoritative:
    /// the old file is never merged again, so a revocation can't be undone
    /// by a stale copy. Migration writes through [`crate::fsutil::write_atomic`],
    /// so a killed run leaves either the complete old world or the complete
    /// new one, and rerunning converges.
    pub fn load_default() -> Result<Self, String> {
        match (
            crate::config::grants_file(),
            crate::config::legacy_grants_file(),
        ) {
            (Some(state), legacy) => Self::load_migrating(state, legacy),
            (None, Some(legacy)) => Self::load(legacy),
            (None, None) => Ok(Self::in_memory()),
        }
    }

    /// Load `state`, or copy `legacy` into it on first use. Kept public so
    /// the migration contract can be tested without touching the home
    /// directory.
    pub fn load_migrating(state: PathBuf, legacy: Option<PathBuf>) -> Result<Self, String> {
        if state.exists() {
            return Self::load(state);
        }
        let Some(legacy) = legacy.filter(|p| p.exists() && p != &state) else {
            return Self::load(state);
        };
        // Read as the old file, but remember that saving goes to the new
        // one. Nothing is removed from the old file, so a failed or killed
        // migration is safe to retry.
        let store = Self::read(&legacy, state)?;
        store.save()?;
        Ok(store)
    }

    fn read(read_path: &Path, save_path: PathBuf) -> Result<Self, String> {
        let mut store = Self {
            path: Some(save_path),
            ..Default::default()
        };
        let src = match std::fs::read_to_string(read_path) {
            Ok(src) => src,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(store),
            Err(e) => return Err(format!("{}: {e}", read_path.display())),
        };
        // Read it as a table first, so the reserved sections can be taken
        // out and the rest still parsed as grants — one section of the
        // file being a different shape shouldn't lose the strictness of
        // every other one (a typo in a plugin's grant is still an error).
        let mut table: toml::Table =
            toml::from_str(&src).map_err(|e| format!("{}: {e}", read_path.display()))?;
        let projects = table.remove(PROJECTS);
        let bindings = table.remove(BINDINGS);
        let grant_times = table.remove(GRANT_TIMES);
        // A plugin grant is always a table. Bare root values are not one,
        // so they are future metadata (not a capability) and survive.
        let mut grants = toml::Table::new();
        for (name, value) in table {
            if value.is_table() {
                grants.insert(name, value);
            } else {
                store.extra.insert(name, value);
            }
        }
        store.grants = grants_from_table(&grants, read_path)?;
        if let Some(p) = projects {
            store.projects = p
                .try_into()
                .map_err(|e| format!("{}[{PROJECTS}]: {e}", read_path.display()))?;
        }
        if let Some(b) = bindings {
            store.bindings = b
                .try_into()
                .map_err(|e| format!("{}[{BINDINGS}]: {e}", read_path.display()))?;
        }
        if let Some(times) = grant_times {
            store.grant_times = times
                .try_into()
                .map_err(|e| format!("{}[{GRANT_TIMES}]: {e}", read_path.display()))?;
        }
        Ok(store)
    }

    pub fn get(&self, plugin: &str) -> Grants {
        self.grants.get(plugin).cloned().unwrap_or_default()
    }

    /// What `plugin` may do for the binary described by `binding`. A grant
    /// bound to another binary gives nothing here; an old unbound grant
    /// still applies (and is bound when the plugin loads).
    pub fn get_for(&self, plugin: &str, binding: &GrantBinding) -> Grants {
        if self
            .bindings
            .get(plugin)
            .is_some_and(|saved| !saved.matches(binding))
        {
            return Grants::default();
        }
        self.get(plugin)
    }

    pub fn binding(&self, plugin: &str) -> Option<&GrantBinding> {
        self.bindings.get(plugin)
    }

    pub fn bindings(&self) -> &BTreeMap<String, GrantBinding> {
        &self.bindings
    }

    /// When `plugin`'s grant of `key` last changed, as Unix seconds.
    pub fn grant_time(&self, plugin: &str, key: &str) -> Option<i64> {
        self.grant_times
            .get(plugin)
            .and_then(|times| times.get(key).copied())
    }

    /// Every recorded grant-change time for `plugin`, by capability key.
    pub fn grant_times(&self, plugin: &str) -> Option<&BTreeMap<String, i64>> {
        self.grant_times.get(plugin)
    }

    /// Was a grant saved for a different binary than `binding`?
    pub fn binding_mismatch(&self, plugin: &str, binding: &GrantBinding) -> bool {
        self.bindings
            .get(plugin)
            .is_some_and(|saved| !saved.matches(binding))
    }

    /// Give an old, unbound grant the identity of the binary that first
    /// used it. Migrated files have no binding until then.
    pub fn bind_if_unbound(
        &mut self,
        plugin: &str,
        binding: &GrantBinding,
    ) -> Result<bool, String> {
        let has_grant = self.grants.get(plugin).is_some_and(|g| !g.is_empty());
        if !has_grant || self.bindings.contains_key(plugin) {
            return Ok(false);
        }
        self.bindings.insert(plugin.to_string(), binding.clone());
        self.save()?;
        Ok(true)
    }

    pub fn all(&self) -> &BTreeMap<String, Grants> {
        &self.grants
    }

    /// Add to a plugin's grants and save.
    pub fn grant(&mut self, plugin: &str, more: &Grants) -> Result<(), String> {
        let current = self.grants.get(plugin).cloned().unwrap_or_default();
        let added = current.missing(more);
        self.grants
            .entry(plugin.to_string())
            .or_default()
            .merge(more);
        self.mark_changed(plugin, &added.capability_keys());
        self.save()
    }

    fn mark_changed(&mut self, plugin: &str, keys: &[String]) {
        if keys.is_empty() {
            return;
        }
        let now = unix_now();
        let times = self.grant_times.entry(plugin.to_string()).or_default();
        for key in keys {
            times.insert(key.clone(), now);
        }
    }

    /// Add to a plugin's grants for a particular binary and save. A grant
    /// already bound to another binary is replaced, not merged: approvals
    /// for two different components must not add up. An old unbound grant
    /// keeps its decisions and gains the current binding.
    pub fn grant_for(
        &mut self,
        plugin: &str,
        more: &Grants,
        binding: &GrantBinding,
    ) -> Result<(), String> {
        match self.bindings.get(plugin) {
            Some(saved) if !saved.matches(binding) => {
                self.grants.insert(plugin.to_string(), more.clone());
                self.grant_times.remove(plugin);
                self.mark_changed(plugin, &more.capability_keys());
            }
            _ => {
                let current = self.grants.get(plugin).cloned().unwrap_or_default();
                let added = current.missing(more);
                self.grants
                    .entry(plugin.to_string())
                    .or_default()
                    .merge(more);
                self.mark_changed(plugin, &added.capability_keys());
            }
        }
        self.bindings.insert(plugin.to_string(), binding.clone());
        self.save()
    }

    /// May Forge run `root`'s own code (its `.forge/init.rhai`)?
    pub fn project_code(&self, root: &Path) -> bool {
        self.projects
            .get(&root.display().to_string())
            .is_some_and(|p| p.code)
    }

    /// The projects that were granted anything, by directory.
    pub fn projects(&self) -> &BTreeMap<String, ProjectGrants> {
        &self.projects
    }

    /// Allow Forge to run a project's own code, and remember the answer.
    pub fn grant_project_code(&mut self, root: &Path) -> Result<(), String> {
        self.projects
            .entry(root.display().to_string())
            .or_default()
            .code = true;
        self.save()
    }

    /// Forget what was granted, by plugin name or by
    /// [`project_key`]. Either way it asks again.
    pub fn revoke(&mut self, plugin: &str) -> Result<bool, String> {
        let mut had = self.grants.remove(plugin).is_some();
        had |= self.bindings.remove(plugin).is_some();
        had |= self.grant_times.remove(plugin).is_some();
        if let Some(root) = plugin.strip_prefix("project:") {
            had |= self.projects.remove(root).is_some();
        }
        self.save()?;
        Ok(had)
    }

    fn save(&self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if self.grants.contains_key(PROJECTS) {
            return Err(format!(
                "a plugin named {PROJECTS:?} cannot be granted anything: that name is \
                 the projects section of {}",
                path.display()
            ));
        }
        if self.grants.contains_key(BINDINGS) {
            return Err(format!(
                "a plugin named {BINDINGS:?} cannot be granted anything: that name is \
                 the plugin-bindings section of {}",
                path.display()
            ));
        }
        if self.grants.contains_key(GRANT_TIMES) {
            return Err(format!(
                "a plugin named {GRANT_TIMES:?} cannot be granted anything: that name is \
                 the plugin-grant-times section of {}",
                path.display()
            ));
        }
        let mut table = self.extra.clone();
        for (name, grants) in &self.grants {
            let value = toml::Value::try_from(grants).map_err(|e| e.to_string())?;
            table.insert(name.clone(), value);
        }
        if !self.projects.is_empty() {
            let value = toml::Value::try_from(&self.projects).map_err(|e| e.to_string())?;
            table.insert(PROJECTS.to_string(), value);
        }
        if !self.bindings.is_empty() {
            let value = toml::Value::try_from(&self.bindings).map_err(|e| e.to_string())?;
            table.insert(BINDINGS.to_string(), value);
        }
        if !self.grant_times.is_empty() {
            let value = toml::Value::try_from(&self.grant_times).map_err(|e| e.to_string())?;
            table.insert(GRANT_TIMES.to_string(), value);
        }
        let body = toml::to_string_pretty(&table).map_err(|e| e.to_string())?;
        let text = format!(
            "# Permissions granted to Forge plugins, and the projects Forge may run\n\
             # code from. Delete a section (or use M-x revoke-plugin-permissions) and\n\
             # it asks again. Approvals are for the binary named in plugin-bindings.\n\n{body}"
        );
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        crate::fsutil::write_atomic(path, text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Everything but the reserved sections, as plugin grants. The file is
/// parsed by hand rather than through one `deserialize` so each plugin's
/// table keeps its `deny_unknown_fields`.
fn grants_from_table(table: &toml::Table, path: &Path) -> Result<BTreeMap<String, Grants>, String> {
    let mut out = BTreeMap::new();
    for (name, value) in table {
        let grants: Grants = value
            .clone()
            .try_into()
            .map_err(|e| format!("{}[{name}]: {e}", path.display()))?;
        out.insert(name.clone(), grants);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const MANIFEST: &str = r#"
        description = "Spell checking"
        [permissions]
        network = true
        memory-mb = 256
        [[permissions.directories]]
        path = "/usr/share/dict"
        why = "word lists"
        [[permissions.directories]]
        path = "/tmp/spell"
        write = true
    "#;

    #[test]
    fn manifests_parse_and_describe() {
        let m = Manifest::parse(MANIFEST).unwrap();
        assert_eq!(m.description(), Some("Spell checking"));
        let lines = m.permissions.describe();
        assert_eq!(
            lines,
            [
                "network: open connections and look up host names",
                "files: read /usr/share/dict (word lists)",
                "files: read and write /tmp/spell",
                "memory: up to 256 MB (default 128 MB)",
            ]
        );
        assert!(
            Manifest::parse("[permissions]\nnetwrok = true")
                .unwrap_err()
                .contains("unknown field")
        );
    }

    #[test]
    fn projects_have_their_own_section_of_the_grants_file() {
        let dir = std::env::temp_dir().join(format!("forge-grants-proj-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("plugin-grants.toml");
        let root = Path::new("/home/me/src/forge");

        let mut store = GrantStore::load(file.clone()).unwrap();
        assert!(!store.project_code(root), "nothing granted yet");
        store
            .grant(
                "spell",
                &Grants {
                    network: true,
                    ..Default::default()
                },
            )
            .unwrap();
        store.grant_project_code(root).unwrap();
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.contains("[spell]"), "{text}");
        assert!(
            text.contains(r#"[projects."/home/me/src/forge"]"#),
            "a project is a section of its own, keyed by the directory: {text}"
        );
        assert!(text.contains("code = true"), "{text}");

        // It reads back, and the plugin's grants are untouched by it.
        let store = GrantStore::load(file.clone()).unwrap();
        assert!(store.project_code(root));
        assert!(store.get("spell").network);
        assert_eq!(
            store.projects().keys().collect::<Vec<_>>(),
            [root.to_str().unwrap()]
        );

        // A file written before projects existed still loads.
        let old = dir.join("old.toml");
        fs::write(&old, "# grants\n\n[spell]\nnetwork = true\n").unwrap();
        let store = GrantStore::load(old).unwrap();
        assert!(store.get("spell").network);
        assert!(
            !store.project_code(root),
            "and asks about a project when it meets one"
        );

        // Revoking is by plugin name or by the project key.
        let mut store = GrantStore::load(file).unwrap();
        assert!(store.revoke(&project_key(root)).unwrap());
        assert!(!store.project_code(root));
        assert!(store.get("spell").network, "the plugin's are its own");
        assert!(
            !store.revoke(&project_key(root)).unwrap(),
            "and it doesn't ask twice"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_typo_in_a_plugins_grants_is_still_an_error() {
        let dir = std::env::temp_dir().join(format!("forge-grants-typo-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("plugin-grants.toml");
        fs::write(&file, "[spell]\nnetwrok = true\n").unwrap();
        let e = GrantStore::load(file.clone()).unwrap_err();
        assert!(e.contains("netwrok"), "{e}");
        // The projects section being a different shape doesn't cost the
        // plugins that strictness.
        fs::write(
            &file,
            "[spell]\nnetwrok = true\n\n[projects.\"/x\"]\ncode = true\n",
        )
        .unwrap();
        let e = GrantStore::load(file).unwrap_err();
        assert!(e.contains("netwrok"), "{e}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_is_what_is_not_yet_granted() {
        let want = Manifest::parse(MANIFEST).unwrap().permissions;
        let mut have = Grants::default();
        assert_eq!(have.missing(&want), want);
        have.merge(&Grants {
            network: true,
            environment: false,
            http: false,
            directories: vec![
                DirGrant {
                    path: "/usr/share/dict".into(),
                    write: false,
                    why: None,
                },
                DirGrant {
                    path: "/tmp/spell".into(),
                    write: false,
                    why: None,
                },
            ],
            memory_mb: None,
            processes: Vec::new(),
        });
        let still = have.missing(&want);
        assert!(!still.network);
        assert_eq!(still.directories.len(), 1, "read-only doesn't cover write");
        assert_eq!(still.memory_mb, Some(256));
        have.merge(&want);
        assert!(have.missing(&want).is_empty());
        assert_eq!(have.memory_mb(), 256);
    }

    #[test]
    fn grants_are_saved_and_revoked() {
        let dir = std::env::temp_dir().join(format!("forge-grants-{}", std::process::id()));
        let path = dir.join("plugin-grants.toml");
        let mut store = GrantStore::load(path.clone()).unwrap();
        store
            .grant(
                "spell",
                &Grants {
                    network: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let again = GrantStore::load(path.clone()).unwrap();
        assert!(again.get("spell").network);
        assert!(std::fs::read_to_string(&path).unwrap().contains("[spell]"));
        let mut again = again;
        assert!(again.revoke("spell").unwrap());
        assert!(!GrantStore::load(path).unwrap().get("spell").network);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn imports_must_be_declared_by_the_manifest() {
        let manifest = Manifest::parse(
            "[permissions]\nnetwork = true\n[[permissions.directories]]\npath = \"/tmp/x\"\n",
        )
        .unwrap();
        let imports = vec![
            "wasi:sockets/ip-name-lookup@0.2.0".to_string(),
            "wasi:filesystem/types@0.2.0".to_string(),
            "forge:editor/buffers@0.1.0".to_string(),
        ];
        assert!(uncovered_imports(Some(&manifest), &imports).is_empty());
        assert_eq!(
            import_permission("wasi:cli/environment@0.2.0").map(PermissionKind::key),
            Some("environment")
        );
        assert_eq!(
            import_permission("forge:editor/http@0.1.0"),
            Some(PermissionKind::Http)
        );

        let uncovered = uncovered_imports(
            Some(&manifest),
            &[
                "forge:editor/http@0.1.0".to_string(),
                "wasi:cli/environment@0.2.0".to_string(),
            ],
        );
        assert_eq!(uncovered.len(), 2);
        assert!(uncovered[0].describe().contains("forge:editor/http@0.1.0"));
        assert!(
            uncovered[1]
                .describe()
                .contains("wasi:cli/environment@0.2.0")
        );

        // A missing manifest declares nothing, so permission imports are
        // mismatches. (The host still loads manifest-less components that
        // only use forge:editor imports.)
        let uncovered = uncovered_imports(None, &imports);
        assert_eq!(uncovered.len(), 2);
        assert!(
            uncovered
                .iter()
                .all(|m| m.import != "forge:editor/buffers@0.1.0")
        );
    }

    #[test]
    fn grant_times_are_saved_and_removed_with_the_grant() {
        let dir = std::env::temp_dir().join(format!("forge-grants-time-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("plugin-grants.toml");
        let mut store = GrantStore::load(path.clone()).unwrap();
        store
            .grant(
                "spell",
                &Grants {
                    network: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let network_time = store.grant_time("spell", "network").expect("a time");
        assert!(network_time > 0);
        assert_eq!(format_grant_time(0), "1970-01-01 00:00 UTC");

        let again = GrantStore::load(path.clone()).unwrap();
        assert_eq!(again.grant_time("spell", "network"), Some(network_time));
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("[plugin-grant-times.spell]"), "{text}");
        assert!(text.contains("network ="), "{text}");

        let mut again = again;
        again.revoke("spell").unwrap();
        assert_eq!(again.grant_time("spell", "network"), None);
        assert!(
            !fs::read_to_string(&path)
                .unwrap()
                .contains("plugin-grant-times")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn grants_migrate_once_without_losing_or_broadening_decisions() {
        let dir = std::env::temp_dir().join(format!("forge-grants-migrate-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let state = dir.join("state/plugin-grants.toml");
        let legacy = dir.join("config/plugin-grants.toml");
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        let root = Path::new("/home/me/src/forge");
        let old = format!(
            "[spell]\nnetwork = true\n\n[projects.\"{}\"]\ncode = true\n",
            root.display()
        );
        fs::write(&legacy, &old).unwrap();

        // The first load copies every decision to the state directory.
        let store = GrantStore::load_migrating(state.clone(), Some(legacy.clone())).unwrap();
        assert!(store.get("spell").network);
        assert!(store.project_code(root));
        assert!(state.exists());
        assert_eq!(
            fs::read_to_string(&legacy).unwrap(),
            old,
            "the source is untouched"
        );

        // A stale change to the old file is not merged into the state file:
        // that would let a revoked grant come back.
        fs::write(&legacy, "[spell]\nnetwork = true\nenvironment = true\n").unwrap();
        let store = GrantStore::load_migrating(state.clone(), Some(legacy.clone())).unwrap();
        assert!(store.get("spell").network);
        assert!(!store.get("spell").environment, "no broadening on re-load");
        assert!(
            store.project_code(root),
            "the project decision survives too"
        );

        // Revoking is durable even though the old file still names it.
        let mut store = store;
        store.revoke("spell").unwrap();
        let store = GrantStore::load_migrating(state.clone(), Some(legacy)).unwrap();
        assert!(!store.get("spell").network);

        // Running it again is a no-op, not a new merge.
        let before = fs::read_to_string(&state).unwrap();
        let store = GrantStore::load_migrating(state.clone(), None).unwrap();
        assert!(!store.get("spell").network);
        assert_eq!(fs::read_to_string(&state).unwrap(), before);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn grants_are_bound_to_the_binary_not_just_the_name() {
        let dir = std::env::temp_dir().join(format!("forge-grants-bind-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("plugin-grants.toml");
        let first = GrantBinding::for_file(Path::new("/plugins/demo.wasm"), b"first");
        let second = GrantBinding::for_file(Path::new("/plugins/demo.wasm"), b"second");
        assert!(first.matches(&first));
        assert!(!first.matches(&second));
        assert!(
            GrantBinding::default().matches(&first),
            "old grants are unbound"
        );

        let mut store = GrantStore::load(path.clone()).unwrap();
        store
            .grant_for(
                "demo",
                &Grants {
                    network: true,
                    ..Default::default()
                },
                &first,
            )
            .unwrap();
        assert!(store.get_for("demo", &first).network);
        assert!(
            store.get_for("demo", &second).is_empty(),
            "another binary gets nothing from the old approval"
        );
        assert!(store.binding_mismatch("demo", &second));

        // Accepting the second binary replaces the first approval instead
        // of adding the two capability sets together.
        let mut store = GrantStore::load(path.clone()).unwrap();
        store
            .grant_for(
                "demo",
                &Grants {
                    environment: true,
                    ..Default::default()
                },
                &second,
            )
            .unwrap();
        let now = store.get_for("demo", &second);
        assert!(now.environment);
        assert!(!now.network, "old binary's capabilities are gone");
        assert!(store.get_for("demo", &first).is_empty());

        // The binding is on disk with the grant.
        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("[plugin-bindings.demo]"), "{written}");
        assert!(
            written.contains(second.sha256.as_deref().unwrap()),
            "{written}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unbound_old_grant_binds_on_first_use() {
        let dir = std::env::temp_dir().join(format!("forge-grants-unbound-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("plugin-grants.toml");
        fs::write(&path, "[spell]\nnetwork = true\n").unwrap();
        let first = GrantBinding::for_file(Path::new("/plugins/spell.wasm"), b"first");
        let second = GrantBinding::for_file(Path::new("/plugins/spell.wasm"), b"second");

        let mut store = GrantStore::load(path.clone()).unwrap();
        assert!(
            store.get_for("spell", &second).network,
            "old files still apply"
        );
        assert!(store.bind_if_unbound("spell", &first).unwrap());
        assert!(
            !store.bind_if_unbound("spell", &first).unwrap(),
            "idempotent"
        );
        assert!(
            !GrantStore::load(path.clone())
                .unwrap()
                .binding_mismatch("spell", &first)
        );
        let mut store = GrantStore::load(path).unwrap();
        assert!(store.binding_mismatch("spell", &second));
        assert!(
            store
                .grant_for("spell", &Grants::default(), &second)
                .is_ok()
        );
        assert!(store.get_for("spell", &first).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_root_and_binding_fields_survive_a_rewrite() {
        let dir = std::env::temp_dir().join(format!("forge-grants-extra-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("plugin-grants.toml");
        fs::write(
            &path,
            r#"future-root = "keep"
[demo]
network = true
[plugin-bindings.demo]
sha256 = "abc"
future-binding = 7
"#,
        )
        .unwrap();
        let mut store = GrantStore::load(path.clone()).unwrap();
        assert!(store.get("demo").network);
        assert_eq!(
            store.binding("demo").unwrap().sha256.as_deref(),
            Some("abc")
        );
        store
            .grant(
                "demo",
                &Grants {
                    environment: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("future-root = \"keep\""), "{written}");
        assert!(written.contains("future-binding = 7"), "{written}");
        assert!(written.contains("environment = true"), "{written}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn declared_settings_are_checked() {
        // (no settings: anything goes)
        assert!(Manifest::parse("").unwrap().check_config("{}").is_ok());
        let m = Manifest::parse(
            "[[settings]]\nname = \"progress\"\nkind = \"boolean\"\ndefault = false\n\n\
             [[settings]]\nname = \"style\"\nkind = \"string\"\n",
        )
        .unwrap();
        assert_eq!(m.settings.len(), 2);
        assert_eq!(m.settings[0].kind, SettingKind::Boolean);
        assert!(m.check_config("{\"progress\":true}").is_ok());
        assert!(m.check_config("{}").is_ok());
        let e = m.check_config("{\"progress\":1}").unwrap_err();
        assert!(e.contains("must be a boolean"), "{e}");
        let e = m.check_config("{\"colour\":true}").unwrap_err();
        assert!(
            e.contains("unknown setting") && e.contains("progress"),
            "{e}"
        );
    }

    #[test]
    fn package_table_carries_the_plugins_doc_fields() {
        let m = Manifest::parse(
            r#"
            [package]
            name = "vertico"
            version = "0.3.0"
            description = "Vertical minibuffer completion."
            homepage = "https://example.com/vertico"
            license = "MIT"
            doc = "README.org"

            [[commands]]
            name = "vertico-sort"
            summary = "Cycle how the vertical list orders its candidates."

            [[options]]
            name = "vertico-count"
            type = "integer"
            default = 10
            summary = "How many rows the list shows."
            "#,
        )
        .unwrap();
        assert_eq!(m.name(), Some("vertico"));
        assert_eq!(m.version(), Some("0.3.0"));
        assert_eq!(m.description(), Some("Vertical minibuffer completion."));
        assert_eq!(m.homepage(), Some("https://example.com/vertico"));
        assert_eq!(m.license(), Some("MIT"));
        assert_eq!(m.long_doc(), Some("README.org"));
        assert_eq!(m.command_names(), ["vertico-sort"]);
        assert_eq!(
            m.commands[0].summary(),
            "Cycle how the vertical list orders its candidates."
        );
        assert_eq!(m.options.len(), 1);
        assert_eq!(m.options[0].name, "vertico-count");
        assert_eq!(m.options[0].kind.as_deref(), Some("integer"));
        assert_eq!(m.options[0].summary, "How many rows the list shows.");
    }

    #[test]
    fn the_old_flat_manifest_still_parses() {
        // name/version/description at the top level, and commands as bare
        // strings: what an older SDK embedded.
        let m = Manifest::parse(
            "name = \"spell\"\nversion = \"0.1.0\"\ndescription = \"Spell checking\"\n\
             commands = [\"spell-check-buffer\"]\n",
        )
        .unwrap();
        assert_eq!(m.name(), Some("spell"));
        assert_eq!(m.version(), Some("0.1.0"));
        assert_eq!(m.description(), Some("Spell checking"));
        assert_eq!(m.command_names(), ["spell-check-buffer"]);
        assert_eq!(m.commands[0].summary(), "");
    }

    #[test]
    fn package_table_wins_over_the_legacy_fields() {
        let m = Manifest::parse("name = \"old\"\n[package]\nname = \"new\"\n").unwrap();
        assert_eq!(m.name(), Some("new"));
    }

    #[test]
    fn dependencies_parse() {
        let m = Manifest::parse("requires = [\"vertico\", \"corfu\"]\n").unwrap();
        assert_eq!(m.requires, ["vertico", "corfu"]);
    }

    #[test]
    fn the_environment_is_a_permission_of_its_own() {
        let want = Grants {
            environment: true,
            ..Default::default()
        };
        assert!(!want.is_empty());
        let mut have = Grants::default();
        assert!(have.missing(&want).environment, "asked for, not granted");
        assert!(
            have.missing(&want)
                .describe()
                .iter()
                .any(|l| l.contains("environment"))
        );
        have.merge(&want);
        assert!(have.missing(&want).is_empty());
        // A manifest can ask for it, and it reads back.
        let m = Manifest::parse("[permissions]\nenvironment = true\n").unwrap();
        assert!(m.permissions.environment);
    }

    #[test]
    fn programs() {
        let g = Grants {
            processes: vec!["pyright-langserver".into()],
            ..Default::default()
        };
        assert!(g.may_run("pyright-langserver"));
        assert!(g.may_run("/usr/local/bin/pyright-langserver"));
        assert!(!g.may_run("sh"));
        assert!(
            Grants {
                processes: vec!["*".into()],
                ..Default::default()
            }
            .may_run("anything")
        );
        let want = Grants {
            processes: vec!["pyright-langserver".into(), "clangd".into()],
            ..Default::default()
        };
        assert_eq!(g.missing(&want).processes, ["clangd"]);
        let mut all = g.clone();
        all.merge(&want);
        assert!(all.missing(&want).is_empty());
        assert!(all.describe().iter().any(|l| l.contains("run clangd")));
    }

    #[test]
    fn capability_diff_names_additions_and_removals() {
        let old = Grants {
            network: true,
            directories: vec![DirGrant {
                path: "~/old".into(),
                write: true,
                why: Some("old reason".into()),
            }],
            memory_mb: Some(256),
            processes: vec!["old-program".into()],
            ..Default::default()
        };
        let new = Grants {
            environment: true,
            directories: vec![
                DirGrant {
                    path: "~/old".into(),
                    write: true,
                    why: Some("new reason".into()),
                },
                DirGrant {
                    path: "~/new".into(),
                    write: true,
                    why: None,
                },
            ],
            memory_mb: Some(512),
            processes: vec!["*".into()],
            ..Default::default()
        };
        let diff = CapabilityDiff::between(&old, &new);
        assert!(diff.has_additions());
        assert!(diff.additions.contains(&CapabilityChange::Environment));
        assert!(diff.additions.contains(&CapabilityChange::Directory {
            path: "~/new".into(),
            write: true,
        }));
        assert!(
            diff.additions
                .contains(&CapabilityChange::Memory { from: 256, to: 512 })
        );
        assert!(
            diff.additions
                .contains(&CapabilityChange::Process("*".into()))
        );
        assert!(diff.removals.contains(&CapabilityChange::Network));
        assert!(
            !diff
                .removals
                .contains(&CapabilityChange::Process("old-program".into())),
            "a wildcard grant still covers the old program"
        );
        assert!(
            !diff.additions.iter().any(|c| matches!(
                c,
                CapabilityChange::Directory { path, .. } if path == "~/old"
            )),
            "the same directory with a different why is not a change"
        );
        let lines = diff.describe_additions();
        assert!(lines.iter().any(|l| l.contains("environment")));
        assert!(lines.iter().any(|l| l.contains("~/new")));
        assert!(lines.iter().any(|l| l.contains("512 MB")));
    }

    #[test]
    fn capability_diff_treats_a_writable_directory_as_an_addition() {
        let read_only = Grants {
            directories: vec![DirGrant {
                path: "~/cache".into(),
                write: false,
                why: None,
            }],
            ..Default::default()
        };
        let writable = Grants {
            directories: vec![DirGrant {
                path: "~/cache".into(),
                write: true,
                why: None,
            }],
            ..Default::default()
        };
        let widened = CapabilityDiff::between(&read_only, &writable);
        assert_eq!(
            widened.additions,
            [CapabilityChange::Directory {
                path: "~/cache".into(),
                write: true,
            }]
        );
        assert!(widened.removals.is_empty());

        let narrowed = CapabilityDiff::between(&writable, &read_only);
        assert!(!narrowed.has_additions(), "narrowing needs no approval");
        assert_eq!(
            narrowed.removals,
            [CapabilityChange::Directory {
                path: "~/cache".into(),
                write: true,
            }]
        );
    }

    #[test]
    fn capability_diff_uses_the_effective_memory_default() {
        assert!(CapabilityDiff::between(&Grants::default(), &Grants::default()).is_empty());
        let old = Grants {
            memory_mb: Some(DEFAULT_MEMORY_MB),
            ..Default::default()
        };
        assert!(
            !CapabilityDiff::between(&old, &Grants::default()).has_additions(),
            "the default and an explicit default are the same limit"
        );
        let more = Grants {
            memory_mb: Some(DEFAULT_MEMORY_MB + 1),
            ..Default::default()
        };
        assert!(CapabilityDiff::between(&Grants::default(), &more).has_additions());
    }
}
