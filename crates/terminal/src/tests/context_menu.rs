// SPDX-License-Identifier: MIT OR Apache-2.0
//! context_menu tests.

use super::*;

#[test]
fn context_menu_mode_is_off_until_asked_for() {
    let mut app = app_with("one two\n");
    screen(&mut app, 40, 12);
    right_click(&mut app, 2, 0);
    assert!(
        !crate::menu::context_open(&app),
        "off by default, as in Forge"
    );
    press(&mut app, "M-x context-menu-mode RET");
    assert!(crate::menu::context_mode_on(&app));
    screen(&mut app, 40, 12);
    right_click(&mut app, 2, 0);
    assert!(crate::menu::context_open(&app));
    press(&mut app, "ESC");
    assert!(!crate::menu::context_open(&app));
    press(&mut app, "M-x context-menu-mode RET");
    assert!(!crate::menu::context_mode_on(&app));
}

#[test]
fn right_click_menu_offers_what_fits_the_context() {
    let mut app = app_with("one two\n");
    press(&mut app, "M-x context-menu-mode RET");
    screen(&mut app, 40, 12);
    // Nothing to undo, no region, empty kill ring: only Select.
    right_click(&mut app, 2, 0);
    let s = screen(&mut app, 40, 12);
    assert!(s.contains("Select"), "{s}");
    assert!(
        !s.contains("Undo") && !s.contains("Cut") && !s.contains("Paste"),
        "{s}"
    );
    press(&mut app, "ESC");

    // After an edit, Undo is offered; after a kill, Paste.
    press(&mut app, "x");
    app.editor.kill_ring.push("pasted".into());
    screen(&mut app, 40, 12);
    right_click(&mut app, 2, 0);
    let s = screen(&mut app, 40, 12);
    assert!(s.contains("Undo"), "{s}");
    assert!(s.contains("Paste"), "{s}");
    assert!(!s.contains("Cut"), "no region, no Cut: {s}");
}

#[test]
fn clicking_paste_inserts_the_kill_where_you_clicked() {
    let mut app = app_with("one two\n");
    press(&mut app, "M-x context-menu-mode RET");
    app.editor.kill_ring.push("XY".into());
    screen(&mut app, 40, 12);
    // Right-click on the 't' of "two" (column 4), then pick Paste.
    right_click(&mut app, 4, 0);
    assert_eq!(point(&app).col, 4, "the click moved point");
    let v = crate::menu::context_view(&app).expect("open");
    let popup = v.popup.expect("panel");
    let paste = popup
        .rows
        .iter()
        .position(|r| r.text.contains("Paste"))
        .expect("Paste row");
    left_click(&mut app, popup.column + 2, popup.row + paste as u16);
    assert!(!crate::menu::context_open(&app), "choosing closes the menu");
    assert_eq!(app.buf_ref().line(0), "one XYtwo");
}

#[test]
fn region_commands_show_when_you_click_inside_the_region() {
    let mut app = app_with("one two\n");
    press(&mut app, "M-x context-menu-mode RET");
    screen(&mut app, 40, 12);
    press(&mut app, "C-SPC M-f");
    assert!(app.buf_ref().active_region().is_some());
    screen(&mut app, 40, 12);
    right_click(&mut app, 1, 0);
    let s = screen(&mut app, 40, 12);
    assert!(
        s.contains("Cut") && s.contains("Copy") && s.contains("Clear"),
        "{s}"
    );
    assert!(
        app.buf_ref().active_region().is_some(),
        "a click inside keeps it"
    );
    press(&mut app, "ESC");
    // A click outside drops the region.
    right_click(&mut app, 6, 0);
    assert!(app.buf_ref().active_region().is_none());
}

#[test]
fn the_context_menu_has_the_menu_keys_and_s_f10_opens_it() {
    let mut app = app_with("one two\n");
    press(&mut app, "M-x context-menu-mode RET");
    press(&mut app, "x");
    screen(&mut app, 40, 12);
    press(&mut app, "S-<f10>");
    assert!(crate::menu::context_open(&app));
    // Undo is the first entry; RET runs it.
    press(&mut app, "RET");
    assert!(!crate::menu::context_open(&app));
    assert_eq!(app.buf_ref().line(0), "one two", "undo ran");
}

