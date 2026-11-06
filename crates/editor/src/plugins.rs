// SPDX-License-Identifier: MIT OR Apache-2.0
//! The terminal's side of plugins: running their commands, asking their
//! questions in the minibuffer, showing their messages, telling them
//! about events. The plugins themselves live in `forge_core::PluginHost`.

use crate::app::App;
use crate::minibuffer::Purpose;
use forge_core::plugin::runtime::Level;
use forge_core::plugin::runtime::is_supported_import;
use forge_core::{
    Effect, Grants, Manifest, Outcome, PluginFile, PluginHost, PluginRequest, Question,
};
use std::path::{Path, PathBuf};

/// How deep plugin commands may run each other (commands.run) before
/// Forge assumes a loop.
const MAX_DEPTH: usize = 16;

/// How often plugin files are checked for rebuilds (when idle).
pub const RELOAD_POLL: std::time::Duration = std::time::Duration::from_secs(1);

/// Idle time before buffer-changed events go out.
pub const CHANGE_EVENT_DELAY: std::time::Duration = std::time::Duration::from_millis(300);

/// A question waiting for the minibuffer to be free.
pub struct PendingQuestion {
    pub plugin: String,
    pub request: u32,
    pub question: Question,
}

/// A plugin waiting for the user to answer the permission question.
pub struct PendingGrant {
    pub request: PluginRequest,
    /// What it asks for that isn't granted yet.
    pub missing: Grants,
}

fn ensure_host(app: &mut App) -> bool {
    if app.plugin_host.is_none() {
        match PluginHost::with_cache_dir(app.plugin_cache.as_deref()) {
            Ok(h) => {
                app.watch.set_plugin_monitor(h.monitor());
                app.plugin_host = Some(h);
            }
            Err(e) => {
                app.error(format!("Can't start the plugin runtime: {e:#}"));
                return false;
            }
        }
    }
    true
}

/// Plugin files in the plugins directory (~/.config/forge/plugins).
pub fn discover() -> Vec<PathBuf> {
    forge_core::config::plugin_dir()
        .map(|d| forge_core::config::discover_plugins(&d))
        .unwrap_or_default()
}

/// Load a plugin at startup: without starting it if it's unchanged since
/// last time (its commands and keys are there; it starts when needed),
/// else like [`load`].
pub fn load_at_startup(app: &mut App, path: &Path) {
    if !ensure_host(app) {
        return;
    }
    let file = match PluginFile::read(path) {
        Ok(f) => f,
        Err(e) => return report_error(app, e),
    };
    if let Err(e) = check_settings(app, &file.name, file.manifest.as_ref()) {
        return app.error(e);
    }
    let granted = effective_grants(app, &file.name);
    let mut host = app.plugin_host.take().expect("just made");
    let r = host.load_dormant(&mut app.editor, file, &granted);
    app.plugin_host = Some(host);
    if r.is_err() {
        load(app, path);
    }
}

/// Load a declared plugin without starting it: the `on` declaration's
/// `commands` stand-ins are registered now, and it starts the first time
/// one runs, one of its `modes` becomes a buffer's, or a file with one of
/// its `extensions` is opened.
pub fn load_lazy(
    app: &mut App,
    path: &Path,
    commands: &[String],
    modes: &[String],
    extensions: &[String],
) {
    if !ensure_host(app) {
        return;
    }
    let file = match PluginFile::read(path) {
        Ok(f) => f,
        Err(e) => return report_error(app, e),
    };
    if let Err(e) = check_settings(app, &file.name, file.manifest.as_ref()) {
        return app.error(e);
    }
    let granted = effective_grants(app, &file.name);
    let mut host = app.plugin_host.take().expect("just made");
    let r = host.load_lazy(&mut app.editor, file, &granted, commands, modes, extensions);
    app.plugin_host = Some(host);
    if let Err(e) = r {
        report_error(app, e);
    }
}

/// M-x reload-plugin: load it again from the file it came from.
pub fn reload(app: &mut App, name: &str) {
    let path = app
        .plugin_host
        .as_ref()
        .and_then(|h| h.source(name))
        .map(Path::to_path_buf);
    match path {
        Some(p) => load(app, &p),
        None => app.error(format!("Plugin {name} wasn't loaded from a file")),
    }
}

