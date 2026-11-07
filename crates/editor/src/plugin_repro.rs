// SPDX-License-Identifier: MIT OR Apache-2.0
//! M-x plugin-repro: write a small, deterministic reproduction bundle for
//! one declared plugin and the `requires` plugins it needs.
//!
//! The bundle has the plugin's component and manifest, a minimal
//! `init.rhai`, a minimal lock file with the exact revisions, the Forge
//! and system versions, and a README. It deliberately does not copy
//! grants, credentials, sessions, or the rest of the user's init file.

use crate::app::App;
use forge_core::{LockEntry, LockFile, Manifest, PluginUse};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const PREFIX: &str = "forge-repro-";

/// The declared plugins that can be reproduced, sorted.
pub fn candidates(app: &App) -> Vec<String> {
    app.rhai
        .as_ref()
        .map(|rhai| {
            rhai.plugin_uses()
                .into_iter()
                .map(|use_| use_.name)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        })
        .unwrap_or_default()
}

/// Write `forge-repro-NAME` in the current directory.
pub fn write(app: &App, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    validate_name(name)?;
    let current = std::env::current_dir()
        .map_err(|error| format!("cannot find the current directory: {error}"))?;
    let dir = current.join(format!("{PREFIX}{name}"));
    write_to(app, name, &dir)?;
    Ok(dir)
}

/// Write the bundle to `dir`, which must not already exist.
pub fn write_to(app: &App, name: &str, dir: &Path) -> Result<(), String> {
    validate_name(name)?;
    if dir.exists() {
        return Err(format!("{} already exists", dir.display()));
    }
    let uses = app
        .rhai
        .as_ref()
        .map(|rhai| rhai.plugin_uses())
        .unwrap_or_default();
    let by_name: BTreeMap<&str, &PluginUse> =
        uses.iter().map(|use_| (use_.name.as_str(), use_)).collect();
    if !by_name.contains_key(name) {
        return Err(format!("{name} isn't declared by use_package"));
    }

    let mut wanted = BTreeSet::new();
    let mut pending = vec![name.to_string()];
    while let Some(name) = pending.pop() {
        if !wanted.insert(name.clone()) {
            continue;
        }
        if let Some(use_) = by_name.get(name.as_str()) {
            pending.extend(use_.requires.iter().cloned());
        }
    }
    for name in &wanted {
        validate_name(name)?;
    }

    let lock = load_lock(app)?;
    let plugins_dir = app
        .packages
        .plugin_dir
        .clone()
        .or_else(forge_core::config::plugin_dir);
    let installed = plugins_dir
        .as_deref()
        .map(forge_core::package::InstalledDb::load)
        .unwrap_or_default();

    let mut artifacts = BTreeMap::new();
    let mut pins = BTreeMap::new();
    for name in &wanted {
        let path = plugin_path(app, name, plugins_dir.as_deref())
            .ok_or_else(|| format!("{name}: cannot find its .wasm file"))?;
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("{name}: reading {}: {error}", path.display()))?;
        let manifest_path = path.with_file_name(format!("{name}.toml"));
        let manifest = match std::fs::read(&manifest_path) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Manifest::embedded_text(&bytes)
                    .map_err(|error| format!("{name}: {error}"))?
                    .map(|text| text.as_bytes().to_vec())
            }
            Err(error) => {
                return Err(format!(
                    "{name}: reading {}: {error}",
                    manifest_path.display()
                ));
            }
        };
        let hash = forge_core::package::sha256_hex(&bytes);
        pins.insert(
            name.clone(),
            pin(
                by_name.get(name.as_str()).copied(),
                lock.get(name),
                installed.packages.get(name),
                hash,
            ),
        );
        artifacts.insert(name.clone(), (bytes, manifest));
    }

    let declared: Vec<&PluginUse> = uses
        .iter()
        .filter(|use_| wanted.contains(&use_.name))
        .collect();
    let init = render_init(name, &declared, &pins, &wanted);
    let lock_text = render_lock(&pins);
    let readme = render_readme(name, &wanted, &declared);
    let info = render_info();

    std::fs::create_dir_all(dir.join("plugins"))
        .map_err(|error| format!("creating {}: {error}", dir.display()))?;
    let write = |path: &Path, text: &[u8]| {
        forge_core::fsutil::write_atomic(path, text)
            .map_err(|error| format!("writing {}: {error}", path.display()))
    };
    write(&dir.join("init.rhai"), init.as_bytes())?;
    write(&dir.join("forge-lock.toml"), lock_text.as_bytes())?;
    write(&dir.join("README.md"), readme.as_bytes())?;
    write(&dir.join("info.txt"), info.as_bytes())?;
    for (name, (bytes, manifest)) in artifacts {
        write(&dir.join("plugins").join(format!("{name}.wasm")), &bytes)?;
        if let Some(manifest) = manifest {
            write(&dir.join("plugins").join(format!("{name}.toml")), &manifest)?;
        }
    }
    Ok(())
}

