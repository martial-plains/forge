// SPDX-License-Identifier: MIT OR Apache-2.0
//! The terminal's side of plugins: running their commands, asking their
//! questions in the minibuffer, showing their messages, telling them
//! about events. The plugins themselves live in `forge_core::PluginHost`.

use crate::app::App;
use crate::minibuffer::Purpose;
use crate::package_ui::{self, Status, Symbols, Text};
use forge_core::decoration::PackageFace;
use forge_core::plugin::runtime::Level;
use forge_core::plugin::runtime::is_supported_import;
use forge_core::{
    Effect, GrantBinding, Grants, Manifest, Outcome, PluginFile, PluginHost, PluginRequest,
    Question,
};
use std::path::{Path, PathBuf};

/// How deep plugin commands may run each other (commands.run) before
/// Forge assumes a loop.
const MAX_DEPTH: usize = 16;

/// The permission review's mode: RET opens or closes the import list.
pub const PERMISSIONS_MODE: &str = "plugin-permissions-mode";
/// The plugin list's mode: RET expands, j/k move, r revokes.
pub const LIST_MODE: &str = "plugin-list-mode";
/// How many imports a folded review shows.
const FOLDED_IMPORTS: usize = 4;

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

/// What the plugin permission and list buffers are showing: which import
/// lists are expanded and which list row is selected.
#[derive(Debug, Clone, Default)]
pub struct PluginUi {
    /// The permissions or import-mismatch buffer is showing every import.
    pub imports_expanded: bool,
    /// The selected row in *Plugins*.
    pub list_selected: usize,
    /// *Plugins* has been shown at least once.
    pub list_initialized: bool,
    /// Rows whose details are open in *Plugins*.
    pub list_expanded: std::collections::BTreeSet<String>,
    /// The import mismatch that is on screen. `PluginRequest` holds a
    /// compiled component and cannot be kept here.
    pub mismatch: Option<MismatchView>,
}

impl PluginUi {
    /// Start a new permission or mismatch screen with its imports folded.
    pub fn begin_review(&mut self, mismatch: Option<MismatchView>) {
        self.imports_expanded = false;
        self.mismatch = mismatch;
    }
}

/// The text needed to redraw an import-mismatch buffer after RET.
#[derive(Debug, Clone)]
pub struct MismatchView {
    pub name: String,
    pub mismatches: Vec<forge_core::ImportMismatch>,
    pub imports: Vec<String>,
}

/// A plugin waiting for the user to answer the permission question.
pub struct PendingGrant {
    pub request: PluginRequest,
    /// What it asks for that isn't granted yet.
    pub missing: Grants,
    /// The binary the answer is being given for.
    pub binding: GrantBinding,
    /// The name was approved for a different binary, so it is being asked
    /// again even when it would otherwise have enough permissions.
    pub different_binary: bool,
}