#[test]
fn select_line_and_string_mark_what_was_clicked() {
    let mut app = app_with("say \"hi there\" ok\nnext\n");
    press(&mut app, "M-x context-menu-mode RET");
    screen(&mut app, 40, 12);
    right_click(&mut app, 7, 0);
    // Open the Select submenu and choose String.
    let v = crate::menu::context_view(&app).unwrap();
    let popup = v.popup.unwrap();
    let sel = popup
        .rows
        .iter()
        .position(|r| r.text.contains("Select"))
        .unwrap();
    left_click(&mut app, popup.column + 2, popup.row + sel as u16);
    let v = crate::menu::context_view(&app).unwrap();
    let sub = v.subpopup.expect("submenu");
    let string = sub
        .rows
        .iter()
        .position(|r| r.text.contains("String"))
        .expect("String offered inside a string");
    left_click(&mut app, sub.column + 2, sub.row + string as u16);
    let (a, z) = app.buf_ref().region().unwrap();
    assert_eq!(
        (a.col, z.col),
        (4, 14),
        "the quoted string, quotes included"
    );
}

#[test]
fn the_context_menu_stays_on_screen_near_the_corner() {
    let mut app = app_with("1\n2\n3\n4\n5\n6\n7\n8\n");
    press(&mut app, "M-x context-menu-mode RET");
    press(&mut app, "x");
    app.editor.kill_ring.push("a".into());
    screen(&mut app, 30, 8);
    right_click(&mut app, 29, 4);
    let v = crate::menu::context_view(&app).unwrap();
    let p = v.popup.unwrap();
    let w = forge_core::menu::panel_width(&p.rows);
    assert!(p.column + w <= 30, "slid left: {} + {w}", p.column);
    assert!(p.row as usize + p.rows.len() <= app.editor.windows.frame().height as usize);
}

#[test]
fn the_buffers_menu_lists_open_buffers_and_picks_one() {
    let mut app = app_with("one\n");
    press(&mut app, "M-x menu-bar-mode RET");
    without_messages(&mut app);
    let other = app.editor.add_buffer(forge_core::Buffer::new("other.txt"));
    let _ = other;
    screen(&mut app, 60, 24);
    // Buffers is the fourth title (column 21).
    left_click(&mut app, 23, 0);
    let s = screen(&mut app, 60, 24);
    assert!(s.contains("other.txt"), "{s}");
    // Down to other.txt (the current buffer is first, then *Messages*), run it.
    press(&mut app, "C-n");
    press(&mut app, "C-n");
    press(&mut app, "RET");
    assert_eq!(app.buf_ref().name(), "other.txt");
}

#[test]
fn right_click_menu_shows_a_mode_menu_a_plugin_defined() {
    let mut app = app_with("one\n");
    press(&mut app, "M-x context-menu-mode RET");
    app.editor
        .menus
        .define(
            "Fundamental",
            vec![forge_core::menu::Item::new("Do It", "undo")],
            "p",
        )
        .unwrap();
    app.editor.context_menu_functions.push("global".into());
    screen(&mut app, 60, 20);
    right_click(&mut app, 1, 0);
    let s = screen(&mut app, 60, 20);
    assert!(s.contains("Fundamental"), "{s}");
}

// ------------------------------------------------------------- fuller help

#[test]
fn describe_function_has_keys_menu_docs_and_related_commands() {
    let mut app = app_with("");
    press(&mut app, "C-h f dired-mark RET");
    let h = help_text(&app);
    for want in [
        "Key bindings:",
        "dired-mode-map",
        "Documentation:",
        "Related commands:",
        "dired-create-directory",
    ] {
        assert!(h.contains(want), "{want} in {h}");
    }
}

