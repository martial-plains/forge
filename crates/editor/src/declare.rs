// SPDX-License-Identifier: MIT OR Apache-2.0
//! `use_package`: the plugins init.rhai declares.
//!
//! After init.rhai has run, [`resolve`] works through what it declared in
//! order (`after` dependencies first): it loads the ones already
//! installed — lazily, for those with `on.commands` — and installs the
//! ones that are missing (from git or a url). Each installed declaration
//! is recorded in `forge-lock.toml` (its commit and sha256), so a second
//! machine that copies `init.rhai` and `forge-lock.toml` gets the same
//! plugins.
//!
//! Declaring a plugin by an index name needs the package index: if it
//! isn't loaded yet, the user is told to install it with
//! `M-x package-install` (the other way of declaring is git or url).

use crate::app::App;
use crate::plugins;
use forge_core::declare::order;
use forge_core::{LockCapabilities, LockEntry, LockFile, Manifest, PluginUse};
use std::path::{Path, PathBuf};

/// The plugin names init.rhai declared, in load order (`after` first).
pub fn declared_order(app: &App) -> Vec<String> {
    let Some(rhai) = app.rhai.as_ref() else {
        return Vec::new();
    };
    order(&rhai.plugin_uses())
}

/// The declaration for `name`, if init.rhai made one.
pub fn declared(app: &App, name: &str) -> Option<PluginUse> {
    uses(app).into_iter().find(|u| u.name == name)
}

fn uses(app: &App) -> Vec<PluginUse> {
    app.rhai
        .as_ref()
        .map(|l| l.plugin_uses())
        .unwrap_or_default()
}

/// The lock file, next to init.rhai (in memory if there's no config dir).
pub fn load_lock() -> Result<LockFile, String> {
    match forge_core::config::config_dir() {
        Some(dir) => LockFile::load(&dir).map_err(|error| error.to_string()),
        None => Ok(LockFile::in_memory()),
    }
}

/// After init.rhai: load the declared plugins that are installed, and
/// start installing the ones that aren't.
pub fn resolve(app: &mut App) {
    if let Err(error) = load_lock() {
        return app.error(format!("Cannot read the package lock: {error}"));
    }
    let uses = uses(app);
    if uses.is_empty() {
        return;
    }
    let by_name: std::collections::BTreeMap<&str, &PluginUse> =
        uses.iter().map(|u| (u.name.as_str(), u)).collect();
    for name in order(&uses) {
        let Some(u) = by_name.get(name.as_str()) else {
            continue;
        };
        let Some(path) = component_path(u) else {
            break;
        };
        if path.exists() {
            load_declared(app, u, &path);
            // Recorded one at a time: an install calls `record` itself, so
            // a lock held across the loop would be stale and clobber it.
            record(app, &name, &path);
        } else {
            install(app, u);
        }
    }
}

/// Where a declaration's component lives: a matching `load_path`, else
/// `NAME.wasm` in the plugins directory.
fn component_path(u: &PluginUse) -> Option<PathBuf> {
    let from_load_path = u.load_path.iter().find_map(|dir| {
        let dir = forge_core::fileops::expand_file_name(dir, None);
        let path = PathBuf::from(dir).join(format!("{}.wasm", u.name));
        path.exists().then_some(path)
    });
    let dir = forge_core::config::plugin_dir()?;
    Some(from_load_path.unwrap_or_else(|| dir.join(format!("{}.wasm", u.name))))
}

/// Where a declared plugin's component lives, if `init.rhai` declares it.
pub(crate) fn declared_component_path(app: &App, name: &str) -> Option<PathBuf> {
    declared(app, name).and_then(|u| component_path(&u))
}

/// After installing a declared plugin, record it in the lock file so the
/// next `init.rhai` + `forge-lock.toml` brings the same bytes.
pub fn record(app: &mut App, name: &str, path: &Path) {
    if let Err(error) = record_checked(app, name, path) {
        app.error(format!("{name}: couldn't update lockfile: {error}"));
    }
}