pub(crate) fn ensure_host(app: &mut App) -> bool {
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

/// Start the plugin-startup stopwatch, if it can be started.
pub fn begin_startup_profile(app: &mut App) -> bool {
    if !ensure_host(app) {
        return false;
    }
    if let Some(host) = app.plugin_host.as_ref() {
        host.enable_profiling();
        host.begin_startup_profile();
    }
    true
}

/// Stop the plugin-startup stopwatch and warn if it went over the budget.
pub fn finish_startup_profile(app: &mut App) {
    let Some(host) = app.plugin_host.as_ref() else {
        return;
    };
    host.finish_startup_profile();
    let Some(total) = host.startup_profile_total() else {
        return;
    };
    let Some(budget) = app.startup_budget else {
        return;
    };
    if total <= budget {
        return;
    }
    app.startup_budget_exceeded = true;
    let total_ms = total.as_secs_f64() * 1000.0;
    let budget_ms = budget.as_secs_f64() * 1000.0;
    crate::messages::log(
        app,
        "startup",
        Level::Warn,
        &format!(
            "Plugin startup took {total_ms:.3} ms, over the {budget_ms:.3} ms startup_budget_ms"
        ),
    );
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
    let binding = file.binding();
    if app.grants.binding_mismatch(&file.name, &binding) {
        // Ask again, with the explanation, rather than starting from a
        // stale approval for another binary.
        return load(app, path);
    }
    bind_legacy(app, &file.name, &binding);
    let granted = effective_grants_for(app, &file.name, &binding);
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
    let binding = file.binding();
    if app.grants.binding_mismatch(&file.name, &binding) {
        return load(app, path);
    }
    bind_legacy(app, &file.name, &binding);
    let granted = effective_grants_for(app, &file.name, &binding);
    let mut host = app.plugin_host.take().expect("just made");
    let r = host.load_lazy(&mut app.editor, file, &granted, commands, modes, extensions);
    app.plugin_host = Some(host);
    if let Err(e) = r {
        report_error(app, e);
    }
}

/// Load a plugin during a bisect without asking permission questions: it
/// was loaded before, so keep the grants it already had. `dormant` puts it
/// back in the lazy state it was in when the bisect started.
pub fn reload_for_bisect(
    app: &mut App,
    path: &Path,
    dormant: bool,
    lazy: Option<(&[String], &[String], &[String])>,
) -> Result<(), String> {
    if !ensure_host(app) {
        return Err("can't start the plugin runtime".into());
    }
    let file = PluginFile::read(path).map_err(|error| format!("{error:#}"))?;
    check_settings(app, &file.name, file.manifest.as_ref())?;
    let binding = file.binding();
    bind_legacy(app, &file.name, &binding);
    let granted = effective_grants_for(app, &file.name, &binding);
    let mut host = app.plugin_host.take().expect("just made");
    let mut out = forge_core::Outcome::default();
    let result = if dormant {
        if let Some((commands, modes, extensions)) = lazy {
            host.load_lazy(&mut app.editor, file, &granted, commands, modes, extensions)
        } else {
            match host.load_dormant(&mut app.editor, file, &granted) {
                Ok(()) => Ok(()),
                Err(file) => {
                    let request = host
                        .inspect_file(&file.path)
                        .map_err(|error| format!("{error:#}"))?;
                    let (result, more) = host.load_request(&mut app.editor, request, &granted);
                    out = more;
                    result
                }
            }
        }
    } else {
        let request = host
            .inspect_file(path)
            .map_err(|error| format!("{error:#}"))?;
        let (result, more) = host.load_request(&mut app.editor, request, &granted);
        out = more;
        result
    };
    app.plugin_host = Some(host);
    apply(app, out);
    app.sync_docs();
    result.map_err(|error| format!("{error:#}"))
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
    let mismatches =
        forge_core::uncovered_imports(request.manifest.as_ref(), &request.inspection.used_imports);
    if !mismatches.is_empty() {
        show_import_mismatch(app, &request, &mismatches);
        let details = mismatches
            .iter()
            .map(forge_core::ImportMismatch::describe)
            .collect::<Vec<_>>()
            .join("; ");
        return app.error(format!(
            "Plugin {} not loaded: its manifest doesn't declare permissions its imports need: {details}",
            request.name()
        ));
    }
    let name = request.name().to_string();
    // A request read from bytes alone has no binary identity; one from a
    // file always does.
    let binding = request.binding().unwrap_or_default();
    let different_binary = app.grants.binding_mismatch(&name, &binding);
    if different_binary {
        // The old approval belongs to another binary. Forget it before
        // asking, so a declined prompt can't leave it usable.
        if let Err(e) = app.grants.revoke(&name) {
            app.error(format!("Couldn't invalidate the old grant: {e}"));
        }
    } else {
        bind_legacy(app, &name, &binding);
    }
    let granted = effective_grants_for(app, &name, &binding);
    let missing = granted.missing(&request.requested());
    if missing.is_empty() {
        return finish_load(app, request, &granted, None);
    }
    if app.unattended {
        if app.packages.approvals.grants {
            let saved = if binding.is_empty() {
                app.grants.grant(&name, &missing)
            } else {
                app.grants.grant_for(&name, &missing, &binding)
            };
            if let Err(e) = saved {
                app.error(format!("Couldn't save the grant: {e}"));
            }
            let granted = effective(app, &name, &binding);
            finish_load(app, request, &granted, None);
            return process_pending_loads(app);
        }
        let error = format!(
            "Plugin {name} not loaded: it needs permissions {}. {}",
            missing.describe().join("; "),
            crate::packages::Approvals::grants_hint()
        );
        app.packages.approval_blocked = Some(error.clone());
        return app.error(error);
    }
    show_permissions(app, &request, &missing, &granted, different_binary);
    app.pending_grant = Some(PendingGrant {
        request,
        missing,
        binding,
        different_binary,
    });
    let prompt = if different_binary {
        format!("same name, different binary. Grant these permissions to {name}? (y or n) ")
    } else {
        format!("Grant these permissions to {name}? (y or n) ")
    };
    app.prompt(prompt, Purpose::PluginGrant);
}

/// The user answered the permission question: Some(true) grant and load,
/// Some(false) load without, None don't load.
pub fn answer_grant(app: &mut App, answer: Option<bool>) {
    let Some(PendingGrant {
        request,
        missing,
        binding,
        ..
    }) = app.pending_grant.take()
    else {
        return;
    };
    let name = request.name().to_string();
    match answer {
        Some(true) => {
            let saved = if binding.is_empty() {
                app.grants.grant(&name, &missing)
            } else {
                app.grants.grant_for(&name, &missing, &binding)
            };
            if let Err(e) = saved {
                app.error(format!("Couldn't save the grant: {e}"));
            }
            let granted = effective(app, &name, &binding);
            finish_load(app, request, &granted, None);
        }
        Some(false) => {
            let granted = effective(app, &name, &binding);
            finish_load(app, request, &granted, Some(&missing));
        }
        None => app.msg(format!("Plugin {name} not loaded")),
    }
    process_pending_loads(app);
}

/// Remember the binary that first used a migrated, unbound approval.
fn bind_legacy(app: &mut App, name: &str, binding: &GrantBinding) {
    if binding.is_empty() {
        return;
    }
    if let Err(e) = app.grants.bind_if_unbound(name, binding) {
        app.error(format!("Couldn't save the grant: {e}"));
    }
}

fn effective(app: &App, name: &str, binding: &GrantBinding) -> Grants {
    if binding.is_empty() {
        effective_grants(app, name)
    } else {
        effective_grants_for(app, name, binding)
    }
}

/// What a plugin may do: granted interactively (plugin-grants.toml) plus
/// pre-granted in init.rhai (forge.plugin.grant).
pub fn effective_grants(app: &App, name: &str) -> Grants {
    let mut g = app.grants.get(name);
    merge_pre_grants(app, name, &mut g);
    g
}

/// What a particular binary of a plugin may do. A migrated, unbound
/// approval still applies; `forge.plugin.grant` always applies.
pub fn effective_grants_for(app: &App, name: &str, binding: &GrantBinding) -> Grants {
    let mut g = app.grants.get_for(name, binding);
    merge_pre_grants(app, name, &mut g);
    g
}

fn merge_pre_grants(app: &App, name: &str, g: &mut Grants) {
    if let Some(pre) = app
        .rhai
        .as_ref()
        .and_then(|l| l.plugin_grants().remove(name))
    {
        g.merge(&pre);
    }
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
    let commands: Vec<&str> = app
        .editor
        .commands
        .names()
        .filter(|c| app.editor.commands.plugin_for(c) == Some(name))
        .collect();

    let unsupported = host
        .get(name)
        .map(|p| p.unsupported.clone())
        .unwrap_or_default();
    let outer = app.win().width.max(20);
    let width = package_ui::inner(outer);
    let symbols = Symbols::for_app(app);
    let version = manifest
        .as_ref()
        .and_then(|m| m.version())
        .map(|v| format!("{name} {v}"))
        .unwrap_or_else(|| name.to_string());
    let mut text = Text::for_width(outer);
    package_ui::title(&mut text, "Plugin", &version, width, symbols);
    package_ui::rule(&mut text, width, symbols);
    text.push_face(name, PackageFace::Heading);
    if dormant {
        text.push(" ");
        text.push_face("not started", PackageFace::BadgeInactive);
    }
    text.newline();
    if let Some(m) = &manifest {
        if let Some(d) = m.description() {
            text.push_face(d, PackageFace::Inactive);
            text.newline();
        }
        for (label, value) in [("home", m.homepage()), ("license", m.license())] {
            if let Some(value) = value {
                text.push_face(&format!("{label}: "), PackageFace::Heading);
                text.push(value);
                text.newline();
            }
        }
    }
    if let Some(p) = &path {
        text.push_face("file: ", PackageFace::Heading);
        text.push(&p.display().to_string());
        text.newline();
    }

    text.newline();
    package_ui::heading(
        &mut text,
        &format!("Commands ({})", commands.len()),
        width,
        symbols,
    );
    text.newline();
    if commands.is_empty() {
        text.push_face("  none", PackageFace::Inactive);
        text.newline();
    } else {
        for c in &commands {
            let doc = app
                .editor
                .commands
                .get(c)
                .map(|i| i.doc.clone())
                .unwrap_or_default();
            text.push("  ");
            text.push_face(c, PackageFace::Primary);
            if !doc.is_empty() {
                text.push("  ");
                text.push_face(
                    &package_ui::fit(&doc, width.saturating_sub(4 + c.len()), symbols.ellipsis),
                    PackageFace::Inactive,
                );
            }
            text.newline();
        }
    }

    text.newline();
    package_ui::heading(&mut text, "Permissions", width, symbols);
    text.newline();
    if grants.describe().is_empty() {
        text.push_face(
            "  none (no files, network or environment)",
            PackageFace::Inactive,
        );
        text.newline();
    } else {
        for (key, line) in grants.describe_keyed() {
            text.push_face(&format!("  {} ", symbols.check), PackageFace::Ok);
            text.push(&line);
            if let Some(time) = app.grants.grant_time(name, &key) {
                text.push_face(
                    &format!("  (last changed {})", forge_core::format_grant_time(time)),
                    PackageFace::Inactive,
                );
            }
            text.newline();
        }
    }
    if !unsupported.is_empty() {
        text.newline();
        text.push_face("Imports Forge doesn't provide", PackageFace::Heading);
        text.newline();
        for import in &unsupported {
            text.push_face(&format!("  {} ", symbols.cross), PackageFace::Error);
            text.push(import);
            text.newline();
        }
    }

    text.newline();
    package_ui::heading(&mut text, "Settings", width, symbols);
    text.newline();
    let settings = manifest
        .as_ref()
        .map(|m| m.settings.as_slice())
        .unwrap_or(&[]);
    if settings.is_empty() {
        text.push_face("  none declared", PackageFace::Inactive);
        text.newline();
    } else {
        for s in settings {
            let value = config
                .get(&s.name)
                .map(|v| v.to_string())
                .unwrap_or_else(|| match &s.default {
                    Some(d) => format!("{d} (default)"),
                    None => "(unset)".to_string(),
                });
            text.push_face(&format!("  {} ", s.name), PackageFace::Primary);
            text.push_face(&format!("({}) ", s.kind.as_str()), PackageFace::Inactive);
            text.push(&format!("value: {value}"));
            if let Some(default) = &s.default {
                text.push_face(&format!("  default: {default}"), PackageFace::Inactive);
            }
            text.newline();
        }
    }
    text.show(app, "*Plugin*", "special-mode", None);
}

fn report_error(app: &mut App, e: anyhow::Error) {
    let text = format!("{e:#}");
    app.error(text);
}

/// A plugin whose imports need a permission its manifest doesn't declare:
/// show the mismatch instead of treating the import as an implicit grant.
fn show_import_mismatch(
    app: &mut App,
    request: &PluginRequest,
    mismatches: &[forge_core::ImportMismatch],
) {
    let view = MismatchView {
        name: request.name().to_string(),
        mismatches: mismatches.to_vec(),
        imports: request.inspection.imports.clone(),
    };
    app.plugin_ui.begin_review(Some(view.clone()));
    show_mismatch(app, &view);
}

fn show_mismatch(app: &mut App, view: &MismatchView) {
    let outer = app.win().width.max(20);
    let width = package_ui::inner(outer);
    let symbols = Symbols::for_app(app);
    let mut text = Text::for_width(outer);
    package_ui::title(&mut text, "Import Mismatch", &view.name, width, symbols);
    package_ui::rule(&mut text, width, symbols);
    text.push_face(
        &format!("{} can't load: a declared permission is missing", view.name),
        PackageFace::Heading,
    );
    text.newline();
    text.newline();
    package_ui::band(
        &mut text,
        Status::Error,
        "the manifest doesn't declare every permission its imports need",
        width,
        symbols,
    );
    text.newline();
    text.push_face("Mismatched imports", PackageFace::Heading);
    text.newline();
    for mismatch in &view.mismatches {
        text.push_face(&format!("{} ", symbols.cross), PackageFace::Error);
        let needs = format!("needs {}, not declared", mismatch.permission.describe());
        text.push_face(&mismatch.import, PackageFace::Primary);
        text.push("  ");
        text.push_face(&needs, PackageFace::Error);
        text.newline();
    }
    text.newline();
    text.push_face("What its code can call", PackageFace::Heading);
    text.push_face(&format!(" ({})", view.imports.len()), PackageFace::Heading);
    text.newline();
    push_imports(
        &mut text,
        &view.imports,
        Some(&view.mismatches),
        app.plugin_ui.imports_expanded,
        width,
        symbols,
    );
    text.newline();
    text.push_face(
        "Add the missing permissions to the plugin's manifest, then load it again.",
        PackageFace::Inactive,
    );
    text.newline();
    text.show(app, "*Plugin Permissions*", PERMISSIONS_MODE, None);
}

/// The *Plugin Permissions* buffer: what a plugin asks for and what its
/// code can call.
fn show_permissions(
    app: &mut App,
    request: &PluginRequest,
    missing: &Grants,
    granted: &Grants,
    different_binary: bool,
) {
    app.plugin_ui.begin_review(None);
    let version = request
        .manifest
        .as_ref()
        .and_then(|m| m.version())
        .map(str::to_string);
    let view = PermissionView {
        name: request.name(),
        version: version.as_deref(),
        imports: &request.inspection.imports,
        missing,
        granted,
        different_binary,
    };
    draw_permissions(app, &view);
}

struct PermissionView<'a> {
    name: &'a str,
    version: Option<&'a str>,
    imports: &'a [String],
    missing: &'a Grants,
    granted: &'a Grants,
    different_binary: bool,
}

