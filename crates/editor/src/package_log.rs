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
    use crate::package_ui::{Status as UiStatus, Symbols, Text};
    use forge_core::decoration::PackageFace;
    use forge_core::package_log::{SourceKind, Status};

    match package_log::read_events() {
        Ok(events) => {
            let symbols = Symbols::for_app(app);
            let mut text = Text::new();
            let title = "Package Operations";
            text.push_face(title, PackageFace::Title);
            let count = events.len();
            let counts = format!(
                "{count} operation{} {} {} v{}",
                if count == 1 { "" } else { "s" },
                symbols.dot,
                package_log::SCHEMA,
                package_log::VERSION
            );
            let width = app.win().width.max(40);
            let gap = width.saturating_sub(
                crate::package_ui::display_width(title) + crate::package_ui::display_width(&counts),
            );
            if gap > 0 {
                text.push(&" ".repeat(gap));
            }
            text.push_face(&counts, PackageFace::Heading);
            text.newline();
            text.newline();

            if events.is_empty() {
                text.push("No package operations recorded.\n");
            } else {
                text.push_face(
                    &format!(
                        "  {}  {}  {}  {}  {:>10}",
                        crate::package_ui::pad("Time", 20, symbols.ellipsis),
                        crate::package_ui::pad("Operation", 10, symbols.ellipsis),
                        crate::package_ui::pad("Package", 12, symbols.ellipsis),
                        crate::package_ui::pad("Status", 10, symbols.ellipsis),
                        "Duration"
                    ),
                    PackageFace::Heading,
                );
                text.newline();
                text.push_face(&symbols.rule.repeat(width.min(80)), PackageFace::Rule);
                text.newline();
            }
            for event in &events {
                let package = event.package.as_deref().unwrap_or("(index)");
                let status = match event.status {
                    Status::Ok => UiStatus::Ok,
                    Status::Failed => UiStatus::Error,
                    Status::Declined | Status::Cancelled => UiStatus::Neutral,
                    Status::Unchanged => UiStatus::Info,
                };
                text.push("  ");
                text.push(&crate::package_ui::pad(
                    &package_log::format_time(event.started_at_ms),
                    20,
                    symbols.ellipsis,
                ));
                text.push("  ");
                text.push_face(
                    &crate::package_ui::pad(
                        package_log::operation_name(event.operation),
                        10,
                        symbols.ellipsis,
                    ),
                    PackageFace::Primary,
                );
                text.push("  ");
                text.push(&crate::package_ui::pad(package, 12, symbols.ellipsis));
                text.push("  ");
                text.push_status(
                    status,
                    package_log::status_name(event.status),
                    10,
                    false,
                    symbols,
                );
                text.push("  ");
                text.push_face(
                    &format!("{:>10.3} ms", event.duration_us as f64 / 1000.0),
                    PackageFace::Heading,
                );
                text.newline();
                if let Some(source) = &event.source {
                    let kind = match source.kind {
                        SourceKind::Index => "index",
                        SourceKind::Url => "url",
                        SourceKind::Path => "path",
                        SourceKind::Git => "git",
                    };
                    text.push_face("    source: ", PackageFace::Heading);
                    text.push(kind);
                    if let Some(location) = &source.location {
                        text.push(" ");
                        text.push(location);
                    }
                    if let Some(revision) = &source.revision {
                        text.push_face(" @ ", PackageFace::Rule);
                        text.push(revision);
                    }
                    text.newline();
                }
                for (key, value) in &event.detail {
                    text.push_face(&format!("    {key}: "), PackageFace::Heading);
                    text.push(value);
                    text.newline();
                }
                if let Some(error) = &event.error {
                    text.push("    ");
                    text.push_face("error: ", PackageFace::Error);
                    text.push(error);
                    text.newline();
                }
            }
            text.show(app, BUFFER, "special-mode", None);
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