#[test]
fn help_buffers_are_coloured() {
    use forge_core::display::Face;
    let mut app = app_with("");
    press(&mut app, "C-h f save-buffer RET");
    let faces = app.buf().syntax_faces(0..400);
    assert!(
        faces.iter().any(|f| f.is_some()),
        "something is highlighted"
    );
    let text = help_text(&app);
    let at = text.find("Key bindings:").unwrap();
    assert_eq!(
        faces[at],
        Some(forge_core::syntax::SyntaxFace::Keyword),
        "headings"
    );
    let _ = Face::Default;
}

#[test]
fn c_h_mode_in_dired_explains_the_keys() {
    let dir = std::env::temp_dir().join(format!("forge-dhelp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "x").unwrap();
    let mut app = app_with("");
    crate::dired::open(&mut app, &dir);
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("? for a key summary")
    );
    press(&mut app, "?");
    assert!(
        app.message.as_deref().unwrap_or("").contains("d flag"),
        "{:?}",
        app.message
    );
    press(&mut app, "C-h m");
    let h = help_text(&app);
    assert!(h.contains("Dired lists a directory"), "{h}");
    assert!(h.contains("dired-flag-file-deletion"), "{h}");
    assert!(h.contains("In Dired: flag"), "doc column: {h}");
}

#[test]
fn apropos_help_for_help_and_lossage() {
    let mut app = app_with("");
    press(&mut app, "C-h a bookmark RET");
    let h = help_text(&app);
    assert!(h.contains("bookmark-set"), "{h}");
    press(&mut app, "C-h ?");
    assert!(help_text(&app).contains("describe-key"));
    press(&mut app, "a b");
    press(&mut app, "C-h l");
    assert!(help_text(&app).contains("a b"), "{}", help_text(&app));
}

#[test]
fn goto_char_and_insert_file() {
    let dir = std::env::temp_dir().join(format!("forge-ins-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("in.txt");
    std::fs::write(&f, "XY").unwrap();
    let mut app = app_with("hello\n");
    press(&mut app, "M-x goto-char RET");
    answer(&mut app, "3");
    assert_eq!(point(&app).col, 2);
    press(&mut app, "M-x insert-file RET");
    answer(&mut app, &f.display().to_string());
    assert_eq!(app.buf_ref().line(0), "heXYllo");
    assert_eq!(point(&app).col, 2, "point stays before the text");
}

#[test]
fn directory_commands_make_copy_list_and_delete() {
    let dir = std::env::temp_dir().join(format!("forge-dircmd-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = app_with("");
    press(&mut app, "M-x make-directory RET");
    answer(&mut app, &dir.join("a/b").display().to_string());
    assert!(dir.join("a/b").is_dir(), "parents are made too");
    press(&mut app, "M-x mkdir RET");
    answer(&mut app, &dir.join("c").display().to_string());
    assert!(dir.join("c").is_dir());
    press(&mut app, "M-x make-empty-file RET");
    answer(&mut app, &dir.join("a/b/f.txt").display().to_string());
    assert!(dir.join("a/b/f.txt").is_file());
    press(&mut app, "M-x copy-directory RET");
    answer(&mut app, &dir.join("a").display().to_string());
    answer(&mut app, &dir.join("c").display().to_string());
    assert!(
        dir.join("c/a/b/f.txt").is_file(),
        "copied into the existing directory"
    );
    press(&mut app, "M-x insert-directory RET");
    answer(&mut app, &dir.display().to_string());
    let text = format!(
        "{}{}{}",
        app.buf_ref().line(1),
        app.buf_ref().line(2),
        app.buf_ref().line(3)
    );
    assert!(text.contains("total") || text.contains('a'), "{text}");
    press(&mut app, "M-x delete-directory RET");
    answer(&mut app, &dir.join("c").display().to_string());
    answer(&mut app, "yes");
    assert!(!dir.join("c").exists(), "a full directory goes after a yes");
    let _ = std::fs::remove_dir_all(&dir);
}