fn draw_permissions(app: &mut App, view: &PermissionView<'_>) {
    let name = view.name;
    let outer = app.win().width.max(20);
    let width = package_ui::inner(outer);
    let symbols = Symbols::for_app(app);
    let version = view
        .version
        .map(|v| format!("{name} {v}"))
        .unwrap_or_else(|| name.to_string());
    let mut text = Text::for_width(outer);
    package_ui::title(&mut text, "Plugin Permissions", &version, width, symbols);
    package_ui::rule(&mut text, width, symbols);
    text.push_face(&format!("{name} asks for permission"), PackageFace::Heading);
    text.newline();
    text.newline();
    if view.different_binary {
        package_ui::band_lines(
            &mut text,
            Status::Warning,
            &[
                "same name, different binary",
                "The saved approval was for a different build, so it was forgotten.",
            ],
            width,
            symbols,
        );
        text.newline();
    }

    let asks = permission_rows(view.missing);
    let have = permission_rows(view.granted);
    let name_w = permission_name_width(asks.iter().chain(&have));
    text.push_face(
        &format!("Asks for ({} new)", asks.len()),
        PackageFace::Heading,
    );
    text.newline();
    for row in &asks {
        push_permission_row(
            &mut text,
            row,
            "+",
            PackageFace::Warning,
            name_w,
            width,
            symbols,
        );
    }

    if !view.granted.is_empty() {
        text.newline();
        text.push_face(
            &format!("Already granted ({})", have.len()),
            PackageFace::Heading,
        );
        text.newline();
        for row in &have {
            push_permission_row(
                &mut text,
                row,
                symbols.check,
                PackageFace::Ok,
                name_w,
                width,
                symbols,
            );
        }
    }

    text.newline();
    text.push_face(
        &format!("What its code can call ({} imports)", view.imports.len()),
        PackageFace::Heading,
    );
    text.newline();
    push_imports(
        &mut text,
        view.imports,
        None,
        app.plugin_ui.imports_expanded,
        width,
        symbols,
    );

    text.newline();
    text.push_face(
        &package_ui::fit(
            "y grant and load   n load without these   C-g don't load   ? help",
            width,
            symbols.ellipsis,
        ),
        PackageFace::Inactive,
    );
    text.newline();
    text.show(app, "*Plugin Permissions*", PERMISSIONS_MODE, None);
}