pub fn record_checked(app: &App, name: &str, path: &Path) -> Result<(), String> {
    let Some(u) = uses(app).into_iter().find(|u| u.name == name) else {
        return Ok(());
    };
    let entry = lock_entry(app, &u, path)
        .map_err(|error| format!("{name}: couldn't record lock entry: {error}"))?;
    let bytes = std::fs::read(path)
        .map_err(|error| format!("{name}: couldn't read component for lockfile: {error}"))?;
    let mut lock = load_lock().map_err(|error| format!("{name}: {error}"))?;
    lock.set(name, entry);
    if let Some(dir) = lock.dir() {
        LockFile::save_artifact(
            dir,
            lock.get(name)
                .and_then(|e| e.artifact_sha256.as_deref())
                .unwrap_or_default(),
            &bytes,
        )
        .map_err(|error| format!("{name}: couldn't save the locked artifact: {error}"))?;
    }
    lock.save()
        .map_err(|error| format!("{name}: couldn't update forge-lock.toml: {error}"))
}

/// Load one declared plugin: lazily when it named `on.commands` (and
/// doesn't ask for `startup`), else as usual.
fn load_declared(app: &mut App, u: &PluginUse, path: &Path) {
    let startup = Manifest::for_component(path, &std::fs::read(path).unwrap_or_default())
        .ok()
        .flatten()
        .is_some_and(|m| m.startup);
    if u.no_require && !u.lazy() {
        return;
    }
    if !startup && u.lazy() {
        plugins::load_lazy(app, path, &u.on_commands, &u.on_modes, &u.on_extensions);
    } else {
        plugins::load_at_startup(app, path);
    }
}

/// The lock entry for an installed declaration: its source (from the
/// installed record, or the declaration), and its manifest.
pub fn lock_entry(app: &App, u: &PluginUse, path: &Path) -> Result<LockEntry, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let artifact_sha256 = forge_core::package::sha256_hex(&bytes);
    let previous = load_lock()
        .map_err(|error| error.to_string())?
        .get(&u.name)
        .cloned();
    let mut e = LockEntry {
        git: u.git.clone(),
        rev: u.rev.clone(),
        url: u.url.clone(),
        sha256: artifact_sha256.clone(),
        artifact_sha256: Some(artifact_sha256),
        spec_hash: Some(static_spec_hash(u)?),
        tier: Some(
            if u.git.is_some() {
                "source"
            } else {
                "prebuilt"
            }
            .into(),
        ),
        recipe_hash: Some(recipe_hash(u)?),
        capabilities: LockCapabilities::default(),
        ..Default::default()
    };
    if let Some(dir) = forge_core::config::plugin_dir()
        && let Some(i) = forge_core::package::InstalledDb::load(&dir)
            .packages
            .get(&u.name)
            .cloned()
    {
        if i.git.is_some() {
            e.git = i.git;
            e.rev = i.rev;
            e.commit = i.commit;
        }
        if !i.url.is_empty() {
            e.url = Some(i.url);
        }
        if !i.sha256.is_empty() {
            e.sha256 = i.sha256;
            e.artifact_sha256 = Some(e.sha256.clone());
        }
        if e.git.is_some() {
            e.tier = Some("source".into());
        }
        e.build = i.build.clone();
        if let Some(build) = i.build {
            e.recipe_hash = Some(recipe_hash_for(
                &build,
                &u.features,
                &u.name,
                u.git_dir.as_deref(),
            )?);
        }
        e.version = Some(i.version);
    }
    if let Some(m) = Manifest::for_component(path, &bytes).ok().flatten() {
        e.version = m.version.clone().or(e.version);
        e.commands = m.command_names();
        e.modes = m.modes;
        e.extensions = m.extensions;
        e.settings = m.settings;
        e.permissions = m.permissions.clone();
        e.capabilities.permissions = m.permissions;
    }
    e.capabilities.imports = if let Some(imports) = previous.as_ref().and_then(|old| {
        (old.artifact_sha256
            .as_deref()
            .unwrap_or(old.sha256.as_str())
            == e.sha256
            && old.spec_hash.is_some())
        .then(|| old.capabilities.imports.clone())
    }) {
        imports
    } else if let Some(host) = app.plugin_host.as_ref() {
        host.inspect_file(path)
            .map_err(|e| format!("inspecting component imports: {e:#}"))?
            .inspection
            .imports
    } else {
        forge_core::PluginRuntime::new()
            .map_err(|e| format!("starting component inspection: {e:#}"))?
            .inspect(&u.name, &bytes)
            .map_err(|e| format!("inspecting component imports: {e:#}"))?
            .imports
    };
    Ok(e)
}