pub fn reloadable_names(app: &App) -> Vec<String> {
    let Some(h) = app.plugin_host.as_ref() else {
        return Vec::new();
    };
    h.names()
        .filter(|n| h.source(n).is_some())
        .map(String::from)
        .collect()
}

/// Something to poll for: plugin files to watch for rebuilds.
pub fn watching_files(app: &App) -> bool {
    app.plugin_auto_reload && app.plugin_host.as_ref().is_some_and(|h| h.watching_files())
}

/// Reload plugins whose files were rebuilt (called when idle).
pub fn reload_changed(app: &mut App) {
    if !app.plugin_auto_reload {
        return;
    }
    let Some(host) = app.plugin_host.as_mut() else {
        return;
    };
    for (name, path) in host.changed_files() {
        log(
            app,
            "forge",
            Level::Info,
            &format!("{} changed: reloading plugin {name}", path.display()),
        );
        load(app, &path);
    }
}

/// Load a plugin file. If it asks for permissions the user hasn't granted
/// yet, show what it wants and ask first (one plugin at a time; others
/// wait their turn).
pub fn load(app: &mut App, path: &Path) {
    if app.pending_grant.is_some() || app.minibuffer.is_some() {
        app.pending_loads.push_back(path.to_path_buf());
        return;
    }
    if !ensure_host(app) {
        return;
    }
    let host = app.plugin_host.as_ref().expect("just made");
    let request = match host.inspect_file(path) {
        Ok(r) => r,
        Err(e) => return report_error(app, e),
    };
    if let Err(e) = check_settings(app, request.name(), request.manifest.as_ref()) {
        return app.error(e);
    }
    let granted = effective_grants(app, request.name());
    let missing = granted.missing(&request.requested());
    if missing.is_empty() {
        return finish_load(app, request, &granted, None);
    }
    show_permissions(app, &request, &missing, &granted);
    let name = request.name().to_string();
    app.pending_grant = Some(PendingGrant { request, missing });
    app.prompt(
        format!("Grant these permissions to {name}? (y or n) "),
        Purpose::PluginGrant,
    );
}

/// The user answered the permission question: Some(true) grant and load,
/// Some(false) load without, None don't load.
pub fn answer_grant(app: &mut App, answer: Option<bool>) {
    let Some(PendingGrant { request, missing }) = app.pending_grant.take() else {
        return;
    };
    let name = request.name().to_string();
    match answer {
        Some(true) => {
            if let Err(e) = app.grants.grant(&name, &missing) {
                app.error(format!("Couldn't save the grant: {e}"));
            }
            let granted = effective_grants(app, &name);
            finish_load(app, request, &granted, None);
        }
        Some(false) => {
            let granted = effective_grants(app, &name);
            finish_load(app, request, &granted, Some(&missing));
        }
        None => app.msg(format!("Plugin {name} not loaded")),
    }
    process_pending_loads(app);
}

/// What a plugin may do: granted interactively (plugin-grants.toml) plus
/// pre-granted in init.rhai (forge.plugin.grant).
pub fn effective_grants(app: &App, name: &str) -> Grants {
    let mut g = app.grants.get(name);
    if let Some(pre) = app
        .rhai
        .as_ref()
        .and_then(|l| l.plugin_grants().remove(name))
    {
        g.merge(&pre);
    }
    g
}

/// Load plugins that waited for a permission question to be answered.
pub fn process_pending_loads(app: &mut App) {
    while app.pending_grant.is_none() && app.minibuffer.is_none() {
        let Some(path) = app.pending_loads.pop_front() else {
            break;
        };
        load(app, &path);
    }
}

fn finish_load(app: &mut App, request: PluginRequest, grants: &Grants, denied: Option<&Grants>) {
    let mut host = app.plugin_host.take().expect("host exists");
    let name = request.name().to_string();
    let (result, out) = host.load_request(&mut app.editor, request, grants);
    app.plugin_host = Some(host);
    apply(app, out);
    app.sync_docs();
    match result {
        Ok(()) => {
            let n = app
                .editor
                .commands
                .names()
                .filter(|c| app.editor.commands.plugin_for(c) == Some(&name))
                .count();
            let s = if n == 1 { "" } else { "s" };
            let without = if denied.is_some() {
                ", without the permissions it asked for"
            } else {
                ""
            };
            if app.message.is_none() || denied.is_some() {
                app.msg(format!("Loaded plugin {name} ({n} command{s}){without}"));
            }
        }
        Err(e) => report_error(app, e),
    }
}

