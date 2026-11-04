// SPDX-License-Identifier: MIT OR Apache-2.0
//! hooks tests.

use super::*;

#[test]
fn bindings_written_with_capitals_answer_to_shift() {
    // C-h I, as a terminal sends Shift+I: 'i' with Shift (kitty), or 'I'
    // with Shift (the rest).
    for shifted in [KeyCode::Char('i'), KeyCode::Char('I')] {
        let mut app = app_with("");
        press(&mut app, "C-h");
        app.handle_key(KeyEvent::new(shifted, KeyModifiers::SHIFT));
        let mb = app.minibuffer.as_ref().expect("asks which input method");
        assert!(
            mb.prompt.starts_with("Describe input method"),
            "{}",
            mb.prompt
        );
    }
    // Typing a capital still types it.
    let mut app = app_with("");
    app.handle_key(KeyEvent::new(KeyCode::Char('I'), KeyModifiers::SHIFT));
    assert_eq!(text(&app), "I");
}

// --------------------------------------------------- init.rhai mode hooks

#[test]
fn mode_hooks_run_in_each_buffer_of_the_mode_once_the_command_exists() {
    let (mut app, dir) = app_with_init(
        r#"
        forge.mode.define_minor("brackets-mode");
        forge.mode.hook("prog-mode", "toggle-brackets");
        forge.mode.hook("rust-mode", |n| {
          forge.buffer.current().insert("rust:" + n.to_string() + " ");
        });
        forge.command.run("toggle-brackets", 1);
        "#,
    );
    let rs = dir.join("a.rs");
    let txt = dir.join("b.txt");
    std::fs::write(&rs, "fn main() {}\n").unwrap();
    std::fs::write(&txt, "notes\n").unwrap();
    for f in [&rs, &txt] {
        let (id, _) = app.editor.open_file(f.to_str().unwrap()).unwrap();
        app.switch_to(id);
    }
    // toggle-brackets doesn't exist when init.rhai runs (a plugin defines
    // it, and plugins load after init.rhai): the hook and init.rhai's command
    // waited for it, and run now.
    let rhai = app.rhai.as_ref().unwrap();
    let out = rhai.load_str(
        &mut app.editor,
        "plugin",
        r#"forge.command.create("toggle-brackets", |n| {
             forge.buffer.current().insert("[" + n.to_string() + "]");
           });"#,
    );
    crate::plugins::apply(&mut app, out);
    app.run_mode_hooks();
    let rs_id = app.editor.find_buffer("a.rs").unwrap();
    let txt_id = app.editor.find_buffer("b.txt").unwrap();
    // Hooks run with prefix 1 (a minor mode is turned on, not toggled),
    // in the buffers of the mode only, prog-mode's before rust-mode's, and the buffer on screen
    // stays.
    assert_eq!(
        app.editor.state.get_buffer(rs_id).unwrap().get_contents(),
        "[1]rust:1 fn main() {}\n"
    );
    // forge.command.run(..., 1) ran once, in the current buffer.
    assert_eq!(
        app.editor.state.get_buffer(txt_id).unwrap().get_contents(),
        "[1]notes\n"
    );
    assert_eq!(app.active_id(), txt_id);
    // Once only, until the buffer's mode changes.
    app.run_mode_hooks();
    assert_eq!(
        app.editor.state.get_buffer(rs_id).unwrap().get_contents(),
        "[1]rust:1 fn main() {}\n"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn hooks_for_commands_nobody_defines_say_so() {
    let (mut app, dir) = app_with_init(
        r#"forge.mode.hook("prog-mode", "no-such-mode");
                                          forge.command.run("no-such-command");"#,
    );
    let rs = dir.join("c.rs");
    std::fs::write(&rs, "fn x() {}\n").unwrap();
    let (id, _) = app.editor.open_file(rs.to_str().unwrap()).unwrap();
    app.switch_to(id);
    app.run_mode_hooks();
    let messages = app
        .editor
        .find_buffer("*Messages*")
        .map(|m| app.editor.state.get_buffer(m).unwrap().get_contents())
        .unwrap_or_default();
    assert!(
        messages.contains("no command named no-such-command"),
        "{messages}"
    );
    assert!(
        messages.contains("forge.mode.hook(\"prog-mode\"): no command named no-such-mode"),
        "{messages}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

// ------------------------------------------------------------ context menu