fn load_lock(app: &App) -> Result<LockFile, String> {
    let dir = app
        .init_path
        .as_deref()
        .and_then(Path::parent)
        .map(ToOwned::to_owned);
    match dir {
        Some(dir) => LockFile::load(&dir).map_err(|error| error.to_string()),
        None => crate::declare::load_lock(),
    }
}

fn plugin_path(app: &App, name: &str, plugins_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = app
        .plugin_host
        .as_ref()
        .and_then(|host| host.source(name))
        .filter(|path| path.is_file())
    {
        return Some(path.to_path_buf());
    }
    if let Some(use_) = crate::declare::declared(app, name) {
        for dir in &use_.load_path {
            let dir = forge_core::fileops::expand_file_name(dir, None);
            let path = PathBuf::from(dir).join(format!("{name}.wasm"));
            if path.is_file() {
                return Some(path);
            }
        }
    }
    let dir = plugins_dir?;
    let flat = dir.join(format!("{name}.wasm"));
    if flat.is_file() {
        return Some(flat);
    }
    let nested = dir.join(name).join(format!("{name}.wasm"));
    nested.is_file().then_some(nested)
}

fn pin(
    declared: Option<&PluginUse>,
    locked: Option<&LockEntry>,
    installed: Option<&forge_core::package::Installed>,
    hash: String,
) -> LockEntry {
    let mut entry = LockEntry {
        sha256: hash.clone(),
        artifact_sha256: Some(hash),
        ..Default::default()
    };
    if let Some(locked) = locked {
        entry.git = locked.git.clone();
        entry.rev = locked.rev.clone();
        entry.url = locked.url.clone();
        entry.commit = locked.commit.clone();
        entry.version = locked.version.clone();
    }
    if let Some(declared) = declared {
        entry.git = entry.git.or_else(|| declared.git.clone());
        entry.rev = entry.rev.or_else(|| declared.rev.clone());
        entry.url = entry.url.or_else(|| declared.url.clone());
        entry.version = entry.version.or_else(|| declared.version.clone());
    }
    if let Some(installed) = installed {
        entry.git = entry.git.or_else(|| installed.git.clone());
        entry.rev = entry.rev.or_else(|| installed.rev.clone());
        entry.commit = entry.commit.or_else(|| installed.commit.clone());
        if entry.url.is_none() && !installed.url.is_empty() {
            entry.url = Some(installed.url.clone());
        }
        if entry.version.is_none() && !installed.version.is_empty() {
            entry.version = Some(installed.version.clone());
        }
    }
    entry.git = entry.git.as_deref().map(|url| sanitize_url(url).0);
    entry.url = entry.url.as_deref().map(|url| sanitize_url(url).0);
    entry
}