/// Refuse to load a plugin whose `forge.plugin.config` values don't match
/// the settings its manifest declares (an unknown name or the wrong type).
fn check_settings(app: &App, name: &str, manifest: Option<&Manifest>) -> Result<(), String> {
    let Some(m) = manifest else {
        return Ok(());
    };
    let Some(config) = app.editor.plugin_config.get(name) else {
        return Ok(());
    };
    m.check_config(config).map_err(|e| format!("{name}: {e}"))
}

/// M-x describe-plugin: everything known about a loaded plugin — what it
/// declares (its manifest), the commands it registered, its permissions,
/// and its settings with their current values.
pub fn describe(app: &mut App, name: &str) {
    let Some(host) = app.plugin_host.as_ref() else {
        return app.error(format!("No plugin named {name}"));
    };
    let Some(grants) = host
        .get(name)
        .map(|p| p.grants.clone())
        .or_else(|| host.dormant_grants(name).cloned())
    else {
        return app.error(format!("No plugin named {name}"));
    };
    let dormant = host.is_dormant(name);
    let path = host.source(name).map(Path::to_path_buf);
    let manifest = path
        .as_ref()
        .and_then(|p| std::fs::read(p).ok().map(|b| (p.clone(), b)))
        .and_then(|(p, b)| Manifest::for_component(&p, &b).ok().flatten());
    let config: serde_json::Value = app
        .editor
        .plugin_config
        .get(name)
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(serde_json::Value::Null);

    let mut t = format!("{name}\n");
    if let Some(m) = &manifest {
        if let Some(v) = m.version() {
            t.push_str(&format!("  version: {v}\n"));
        }
        if let Some(d) = m.description() {
            t.push_str(&format!("  {d}\n"));
        }
        if let Some(h) = m.homepage() {
            t.push_str(&format!("  home: {h}\n"));
        }
        if let Some(l) = m.license() {
            t.push_str(&format!("  license: {l}\n"));
        }
    }
    if dormant {
        t.push_str("  (not started yet)\n");
    }
    if let Some(p) = &path {
        t.push_str(&format!("  file: {}\n", p.display()));
    }

    let commands: Vec<&str> = app
        .editor
        .commands
        .names()
        .filter(|c| app.editor.commands.plugin_for(c) == Some(name))
        .collect();
    t.push_str("\nCommands:\n");
    if commands.is_empty() {
        t.push_str("  none\n");
    } else {
        for c in &commands {
            let doc = app
                .editor
                .commands
                .get(c)
                .map(|i| i.doc.clone())
                .unwrap_or_default();
            if doc.is_empty() {
                t.push_str(&format!("  {c}\n"));
            } else {
                t.push_str(&format!("  {c}  -- {doc}\n"));
            }
        }
    }

    t.push_str("\nPermissions:\n");
    if grants.describe().is_empty() {
        t.push_str("  none (no files, network or environment)\n");
    } else {
        for line in grants.describe() {
            t.push_str(&format!("  - {line}\n"));
        }
    }

    t.push_str("\nSettings:\n");
    let settings = manifest
        .as_ref()
        .map(|m| m.settings.as_slice())
        .unwrap_or(&[]);
    if settings.is_empty() {
        t.push_str("  none declared\n");
    } else {
        for s in settings {
            let value = config
                .get(&s.name)
                .map(|v| v.to_string())
                .unwrap_or_else(|| match &s.default {
                    Some(d) => format!("{d} (default)"),
                    None => "(unset)".to_string(),
                });
            let default = s
                .default
                .as_ref()
                .map(|d| format!("  default: {d}"))
                .unwrap_or_default();
            t.push_str(&format!(
                "  {} ({}){default}\n    value: {value}\n",
                s.name,
                s.kind.as_str()
            ));
        }
    }
    app.show_special_buffer("*Plugin*", "special-mode", &t);
}

fn report_error(app: &mut App, e: anyhow::Error) {
    let text = format!("{e:#}");
    app.error(text);
}

