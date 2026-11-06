// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where Forge keeps its configuration.

use std::path::PathBuf;

/// Forge's config directory: `$FORGE_CONFIG_DIR`, else
/// `$XDG_CONFIG_HOME/forge`, else `~/.config/forge`.
pub fn config_dir() -> Option<PathBuf> {
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    env("FORGE_CONFIG_DIR")
        .or_else(|| env("XDG_CONFIG_HOME").map(|d| d.join("forge")))
        .or_else(|| {
            env("HOME")
                .or_else(|| env("USERPROFILE"))
                .map(|h| h.join(".config").join("forge"))
        })
}

/// `init.rhai`: settings, key bindings and commands (see `crate::rhai`).
pub fn init_file() -> Option<PathBuf> {
    config_dir().map(|d| d.join("init.rhai"))
}

/// The directory a theme file lives in: `themes/` in the config dir. Its
/// `*.rhai` files load after init.rhai and define named themes.
pub fn theme_dir() -> Option<PathBuf> {
    config_dir().map(|d| d.join("themes"))
}

/// Rhai theme files, sorted by name, so the load order is stable.
pub fn theme_files() -> Vec<PathBuf> {
    let Some(dir) = theme_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "rhai"))
        .collect();
    out.sort();
    out
}

/// Plugins found here load at startup: `plugins/NAME.wasm`, or
/// `plugins/NAME/NAME.wasm` for plugins that come with other files.
pub fn plugin_dir() -> Option<PathBuf> {
    config_dir().map(|d| d.join("plugins"))
}

/// Forge's cache: `$FORGE_CACHE_DIR`, else `$XDG_CACHE_HOME/forge`, else
/// `~/.cache/forge`. Safe to delete at any time.
pub fn cache_dir() -> Option<PathBuf> {
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    env("FORGE_CACHE_DIR")
        .or_else(|| env("XDG_CACHE_HOME").map(|d| d.join("forge")))
        .or_else(|| {
            env("HOME")
                .or_else(|| env("USERPROFILE"))
                .map(|h| h.join(".cache").join("forge"))
        })
}

/// Forge's data directory: `$FORGE_DATA_DIR`, else `$XDG_DATA_HOME/forge`,
/// else `~/.local/share/forge`.
pub fn data_dir() -> Option<PathBuf> {
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    env("FORGE_DATA_DIR")
        .or_else(|| env("XDG_DATA_HOME").map(|d| d.join("forge")))
        .or_else(|| {
            env("HOME")
                .or_else(|| env("USERPROFILE"))
                .map(|h| h.join(".local").join("share").join("forge"))
        })
}

/// Tree-sitter grammars installed by Forge: `tree-sitter/` in the data dir.
pub fn grammar_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("tree-sitter"))
}

/// What Forge remembers between sessions (session.rs): `$FORGE_STATE_DIR`,
/// else `$XDG_STATE_HOME/forge`, else `~/.local/state/forge`.
pub fn state_dir() -> Option<PathBuf> {
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    env("FORGE_STATE_DIR")
        .or_else(|| env("XDG_STATE_HOME").map(|d| d.join("forge")))
        .or_else(|| {
            env("HOME")
                .or_else(|| env("USERPROFILE"))
                .map(|h| h.join(".local").join("state").join("forge"))
        })
}

/// Interactive plugin approvals: `plugin-grants.toml` in the state
/// directory, because they are machine-local and must not travel with a
/// shared configuration.
pub fn grants_file() -> Option<PathBuf> {
    state_dir().map(|d| d.join("plugin-grants.toml"))
}

/// Where older Forge versions kept interactive approvals. Read only as a
/// migration source when the state file does not exist yet.
pub fn legacy_grants_file() -> Option<PathBuf> {
    config_dir().map(|d| d.join("plugin-grants.toml"))
}

/// Plugin files in a plugins directory, sorted by name.
pub fn discover_plugins(dir: &std::path::Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let p = e.path();
            if p.is_file() && p.extension().is_some_and(|x| x == "wasm") {
                Some(p)
            } else if p.is_dir() {
                let name = p.file_name()?.to_owned();
                let inner = p.join(name).with_extension("wasm");
                inner.is_file().then_some(inner)
            } else {
                None
            }
        })
        .collect();
    out.sort();
    out
}

/// The TOML file older Forge versions read, now replaced by init.rhai.
pub fn old_keys_file() -> Option<PathBuf> {
    config_dir().map(|d| d.join("keys.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_and_theme_files_use_rhai() {
        let dir = std::env::temp_dir().join(format!("forge-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("themes")).unwrap();
        std::fs::write(dir.join("themes/a.rhai"), "").unwrap();
        std::fs::write(dir.join("themes/b.lua"), "").unwrap();
        // SAFETY: single-threaded test setup.
        unsafe { std::env::set_var("FORGE_CONFIG_DIR", &dir) };
        assert_eq!(init_file(), Some(dir.join("init.rhai")));
        assert_eq!(theme_files(), vec![dir.join("themes/a.rhai")]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