fn render_init(
    root: &str,
    declared: &[&PluginUse],
    pins: &BTreeMap<String, LockEntry>,
    wanted: &BTreeSet<String>,
) -> String {
    let mut out = format!(
        "// Minimal reproduction for {root}.\n\
         // Generated by M-x plugin-repro. Secret-looking config values and URL\n\
         // credentials are redacted.\n\n"
    );
    for use_ in declared {
        out.push_str("use_package(");
        out.push_str(&quote(&use_.name));
        out.push_str(", #{");
        let pin = pins.get(&use_.name);
        let commit = pin.and_then(|pin| pin.commit.as_deref());
        let rev = commit.or(use_.rev.as_deref());
        let mut fields = Vec::new();
        if let Some(git) = &use_.git {
            fields.push(format!("git: {}", quote(&sanitize_url(git).0)));
        }
        if use_.git.is_some()
            && let Some(rev) = rev
        {
            fields.push(format!("pin: {}", quote(rev)));
        }
        if let Some(index) = &use_.index {
            fields.push(format!("index: {}", quote(&sanitize_url(index).0)));
        }
        if let Some(version) = &use_.version {
            fields.push(format!("version: {}", quote(version)));
        }
        if let Some(url) = &use_.url {
            fields.push(format!("url: {}", quote(&sanitize_url(url).0)));
        }
        if let Some(sha256) = use_
            .sha256
            .as_ref()
            .or_else(|| use_.url.as_ref().and_then(|_| pin.map(|pin| &pin.sha256)))
        {
            fields.push(format!("sha256: {}", quote(sha256)));
        }
        let requires: Vec<String> = use_
            .requires
            .iter()
            .filter(|name| wanted.contains(*name))
            .map(|name| quote(name))
            .collect();
        if !requires.is_empty() {
            fields.push(format!("requires: [{}]", requires.join(", ")));
        }
        if let Some(config) = &use_.config {
            let (config, _) = sanitize_config(config)
                .unwrap_or_else(|_| (Value::String("<invalid config>".into()), true));
            fields.push(format!("config: {}", render_json(&config)));
        }
        fields.push("freeze: true".to_string());
        out.push(' ');
        out.push_str(&fields.join(", "));
        out.push_str(" });\n");
    }
    out
}

fn render_lock(pins: &BTreeMap<String, LockEntry>) -> String {
    let mut out = String::from("# Minimal pins for the reproduction bundle.\nlock_version = 1\n");
    for (name, pin) in pins {
        out.push_str(&format!("\n[packages.{}]\n", quote(name)));
        if let Some(git) = &pin.git {
            out.push_str(&format!("git = {}\n", quote(git)));
        }
        if let Some(rev) = &pin.rev {
            out.push_str(&format!("rev = {}\n", quote(rev)));
        }
        if let Some(url) = &pin.url {
            out.push_str(&format!("url = {}\n", quote(url)));
        }
        if let Some(commit) = &pin.commit {
            out.push_str(&format!("commit = {}\n", quote(commit)));
        }
        out.push_str(&format!("sha256 = {}\n", quote(&pin.sha256)));
        if let Some(hash) = &pin.artifact_sha256 {
            out.push_str(&format!("artifact_sha256 = {}\n", quote(hash)));
        }
        if let Some(version) = &pin.version {
            out.push_str(&format!("version = {}\n", quote(version)));
        }
    }
    out
}

fn render_readme(root: &str, wanted: &BTreeSet<String>, declared: &[&PluginUse]) -> String {
    let declared_names: BTreeSet<&str> = declared.iter().map(|use_| use_.name.as_str()).collect();
    let extra: Vec<&String> = wanted
        .iter()
        .filter(|name| !declared_names.contains(name.as_str()))
        .collect();
    let mut out = format!(
        "# Forge reproduction: {root}\n\n\
         This is a minimal bundle for the declared plugin `{root}` and the\n\
         plugins it requires. Run it with:\n\n\
         ```sh\n\
         forge --init-directory \"$PWD\"\n\
         ```\n\n\
         ## Contents\n\n\
         - `init.rhai`: only this plugin and its `requires` closure.\n\
         - `forge-lock.toml`: exact revisions and artifact hashes.\n\
         - `plugins/`: the components and their manifests.\n\
         - `info.txt`: the Forge and system versions.\n\n"
    );
    out.push_str(&format!(
        "Plugins: {}\n\n",
        wanted.iter().cloned().collect::<Vec<_>>().join(", ")
    ));
    if !extra.is_empty() {
        out.push_str(&format!(
            "Not declared by `use_package` (included as artifacts only): {}\n\n",
            extra
                .iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    out.push_str(
        "For safety this bundle does not include `plugin-grants.toml`,\n\
         credentials, `session.toml`, or unrelated configuration. Secret-looking\n\
         config keys and URL credentials are replaced with `<redacted>`, so the\n\
         plugin may ask for permissions again and private sources may need\n\
         credentials supplied locally.\n",
    );
    out
}

fn render_info() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    format!(
        "Forge version: {}\nOS: {}\nArchitecture: {}\nFamily: {}\nGenerated (unix seconds): {seconds}\n",
        forge_core::VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::env::consts::FAMILY,
    )
}

fn sanitize_config(json: &str) -> Result<(Value, bool), String> {
    let mut value: Value =
        serde_json::from_str(json).map_err(|error| format!("config isn't valid JSON: {error}"))?;
    let mut changed = false;
    redact_json(&mut value, &mut changed);
    Ok((value, changed))
}

fn redact_json(value: &mut Value, changed: &mut bool) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if is_secret_key(key) {
                    *value = Value::String("<redacted>".into());
                    *changed = true;
                } else {
                    redact_json(value, changed);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_json(item, changed);
            }
        }
        Value::String(text) => {
            let (clean, redacted) = sanitize_url(text);
            if redacted {
                *text = clean;
                *changed = true;
            }
        }
        _ => {}
    }
}

