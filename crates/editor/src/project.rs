// SPDX-License-Identifier: MIT OR Apache-2.0
//! Projects: a directory's own key bindings, and its `init.rhai`.
//!
//! The bindings are `forge_core::keymap::Scope::Project`, which applies to
//! every file inside the project (see `forge_core::project` for how one is
//! found). init.rhai binds into them; the user can from theirs.
//!
//! A project's `.forge/init.rhai` is code from a directory the user may
//! have just cloned, so it only runs once they have granted it — the same
//! grant file plugins use, under `project:<directory>`. Saying no is
//! remembered for the session, so a project never asks twice, and
//! `M-x revoke-plugin-permissions` forgets the answer.

use crate::app::App;
use crate::minibuffer::Purpose;
use forge_core::project;
use std::path::{Path, PathBuf};

/// Load the active buffer's project's init.rhai, asking first. Called
/// whenever a file becomes the active one; cheap when there is nothing to
/// ask about, since the project is only looked for once per buffer.
pub fn load_if_granted(app: &mut App) {
    let Some(path) = app.buf_ref().path().map(Path::to_path_buf) else {
        return;
    };
    // A prompt is already up (the file was opened from one): ask then, or
    // not at all.
    if app.minibuffer.is_some() {
        return;
    }
    // Without a Rhai state there is nowhere to run it in: -q means no
    // configuration at all, and a project's is configuration. Checked
    // before the project is marked as asked, so starting up and loading
    // init.rhai afterwards still gets its turn.
    if app.rhai.is_none() {
        return;
    }
    let Some(root) = project::root(&path) else {
        return;
    };
    let Some(file) = project::init_file(&root) else {
        return;
    };
    // Asked about once per session, whichever way it went.
    if !app.projects_asked.insert(root.clone()) {
        return;
    }
    if app.grants.project_code(&root) {
        run(app, &root, &file);
        return;
    }
    let prompt = format!(
        "Run {}? It is code from this directory, which may not be yours. (y or n) ",
        file.display()
    );
    app.prompt(prompt, Purpose::ProjectGrant { root });
}

/// The answer to "run this project's init.rhai?". Yes saves the grant so it
/// isn't asked again; no and C-g don't, and neither asks again this
/// session.
pub fn answered(app: &mut App, root: &Path, answer: Option<bool>) {
    let Some(file) = project::init_file(root) else {
        return;
    };
    if answer == Some(true) {
        if let Err(e) = app.grants.grant_project_code(root) {
            app.error(format!("Couldn't save the grant: {e}"));
        }
        run(app, root, &file);
        return;
    }
    app.msg(format!(
        "Not running {} (M-x revoke-plugin-permissions forgets the answer)",
        file.display()
    ));
}

/// Run a project's init.rhai in the editor's Rhai state, the one init.rhai
/// used, so bindings made in it are visible to everything else.
fn run(app: &mut App, root: &Path, file: &Path) {
    let src = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => return app.error(format!("{}: {e}", file.display())),
    };
    let Some(rhai) = app.rhai.as_ref() else {
        return;
    };
    let out = rhai.load_project_file(&mut app.editor, root, &src);
    crate::plugins::apply(app, out);
    // The project map is what the file is for, so say it ran even if the
    // file was empty.
    app.msg(format!("Loaded {}", file.display()));
}

/// Project roots that have bindings right now, for `M-x list-projects`.
pub fn loaded(app: &App) -> Vec<&Path> {
    app.editor
        .keymaps
        .bound_projects()
        .into_iter()
        .map(PathBuf::as_path)
        .collect()
}

/// M-x list-projects: the projects whose keys are in play here, and what
/// Forge trusts each one with.
pub fn list(app: &mut App) {
    let mut t = String::from("Projects\n\n");
    let here = app
        .editor
        .key_context
        .project
        .as_ref()
        .map(|p| p.display().to_string());
    match &here {
        Some(p) => t.push_str(&format!("The buffer on screen is in {p}\n\n")),
        None => t.push_str("The buffer on screen is in no project (no .forge above it).\n\n"),
    }
    let roots = loaded(app);
    if roots.is_empty() {
        t.push_str(
            "No project has key bindings of its own.\n\
            A project is the nearest directory above a file with a .forge/ in it;\n\
            bind into one from init.rhai with { project = \"/path/to/project\" }.\n",
        );
    }
    for root in roots {
        t.push_str(&format!("{}\n", root.display()));
        for (keys, command) in app
            .editor
            .keymaps
            .get(&forge_core::Scope::Project(root.to_path_buf()))
            .into_iter()
            .flat_map(|m| m.bindings())
        {
            t.push_str(&format!("  {keys:<21} {command}\n"));
        }
        let trusted = app.grants.project_code(root);
        t.push_str(&format!(
            "  may run its init.rhai: {}  [{}]\n",
            if trusted { "yes" } else { "no (it will ask)" },
            forge_core::permissions::project_key(root)
        ));
        t.push('\n');
    }
    app.show_special_buffer("*Projects*", "special-mode", &t);
}
