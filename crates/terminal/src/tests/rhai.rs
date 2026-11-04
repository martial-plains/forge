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