#[derive(Debug)]
struct PermissionRow {
    name: &'static str,
    description: String,
    risk: bool,
}

fn permission_rows(grants: &Grants) -> Vec<PermissionRow> {
    let mut rows = Vec::new();
    if grants.network {
        rows.push(PermissionRow {
            name: "network",
            description: "connect to the internet".into(),
            risk: true,
        });
    }
    if grants.http {
        rows.push(PermissionRow {
            name: "http",
            description: "make web requests".into(),
            risk: true,
        });
    }
    if grants.environment {
        rows.push(PermissionRow {
            name: "environment",
            description: "read environment variables".into(),
            risk: false,
        });
    }
    for dir in &grants.directories {
        let mode = if dir.write { "write" } else { "read" };
        let mut description = format!("{mode} {}", dir.path);
        if let Some(why) = &dir.why {
            description.push_str(&format!(" ({why})"));
        }
        rows.push(PermissionRow {
            name: "directory",
            description,
            risk: dir.write,
        });
    }
    if let Some(memory) = grants.memory_mb {
        rows.push(PermissionRow {
            name: "memory",
            description: format!("{memory} MB"),
            risk: false,
        });
    }
    for process in &grants.processes {
        rows.push(PermissionRow {
            name: "processes",
            description: format!("run {process}"),
            risk: true,
        });
    }
    rows
}

fn permission_name_width<'a>(rows: impl Iterator<Item = &'a PermissionRow>) -> usize {
    rows.map(|row| row.name.len() + 1)
        .max()
        .unwrap_or(0)
        .max(11)
}

fn push_permission_row(
    text: &mut Text,
    row: &PermissionRow,
    mark: &str,
    mark_face: PackageFace,
    name_w: usize,
    width: usize,
    symbols: Symbols,
) {
    let tag = row.risk.then_some(("higher risk", PackageFace::Warning));
    package_ui::capability_line(
        text,
        mark,
        mark_face,
        row.name,
        &row.description,
        tag,
        name_w,
        width,
        symbols,
    );
}

fn push_imports(
    text: &mut Text,
    imports: &[String],
    mismatches: Option<&[forge_core::ImportMismatch]>,
    expanded: bool,
    width: usize,
    symbols: Symbols,
) {
    let shown = if expanded {
        imports.len()
    } else {
        imports.len().min(FOLDED_IMPORTS)
    };
    let bare = |import: &str| import.split('@').next().unwrap_or(import).to_string();
    let name_w = imports[..shown]
        .iter()
        .map(|import| package_ui::display_width(&bare(import)) + 2)
        .max()
        .unwrap_or(0)
        .clamp(30, 40)
        .min(width.saturating_sub(12).max(12));
    for import in &imports[..shown] {
        let mismatch = mismatches.and_then(|all| all.iter().find(|m| m.import == *import));
        let note = if let Some(mismatch) = mismatch {
            format!("needs {}, not declared", mismatch.permission.describe())
        } else if !is_supported_import(import) {
            "not provided by Forge: calls fail".to_string()
        } else if import.starts_with("wasi:filesystem/") {
            "only granted directories".to_string()
        } else if import.starts_with("wasi:sockets/") {
            "only with network permission".to_string()
        } else {
            String::new()
        };
        text.push("  ");
        let label = if mismatch.is_some() {
            import.clone()
        } else {
            bare(import)
        };
        let label = package_ui::pad(&label, name_w, symbols.ellipsis);
        let room = width.saturating_sub(2);
        if mismatch.is_some() {
            text.push_face(
                &package_ui::fit(&format!("{label}{note}"), room, symbols.ellipsis),
                PackageFace::Error,
            );
        } else {
            text.push(&package_ui::fit(&label, room, symbols.ellipsis));
            text.push_face(
                &package_ui::fit(&note, room.saturating_sub(name_w), symbols.ellipsis),
                PackageFace::Inactive,
            );
        }
        text.newline();
    }
    if imports.len() > FOLDED_IMPORTS {
        let hint = if expanded {
            "RET to fold".to_string()
        } else {
            format!("+ {} more   RET to expand", imports.len() - shown)
        };
        text.push_face(&format!("  {hint}"), PackageFace::Inactive);
        text.newline();
    }
}