/// The *Plugin Permissions* buffer: what a plugin asks for and what its
/// code can call.
fn show_permissions(app: &mut App, request: &PluginRequest, missing: &Grants, granted: &Grants) {
    let name = request.name();
    let mut t = format!("Plugin {name} asks for permission\n\n");
    if let Some(m) = &request.manifest {
        let version = m
            .version
            .as_deref()
            .map(|v| format!(" {v}"))
            .unwrap_or_default();
        let desc = m
            .description
            .as_deref()
            .map(|d| format!(": {d}"))
            .unwrap_or_default();
        t.push_str(&format!("{name}{version}{desc}\n\n"));
    }
    t.push_str("It asks for:\n");
    for line in missing.describe() {
        t.push_str(&format!("  - {line}\n"));
    }
    if !granted.is_empty() {
        t.push_str("\nAlready granted:\n");
        for line in granted.describe() {
            t.push_str(&format!("  - {line}\n"));
        }
    }
    t.push_str("\nWhat its code can call (component imports):\n");
    for i in &request.inspection.imports {
        let note = if !is_supported_import(i) {
            "  (not provided by Forge: calls fail)"
        } else if i.starts_with("wasi:filesystem/") {
            "  (only granted directories)"
        } else if i.starts_with("wasi:sockets/") {
            "  (only with network permission)"
        } else {
            ""
        };
        t.push_str(&format!("  {i}{note}\n"));
    }
    t.push_str(
        "\ny: grant and load    n: load without these permissions    C-g: don't load\n\
         Grants are kept in plugin-grants.toml; M-x revoke-plugin-permissions forgets them.\n",
    );
    app.show_special_buffer("*Plugin Permissions*", "special-mode", &t);
}

/// M-: evaluate Rhai in init.rhai's state (made on first use if there is
/// no init.rhai) and show the value.
pub fn eval_rhai(app: &mut App, src: &str) {
    if let Some(v) = eval_rhai_value(app, src)
        && app.message.is_none()
    {
        app.msg(if v.is_empty() { "nil".to_string() } else { v });
    }
}

/// Evaluate Rhai for a client (forgeclient --eval): the value, or the
/// error message the evaluation produced.
pub fn eval_rhai_string(app: &mut App, src: &str) -> Result<String, String> {
    app.message = None;
    match eval_rhai_value(app, src) {
        Some(v) => Ok(if v.is_empty() { "nil".to_string() } else { v }),
        None => Err(app
            .message
            .as_ref()
            .map(|m| m.to_string())
            .unwrap_or_else(|| "evaluation failed".to_string())),
    }
}

/// Evaluate Rhai and return its value, without showing it. Used by M-:
/// and by the keys in *scratch* that print the value into the buffer.
fn eval_rhai_value(app: &mut App, src: &str) -> Option<String> {
    if app.rhai.is_none() {
        match forge_core::RhaiConfig::new() {
            Ok(l) => {
                app.watch.set_rhai_monitor(Some(l.monitor()));
                app.rhai = Some(l);
            }
            Err(e) => {
                app.error(format!("Can't start Rhai: {e}"));
                return None;
            }
        }
    }
    let rhai = app.rhai.take().expect("just made");
    let (value, out) = rhai.eval(&mut app.editor, src);
    app.rhai = Some(rhai);
    app.apply_rhai_settings();
    app.buf().break_undo_group();
    apply(app, out);
    value
}

/// C-x C-e and C-j in *scratch*: evaluate the Rhai sexp before point.
/// With `print`, insert the value after point the way Forge's
/// `eval-print-last-sexp` does; otherwise show it in the echo area.
pub fn eval_rhai_sexp_before(app: &mut App, print: bool) {
    let Some(src) = rhai_sexp_before(app) else {
        app.error("No sexp before point");
        return;
    };
    if print {
        if let Some(v) = eval_rhai_value(app, &src) {
            let text = if v.is_empty() { "nil" } else { &v };
            app.buf().insert_str(&format!("\n{text}"));
            app.buf().break_undo_group();
        }
    } else {
        eval_rhai(app, &src);
    }
}

/// The Rhai expression before point, using the buffer's grammar.
fn rhai_sexp_before(app: &mut App) -> Option<String> {
    let id = app.editor.state.active_buffer?;
    app.editor.sync_language(id);
    let offset = app.buf_ref().point_offset();
    let range = forge_core::treesit::sexp_before(app.buf(), offset)?;
    Some(app.buf_ref().text_offsets(range.start, range.end))
}

