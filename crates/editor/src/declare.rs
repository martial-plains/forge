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
use forge_core::{LockEntry, LockFile, Manifest, PluginUse};
use std::path::Path;

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
pub fn load_lock() -> LockFile {
    forge_core::config::config_dir()
        .map(|d| LockFile::load(&d))
        .unwrap_or_else(LockFile::in_memory)
}

/// After init.rhai: load the declared plugins that are installed, and
/// start installing the ones that aren't.
pub fn resolve(app: &mut App) {
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
        let Some(dir) = forge_core::config::plugin_dir() else {
            break;
        };
        let from_load_path = u.load_path.iter().find_map(|d| {
            let d = forge_core::fileops::expand_file_name(d, None);
            let p = std::path::PathBuf::from(d).join(format!("{name}.wasm"));
            p.exists().then_some(p)
        });
        let path = from_load_path.unwrap_or_else(|| dir.join(format!("{name}.wasm")));
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

/// After installing a declared plugin, record it in the lock file so the
/// next `init.rhai` + `forge-lock.toml` brings the same bytes.
pub fn record(app: &App, name: &str, path: &Path) {
    let Some(u) = uses(app).into_iter().find(|u| u.name == name) else {
        return;
    };
    if let Some(entry) = lock_entry(&u, path) {
        let mut lock = load_lock();
        lock.set(name, entry);
        let _ = lock.save();
    }
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
pub fn lock_entry(u: &PluginUse, path: &Path) -> Option<LockEntry> {
    let bytes = std::fs::read(path).ok()?;
    let mut e = LockEntry {
        git: u.git.clone(),
        rev: u.rev.clone(),
        url: u.url.clone(),
        sha256: forge_core::package::sha256_hex(&bytes),
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
        }
    }
    if let Some(m) = Manifest::for_component(path, &bytes).ok().flatten() {
        e.commands = m.command_names();
        e.modes = m.modes;
        e.extensions = m.extensions;
        e.settings = m.settings;
        e.permissions = m.permissions;
    }
    Some(e)
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
