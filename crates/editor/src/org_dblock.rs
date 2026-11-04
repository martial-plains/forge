// SPDX-License-Identifier: MIT OR Apache-2.0
//! Org dynamic blocks: update, update all, insert. The text work is in
//! `forge_core::org_dblock`; this runs a custom block's writer command
//! (`org-dblock-write:NAME`, from a plugin or init.rhai) and asks.

use crate::app::App;
use crate::org_cmds::{Ask, choose};
use forge_core::Pos;
use forge_core::org_dblock::{self as db, Updated};
use forge_core::org_time::now_local;

pub const COMMANDS: &[forge_docs::CommandDoc] = &[
    forge_docs::CommandDoc::new(
        "org-dblock-update",
        "Org: rewrite the dynamic block at point (all of them with a prefix).",
    )
    .body("Built in: clocktable and columnview. Any other NAME runs the command"),
    forge_docs::CommandDoc::line(
        "org-update-all-dblocks",
        "Org: rewrite every dynamic block in the buffer.",
    ),
    forge_docs::CommandDoc::line(
        "org-dynamic-block-insert-dblock",
        "Org: insert a dynamic block, choosing its kind.",
    ),
];

const WRITER: &str = "org-dblock-write:";

fn in_org(app: &mut App) -> bool {
    if app.buf_ref().major_mode() != forge_core::org::MODE {
        app.error("Not in an Org buffer");
        return false;
    }
    true
}

/// Whether point is in a dynamic block.
pub fn at_block(app: &App) -> bool {
    let b = app.buf_ref();
    db::block_at(b, b.point().line).is_some()
}

/// Update the block around `line`, running a custom writer if it needs
/// one. Returns false (and says why) if it couldn't.
fn update_at(app: &mut App, line: usize) -> bool {
    let (today, now) = now_local();
    match app.buf().org_dblock_update(line, today, now) {
        Err(e) => {
            app.error(e);
            false
        }
        Ok(Updated::Done(_)) => true,
        Ok(Updated::Custom(k)) => {
            let cmd = format!("{WRITER}{}", k.name);
            let known = app.editor.commands.contains(&cmd);
            if !known {
                app.error(format!("No dynamic block writer: {cmd}"));
                return false;
            }
            // The writer inserts at point: the start of the `#+END:` line.
            app.buf().set_point(Pos::new(k.end, 0));
            let ran = crate::plugins::run_command(app, &cmd)
                || crate::plugins::run_rhai_command(app, &cmd);
            if !ran {
                app.error(format!("Cannot run {cmd}"));
                return false;
            }
            app.buf().org_dblock_mend_end(k.begin);
            true
        }
    }
}

fn update_all(app: &mut App) {
    let blocks = db::all_blocks(app.buf_ref());
    let n = blocks.len();
    for k in blocks.iter().rev() {
        update_at(app, k.begin);
    }
    app.msg(format!(
        "Updated {n} dynamic block{}",
        if n == 1 { "" } else { "s" }
    ));
}

/// Before a save: with `forge.opt.org_update_dblocks_on_save`, rewrite the
/// dynamic blocks of an Org buffer first, as a `before-save-hook` running
/// `org-update-all-dblocks` does. Silent.
pub fn before_save(app: &mut App) {
    if app.buf_ref().major_mode() != forge_core::org::MODE
        || !crate::org_flow::setting_flag(app, "org_update_dblocks_on_save")
    {
        return;
    }
    let blocks = db::all_blocks(app.buf_ref());
    let at = app.buf_ref().point();
    for k in blocks.iter().rev() {
        update_at(app, k.begin);
    }
    if !blocks.is_empty() {
        let max = app.buf_ref().line_count().saturating_sub(1);
        app.buf().set_point(Pos::new(at.line.min(max), at.col));
    }
}

/// Run an Org dynamic-block command. False if `cmd` isn't one.
pub fn command(app: &mut App, cmd: &str) -> bool {
    if !COMMANDS.iter().any(|cd| cd.name() == cmd) {
        return false;
    }
    if !in_org(app) {
        return true;
    }
    let line = app.buf_ref().point().line;
    match cmd {
        "org-dblock-update" if app.has_arg() => update_all(app),
        "org-dblock-update" => {
            let at = app.buf_ref().point();
            if update_at(app, line) {
                let max = app.buf_ref().line_count().saturating_sub(1);
                app.buf().set_point(Pos::new(at.line.min(max), at.col));
            }
        }
        "org-update-all-dblocks" => update_all(app),
        "org-dynamic-block-insert-dblock" => {
            let mut names = vec!["clocktable".to_string(), "columnview".to_string()];
            let mut custom: Vec<String> = app
                .editor
                .commands
                .names()
                .filter_map(|n| n.strip_prefix(WRITER))
                .map(String::from)
                .collect();
            custom.sort();
            names.extend(custom);
            choose(app, "Dynamic block: ", Ask::DblockInsert, names);
        }
        _ => return false,
    }
    true
}

