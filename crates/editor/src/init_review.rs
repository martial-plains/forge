// SPDX-License-Identifier: MIT OR Apache-2.0
//! Ask before evaluating an `init.rhai` that changed outside Forge.
//!
//! Startup keeps this decision before any Rhai runs. Declining does not
//! evaluate the file and leaves the editor in the `-q` state.

use crate::app::App;
use crate::minibuffer::{ConfirmAction, Purpose};
use forge_core::GrantStore;
use forge_core::init_review::{InitReview, snapshot_path};

pub const BUFFER: &str = "*Init Review*";

/// Startup work that waits until the init review is answered.
#[derive(Debug)]
pub(crate) struct PendingStartup {
    pub plugin_files: Vec<String>,
    pub profile_startup: bool,
    pub config: bool,
}

/// The changed file's new permissions, when it has any to review.
pub fn pending() -> Option<InitReview> {
    let path = forge_core::config::init_file()?;
    let previous = std::fs::read_to_string(snapshot_path()?).ok()?;
    let current = std::fs::read_to_string(path).ok()?;
    forge_core::init_review::review(&previous, &current)
}

/// Show the diff and ask whether to evaluate the changed init file.
pub fn ask(app: &mut App, review: InitReview) {
    let path = forge_core::config::init_file();
    let current = path
        .as_deref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default();
    let previous = snapshot_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default();
    let diff = crate::vc::diff::unified(
        &previous,
        &current,
        "init.rhai (last evaluated)",
        "init.rhai (on disk)",
        3,
    );
    let mut text =
        String::from("init.rhai changed outside Forge and asks for new permissions:\n\n");
    for addition in &review.additions {
        text.push_str("  + ");
        text.push_str(addition);
        text.push('\n');
    }
    text.push_str("\nReview the change before Forge evaluates it:\n\n");
    text.push_str(&diff);
    app.show_special_buffer(BUFFER, "special-mode", &text);
    app.prompt(
        "Evaluate the changed init.rhai? (y or n) ",
        Purpose::Confirm(ConfirmAction::InitReview),
    );
}

/// Evaluate the reviewed file, then finish the startup that waited on it.
pub fn accepted(app: &mut App) {
    app.load_init_file();
    crate::finish_startup(app);
}

/// Behave like `-q`: no init, grants, plugins directory, cache or session.
pub fn declined(app: &mut App) {
    app.init_path = None;
    app.grants = GrantStore::in_memory();
    app.plugin_cache = None;
    app.rhai = None;
    app.init_errors.clear();
    // -q does not read or later save session state either.
    app.editor.session = forge_core::session::Session::default();
    if let Some(pending) = app.pending_startup.as_mut() {
        pending.config = false;
    }
    app.msg("init.rhai was not evaluated");
    crate::finish_startup(app);
}

/// Remember a save of init.rhai as Forge's own change, not an external one.
pub fn record_saved(app: &App) {
    if let Some(path) = app.buf_ref().path() {
        forge_core::init_review::record(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new(forge_core::Editor::default());
        let id = app.editor.add_buffer(forge_core::Buffer::new("*scratch*"));
        app.editor.show_buffer(id);
        app
    }

    #[test]
    fn declining_evaluates_no_init_and_leaves_no_rhai_state() {
        let dir = std::env::temp_dir().join(format!("forge-init-review-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("ran");
        let init = dir.join("init.rhai");
        std::fs::write(
            &init,
            format!(
                "forge.fs.write({:?}, \"evaluated\");\n",
                marker.display().to_string()
            ),
        )
        .unwrap();
        let mut app = app();
        app.init_path = Some(init);
        app.pending_startup = Some(PendingStartup {
            plugin_files: Vec::new(),
            profile_startup: false,
            config: true,
        });
        declined(&mut app);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!marker.exists(), "the init file was evaluated");
        assert!(app.init_path.is_none());
        assert!(app.rhai.is_none());
    }
}
