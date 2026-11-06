// SPDX-License-Identifier: MIT OR Apache-2.0
//! M-x package-doctor and `forge package doctor`: collect the package
//! state once, run every check, and render the same findings everywhere.

use crate::app::App;
use forge_core::package::InstalledDb;
use forge_core::package_doctor::{CheckId, DoctorContext, Finding, PluginInfo, has_errors, run};
use forge_core::{GrantStore, LockFile, Manifest, PluginRuntime};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const BUFFER: &str = "*Package Doctor*";

/// Collect the current app's package state and run the doctor checks.
pub fn findings(app: &App) -> Vec<Finding> {
    let (ctx, lock_error) = context(app);
    finish(ctx, lock_error)
}

fn finish(mut ctx: DoctorContext, lock_error: Option<String>) -> Vec<Finding> {
    let report_has_unknown_key = ctx.report.iter().any(|line| line.contains("unknown key"));
    ctx.init_errors
        .retain(|error| !(report_has_unknown_key && error.contains("unknown key")));
    let mut findings = run(&ctx);
    if let Some(error) = lock_error {
        findings.push(Finding::error(
            CheckId::LockHash,
            None,
            None,
            format!("cannot read forge-lock.toml: {error}"),
            "Fix or remove forge-lock.toml, then run the doctor again.",
        ));
    }
    findings
}

fn load_lock() -> Result<LockFile, String> {
    crate::declare::load_lock()
}

fn context(app: &App) -> (DoctorContext, Option<String>) {
    let config_dir = forge_core::config::config_dir();
    let plugins_dir = app
        .packages
        .plugin_dir
        .clone()
        .or_else(forge_core::config::plugin_dir);
    let data_dir = forge_core::config::data_dir();
    let state_dir = forge_core::config::state_dir();
    let auth_files = crate::authinfo::files();
    let cache_dir = app
        .plugin_cache
        .clone()
        .or_else(forge_core::config::cache_dir);
    let uses = app
        .rhai
        .as_ref()
        .map(|rhai| rhai.plugin_uses())
        .unwrap_or_default();
    let report = app
        .rhai
        .as_ref()
        .map(|rhai| rhai.use_package_report())
        .unwrap_or_default();
    let installed = plugins_dir
        .as_deref()
        .map(InstalledDb::load)
        .unwrap_or_default();
    let (lock, lock_error) = match load_lock() {
        Ok(lock) => (lock, None),
        Err(error) => (LockFile::in_memory(), Some(error)),
    };
    let plugins = plugin_info(
        app,
        &uses,
        &installed,
        &lock,
        plugins_dir.as_deref(),
        cache_dir.as_deref(),
    );
    let mut names: BTreeSet<String> = uses.iter().map(|u| u.name.clone()).collect();
    names.extend(installed.packages.keys().cloned());
    names.extend(lock.packages.keys().cloned());
    names.extend(plugins.keys().cloned());
    if let Some(host) = &app.plugin_host {
        names.extend(host.names().map(str::to_string));
    }
    let grants = names
        .into_iter()
        .map(|name| {
            let grants = crate::plugins::effective_grants(app, &name);
            (name, grants)
        })
        .collect();
    (
        DoctorContext {
            uses,
            report,
            init_errors: app.init_errors.clone(),
            lock,
            installed,
            grants,
            plugins,
            config_dir,
            data_dir,
            cache_dir,
            state_dir,
            auth_files,
        },
        lock_error,
    )
}

fn plugin_info(
    app: &App,
    uses: &[forge_core::PluginUse],
    installed: &InstalledDb,
    lock: &LockFile,
    plugins_dir: Option<&Path>,
    cache_dir: Option<&Path>,
) -> BTreeMap<String, PluginInfo> {
    let mut names: BTreeSet<String> = uses.iter().map(|u| u.name.clone()).collect();
    names.extend(installed.packages.keys().cloned());
    names.extend(lock.packages.keys().cloned());
    if let Some(dir) = plugins_dir {
        for path in forge_core::config::discover_plugins(dir) {
            if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                names.insert(name.to_string());
            }
        }
    }
    if let Some(host) = &app.plugin_host {
        names.extend(host.names().map(str::to_string));
    }
    let mut runtime = None;
    let mut out = BTreeMap::new();
    for name in names {
        let path = plugin_path(app, &name, plugins_dir);
        let Some(path) = path else {
            continue;
        };
        let bytes = std::fs::read(&path).ok();
        let size = bytes
            .as_ref()
            .map(|b| b.len() as u64)
            .or_else(|| std::fs::metadata(&path).ok().map(|m| m.len()))
            .unwrap_or(0);
        let sha256 = bytes
            .as_ref()
            .map(|b| forge_core::package::sha256_hex(b))
            .unwrap_or_default();
        let manifest = bytes
            .as_ref()
            .and_then(|b| Manifest::for_component(&path, b).ok().flatten());
        let imports = if let Some(loaded) = app.plugin_host.as_ref().and_then(|h| h.get(&name)) {
            loaded.imports.clone()
        } else if let Some(host) = &app.plugin_host {
            host.inspect_file(&path)
                .map(|request| request.inspection.imports)
                .unwrap_or_default()
        } else {
            let bytes = bytes.as_deref().unwrap_or_default();
            let runtime = runtime.get_or_insert_with(|| {
                let cache = cache_dir.map(|d| d.join("wasmtime"));
                PluginRuntime::with_cache(cache.as_deref()).ok()
            });
            runtime
                .as_ref()
                .and_then(|runtime| runtime.inspect(&name, bytes).ok())
                .map(|inspection| inspection.imports)
                .unwrap_or_default()
        };
        out.insert(
            name.clone(),
            PluginInfo {
                name,
                path,
                size,
                sha256,
                manifest,
                imports,
            },
        );
    }
    out
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
    let path = plugins_dir?.join(format!("{name}.wasm"));
    path.is_file().then_some(path)
}