fn is_secret_key(key: &str) -> bool {
    let key: String = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    matches!(key.as_str(), "auth" | "authorization" | "bearer" | "cookie")
        || key.contains("password")
        || key.contains("passwd")
        || key.contains("secret")
        || key.contains("token")
        || key.contains("credential")
        || key.contains("apikey")
        || key.contains("privatekey")
        || key.contains("accesskey")
}

fn sanitize_params(text: &str, changed: &mut bool) -> String {
    text.split('&')
        .map(|part| {
            let Some((key, value)) = part.split_once('=') else {
                return part.to_string();
            };
            if is_secret_key(key) {
                *changed = true;
                format!("{key}=<redacted>")
            } else {
                let (value, redacted) = sanitize_url(value);
                *changed |= redacted;
                format!("{key}={value}")
            }
        })
        .collect::<Vec<_>>()
        .join("&")
}

fn sanitize_url(url: &str) -> (String, bool) {
    let Some((scheme, rest)) = url.split_once("://") else {
        return (url.to_string(), false);
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let mut changed = false;
    let authority = match authority.rsplit_once('@') {
        Some((_, host)) => {
            changed = true;
            format!("<redacted>@{host}")
        }
        None => authority.to_string(),
    };
    let tail = rest[end..].to_string();
    let (before, fragment) = match tail.split_once('#') {
        Some((before, fragment)) => (before.to_string(), Some(fragment)),
        None => (tail, None),
    };
    let mut tail = match before.split_once('?') {
        Some((path, query)) => format!("{path}?{}", sanitize_params(query, &mut changed)),
        None => before,
    };
    if let Some(fragment) = fragment {
        tail.push('#');
        tail.push_str(&sanitize_params(fragment, &mut changed));
    }
    (format!("{scheme}://{authority}{tail}"), changed)
}

fn render_json(value: &Value) -> String {
    match value {
        Value::Null => "()".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => quote(value),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(render_json).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(map) => {
            let mut fields: Vec<(&String, &Value)> = map.iter().collect();
            fields.sort_by(|a, b| a.0.cmp(b.0));
            format!(
                "#{{ {} }}",
                fields
                    .into_iter()
                    .map(|(key, value)| format!("{}: {}", quote(key), render_json(value)))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}

fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.starts_with('.')
        || name.contains(['/', '\\'])
        || name.bytes().any(|b| b.is_ascii_control())
    {
        return Err(format!("invalid plugin name {name:?}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_keys_and_url_credentials_are_redacted() {
        let (value, changed) = sanitize_config(
            r#"{"style":"dark","api_token":"topsecret","nested":{"password":"hunter2","safe":"kept"},"url":"https://user:pw@example.com/x?token=abc&ok=1#access_token=fragsecret"}"#,
        )
        .unwrap();
        assert!(changed);
        let text = render_json(&value);
        assert!(text.contains("\"api_token\": \"<redacted>\""));
        assert!(text.contains("\"password\": \"<redacted>\""));
        assert!(text.contains("\"safe\": \"kept\""));
        assert!(text.contains(
            "https://<redacted>@example.com/x?token=<redacted>&ok=1#access_token=<redacted>"
        ));
        assert!(!text.contains("topsecret"));
        assert!(!text.contains("hunter2"));
        assert!(!text.contains("user:pw"));
        assert!(!text.contains("fragsecret"));
    }

    #[test]
    fn names_cannot_escape_the_bundle() {
        assert!(validate_name("demo").is_ok());
        assert!(validate_name("../demo").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name(".hidden").is_err());
    }
}
