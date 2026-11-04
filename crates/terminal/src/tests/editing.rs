// SPDX-License-Identifier: MIT OR Apache-2.0
//! editing tests.

use super::*;

#[test]
fn typing_and_basic_motion() {
    let mut app = app_with("");
    press(&mut app, "hello RET world C-a C-f C-f");
    assert_eq!(text(&app), "hello\nworld");
    assert_eq!(point(&app), Pos::new(1, 2));
    press(&mut app, "C-p C-e");
    assert_eq!(point(&app), Pos::new(0, 5));
}

#[test]
fn esc_is_meta() {
    let mut app = app_with("one two");
    press(&mut app, "ESC f");
    assert_eq!(point(&app), Pos::new(0, 3));
    press(&mut app, "M-f M-b");
    assert_eq!(point(&app), Pos::new(0, 4));
}

#[test]
fn repeated_kill_line_yanks_back_as_one() {
    let mut app = app_with("aaa\nbbb\nccc");
    press(&mut app, "C-k");
    assert_eq!(text(&app), "\nbbb\nccc");
    press(&mut app, "C-k C-k C-k");
    assert_eq!(text(&app), "ccc");
    press(&mut app, "C-y");
    assert_eq!(text(&app), "aaa\nbbb\nccc");
}

#[test]
fn region_kill_copy_and_yank_pop() {
    let mut app = app_with("alpha beta gamma");
    press(&mut app, "C-SPC M-f M-w");
    assert_eq!(app.editor.kill_ring.current(), Some("alpha"));
    press(&mut app, "C-SPC M-f C-w");
    assert_eq!(text(&app), "alpha gamma");
    press(&mut app, "C-e C-y");
    assert_eq!(text(&app), "alpha gamma beta");
    press(&mut app, "M-y");
    assert_eq!(text(&app), "alpha gammaalpha");
}

#[test]
fn undo_and_redo() {
    let mut app = app_with("");
    press(&mut app, "abc RET def");
    press(&mut app, "C-/");
    assert_eq!(text(&app), "abc\n");
    press(&mut app, "C-x u");
    assert_eq!(text(&app), "abc");
    press(&mut app, "C-M-_");
    assert_eq!(text(&app), "abc\n");
}

#[test]
fn prefix_key_undefined_and_cancel() {
    let mut app = app_with("");
    press(&mut app, "C-x");
    assert_eq!(app.message.as_deref(), Some("C-x-"));
    press(&mut app, "C-g");
    assert!(app.pending.is_empty());
    press(&mut app, "C-x z");
    assert_eq!(app.message.as_deref(), Some("C-x z is undefined"));
}

#[test]
fn isearch_finds_repeats_and_aborts() {
    let mut app = app_with("foo bar foo baz");
    press(&mut app, "C-s foo");
    assert_eq!(point(&app), Pos::new(0, 3));
    press(&mut app, "C-s");
    assert_eq!(point(&app), Pos::new(0, 11));
    press(&mut app, "RET");
    assert!(app.minibuffer.is_none());
    assert_eq!(app.buf_ref().mark(), Some(Pos::new(0, 0)));

    press(&mut app, "C-r bar C-g");
    assert_eq!(point(&app), Pos::new(0, 11));
}

#[test]
fn isearch_exits_on_other_command() {
    let mut app = app_with("xx needle yy");
    press(&mut app, "C-s nee C-e");
    assert!(app.minibuffer.is_none());
    assert_eq!(point(&app), Pos::new(0, 12));
}

#[test]
fn m_x_with_completion() {
    let mut app = app_with("abc");
    press(&mut app, "M-x forward-w TAB RET");
    assert_eq!(point(&app), Pos::new(0, 3));
    assert!(app.message.as_deref().unwrap().contains("M-f"));
}

#[test]
fn word_case_commands() {
    let mut app = app_with("hello world again");
    press(&mut app, "M-u M-c M-l");
    assert_eq!(text(&app), "HELLO World again");
}

#[test]
fn find_file_save_and_quit() {
    let dir = std::env::temp_dir().join(format!("forge-ui-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("note.txt");
    let mut app = app_with("");

    press(&mut app, "C-x C-f");
    type_path(&mut app, &path);
    press(&mut app, "RET");
    assert_eq!(app.buf_ref().name(), "note.txt");
    assert_eq!(app.message.as_deref(), Some("(New file)"));

    press(&mut app, "hi C-x C-c");
    assert!(
        app.minibuffer.is_some(),
        "asks before quitting with unsaved changes"
    );
    press(&mut app, "n C-x C-s");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "hi");
    press(&mut app, "C-x C-c");
    assert!(app.quit);

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn find_file_uses_visiting_buffers_directory_for_relative_paths_and_completion() {
    let dir = std::env::temp_dir().join(format!("forge-find-relative-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("subdir")).unwrap();
    let current = dir.join("current.txt");
    let sibling = dir.join("sibling.txt");
    let nested = dir.join("subdir/nested.txt");
    std::fs::write(&current, "current").unwrap();
    std::fs::write(&sibling, "sibling").unwrap();
    std::fs::write(&nested, "nested").unwrap();

    let mut app = app_with("");
    let (id, _) = app.editor.open_file(&current).unwrap();
    app.switch_to(id);

    press(&mut app, "C-x C-f sib TAB RET");
    assert_eq!(
        app.buf_ref().path(),
        Some(sibling.canonicalize().unwrap().as_path())
    );
    assert_eq!(text(&app), "sibling");

    press(&mut app, "C-x C-f subdir/nested.txt RET");
    assert_eq!(
        app.buf_ref().path(),
        Some(nested.canonicalize().unwrap().as_path())
    );
    assert_eq!(text(&app), "nested");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn switch_buffers() {
    let mut app = app_with("scratch text");
    press(&mut app, "C-x b other RET");
    assert_eq!(app.buf_ref().name(), "other");
    press(&mut app, "C-x b RET");
    assert_eq!(app.buf_ref().name(), "*scratch*");
    press(&mut app, "C-x k other RET");
    assert!(app.editor.find_buffer("other").is_none());
}

#[test]
fn renders_without_panicking() {
    let mut app = app_with(&"a\tb é 漢字\n".repeat(200));
    let mut term = Terminal::new(TestBackend::new(40, 12)).unwrap();
    press(&mut app, "C-SPC C-v C-v M-> C-l C-s b C-h b");
    term.draw(|f| {
        ui::render(f, &mut app);
    })
    .unwrap();
    let screen = format!("{:?}", term.backend().buffer());
    assert!(screen.contains("*Help*"));
}

#[test]
fn pinned_pictures_come_back_as_placements() {
    let dir = std::env::temp_dir().join(format!("forge-ui-gfx-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("p.png");
    std::fs::write(&path, b"fake").unwrap();
    let mut app = app_with("one\ntwo\nthree\n");
    // Pin a picture to buffer line 1 (point stays on line 0).
    app.buf().inline_images.insert(1, path.clone());
    forge_core::org_display::set_inline_image_rows(8);
    let mut term = Terminal::new(TestBackend::new(40, 12)).unwrap();
    let mut slots = Vec::new();
    term.draw(|f| {
        slots = ui::render(f, &mut app);
    })
    .unwrap();
    forge_core::org_display::set_inline_image_rows(0);
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0].path, path.to_string_lossy());
    assert!(slots[0].h >= 1, "{:?}", slots[0]);
    // Rows off: no placements, placeholders instead.
    let mut slots = Vec::new();
    term.draw(|f| {
        slots = ui::render(f, &mut app);
    })
    .unwrap();
    assert!(slots.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------- Phase 1: keymaps