/// Run the doctor and show its report in a special buffer.
pub fn show(app: &mut App) {
    let findings = findings(app);
    let text = render_text(&findings);
    app.show_special_buffer(BUFFER, "special-mode", &text);
    let errors = findings
        .iter()
        .filter(|f| f.severity == forge_core::package_doctor::Severity::Error)
        .count();
    let warnings = findings
        .iter()
        .filter(|f| f.severity == forge_core::package_doctor::Severity::Warning)
        .count();
    if errors > 0 {
        app.msg(format!(
            "Package doctor: {errors} error{}, {warnings} warning{}",
            plural(errors),
            plural(warnings)
        ));
    } else if warnings > 0 {
        app.msg(format!(
            "Package doctor: {warnings} warning{}",
            plural(warnings)
        ));
    } else {
        app.msg("Package doctor: no problems found");
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// The text report, used by both M-x package-doctor and package doctor.
pub fn render_text(findings: &[Finding]) -> String {
    if findings.is_empty() {
        return "No package problems found.\n".to_string();
    }
    let mut out = String::new();
    for finding in findings {
        out.push_str(finding.severity.as_str());
        out.push_str(" [");
        out.push_str(finding.check.as_str());
        out.push(']');
        let in_message = finding
            .plugin
            .as_ref()
            .is_some_and(|plugin| finding.message.starts_with(&format!("{plugin}:")));
        if let Some(plugin) = &finding.plugin
            && !in_message
        {
            out.push(' ');
            out.push_str(plugin);
        }
        if let Some(line) = finding.line {
            out.push_str(&format!(" (init.rhai:{line})"));
        }
        out.push_str(": ");
        out.push_str(&finding.message);
        out.push('\n');
        if let Some(fix) = &finding.fix {
            out.push_str("  fix: ");
            out.push_str(fix);
            out.push('\n');
        }
    }
    out
}

/// Build an app with init.rhai and grants, but without loading plugins or
/// starting a package update. The doctor only reads state.
fn headless_app() -> anyhow::Result<App> {
    let mut editor = forge_core::Editor::new();
    editor.create_buffer("*scratch*");
    editor
        .state
        .active_buffer_mut()
        .expect("the scratch buffer")
        .set_major_mode(forge_core::rhai::INTERACTION_MODE);
    let mut app = App::new(editor);
    crate::messages::messages_buffer(&mut app);
    app.plugin_cache = forge_core::config::cache_dir();
    match GrantStore::load_default() {
        Ok(grants) => app.grants = grants,
        Err(error) => {
            return Err(anyhow::anyhow!("plugin grants not loaded: {error}"));
        }
    }
    app.init_path = forge_core::config::init_file();
    if let Some(path) = app.init_path.clone()
        && path.exists()
    {
        let rhai = forge_core::RhaiConfig::new()
            .map_err(|error| anyhow::anyhow!("can't start Rhai: {error}"))?;
        let mut init_errors = rhai.load_file(&mut app.editor, &path).errors;
        for theme in forge_core::config::theme_files() {
            init_errors.extend(rhai.load_file(&mut app.editor, &theme).errors);
        }
        app.rhai = Some(rhai);
        app.init_errors = init_errors;
    }
    Ok(app)
}

/// `forge package doctor [--json]`. True means no error findings.
pub fn run_headless(json: bool) -> anyhow::Result<bool> {
    let app = headless_app()?;
    let (ctx, lock_error) = context(&app);
    let findings = finish(ctx, lock_error);
    if json {
        println!("{}", serde_json::to_string_pretty(&findings)?);
    } else {
        print!("{}", render_text(&findings));
    }
    Ok(!has_errors(&findings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::package_doctor::{CheckId, Finding, Severity};

    #[test]
    fn text_report_names_the_plugin_once() {
        let finding = Finding::new(
            Severity::Error,
            CheckId::UsePackage,
            Some("demo".into()),
            Some(4),
            "demo: skipped, unknown key \"bogus\"",
            Some("Fix the declaration.".into()),
        );
        let text = render_text(&[finding]);
        assert_eq!(text.matches("demo").count(), 1);
        assert!(text.contains("error [use-package] (init.rhai:4): demo:"));
        assert!(text.contains("fix: Fix the declaration."));
    }

    #[test]
    fn empty_report_says_so() {
        assert_eq!(render_text(&[]), "No package problems found.\n");
    }
}