/// M-x list-plugins: loaded plugins and what they're allowed.
pub fn list(app: &mut App) {
    let mut t = String::from("Plugins\n\n");
    let Some(host) = app.plugin_host.as_ref().filter(|h| !h.is_empty()) else {
        t.push_str("No plugins are loaded. M-x load-plugin loads one.\n");
        return app.show_special_buffer("*Plugins*", "special-mode", &t);
    };
    for name in host.names() {
        let grants = match host.get(name) {
            Some(p) => p.grants.clone(),
            None => host.dormant_grants(name).cloned().unwrap_or_default(),
        };
        let commands: Vec<&str> = app
            .editor
            .commands
            .names()
            .filter(|c| app.editor.commands.plugin_for(c) == Some(name))
            .collect();
        t.push_str(&format!("{name}\n"));
        if host.is_dormant(name) {
            t.push_str("  not started yet: starts when one of its commands runs or an event it wants happens\n");
        }
        if let Some(path) = host.source(name) {
            t.push_str(&format!("  file: {}\n", path.display()));
        }
        t.push_str(&format!(
            "  commands: {}\n",
            if commands.is_empty() {
                "none".into()
            } else {
                commands.join(", ")
            }
        ));
        if grants.describe().is_empty() {
            t.push_str("  permissions: none (no files, network or environment)\n");
        }
        let pre = app
            .rhai
            .as_ref()
            .and_then(|l| l.plugin_grants().remove(name))
            .unwrap_or_default();
        for line in pre.describe() {
            t.push_str(&format!("  {line}  [init.rhai]\n"));
        }
        for line in app.grants.get(name).describe() {
            t.push_str(&format!("  {line}  [granted when asked]\n"));
        }
        t.push_str(&format!("  memory limit: {} MB\n", grants.memory_mb()));
        if let Some(p) = host.get(name).filter(|p| !p.unsupported.is_empty()) {
            t.push_str(&format!(
                "  imports Forge doesn't provide: {}\n",
                p.unsupported.join(", ")
            ));
        }
        t.push('\n');
    }
    app.show_special_buffer("*Plugins*", "special-mode", &t);
}

/// Forget what was granted: a plugin's permissions (by its name), or a
/// project's (by `project:<directory>`). Either way it asks again.
pub fn revoke(app: &mut App, name: &str) {
    match app.grants.revoke(name) {
        Ok(true) => app.msg(if name.starts_with("project:") {
            format!("Forgot {name}'s grant; Forge will ask again when a file in it is opened")
        } else {
            format!(
                "Forgot {name}'s permissions; it will ask again when loaded (reload it to apply)"
            )
        }),
        Ok(false) => app.msg(format!("{name} had nothing granted")),
        Err(e) => app.error(e),
    }
}

/// Everything in the grants file that could be revoked: plugin names, and
/// projects by their key.
pub fn granted_names(app: &App) -> Vec<String> {
    let mut names: Vec<String> = app.grants.all().keys().cloned().collect();
    names.extend(
        app.grants
            .projects()
            .keys()
            .map(|root| forge_core::permissions::project_key(std::path::Path::new(root))),
    );
    names.sort();
    names
}

pub fn unload(app: &mut App, name: &str) {
    let Some(mut host) = app.plugin_host.take() else {
        return app.error("No plugins are loaded");
    };
    let done = host.unload(&mut app.editor, name);
    app.plugin_host = Some(host);
    app.sync_docs();
    if done {
        app.msg(format!("Unloaded plugin {name}"));
    } else {
        app.error(format!("No plugin named {name}"));
    }
}

pub fn loaded_names(app: &App) -> Vec<String> {
    app.plugin_host
        .as_ref()
        .map(|h| h.names().map(String::from).collect())
        .unwrap_or_default()
}

/// Run a plugin's command. Returns false if `name` isn't one.
pub fn run_command(app: &mut App, name: &str) -> bool {
    if app.editor.commands.plugin_for(name).is_none() {
        return false;
    }
    if app.plugin_depth >= MAX_DEPTH {
        app.error(format!(
            "{name}: plugin commands are running each other too deeply"
        ));
        return true;
    }
    let prefix = app.prefix_arg.map(|a| a.value());
    let Some(mut host) = app.plugin_host.take() else {
        return false;
    };
    let out = host.run_command(&mut app.editor, name, prefix);
    app.plugin_host = Some(host);
    app.plugin_depth += 1;
    apply(app, out);
    app.plugin_depth -= 1;
    true
}