pub fn toggle_imports(app: &mut App) {
    if app.buf_ref().major_mode() != PERMISSIONS_MODE {
        return app.error("Not in a plugin permission review");
    }
    app.plugin_ui.imports_expanded = !app.plugin_ui.imports_expanded;
    if let Some(pending) = app.pending_grant.take() {
        let name = pending.request.name().to_string();
        let granted = effective(app, &name, &pending.binding);
        let version = pending
            .request
            .manifest
            .as_ref()
            .and_then(|m| m.version())
            .map(str::to_string);
        let view = PermissionView {
            name: pending.request.name(),
            version: version.as_deref(),
            imports: &pending.request.inspection.imports,
            missing: &pending.missing,
            granted: &granted,
            different_binary: pending.different_binary,
        };
        draw_permissions(app, &view);
        app.pending_grant = Some(pending);
    } else if let Some(view) = app.plugin_ui.mismatch.clone() {
        show_mismatch(app, &view);
    }
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
    let outer = app.win().width.max(20);
    let width = package_ui::inner(outer);
    let symbols = Symbols::for_app(app);
    let Some(host) = app.plugin_host.as_ref().filter(|h| !h.is_empty()) else {
        let mut text = Text::for_width(outer);
        package_ui::title(&mut text, "Plugins", "", width, symbols);
        package_ui::rule(&mut text, width, symbols);
        text.push_face(
            "No plugins are loaded. M-x load-plugin loads one.",
            PackageFace::Inactive,
        );
        text.newline();
        text.show(app, "*Plugins*", LIST_MODE, None);
        return;
    };

    let mut rows = Vec::new();
    for name in host.names() {
        let grants = match host.get(name) {
            Some(p) => p.grants.clone(),
            None => host.dormant_grants(name).cloned().unwrap_or_default(),
        };
        let dormant = host.is_dormant(name);
        let unsupported = host
            .get(name)
            .map(|p| p.unsupported.clone())
            .unwrap_or_default();
        let file = host.source(name).map(|p| p.display().to_string());
        let manifest = host
            .source(name)
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| {
                host.source(name)
                    .and_then(|p| Manifest::for_component(p, &bytes).ok().flatten())
            });
        let commands: Vec<String> = app
            .editor
            .commands
            .names()
            .filter(|c| app.editor.commands.plugin_for(c) == Some(name))
            .map(String::from)
            .collect();
        let pre = app
            .rhai
            .as_ref()
            .and_then(|l| l.plugin_grants().remove(name))
            .unwrap_or_default()
            .describe_keyed();
        let asked = app
            .grants
            .get(name)
            .describe_keyed()
            .into_iter()
            .map(|(key, line)| {
                let changed = app.grants.grant_time(name, &key);
                (line, changed)
            })
            .collect();
        rows.push(ListRow {
            name: name.to_string(),
            section: if dormant {
                ListSection::NotStarted
            } else if unsupported.is_empty() {
                ListSection::Loaded
            } else {
                ListSection::Attention
            },
            version: manifest
                .as_ref()
                .and_then(|m| m.version())
                .unwrap_or_default()
                .to_string(),
            description: manifest
                .as_ref()
                .and_then(|m| m.description())
                .unwrap_or_default()
                .to_string(),
            file,
            commands,
            chips: grant_chips(&grants),
            pre,
            asked,
            memory: grants.memory_mb(),
            memory_is_default: grants.memory_mb.is_none(),
            unsupported,
        });
    }

    draw_list(app, rows);
}

fn draw_list(app: &mut App, rows: Vec<ListRow>) {
    let outer = app.win().width.max(20);
    let width = package_ui::inner(outer);
    let symbols = Symbols::for_app(app);
    if !app.plugin_ui.list_initialized {
        app.plugin_ui.list_initialized = true;
        if let Some(first) = rows.first() {
            app.plugin_ui.list_expanded.insert(first.name.clone());
        }
    }
    if app.plugin_ui.list_selected >= rows.len() {
        app.plugin_ui.list_selected = rows.len().saturating_sub(1);
    }
    let selected = app.plugin_ui.list_selected;
    let name_w = rows
        .iter()
        .map(|row| package_ui::display_width(&row.name))
        .max()
        .unwrap_or(4)
        .clamp(18, 24);

    let count = |section: ListSection| rows.iter().filter(|row| row.section == section).count();
    let mut summary = format!(
        "{} loaded {} {} not started",
        count(ListSection::Loaded),
        symbols.dot,
        count(ListSection::NotStarted),
    );
    if count(ListSection::Attention) > 0 {
        summary.push_str(&format!(
            " {} {} need attention",
            symbols.dot,
            count(ListSection::Attention)
        ));
    }
    let mut text = Text::for_width(outer);
    package_ui::title(&mut text, "Plugins", &summary, width, symbols);
    package_ui::rule(&mut text, width, symbols);
    let mut selected_line = 0;
    let mut first = true;
    for section in [
        ListSection::Loaded,
        ListSection::NotStarted,
        ListSection::Attention,
    ] {
        let in_section: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.section == section)
            .map(|(index, _)| index)
            .collect();
        if in_section.is_empty() {
            continue;
        }
        if !first {
            text.newline();
        }
        first = false;
        let label = format!("{} ({})", section.label(), in_section.len());
        package_ui::heading(&mut text, &label, width, symbols);
        text.newline();
        for index in in_section {
            let row = &rows[index];
            let is_selected = index == selected;
            if is_selected {
                selected_line = text.line_count();
            }
            push_plugin_row(
                &mut text,
                row,
                is_selected,
                app.plugin_ui.list_expanded.contains(&row.name),
                name_w,
                width,
                symbols,
            );
        }
    }
    text.newline();
    text.push_face(
        &package_ui::fit(
            "RET expand   r revoke   g refresh   ? help",
            width,
            symbols.ellipsis,
        ),
        PackageFace::Inactive,
    );
    text.newline();
    text.show(
        app,
        "*Plugins*",
        LIST_MODE,
        Some(forge_core::Pos::new(selected_line, 0)),
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListSection {
    Loaded,
    NotStarted,
    Attention,
}

impl ListSection {
    fn label(self) -> &'static str {
        match self {
            Self::Loaded => "Loaded",
            Self::NotStarted => "Not started",
            Self::Attention => "Needs attention",
        }
    }
}

