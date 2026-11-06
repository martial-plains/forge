// SPDX-License-Identifier: MIT OR Apache-2.0
//! M-x package-log and package-log-export: the structured log of package
//! fetches, builds, installs, upgrades and rollbacks.

use crate::app::App;
use crate::minibuffer::Purpose;
use forge_core::package_log::{self, Event};

pub const BUFFER: &str = "*Package Operations*";
pub const JSON_BUFFER: &str = "*Package Operations JSON Lines*";

pub fn record(event: Event) {
    let _ = package_log::append(&event);
}

pub fn show(app: &mut App) {
    match package_log::read_events() {
        Ok(events) => {
            let text = package_log::render_text(&events);
            app.show_special_buffer(BUFFER, "special-mode", &text);
            let count = events.len();
            app.msg(format!(
                "{count} package operation{}",
                if count == 1 { "" } else { "s" }
            ));
        }
        Err(error) => app.error(format!("Couldn't read the package operation log: {error}")),
    }
}

pub fn export(app: &mut App) {
    match package_log::read_json_lines() {
        Ok(text) => {
            app.show_special_buffer(JSON_BUFFER, "special-mode", &text);
            app.prompt("Export package operation log to: ", Purpose::WriteFile);
        }
        Err(error) => app.error(format!("Couldn't read the package operation log: {error}")),
    }
}