/// Run a command defined in init.rhai. Returns false if `name` isn't one.
pub fn run_rhai_command(app: &mut App, name: &str) -> bool {
    if !app.editor.commands.is_config(name) {
        return false;
    }
    if app.plugin_depth >= MAX_DEPTH {
        app.error(format!(
            "{name}: commands are running each other too deeply"
        ));
        return true;
    }
    let prefix = app.prefix_arg.map(|a| a.value());
    let Some(rhai) = app.rhai.take() else {
        return false;
    };
    let out = rhai.run_command(&mut app.editor, name, prefix);
    app.rhai = Some(rhai);
    app.plugin_depth += 1;
    apply(app, out);
    app.plugin_depth -= 1;
    true
}

/// The user answered a plugin's (or init.rhai's) question (None: cancelled).
pub fn answer(app: &mut App, plugin: &str, request: u32, answer: Option<&str>) {
    if plugin == forge_core::rhai::SOURCE {
        let Some(rhai) = app.rhai.take() else { return };
        let out = rhai.answer(&mut app.editor, request, answer);
        app.rhai = Some(rhai);
        app.buf().break_undo_group();
        return apply(app, out);
    }
    let Some(mut h) = app.plugin_host.take() else {
        return;
    };
    let out = h.answer(&mut app.editor, plugin, request, answer);
    app.plugin_host = Some(h);
    app.buf().break_undo_group();
    apply(app, out);
}

/// The user answered a multiple-value question (`forge.ui.select` with
/// multiple): the callback gets the list. A WASM plugin can't receive a
/// list, so it is answered through the ordinary `on-answer` channel with
/// the fields joined by `crm-separator` commas.
pub fn answer_list(app: &mut App, plugin: &str, request: u32, items: Option<&[String]>) {
    if plugin == forge_core::rhai::SOURCE {
        let Some(rhai) = app.rhai.take() else { return };
        let out = rhai.answer_list(&mut app.editor, request, items);
        app.rhai = Some(rhai);
        app.buf().break_undo_group();
        return apply(app, out);
    }
    let joined = items.map(|fields| fields.join(","));
    answer(app, plugin, request, joined.as_deref());
}

/// A program init.rhai ran finished: call its callback with
/// `{ code, stdout, stderr }`. Nothing answers when init.rhai is gone
/// (reloaded while it ran): the callback went with it.
pub fn answer_rhai_process(
    app: &mut App,
    request: u32,
    code: Option<i32>,
    stdout: String,
    stderr: String,
) {
    let Some(rhai) = app.rhai.take() else { return };
    let out = rhai.answer_process(&mut app.editor, request, code, stdout, stderr);
    app.rhai = Some(rhai);
    apply(app, out);
}

/// A file operation finished: call its callback. Nothing answers when
/// init.rhai is gone.
pub fn answer_rhai_fs(app: &mut App, request: u32, answer: forge_core::process::FsAnswer) {
    let Some(rhai) = app.rhai.take() else { return };
    let out = rhai.answer_fs(&mut app.editor, request, answer);
    app.rhai = Some(rhai);
    apply(app, out);
}

/// Hand init.rhai the answers to the file calls it made (idle time).
pub fn poll_rhai_fs(app: &mut App) {
    if !app.rhai_fs.any_running() {
        return;
    }
    for (request, answer) in app.rhai_fs.poll() {
        answer_rhai_fs(app, request, answer);
    }
}

/// Start a web request init.rhai asked for (`forge.http`). The request runs
/// on its own thread; `poll_rhai_http` answers the callback when it lands.
fn start_rhai_http(
    app: &mut App,
    request: u32,
    method: &str,
    url: &str,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
) {
    let id = app.rhai_http.start(method, url, headers, body);
    app.rhai_http_ids.insert(id, request);
}

/// A web request init.rhai made finished: call its callback with
/// `({ status, headers, body }, nil)` or `(nil, err)`. Nothing answers
/// when init.rhai is gone (reloaded while it ran).
pub fn answer_rhai_http(app: &mut App, request: u32, answer: forge_core::http::Answer) {
    let Some(rhai) = app.rhai.take() else { return };
    let out = rhai.answer_http(&mut app.editor, request, answer);
    app.rhai = Some(rhai);
    apply(app, out);
}

