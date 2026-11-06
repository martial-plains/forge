// SPDX-License-Identifier: MIT OR Apache-2.0
//! M-x plugin-profile and `forge --profile-startup`: where plugin startup
//! and later calls spent their time.

use crate::app::App;
use crate::plugins;
use forge_core::plugin::profile::ProfileReport;
use std::time::Duration;

pub const BUFFER: &str = "*Plugin Profile*";

/// Arguments for `forge --profile-startup`.
pub struct Options {
    pub no_config: bool,
    pub files: Vec<String>,
    pub plugin_files: Vec<String>,
    pub json: bool,
}

/// Start Forge with profiling on, print the report, and say whether the
/// startup budget was respected.
pub fn run(opts: Options) -> anyhow::Result<bool> {
    let app = crate::start(&crate::StartOptions {
        no_config: opts.no_config,
        files: opts.files,
        plugin_files: opts.plugin_files,
        graphical: false,
        profile_startup: true,
    })?;
    let report = report(&app);
    if opts.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render_text());
    }
    Ok(!report.over_budget && !app.startup_budget_exceeded)
}

/// The current report, even when no plugin host was ever made.
pub fn report(app: &App) -> ProfileReport {
    if let Some(host) = app.plugin_host.as_ref() {
        return host.profile_report(app.startup_budget);
    }
    ProfileReport {
        startup_ns: None,
        budget_ns: app.startup_budget.map(ns),
        over_budget: false,
        plugins: Vec::new(),
    }
}

/// M-x plugin-profile: show the timings collected so far.
pub fn show(app: &mut App) {
    if !plugins::ensure_host(app) {
        return;
    }
    let enabled = app
        .plugin_host
        .as_ref()
        .is_some_and(|host| host.profiling_enabled());
    if !enabled && let Some(host) = app.plugin_host.as_ref() {
        host.enable_profiling();
    }
    let report = report(app);
    let text = report.render_text();
    app.show_special_buffer(BUFFER, "special-mode", &text);
    if enabled {
        app.msg("Plugin profile");
    } else {
        app.msg(
            "Plugin profiling enabled; startup timings were not recorded (restart with --profile-startup for those)",
        );
    }
}

fn ns(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}
