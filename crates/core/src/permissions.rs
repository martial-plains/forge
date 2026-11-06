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
//! The user's answers are kept in `~/.config/forge/plugin-grants.toml`, so
//! Forge only asks again when a plugin wants something new. Without a
//! grant a plugin gets no directories, no network, no environment and the
//! default memory limit. Using a denied capability fails the way WASI
//! reports it (access denied, no such directory); a denied environment
//! read returns an error naming the permission; neither crashes the
//! plugin.

use serde::{Deserialize, Serialize};
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
        let mut out = Vec::new();
        if self.network {
            out.push("network: open connections and look up host names".to_string());
        }
        if self.http {
            out.push("http: make web requests (a request can send data out)".to_string());
        }
        if self.environment {
            out.push(
                "environment: read the process environment (variables may hold secrets)"
                    .to_string(),
            );
        }
        for d in &self.directories {
            let mode = if d.write { "read and write" } else { "read" };
            let why = d
                .why
                .as_deref()
                .map(|w| format!(" ({w})"))
                .unwrap_or_default();
            out.push(format!("files: {mode} {}{why}", d.path));
        }
        if let Some(m) = self.memory_mb {
            out.push(format!(
                "memory: up to {} MB (default {DEFAULT_MEMORY_MB} MB)",
                m.min(MAX_MEMORY_MB)
            ));
        }
        for p in &self.processes {
            let what = if p == "*" {
                "any program".to_string()
            } else {
                p.clone()
            };
            out.push(format!(
                "programs: run {what} (outside the sandbox, with your rights)"
            ));
        }
        out
    }
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

/// What the user has granted, per plugin name and per project, saved in
/// one TOML file.
///
/// A plugin's grant is a table of its own (`[spell]`), as it has always
/// been. A project's is in one section of its own, keyed by the directory
/// (`[projects."/home/me/src/forge"]`), so it can't be asked for by a
/// manifest and doesn't have to pretend to be a capability.
#[derive(Debug, Default)]
pub struct GrantStore {
    path: Option<PathBuf>,
    grants: BTreeMap<String, Grants>,
    projects: BTreeMap<String, ProjectGrants>,
}

/// The one table name the file reserves for itself, so a plugin can't
/// take it over. A plugin with that name can be granted nothing.
const PROJECTS: &str = "projects";

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
        let mut store = Self {
            path: Some(path.clone()),
            ..Default::default()
        };
        let src = match std::fs::read_to_string(&path) {
            Ok(src) => src,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(store),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        // Read it as a table first, so the projects section can be taken
        // out and the rest still parsed as grants — one section of the
        // file being a different shape shouldn't lose the strictness of
        // every other one (a typo in a plugin's grant is still an error).
        let mut table: toml::Table =
            toml::from_str(&src).map_err(|e| format!("{}: {e}", path.display()))?;
        let projects = table.remove(PROJECTS);
        store.grants = grants_from_table(&table, &path)?;
        if let Some(p) = projects {
            store.projects = p
                .try_into()
                .map_err(|e| format!("{}[{PROJECTS}]: {e}", path.display()))?;
        }
        Ok(store)
    }

    pub fn get(&self, plugin: &str) -> Grants {
        self.grants.get(plugin).cloned().unwrap_or_default()
    }

    pub fn all(&self) -> &BTreeMap<String, Grants> {
        &self.grants
    }

    /// Add to a plugin's grants and save.
    pub fn grant(&mut self, plugin: &str, more: &Grants) -> Result<(), String> {
        self.grants
            .entry(plugin.to_string())
            .or_default()
            .merge(more);
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
        let mut table = toml::Table::new();
        for (name, grants) in &self.grants {
            let value = toml::Value::try_from(grants).map_err(|e| e.to_string())?;
            table.insert(name.clone(), value);
        }
        if !self.projects.is_empty() {
            let value = toml::Value::try_from(&self.projects).map_err(|e| e.to_string())?;
            table.insert(PROJECTS.to_string(), value);
        }
        let body = toml::to_string_pretty(&table).map_err(|e| e.to_string())?;
        let text = format!(
            "# Permissions granted to Forge plugins, and the projects Forge may run\n\
             # code from. Delete a section (or use M-x revoke-plugin-permissions) and\n\
             # it asks again.\n\n{body}"
        );
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        crate::fsutil::write_atomic(path, text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Everything but the projects section, as plugin grants. The file is
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
}
