// SPDX-License-Identifier: MIT OR Apache-2.0
//! rhai tests.

use super::*;

#[test]
fn rhai_event_handlers() {
    let dir = std::env::temp_dir().join(format!("forge-rhai-on-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("init.rhai"),
        r#"
        forge.on("buffer-changed", |b| print("changed " + b.name()));
        forge.on("buffer-opened", |b| b.set_option("line-numbers", "absolute"));
        "#,
    )
    .unwrap();
    let mut app = app_with("");
    app.init_path = Some(dir.join("init.rhai"));
    app.load_init_file();
    press(&mut app, "a");
    app.on_idle();
    assert!(
        messages_buffer(&app).contains("changed *scratch*"),
        "{}",
        messages_buffer(&app)
    );
    press(&mut app, "C-x b other RET");
    assert_eq!(
        app.buf_ref().option("line-numbers").as_deref(),
        Some("absolute")
    );
    std::fs::write(dir.join("init.rhai"), "forge.on(\"nope\", || ());").unwrap();
    app.load_init_file();
    assert!(
        app.message.as_deref().unwrap().contains("unknown event"),
        "{:?}",
        app.message
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// `forge.opt["menu_bar"] = true` turns the menu bar on for the whole
/// session, the way `menu-bar-mode` is global: the bar frames the
/// window rather than belonging to a buffer, so opening a file keeps it.
#[test]
fn init_rhai_can_turn_the_menu_bar_on() {
    let dir = std::env::temp_dir().join(format!("forge-menu-bar-opt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let init = dir.join("init.rhai");
    std::fs::write(&init, "forge.opt[\"menu_bar\"] = true;").unwrap();
    let mut app = app_with("hi\n");
    app.init_path = Some(init);
    app.load_init_file();
    assert!(
        forge_editor::menu::mode_on(&app),
        "the option turns menu-bar-mode on"
    );
    assert!(
        forge_editor::menu::view(&app).is_some(),
        "and the bar is drawn"
    );
    // A file opened afterwards still has it: the mode is global, not
    // buffer-local.
    let id = app.editor.create_buffer("*other*");
    app.switch_to(id);
    assert!(
        forge_editor::menu::mode_on(&app),
        "still on in the new buffer"
    );
    assert!(forge_editor::menu::view(&app).is_some());
    std::fs::write(
        app.init_path.clone().unwrap(),
        "forge.opt[\"menu_bar\"] = false;",
    )
    .unwrap();
    app.load_init_file();
    assert!(!forge_editor::menu::mode_on(&app));
    assert!(forge_editor::menu::view(&app).is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// `forge.opt["mark_ring_max"]` says how many marks the ring keeps, and
/// `forge.opt["set_mark_command_repeat_pop"]` makes a plain C-SPC pop the
/// ring again after a C-u C-SPC.
#[test]
fn init_rhai_can_set_the_mark_ring_options() {
    let dir = std::env::temp_dir().join(format!("forge-mark-ring-opt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let init = dir.join("init.rhai");
    std::fs::write(
        &init,
        "forge.opt[\"mark_ring_max\"] = 3;\nforge.opt[\"set_mark_command_repeat_pop\"] = true;",
    )
    .unwrap();
    let mut app = app_with("one two three four");
    app.init_path = Some(init);
    app.load_init_file();

    // Four marks set: the ring keeps three of them.
    for _ in 0..4 {
        press(&mut app, "C-SPC C-f C-f");
    }
    let ring: Vec<_> = app
        .buf_ref()
        .mark_ring()
        .into_iter()
        .map(|(_, p)| p)
        .collect();
    assert_eq!(ring.len(), 3, "the ring keeps what mark_ring_max says");
    assert_eq!(app.buf_ref().mark(), Some(forge_core::Pos::new(0, 6)));

    // C-u C-SPC jumps to the mark and pops the ring; a plain C-SPC right
    // after pops again.
    press(&mut app, "C-u C-SPC");
    assert_eq!(app.buf_ref().point(), forge_core::Pos::new(0, 6));
    assert_eq!(app.buf_ref().mark(), Some(forge_core::Pos::new(0, 4)));
    press(&mut app, "C-SPC");
    assert_eq!(app.buf_ref().point(), forge_core::Pos::new(0, 4));
    assert_eq!(app.buf_ref().mark(), Some(forge_core::Pos::new(0, 2)));

    // The setting is global, so put it back for the other tests.
    forge_core::buffer::set_mark_ring_max(16);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn rhai_can_replace_the_open_prompt_input() {
    let (mut app, dir) = app_with_init(
        r#"
        forge.command.create("set-hello", || {
            forge.minibuffer.set_input("hello");
        });
        forge.keymap.set("<f5>", "set-hello", #{ prompt: "find-file" });
        "#,
    );
    press(&mut app, "C-x C-f");
    assert!(app.minibuffer.is_some());
    press(&mut app, "<f5>");
    assert_eq!(
        app.minibuffer.as_ref().map(|m| m.input.as_str()),
        Some("hello")
    );
    assert_eq!(app.editor.prompt_input, "hello");
    press(&mut app, "C-g");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn rhai_can_group_and_lay_out_the_prompt_list() {
    let (mut app, dir) = app_with_init(
        r#"
        forge.completion.set_layout("vertical", 10);
        forge.completion.set_group_format("-- %s --");
        forge.completion.group("candidate", |c| {
            let out = [];
            for name in c {
                out.push(if name == "gamma" { "B" } else { "A" });
            }
            out
        });
        forge.completion.set_row_labels(["one", "two", "three"]);
        forge.command.create("pick", || {
            forge.ui.select(["alpha", "beta", "gamma"], #{ prompt: "Pick: " }, |choice| ());
        });
        forge.keymap.set("<f5>", "pick");
        "#,
    );
    press(&mut app, "M-x pick RET");
    let (rows, _) = forge_editor::minibuffer::vertical_view(&mut app).expect("the list is up");
    let rows: Vec<(String, bool, String)> = rows
        .into_iter()
        .map(|r| (r.text, r.header, r.label))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("-- A --".to_string(), true, String::new()),
            ("alpha".to_string(), false, "one".to_string()),
            ("beta".to_string(), false, "two".to_string()),
            ("-- B --".to_string(), true, String::new()),
            ("gamma".to_string(), false, "three".to_string()),
        ]
    );
    press(&mut app, "C-g");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn rhai_can_suspend_resume_and_jump_in_the_prompt_list() {
    let (mut app, dir) = app_with_init(
        r#"
        forge.completion.set_layout("vertical", 5);
        forge.command.create("fill", || {
            if forge.minibuffer.active() {
                forge.minibuffer.set_input("");
            }
        });
        forge.minibuffer.on_open("fill");
        forge.command.create("pick", || {
            forge.ui.select(["alpha", "beta"], #{ prompt: "Pick: " }, |choice| ());
        });
        forge.command.create("hold", || forge.minibuffer.suspend());
        forge.command.create("back", || forge.completion.resume(0));
        forge.command.create("second", || forge.completion.jump_to_row(1, false));
        forge.keymap.set("<f5>", "pick");
        forge.keymap.set("<f6>", "hold", #{ prompt: "completion-list" });
        forge.keymap.set("<f7>", "back");
        forge.keymap.set("<f8>", "second", #{ prompt: "completion-list" });
        "#,
    );
    press(&mut app, "M-x pick RET");
    assert_eq!(
        app.minibuffer.as_ref().map(|m| m.input.as_str()),
        Some(""),
        "minibuffer.on-open ran after the prompt opened"
    );
    press(&mut app, "<f8>");
    assert_eq!(
        app.minibuffer
            .as_ref()
            .and_then(|m| m.list.as_ref())
            .map(|l| l.selected),
        Some(1)
    );
    press(&mut app, "<f6>");
    assert!(app.minibuffer.is_none(), "the prompt was suspended");
    assert_eq!(app.suspended_prompts.len(), 1);
    let sessions = &app.editor.completion_sessions;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].prompt, "Pick: ");
    assert_eq!(sessions[0].total, 2);
    press(&mut app, "<f7>");
    assert!(
        app.minibuffer.is_some(),
        "completion.resume brought the prompt back"
    );
    assert!(app.suspended_prompts.is_empty());
    press(&mut app, "C-g");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn rhai_can_choose_each_prompt_list_layout() {
    let (mut app, dir) = app_with_init(
        r#"
        forge.completion.set_layout("vertical", 4);
        forge.command.create("pick", || {
            forge.ui.select(["alpha", "beta", "gamma", "delta"],
                #{ prompt: "Pick: " }, |choice| ());
        });
        forge.command.create("flat", || forge.completion.set_layout("flat", 4));
        forge.command.create("grid", || forge.completion.set_layout("grid", 2));
        forge.command.create("reverse", || forge.completion.set_layout("reverse", 2));
        forge.command.create("unobtrusive", || {
            forge.completion.set_layout("unobtrusive", 4)
        });
        forge.command.create("buffer", || forge.completion.set_layout("buffer", 4));
        forge.keymap.set("<f5>", "pick");
        forge.keymap.set("<f6>", "flat", #{ prompt: "completion-list" });
        forge.keymap.set("<f7>", "grid", #{ prompt: "completion-list" });
        forge.keymap.set("<f8>", "reverse", #{ prompt: "completion-list" });
        forge.keymap.set("<f9>", "unobtrusive", #{ prompt: "completion-list" });
        forge.keymap.set("<f11>", "buffer", #{ prompt: "completion-list" });
        "#,
    );
    fn view(app: &mut App) -> Vec<(String, bool)> {
        let (rows, _) = forge_editor::minibuffer::vertical_view(app).expect("the list is up");
        rows.into_iter().map(|r| (r.text, r.selected)).collect()
    }

    press(&mut app, "M-x pick RET");
    assert_eq!(
        view(&mut app),
        vec![
            ("alpha".to_string(), true),
            ("beta".to_string(), false),
            ("gamma".to_string(), false),
            ("delta".to_string(), false),
        ]
    );

    // Flat: every candidate on one row.
    press(&mut app, "<f6>");
    assert_eq!(
        view(&mut app),
        vec![("alpha  beta  gamma  delta".to_string(), true)]
    );

    // Grid: fill two rows, as many candidates per row as fit.
    press(&mut app, "<f7>");
    assert_eq!(
        view(&mut app),
        vec![
            ("alpha  beta".to_string(), true),
            ("gamma  delta".to_string(), false),
        ]
    );

    // Reverse: the selected row sits last, near the prompt.
    press(&mut app, "<f8>");
    assert_eq!(
        view(&mut app),
        vec![("beta".to_string(), false), ("alpha".to_string(), true),]
    );

    // Unobtrusive: only the selection.
    press(&mut app, "<f9>");
    assert_eq!(view(&mut app), vec![("alpha".to_string(), true)]);

    // Buffer: no rows under the prompt; the list is in *Completions*.
    press(&mut app, "<f11>");
    let (rows, count) = forge_editor::minibuffer::vertical_view(&mut app).expect("the list is up");
    assert!(
        rows.is_empty(),
        "the buffer layout draws no rows under the prompt"
    );
    assert_eq!(count, "1/4");
    let shown = buffer_text(&app, "*Completions*");
    for name in ["alpha", "beta", "gamma", "delta"] {
        assert!(shown.contains(name), "{shown:?}");
    }
    press(&mut app, "C-g");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn rhai_can_copy_text_to_the_kill_ring() {
    let (mut app, dir) = app_with_init(
        r#"
        forge.command.create("copy-it", || forge.kill("candidate text"));
        forge.keymap.set("C-c y", "copy-it");
        "#,
    );
    press(&mut app, "C-c y");
    assert_eq!(app.editor.kill_ring.current(), Some("candidate text"));
    let clip = app
        .clipboard_provider
        .get_text(forge_core::clipboard::SelectionTarget::Clipboard);
    assert_eq!(clip.as_deref(), Some("candidate text"));
    std::fs::remove_dir_all(&dir).unwrap();
}