/// The prompt answered in `Ask::DblockInsert`.
pub fn answer(app: &mut App, a: &Ask, input: &str) -> bool {
    if !matches!(a, Ask::DblockInsert) {
        return false;
    }
    let name = input.trim();
    if name.is_empty() || name.contains(char::is_whitespace) {
        app.error("Not a dynamic block name");
        return true;
    }
    let line = app.buf_ref().point().line;
    let at_line = app.buf().org_dblock_insert(line, name);
    update_at(app, at_line);
    app.buf().set_point(Pos::new(at_line, 0));
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::Buffer;

    fn app_with(text: &str) -> App {
        let mut app = App::new(forge_core::Editor::default());
        let mut b = Buffer::new("t.org");
        b.set_contents(text);
        b.set_major_mode(forge_core::org::MODE);
        let id = app.editor.add_buffer(b);
        app.editor.show_buffer(id);
        app
    }

    const EFFORT: &str = "* A\n:PROPERTIES:\n:Effort: 1:00\n:END:\n";

    #[test]
    fn update_and_ctrl_c_rewrite_the_block() {
        let mut app = app_with(&format!(
            "{EFFORT}#+BEGIN: columnview :id global :format \"%ITEM %Effort\"\nold\n#+END:\n"
        ));
        app.buf().set_point(Pos::new(5, 0));
        app.run_command("org-dblock-update");
        let t = app.buf_ref().get_contents();
        assert!(t.contains("| ITEM") && !t.contains("old"), "{t}");
        // C-c C-c on the #+END: line does the same.
        let last = app.buf_ref().line_count() - 2;
        app.buf().set_point(Pos::new(last, 0));
        assert!(at_block(&app));
        app.run_command("org-ctrl-c-ctrl-c");
        assert_eq!(app.buf_ref().get_contents(), t);
    }

    #[test]
    fn dblocks_update_on_save_only_when_asked() {
        let dir = std::env::temp_dir().join(format!("forge-dbsave-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let text = format!("{EFFORT}#+BEGIN: columnview :id global :format \"%ITEM\"\n#+END:\n");
        let run = |on: bool| -> String {
            let file = dir.join(if on { "on.org" } else { "off.org" });
            std::fs::write(&file, &text).unwrap();
            let mut app = App::new(forge_core::Editor::default());
            let (id, _) = app.editor.open_file(&file).unwrap();
            app.editor.show_buffer(id);
            if on {
                let init = dir.join("init.rhai");
                std::fs::write(&init, "forge.opt[\"org_update_dblocks_on_save\"] = true;\n")
                    .unwrap();
                app.init_path = Some(init);
                app.load_init_file();
            }
            crate::commands::save(&mut app);
            std::fs::read_to_string(file).unwrap()
        };
        assert!(
            run(true).contains("| ITEM"),
            "the block was rewritten before it was saved"
        );
        assert!(!run(false).contains("| ITEM"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_all_and_errors() {
        let mut app = app_with(&format!(
            "{EFFORT}#+BEGIN: columnview :id global :format \"%ITEM\"\n#+END:\n#+BEGIN: columnview :id global :format \"%Effort\"\nx\n#+END:\n"
        ));
        app.run_command("org-update-all-dblocks");
        let t = app.buf_ref().get_contents();
        assert!(
            t.contains("| ITEM") && t.contains("| Effort") && !t.contains("\nx\n"),
            "{t}"
        );
        app.buf().set_point(Pos::new(0, 0));
        app.run_command("org-dblock-update");
        assert!(!at_block(&app));
        // Unknown writer.
        let mut app = app_with("#+BEGIN: nothing\n#+END:\n");
        app.run_command("org-dblock-update");
        assert_eq!(app.buf_ref().get_contents(), "#+BEGIN: nothing\n#+END:\n");
    }

    #[test]
    fn insert_asks_then_makes_the_block() {
        let mut app = app_with("* A\n");
        app.buf().set_point(Pos::new(0, 0));
        app.run_command("org-dynamic-block-insert-dblock");
        assert!(app.minibuffer.is_some());
        app.minibuffer = None;
        assert!(answer(&mut app, &Ask::DblockInsert, "clocktable"));
        let t = app.buf_ref().get_contents();
        assert!(t.starts_with("* A\n#+BEGIN: clocktable"), "{t}");
        assert!(t.contains("*Total time*") && t.contains("#+END:"), "{t}");
    }
}