struct ListRow {
    name: String,
    section: ListSection,
    version: String,
    description: String,
    file: Option<String>,
    commands: Vec<String>,
    chips: Vec<(&'static str, PackageFace)>,
    pre: Vec<(String, String)>,
    asked: Vec<(String, Option<i64>)>,
    memory: u32,
    memory_is_default: bool,
    unsupported: Vec<String>,
}

fn grant_chips(grants: &Grants) -> Vec<(&'static str, PackageFace)> {
    let mut chips = Vec::new();
    if grants.network {
        chips.push(("net", PackageFace::BadgeWarning));
    }
    if grants.http {
        chips.push(("http", PackageFace::BadgeWarning));
    }
    if grants.environment {
        chips.push(("env", PackageFace::BadgeInfo));
    }
    for dir in &grants.directories {
        let (label, face) = if dir.write {
            ("dir:w", PackageFace::BadgeWarning)
        } else {
            ("dir:r", PackageFace::BadgeInfo)
        };
        if !chips.iter().any(|(l, _)| *l == label) {
            chips.push((label, face));
        }
    }
    if !grants.processes.is_empty() {
        chips.push(("proc", PackageFace::BadgeWarning));
    }
    if chips.is_empty() {
        chips.push(("no permissions", PackageFace::BadgeInactive));
    }
    chips
}

fn push_plugin_row(
    text: &mut Text,
    row: &ListRow,
    selected: bool,
    expanded: bool,
    name_w: usize,
    width: usize,
    symbols: Symbols,
) {
    let base = selected.then_some(PackageFace::Selected);
    text.begin_line(base);
    let mut used = 0;
    let (mark, mark_face) = match row.section {
        ListSection::Loaded => (symbols.ok, PackageFace::Ok),
        ListSection::NotStarted => (symbols.neutral, PackageFace::Inactive),
        ListSection::Attention => (symbols.error, PackageFace::Error),
    };
    text.push_face(mark, mark_face);
    used += package_ui::display_width(mark);
    list_base(text, &mut used, " ", base);
    let name = package_ui::pad(&row.name, name_w, symbols.ellipsis);
    list_base(text, &mut used, &name, base);
    list_base(text, &mut used, " ", base);
    let version = package_ui::pad(&row.version, 7, symbols.ellipsis);
    let version_face = if selected {
        base
    } else {
        Some(PackageFace::Inactive)
    };
    list_base(text, &mut used, &version, version_face);
    let chip = |text: &mut Text, used: &mut usize, label: &str, face: PackageFace| {
        let label = format!(" {label} ");
        if *used + 1 + package_ui::display_width(&label) > width {
            return;
        }
        list_base(text, used, " ", base);
        text.push_face(&label, face);
        *used += package_ui::display_width(&label);
    };
    if row.section == ListSection::Attention {
        chip(text, &mut used, "not loaded", PackageFace::BadgeError);
    } else {
        for (label, chip_face) in &row.chips {
            chip(text, &mut used, label, *chip_face);
        }
    }
    if row.section == ListSection::NotStarted {
        let note = package_ui::fit(
            "starts on first use",
            width.saturating_sub(used + 1),
            symbols.ellipsis,
        );
        list_base(text, &mut used, " ", base);
        let face = if selected {
            base
        } else {
            Some(PackageFace::Inactive)
        };
        list_base(text, &mut used, &note, face);
    }
    if let Some(face) = base {
        if used < width {
            let fill = " ".repeat(width - used);
            list_base(text, &mut used, &fill, Some(face));
        }
        text.end_line_fill(face);
    }
    text.newline();
    let unsupported = (!row.unsupported.is_empty()).then(|| {
        format!(
            "imports Forge doesn't provide: {}",
            row.unsupported.join(", ")
        )
    });
    if let Some(reason) = unsupported.as_ref().filter(|_| !expanded) {
        text.push(&" ".repeat(4));
        text.push_face(
            &package_ui::fit(reason, width.saturating_sub(4), symbols.ellipsis),
            PackageFace::Error,
        );
        text.newline();
    }
    if !expanded {
        return;
    }

    const INDENT: usize = 4;
    let field = |text: &mut Text, label: &str, value: &str, face: Option<PackageFace>| {
        package_ui::field(text, INDENT, label, value, face, width, symbols);
    };
    if !row.description.is_empty() {
        field(text, "about", &row.description, None);
    }
    if let Some(file) = &row.file {
        field(text, "file", file, None);
    }
    let commands = if row.commands.is_empty() {
        "none".to_string()
    } else {
        row.commands.join(", ")
    };
    field(text, "commands", &commands, None);
    let mut permissions = 0;
    for (line, source, changed) in row.grant_lines() {
        permissions += 1;
        let (label, value) = split_grant_line(&line);
        let tag = match (&source, changed) {
            (Some(source), _) => format!("   [{source}]"),
            (None, _) => "   [granted when asked]".to_string(),
        };
        text.push(&" ".repeat(INDENT));
        text.push_face(
            &package_ui::pad(&label, package_ui::FIELD_LABEL, symbols.ellipsis),
            PackageFace::Inactive,
        );
        let room = width.saturating_sub(INDENT + package_ui::FIELD_LABEL + tag.len());
        text.push(&package_ui::fit(&value, room, symbols.ellipsis));
        text.push_face(&tag, PackageFace::Inactive);
        text.newline();
        if let Some(time) = changed {
            text.push(&" ".repeat(INDENT + package_ui::FIELD_LABEL));
            text.push_face(
                &format!(
                    "last changed {}",
                    forge_core::format_grant_time(time)
                        .split(' ')
                        .next()
                        .unwrap_or_default()
                ),
                PackageFace::Inactive,
            );
            text.newline();
        }
    }
    if permissions == 0 {
        field(
            text,
            "access",
            "none (no files, network or environment)",
            Some(PackageFace::Inactive),
        );
    }
    let memory = if row.memory_is_default {
        format!("{} MB (default)", row.memory)
    } else {
        format!("{} MB", row.memory)
    };
    field(text, "memory", &memory, Some(PackageFace::Inactive));
    if let Some(reason) = &unsupported {
        text.push(&" ".repeat(INDENT));
        text.push_face(
            &package_ui::fit(reason, width.saturating_sub(INDENT), symbols.ellipsis),
            PackageFace::Error,
        );
        text.newline();
    }
}

fn list_base(text: &mut Text, used: &mut usize, value: &str, face: Option<PackageFace>) {
    match face {
        Some(face) => text.push_face(value, face),
        None => text.push(value),
    }
    *used += package_ui::display_width(value);
}

/// Split a grant description such as `network: open connections` or
/// `files: read ~/x` into a short label and the rest.
fn split_grant_line(line: &str) -> (String, String) {
    match line.split_once(": ") {
        Some((label, rest)) => {
            let label = match label {
                "files" => "directory",
                "programs" => "processes",
                other => other,
            };
            (label.to_string(), rest.to_string())
        }
        None => ("access".to_string(), line.to_string()),
    }
}

impl ListRow {
    /// Every permission line with where it came from: the init file or a
    /// grant given when asked.
    fn grant_lines(&self) -> Vec<(String, Option<&'static str>, Option<i64>)> {
        let mut lines: Vec<_> = self
            .pre
            .iter()
            .map(|(_, line)| (line.clone(), Some("init.rhai"), None))
            .collect();
        lines.extend(
            self.asked
                .iter()
                .map(|(line, changed)| (line.clone(), None, *changed)),
        );
        lines
    }
}

fn list_names(app: &App) -> Vec<String> {
    app.plugin_host
        .as_ref()
        .map(|h| h.names().map(String::from).collect())
        .unwrap_or_default()
}

fn selected_list_name(app: &App) -> Option<String> {
    let names = list_names(app);
    names.get(app.plugin_ui.list_selected).cloned()
}

pub fn list_next(app: &mut App) {
    if app.buf_ref().major_mode() != LIST_MODE {
        return app.error("Not in the plugin list");
    }
    let len = list_names(app).len();
    if len > 0 {
        app.plugin_ui.list_selected = (app.plugin_ui.list_selected + 1).min(len - 1);
    }
    list(app);
}

pub fn list_prev(app: &mut App) {
    if app.buf_ref().major_mode() != LIST_MODE {
        return app.error("Not in the plugin list");
    }
    app.plugin_ui.list_selected = app.plugin_ui.list_selected.saturating_sub(1);
    list(app);
}

pub fn list_toggle(app: &mut App) {
    if app.buf_ref().major_mode() != LIST_MODE {
        return app.error("Not in the plugin list");
    }
    if let Some(name) = selected_list_name(app)
        && !app.plugin_ui.list_expanded.remove(&name)
    {
        app.plugin_ui.list_expanded.insert(name);
    }
    list(app);
}

pub fn list_revoke(app: &mut App) {
    if app.buf_ref().major_mode() != LIST_MODE {
        return app.error("Not in the plugin list");
    }
    let Some(name) = selected_list_name(app) else {
        return app.error("No plugin selected");
    };
    revoke(app, &name);
    list(app);
}

pub fn list_describe(app: &mut App) {
    if app.buf_ref().major_mode() != LIST_MODE {
        return app.error("Not in the plugin list");
    }
    let Some(name) = selected_list_name(app) else {
        return app.error("No plugin selected");
    };
    describe(app, &name);
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

#[cfg(test)]
mod review_snapshots {
    use super::*;
    use forge_core::{Buffer, Editor, Grants};

    fn app() -> App {
        let mut app = App::new(Editor::default());
        let id = app.editor.add_buffer(Buffer::new("*scratch*"));
        app.editor.show_buffer(id);
        app.win_mut().width = 80;
        app
    }

    fn view<'a>(
        name: &'a str,
        version: Option<&'a str>,
        imports: &'a [String],
        missing: &'a Grants,
        granted: &'a Grants,
        different_binary: bool,
    ) -> PermissionView<'a> {
        PermissionView {
            name,
            version,
            imports,
            missing,
            granted,
            different_binary,
        }
    }