/// Hand init.rhai the answers to the web requests it made (idle time).
pub fn poll_rhai_http(app: &mut App) {
    if !app.rhai_http.any_running() {
        return;
    }
    for ev in app.rhai_http.poll() {
        let Some(request) = app.rhai_http_ids.remove(&ev.id) else {
            continue;
        };
        answer_rhai_http(app, request, ev.answer);
    }
}

/// After each key: tell plugins what changed, and ask any waiting question.
pub fn after_key(app: &mut App) {
    notice(app, false);
    show_next_question(app);
    process_pending_loads(app);
}

/// Deliver events (`idle`: include buffer-changed) to plugins and to
/// init.rhai's forge.on handlers.
pub fn notice(app: &mut App, idle: bool) {
    if let Some(rhai) = app.rhai.take() {
        let out = rhai.notice_changes(&mut app.editor, idle);
        app.rhai = Some(rhai);
        if !out.logs.is_empty() || !out.effects.is_empty() || !out.errors.is_empty() {
            app.buf().break_undo_group();
        }
        apply(app, out);
    }
    let Some(mut h) = app.plugin_host.take() else {
        return;
    };
    if h.is_empty() {
        app.plugin_host = Some(h);
        return;
    }
    let out = h.notice_changes(&mut app.editor, idle);
    app.plugin_host = Some(h);
    if !out.logs.is_empty() || !out.effects.is_empty() || !out.errors.is_empty() {
        app.buf().break_undo_group();
    }
    apply(app, out);
}

/// Hand plugins the output of the programs they run, and merge in
/// finished background calls (idle time).
pub fn poll_processes(app: &mut App) {
    let Some(mut h) = app.plugin_host.take() else {
        return;
    };
    if !h.processes_running() && !h.background_running() && !h.http_running() {
        app.plugin_host = Some(h);
        return;
    }
    let mut out = h.poll_background(&mut app.editor);
    for more in [
        h.poll_processes(&mut app.editor),
        h.poll_http(&mut app.editor),
    ] {
        out.effects.extend(more.effects);
        out.logs.extend(more.logs);
        out.errors.extend(more.errors);
        out.unloaded.extend(more.unloaded);
    }
    app.plugin_host = Some(h);
    apply(app, out);
}

/// Plugins have processes running or are busy in the background.
pub fn processes_running(app: &App) -> bool {
    app.plugin_host
        .as_ref()
        .is_some_and(|h| h.processes_running() || h.background_running() || h.http_running())
}

pub fn changes_pending(app: &App) -> bool {
    app.plugin_host
        .as_ref()
        .is_some_and(|h| h.changes_pending())
        || app.rhai.as_ref().is_some_and(|l| l.changes_pending())
}

/// Carry out what plugin calls left behind.
pub fn apply(app: &mut App, out: Outcome) {
    let had_unloads = !out.unloaded.is_empty();
    for (plugin, line) in out.logs {
        log(app, &plugin, line.level, &line.message);
        // Warnings and errors also show in the echo area;
        // info and debug lines are recorded but not echoed.
        if matches!(line.level, Level::Warn | Level::Error) {
            app.echo(format!("{plugin}: {}", line.message));
        }
    }
    for e in out.errors {
        app.error(e);
    }
    for effect in out.effects {
        match effect {
            Effect::Message(text) => {
                app.msg(text);
            }
            Effect::BrowseUrl(url) => crate::webkit::browse_url(app, &url),
            Effect::Kill(text) => app.kill_ring_save(text),
            Effect::LoadRhai(path) => {
                let path = forge_core::permissions::expand_home(&path);
                if let Some(rhai) = app.rhai.take() {
                    let out = rhai.load_file(&mut app.editor, &path);
                    app.rhai = Some(rhai);
                    apply(app, out);
                    app.apply_rhai_settings();
                }
            }
            Effect::Popup(items) => {
                let (x, y) = app.pointer;
                let items = items
                    .into_iter()
                    .map(|(l, c)| forge_core::menu::Item::new(l, c))
                    .collect();
                crate::mode_line::popup(app, items, x, y);
            }
            Effect::OpenDir(dir) => crate::dired::open(app, std::path::Path::new(&dir)),
            Effect::RunCommand(name) => app.run_command(&name),
            Effect::RunCommandWithArg(name, n) => {
                app.forced_arg = Some(crate::app::PrefixArg::Number(n));
                app.run_command(&name);
            }
            Effect::RunCommandLater(name, arg) => app.deferred_commands.push((name, arg)),
            Effect::ExecuteKeys(keys) => app.execute_keys(&keys),
            Effect::Completions {
                request,
                start,
                candidates,
                ..
            } => crate::capf::delivered(app, request, start, candidates),
            Effect::Ask {
                plugin,
                request,
                question,
            } => {
                app.pending_questions.push_back(PendingQuestion {
                    plugin,
                    request,
                    question,
                });
            }
            Effect::Process {
                request,
                program,
                dir,
                stdin,
                timeout,
                env,
            } => {
                crate::compile::start_lua(app, request, program, dir, stdin, timeout, env);
            }
            Effect::Fs { request, op } => {
                app.rhai_fs.start(request, op);
            }
            Effect::Http {
                request,
                method,
                url,
                headers,
                body,
            } => {
                start_rhai_http(app, request, &method, &url, headers, body);
            }
            Effect::Locate => locate(app),
            Effect::SetPromptInput(text) => {
                let input = app.minibuffer.as_mut().map(|mb| {
                    mb.set_input(text);
                    mb.current_input()
                });
                if let Some(input) = input {
                    app.editor.prompt_input = input;
                }
            }
            Effect::SuspendPrompt => crate::minibuffer::suspend(app),
            Effect::ResumeSession(index) => crate::minibuffer::resume(app, index as usize),
            Effect::JumpToRow { row, accept } => crate::minibuffer::jump_to_row(app, row, accept),
        }
    }
    if had_unloads {
        app.sync_docs();
    }
    show_next_question(app);
}

