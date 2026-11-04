// SPDX-License-Identifier: MIT OR Apache-2.0
//! Forge's editor application: keys to commands, the minibuffer, plugins
//! and init.rhai, on top of forge-core.
//!
//! Both frontends drive this: the terminal one (crates/terminal, which
//! adds ratatui painting, terminal capabilities and a watchdog thread)
//! and the macOS app (crates/ffi, behind UniFFI). Everything that decides
//! what a key or a click does lives here, so neither frontend can decide
//! it differently.

pub mod app;
pub mod authinfo;
pub mod bookmarks;
pub mod builtins;
pub mod canvas;
pub mod capf;
pub mod clipboard;
pub mod commands;
pub mod compile;
pub mod declare;
pub mod dialog;
pub mod dired;
pub mod docs_bridge;
pub mod docs_tables;
pub mod docview;
pub mod few;
pub mod files;
pub mod frames;
pub mod grammar_install;
pub mod grep;
pub mod hangul;
pub mod help;
pub mod icon;
pub mod image_mode;
pub mod info;
pub mod input_method;
pub mod iso_transl;
pub mod keymap;
pub mod kkc;
pub mod kmacro;
pub mod mode_line;
pub mod server;
pub mod term;
pub mod webkit;

pub mod hover;
pub mod menu;
pub mod messages;
pub mod minibuffer;
pub use minibuffer::start_in_home_from_gui;
pub mod mouse;
pub mod one_key;
pub mod org_agenda_ui;
pub mod org_cmds;
pub mod org_crypt;
pub mod org_dblock;
pub mod org_export;
pub mod org_extra;
pub mod org_flow;
#[cfg(test)]
mod org_keys_tests;
pub mod org_publish;
pub mod org_rich;
pub mod org_table_cmds;
pub mod package_vc;
pub mod packages;
pub mod plugins;
pub mod project;
pub mod query;
pub mod registers;
pub mod search;
pub mod tabs;
pub mod tether;
pub mod toolbar;
pub mod tutorial;
pub mod ui;
pub mod vc;
pub mod watch;
pub mod window_extras;
pub mod windows;

pub use app::App;
pub use clipboard::{ClipboardProvider, InMemoryClipboard, SystemClipboard};
use forge_core::Editor;

/// How to start the editor.
#[derive(Debug, Clone, Default)]
pub struct StartOptions {
    /// -q: no init.rhai, grants, plugins directory, cache or session.
    pub no_config: bool,
    /// Files to open.
    pub files: Vec<String>,
    /// --plugin files to load.
    pub plugin_files: Vec<String>,
    /// The frontend has a GUI: it can show the tool bar and modifier bar.
    /// The terminal frontend leaves this false has no tool bar
    /// in a text frame.
    pub graphical: bool,
}

/// Make the editor as a frontend starts it: session, files, grants,
/// init.rhai, then plugins.
pub fn start(opts: &StartOptions) -> anyhow::Result<App> {
    let mut editor = Editor::new();
    editor.create_buffer("*scratch*");
    editor
        .state
        .active_buffer_mut()
        .expect("the scratch buffer")
        .set_major_mode(forge_core::rhai::INTERACTION_MODE);
    // Places in files, recent files, minibuffer history (not with -q).
    if !opts.no_config {
        let file = forge_core::config::state_dir().map(|d| d.join("session.toml"));
        editor.session = forge_core::session::Session::load(file);
    }
    let mut dirs = Vec::new();
    for arg in &opts.files {
        // A directory opens in Dired, like C-x C-f on one does, instead of
        // failing with EISDIR.
        if std::fs::metadata(arg).map(|m| m.is_dir()).unwrap_or(false) {
            dirs.push(arg.clone());
            continue;
        }
        match editor.open_file(arg) {
            Ok((id, _)) => editor.show_buffer(id),
            Err(e) => anyhow::bail!("{arg}: {e}"),
        }
    }

    let mut app = App::new(editor);
    // Like *scratch*, *Messages* is there from the start; plugins never see events from it.
    crate::messages::messages_buffer(&mut app);
    app.graphical = opts.graphical;
    // The frame furniture is on by default: the menu bar, tab bar and the
    // vertical scroll bar in every frontend, and the tool bar in a GUI.
    // init.rhai can still turn any of them off, and `load_init_file` resets
    // the scroll bar to `default_scroll_bar`, so a reload keeps it.
    crate::menu::set_mode(&mut app, true);
    if opts.graphical {
        app.tool_bar_style = forge_core::toolbar::Style::Image;
        crate::toolbar::set_mode(&mut app, true);
    }
    crate::tabs::set_mode(&mut app, true);
    app.default_scroll_bar = true;
    app.scroll_bar = true;
    for dir in &dirs {
        crate::dired::open(&mut app, std::path::Path::new(dir));
    }
    app.msg("Welcome to Forge.  C-x C-c quits, C-h b lists key bindings.");
    // Grants live next to init.rhai; with -q nothing is read or saved.
    if !opts.no_config {
        if let Some(dir) = forge_core::config::config_dir() {
            match forge_core::GrantStore::load(dir.join("plugin-grants.toml")) {
                Ok(g) => app.grants = g,
                Err(e) => app.error(format!("Plugin grants not loaded: {e}")),
            }
        }
        // Grammars installed with M-x treesit-install-language-grammar:
        // their modes are owned by the installer, so reloading init.rhai
        // leaves them alone.
        let grammar_errors = forge_core::grammar_install::load_all(&mut app.editor);
        if !grammar_errors.is_empty() {
            app.error(format!(
                "Installed grammars not loaded: {}",
                grammar_errors.join("; ")
            ));
        }
        // init.rhai first: it can pre-grant plugins their permissions.
        app.init_path = forge_core::config::init_file();
        app.load_init_file();
    }
    // Then plugins: the plugins directory, then --plugin files. One that
    // needs permission asks once the editor is up; others wait their turn.
    if !opts.no_config {
        app.plugin_cache = forge_core::config::cache_dir();
        // Plugins declared in init.rhai first, in `after` order: installed
        // ones load (or wait lazily), missing ones start installing.
        crate::declare::resolve(&mut app);
        let declared = crate::declare::declared_order(&app);
        for p in plugins::discover() {
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if declared.iter().any(|n| n == stem) {
                continue; // resolve() already loaded it
            }
            plugins::load_at_startup(&mut app, &p);
        }
    }
    for p in &opts.plugin_files {
        plugins::load(&mut app, std::path::Path::new(p));
    }
    app.report_unknown_bindings();
    // init.rhai's forge.mode.hook and its forge.command.run of plugin
    // commands, now that the plugins are here.
    app.run_mode_hooks();
    crate::vc::ensure_state(&mut app);
    // Like after-find-file for the files on the command line.
    if app.minibuffer.is_none() {
        crate::files::warn_about_auto_save_data(&mut app);
    }
    Ok(app)
}