    fn text(app: &App) -> String {
        app.buf_ref()
            .get_contents()
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn imports() -> Vec<String> {
        vec![
            "forge:editor/log@0.1.0".into(),
            "forge:editor/buffers@0.1.0".into(),
        ]
    }

    fn missing() -> Grants {
        Grants {
            network: true,
            directories: vec![forge_core::permissions::DirGrant {
                path: "~/.cache/spellcheck".into(),
                write: true,
                why: Some("word lists".into()),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn fresh_permission_prompt_snapshot() {
        let imports = imports();
        let missing = missing();
        let granted = Grants::default();
        let mut app = app();
        draw_permissions(
            &mut app,
            &view(
                "spellcheck",
                Some("0.3.0"),
                &imports,
                &missing,
                &granted,
                false,
            ),
        );
        insta_like(
            &text(&app),
            r#" Plugin Permissions                                            spellcheck 0.3.0
--------------------------------------------------------------------------------

spellcheck asks for permission

Asks for (2 new) ---------------------------------------------------------------
  + network     connect to the internet                             higher risk
  + directory   write ~/.cache/spellcheck (word lists)              higher risk

What its code can call (2 imports)
  forge:editor/log@0.1.0
  forge:editor/buffers@0.1.0

y grant and load   n load without permissions   C-g don't load
Grants are kept in the state directory; M-x revoke-plugin-permissions forgets them."#,
        );
    }

    #[test]
    fn different_binary_permission_prompt_snapshot() {
        let imports = imports();
        let missing = missing();
        let granted = Grants::default();
        let mut app = app();
        draw_permissions(
            &mut app,
            &view(
                "spellcheck",
                Some("0.3.0"),
                &imports,
                &missing,
                &granted,
                true,
            ),
        );
        insta_like(
            &text(&app),
            r#" Plugin Permissions                                            spellcheck 0.3.0
--------------------------------------------------------------------------------

spellcheck asks for permission

 ! same name, different binary
The saved approval was for a different build, so it was forgotten.

Asks for (2 new) ---------------------------------------------------------------
  + network     connect to the internet                             higher risk
  + directory   write ~/.cache/spellcheck (word lists)              higher risk

What its code can call (2 imports)
  forge:editor/log@0.1.0
  forge:editor/buffers@0.1.0

y grant and load   n load without permissions   C-g don't load
Grants are kept in the state directory; M-x revoke-plugin-permissions forgets them."#,
        );
    }

    #[test]
    fn import_mismatch_snapshot() {
        let imports = vec![
            "wasi:sockets/instance-network@0.2.12".to_string(),
            "forge:editor/log@0.1.0".to_string(),
        ];
        let mismatch = forge_core::ImportMismatch {
            import: imports[0].clone(),
            permission: forge_core::PermissionKind::Network,
        };
        let view = MismatchView {
            name: "sockets".into(),
            mismatches: vec![mismatch],
            imports: imports.clone(),
        };
        let mut app = app();
        show_mismatch(&mut app, &view);
        insta_like(
            &text(&app),
            r#" Import Mismatch                                                        sockets
--------------------------------------------------------------------------------

sockets can't load: a declared permission is missing

 x the manifest doesn't declare every permission its imports need

Mismatched imports
  x wasi:sockets/instance-network@0.2.12  needs network, not declared

What its code can call (2)
  wasi:sockets/instance-network@0.2.12  (needs network, not declared)
  forge:editor/log@0.1.0

Add the missing permissions to the plugin's manifest, then load it again."#,
        );
    }

    #[test]
    fn dump_permissions_and_list() {
        std::fs::create_dir_all("/tmp/actual").unwrap();
        let imports: Vec<String> = [
            "wasi:sockets/tcp@0.2.12",
            "wasi:filesystem/types@0.2.12",
            "forge:editor/buffers@0.1.0",
            "forge:editor/decorations@0.1.0",
            "forge:editor/log@0.1.0",
            "forge:editor/commands@0.1.0",
            "forge:editor/events@0.1.0",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let missing = Grants {
            network: true,
            directories: vec![forge_core::permissions::DirGrant {
                path: "~/.cache/spellcheck".into(),
                write: true,
                why: None,
            }],
            ..Default::default()
        };
        let granted = Grants {
            memory_mb: Some(256),
            ..Default::default()
        };
        let mut prompt = app();
        draw_permissions(
            &mut prompt,
            &view(
                "spellcheck",
                Some("0.3.0"),
                &imports,
                &missing,
                &granted,
                true,
            ),
        );
        std::fs::write("/tmp/actual/permission_prompt_dark.txt", text(&prompt)).unwrap();

        let row =
            |name: &str, section, version: &str, chips: Vec<(&'static str, PackageFace)>| ListRow {
                name: name.into(),
                section,
                version: version.into(),
                description: String::new(),
                file: None,
                commands: Vec::new(),
                chips,
                pre: Vec::new(),
                asked: Vec::new(),
                memory: 128,
                memory_is_default: true,
                unsupported: Vec::new(),
            };
        let warn = PackageFace::BadgeWarning;
        let info = PackageFace::BadgeInfo;
        let mut org = row(
            "org-modern",
            ListSection::Loaded,
            "0.3.1",
            vec![("net", warn), ("dir:w", warn)],
        );
        org.file = Some("~/.config/forge/plugins/org-modern.wasm".into());
        org.commands = vec!["org-modern-mode".into(), "org-modern-refresh".into()];
        org.asked = vec![(
            "network: connect to api.example.com".into(),
            Some(1_790_942_400),
        )];
        org.pre = vec![(
            "directory".into(),
            "files: write ~/.cache/org-modern".into(),
        )];
        let mut old = row("old-theme", ListSection::Attention, "0.1.0", vec![]);
        old.unsupported = vec!["forge:editor/legacy-faces".into()];
        let rows = vec![
            org,
            row(
                "spellcheck",
                ListSection::Loaded,
                "0.3.0",
                vec![("net", warn), ("dir:w", warn)],
            ),
            row(
                "rainbow-delimiters",
                ListSection::Loaded,
                "0.4.1",
                vec![("no permissions", PackageFace::BadgeInactive)],
            ),
            row(
                "git-status",
                ListSection::Loaded,
                "1.1.0",
                vec![("proc", warn), ("dir:r", info)],
            ),
            row(
                "snippets",
                ListSection::NotStarted,
                "2.0.0",
                vec![("dir:r", info)],
            ),
            row(
                "lsp-helper",
                ListSection::NotStarted,
                "0.7.2",
                vec![("net", warn), ("proc", warn)],
            ),
            old,
        ];
        let mut app = app();
        app.plugin_ui.list_initialized = true;
        app.plugin_ui.list_expanded.insert("org-modern".into());
        app.plugin_ui.list_selected = 0;
        draw_list(&mut app, rows);
        std::fs::write("/tmp/actual/plugins_list_dark.txt", text(&app)).unwrap();
    }

    fn insta_like(actual: &str, expected: &str) {
        assert_eq!(actual, expected, "\n--- actual ---\n{actual}\n--- end ---");
    }
}