/// Ask the completion sources where the selected candidate is defined
/// (corfu-info-location). A source answers in its own time.
pub fn locate(app: &mut App) {
    let Some(mut host) = app.plugin_host.take() else {
        return;
    };
    let out = host.locate(&mut app.editor);
    app.plugin_host = Some(host);
    apply(app, out);
}

/// Open the minibuffer for the oldest waiting question, if it's free. A
/// completing-read that asks to go `over` a prompt already up (a plugin
/// offering that prompt's history) suspends it and takes its place.
pub fn show_next_question(app: &mut App) {
    let busy = app.minibuffer.is_some()
        || app.query.is_some()
        || app.key_reader.is_some()
        || app.plugin_key.is_some();
    // Only a prompt can be taken over, and only by an over question.
    let over = app.minibuffer.is_some()
        && app.query.is_none()
        && app.key_reader.is_none()
        && app.plugin_key.is_none()
        && matches!(
            app.pending_questions.front(),
            Some(q) if matches!(q.question, Question::Completing { over: true, .. })
        );
    if busy && !over {
        return;
    }
    let Some(q) = app.pending_questions.pop_front() else {
        return;
    };
    if over && let Some(prev) = app.minibuffer.take() {
        app.suspended_prompt = Some(prev);
    }
    match q.question {
        Question::Text { prompt, initial } => {
            app.prompt(
                prompt,
                Purpose::PluginAsk {
                    plugin: q.plugin,
                    request: q.request,
                    candidates: None,
                    require_match: false,
                    multiple: false,
                    over: false,
                },
            );
            if let Some(mb) = app.minibuffer.as_mut() {
                mb.input = initial;
            }
        }
        Question::Completing {
            prompt,
            candidates,
            require_match,
            multiple,
            over,
        } => {
            let purpose = Purpose::PluginAsk {
                plugin: q.plugin,
                request: q.request,
                candidates: Some(candidates),
                require_match,
                multiple,
                over,
            };
            app.prompt(prompt, purpose);
            // With a vertical list up, build it now so the first
            // candidate is selected and published: a preview command
            // (consult) sees it before the first key in the prompt.
            if app.editor.completion.vertical_lines().is_some() {
                if let Some(mut mb) = app.minibuffer.take() {
                    crate::minibuffer::refresh(app, &mut mb);
                    app.minibuffer = Some(mb);
                }
                crate::capf::publish_selection(app);
            }
        }
        Question::YOrN { prompt } => {
            app.prompt(
                format!("{prompt} (y or n) "),
                Purpose::PluginYOrN {
                    plugin: q.plugin,
                    request: q.request,
                },
            );
        }
        Question::Key { prompt } => {
            if !prompt.is_empty() {
                app.msg(prompt);
            }
            app.plugin_key = Some((q.plugin, q.request));
        }
    }
}

pub use crate::messages::{log, messages_buffer};