fn static_spec_hash(u: &PluginUse) -> Result<String, String> {
    let static_spec = serde_json::json!({
        "name": u.name,
        "git": u.git,
        "rev": u.rev,
        "index": u.index,
        "url": u.url,
        "sha256": u.sha256,
        "version": u.version,
        "frozen": u.frozen,
        "config": u.config,
        "grants": u.grants,
        "hooks": u.hooks,
        "commands": u.on_commands,
        "modes": u.on_modes,
        "extensions": u.on_extensions,
        "after": u.after,
        "load_path": u.load_path,
        "no_require": u.no_require,
        "git_dir": u.git_dir,
        "build": u.build,
        "features": u.features,
    });
    serde_json::to_vec(&static_spec)
        .map(|bytes| forge_core::package::sha256_hex(&bytes))
        .map_err(|e| format!("serializing static declaration: {e}"))
}

fn recipe_hash(u: &PluginUse) -> Result<String, String> {
    let command = u.build.as_deref().unwrap_or("");
    recipe_hash_for(command, &u.features, &u.name, u.git_dir.as_deref())
}

fn recipe_hash_for(
    command: &str,
    features: &[String],
    name: &str,
    source_subdir: Option<&str>,
) -> Result<String, String> {
    let toolchain = forge_core::package::vc_dir(name)
        .map(|root| source_subdir.map_or(root.clone(), |subdir| root.join(subdir)))
        .and_then(|root| {
            ["rust-toolchain.toml", "rust-toolchain"]
                .iter()
                .find_map(|file| std::fs::read_to_string(root.join(file)).ok())
        })
        .unwrap_or_default();
    let recipe = serde_json::json!({
        "command": command,
        "toolchain": toolchain,
        "features": features,
        "target": "wasm32-wasip2",
    });
    serde_json::to_vec(&recipe)
        .map(|bytes| forge_core::package::sha256_hex(&bytes))
        .map_err(|e| format!("serializing build recipe: {e}"))
}

/// Start installing a declared plugin that isn't installed yet.
fn install(app: &mut App, u: &PluginUse) {
    if let Some(git) = &u.git {
        app.msg(format!(
            "Installing declared plugin {} from {git}...",
            u.name
        ));
        crate::package_vc::install(app, &u.name, git, u.rev.clone(), false);
    } else if let Some(url) = &u.url {
        let Some(sha256) = &u.sha256 else {
            return app.error(format!(
                "{}: declaring a url needs sha256 (for its checksum)",
                u.name
            ));
        };
        app.msg(format!("Downloading declared plugin {}...", u.name));
        crate::packages::install_url(app, &u.name, url, sha256);
    } else if let Some(index) = &u.index {
        if app.packages.index.is_some() {
            crate::packages::install_named(app, index);
        } else {
            app.error(format!(
                "{}: {index} isn't installed; run M-x package-install (or add git = ...)",
                u.name
            ));
        }
    } else {
        app.error(format!(
            "{} is declared but not installed, and the declaration has no git, url or index",
            u.name
        ));
    }
}
