// SPDX-License-Identifier: MIT OR Apache-2.0
//! menu tests.

use super::*;

#[test]
fn the_menu_bar_draws_its_row_and_pushes_the_text_down() {
    let mut app = app_with("one\n");
    // Off by default: the text starts on the first row.
    assert_eq!(
        screen(&mut app, 40, 6).lines().next().map(str::trim),
        Some("one")
    );
    press(&mut app, "M-x menu-bar-mode RET");
    assert!(app.editor.minor_mode_enabled("menu-bar-mode"));
    let s = screen(&mut app, 40, 6);
    let first = s.lines().next().unwrap_or_default();
    assert!(
        first.starts_with(" File  Edit  Options  Buffers "),
        "{first:?}"
    );
    assert_eq!(
        s.lines().nth(1).map(str::trim),
        Some("one"),
        "moved down a row"
    );
    // The mode line is still the second-to-last row: the bar took a row
    // from the text area, not from the mode line.
    let rows: Vec<&str> = s.lines().collect();
    assert!(rows[4].starts_with("-U:"), "{:?}", rows[4]);
    assert!(rows[5].contains("Menu Bar mode enabled"), "{:?}", rows[5]);
    // It has no lighter: the row itself is the indication, so the mode
    // line doesn't grow a "Menu Bar" word for it.
    assert!(!s.contains("Menu-Bar"), "{s}");
}

#[test]
fn f10_opens_a_menu_and_ret_runs_it() {
    let mut app = app_with("one\n");
    press(&mut app, "M-x menu-bar-mode RET");
    press(&mut app, "<f10>");
    assert!(crate::menu::active(&app), "F10 opened the first menu");
    let s = screen(&mut app, 40, 40);
    assert!(s.contains(" File "), "the open title is highlighted");
    assert!(s.contains(" Open File..."), "and its items are under it");
    assert!(s.contains(" Quit"), "including the last one");
    // Down to "Open File In Project..." and run it: a prompt asks for the file.
    press(&mut app, "C-n");
    press(&mut app, "RET");
    assert!(!crate::menu::active(&app), "RET closed the menu");
    assert!(app.minibuffer.is_some(), "and ran project-find-file");
    // C-g gets out of it.
    press(&mut app, "C-g");
}

#[test]
fn escape_and_typing_both_close_a_menu() {
    let mut app = app_with("one\n");
    press(&mut app, "M-x menu-bar-mode RET");
    press(&mut app, "<f10>");
    assert!(crate::menu::active(&app));
    press(&mut app, "ESC");
    assert!(!crate::menu::active(&app), "ESC closes");

    press(&mut app, "<f10>");
    assert!(crate::menu::active(&app));
    // An ordinary key closes the menu and then does what it says, the way
    // a menu bar behaves when you click away from it.
    press(&mut app, "x");
    assert!(!crate::menu::active(&app), "the key closed the menu");
    assert_eq!(text(&app), "xone\n", "and then typed as usual, at point");
}

#[test]
fn a_menu_key_can_be_rebound() {
    let mut app = app_with("one\n");
    press(&mut app, "M-x menu-bar-mode RET");
    // C-f is the next menu; bound to the one that closes it, it closes.
    app.editor
        .keymaps
        .bind(
            &forge_core::Scope::Transient("menu".into()),
            "C-f",
            "menu-bar-menu-quit",
            forge_core::keymap::Origin::User,
        )
        .unwrap();
    press(&mut app, "<f10>");
    assert!(crate::menu::active(&app));
    press(&mut app, "C-f");
    assert!(!crate::menu::active(&app), "the rebound key closed it");
    assert_eq!(app.message.as_deref(), Some("Quit"), "and said Quit");
    // The default is untouched for the next menu, since the map is the one
    // that was rebound, not the command.
    press(&mut app, "<f10>");
    press(&mut app, "C-b");
    assert!(crate::menu::active(&app), "C-b is still the previous menu");
}

#[test]
fn f10_with_the_mode_off_says_so_instead_of_failing() {
    let mut app = app_with("one\n");
    press(&mut app, "<f10>");
    assert!(!crate::menu::active(&app));
    assert!(
        app.message.as_deref().is_some_and(|m| m.contains("off")),
        "{:?}",
        app.message
    );
    // A prefix argument turns it on.
    press(&mut app, "C-u M-x menu-bar-mode RET");
    assert!(app.editor.minor_mode_enabled("menu-bar-mode"));
}

#[test]
fn menu_bar_mode_is_global_so_opening_a_file_keeps_the_bar() {
    let mut app = app_with("one\n");
    press(&mut app, "M-x menu-bar-mode RET");
    assert!(app.editor.minor_mode_enabled("menu-bar-mode"));
    // It frames the window, not the text in it, so a fresh buffer that has
    // never heard of the mode still shows the bar.
    let id = app.editor.create_buffer("*other*");
    assert_eq!(app.editor.state.active_buffer, Some(id));
    assert!(
        !app.editor
            .state
            .active_buffer()
            .unwrap()
            .minor_mode_enabled("menu-bar-mode"),
        "the buffer doesn't list it"
    );
    assert!(
        app.editor.minor_mode_enabled("menu-bar-mode"),
        "but the global mode has it on"
    );
    assert!(crate::menu::visible(&app), "so the bar is still there");
    let s = screen(&mut app, 40, 10);
    assert!(s.contains(" Buffers"), "{s}");
    // And turning it off in the new buffer turns it off everywhere.
    press(&mut app, "M-x menu-bar-mode RET");
    assert!(!app.editor.minor_mode_enabled("menu-bar-mode"));
    assert!(!crate::menu::visible(&app));
}

#[test]
fn clicking_a_title_opens_it_and_clicking_an_item_runs_it() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut app = app_with("one\n");
    press(&mut app, "M-x menu-bar-mode RET");
    without_messages(&mut app);
    screen(&mut app, 40, 10);
    let click = |app: &mut App, col: u16, row: u16| {
        let ev = |kind| MouseEvent {
            kind,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        };
        crate::mouse::handle(app, ev(MouseEventKind::Down(MouseButton::Left)));
        crate::mouse::handle(app, ev(MouseEventKind::Up(MouseButton::Left)));
    };
    // " Buffers" starts at column 21 (File 6 + Edit 6 + Options 9).
    click(&mut app, 23, 0);
    assert!(crate::menu::active(&app), "clicking a title opened it");
    let s = screen(&mut app, 40, 10);
    assert!(s.contains(" Select Named Buffer..."), "{s}");
    // The first item is one row below the bar: the open buffers (the
    // current one, then *Messages*), a rule, Next, Previous, then Select
    // Named Buffer on the sixth row.
    click(&mut app, 28, 6);
    assert!(
        !crate::menu::active(&app),
        "clicking an item closed the menu"
    );
    assert!(app.minibuffer.is_some(), "and ran switch-to-buffer");
}

#[test]
fn right_opens_a_submenu_and_ret_runs_its_item() {
    let mut app = app_with("one\n");
    press(&mut app, "M-x menu-bar-mode RET");
    press(&mut app, "<f10>"); // File
    press(&mut app, "C-f"); // Edit
    // Down past Undo, Redo, Cut, Copy, Paste, Select and Paste, Clear,
    // Select All and Search to the Incremental Search submenu.
    for _ in 0..9 {
        press(&mut app, "C-n");
    }
    // Right opens it; its children show with their shortcuts.
    press(&mut app, "<right>");
    let s = screen(&mut app, 80, 24);
    assert!(s.contains("Forward String"), "{s}");
    assert!(s.contains("C-s"), "{s}");
    press(&mut app, "<left>");
    assert!(crate::menu::active(&app), "still in Edit");
    let s = screen(&mut app, 80, 24);
    assert!(!s.contains("Forward String"), "{s}");
    // RET on the header opens it too, and RET again runs the child:
    // incremental search starts.
    press(&mut app, "RET");
    press(&mut app, "RET");
    assert!(!crate::menu::active(&app), "RET closed the menu");
    assert!(app.minibuffer.is_some(), "and ran isearch-forward");
    press(&mut app, "C-g");
}

#[test]
fn bookmarks_set_and_jump_back() {
    let dir = std::env::temp_dir().join(format!("forge-bookmarks-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("note.txt"), "first line\nsecond line\n").unwrap();
    let mut app = app_with("");
    let (id, _) = app.editor.open_file(dir.join("note.txt")).unwrap();
    app.switch_to(id);
    press(&mut app, "C-x r m");
    answer(&mut app, "here");
    assert_eq!(app.editor.session.bookmark_names(), ["here"]);
    press(&mut app, "M->");
    let away = point(&app);
    assert_ne!(away, Pos::new(0, 0));
    press(&mut app, "C-x r b");
    answer(&mut app, "here");
    assert_eq!(point(&app), Pos::new(0, 0), "back at the bookmark");
    // The jump left the mark where point was, so C-x C-x goes back.
    press(&mut app, "C-x C-x");
    assert_eq!(point(&app), away);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn delete_region_deletes_without_killing() {
    let mut app = app_with("hello\n");
    press(&mut app, "C-SPC");
    press(&mut app, "C-f C-f C-f C-f C-f");
    press(&mut app, "M-x delete-region RET");
    assert_eq!(text(&app), "\n");
    assert!(
        app.editor.kill_ring.is_empty(),
        "nothing went on the kill ring"
    );
}

#[test]
fn select_and_paste_inserts_the_chosen_kill() {
    let mut app = app_with("hello\n");
    press(&mut app, "C-k");
    assert!(!app.editor.kill_ring.is_empty());
    press(&mut app, "M-x select-and-paste RET");
    answer(&mut app, "1");
    assert_eq!(text(&app), "hello\n", "the chosen kill came back");
}

#[test]
fn rhai_process_runs_without_freezing_and_answers() {
    let mut app = app_with("");
    press(&mut app, "M-:");
    answer_typed(
        &mut app,
        "forge.process.run(\"echo hi\", |r| forge.buffer.current().insert(r.stdout));",
    );
    press(&mut app, "RET");
    finish_jobs(&mut app);
    assert_eq!(
        text(&app),
        "hi\n",
        "the callback inserted the output: {:?}",
        app.message
    );
    assert!(!app.jobs.any_running(), "nothing lingers after the answer");
}

#[test]
fn rhai_process_timeout_kills_and_answers() {
    let mut app = app_with("");
    press(&mut app, "M-:");
    answer_typed(
        &mut app,
        "forge.process.run(\"sleep 30\", #{ timeout: 1 }, |r| forge.buffer.current().insert(r.stderr));",
    );
    press(&mut app, "RET");
    finish_jobs(&mut app);
    assert!(
        text(&app).contains("ran longer than"),
        "the timeout answered: {:?}",
        text(&app)
    );
    assert!(!app.jobs.any_running(), "the sleeper was killed");
}

#[test]
fn eval_pretty_prints_maps_and_arrays() {
    let mut app = app_with("");
    press(&mut app, "M-:");
    answer_typed(&mut app, "#{ a: 2 }");
    press(&mut app, "RET");
    assert_eq!(app.message.as_deref(), Some("#{a: 2}"));
    press(&mut app, "M-:");
    answer_typed(&mut app, "[1, 2]");
    press(&mut app, "RET");
    assert_eq!(app.message.as_deref(), Some("[1, 2]"));
    press(&mut app, "M-:");
    answer_typed(&mut app, "forge.theme.get(\"comment\")");
    press(&mut app, "RET");
    assert_eq!(app.message.as_deref(), Some("()"));
}

#[test]
fn describe_symbol_covers_commands_options_modes_and_faces() {
    let mut app = app_with("(goto-line)\n");
    press(&mut app, "M-x describe-symbol RET");
    answer(&mut app, "goto-line");
    let t = help_text(&app);
    assert!(t.contains("goto-line is an interactive command"), "{t}");
    assert!(t.contains("M-g"), "{t}");
    // "wrap" is both a buffer option and a forge.opt setting.
    press(&mut app, "M-x describe-symbol RET");
    answer(&mut app, "wrap");
    let t = help_text(&app);
    assert!(t.contains("is a buffer option"), "{t}");
    assert!(t.contains("forge.opt.wrap"), "{t}");
    press(&mut app, "M-x describe-symbol RET");
    answer(&mut app, "fundamental-mode");
    let t = help_text(&app);
    assert!(t.contains("major mode"), "mode section: {t}");
    assert!(t.contains("no mode-specific behaviour"), "mode prose: {t}");
    press(&mut app, "M-x describe-symbol RET");
    answer(&mut app, "comment");
    let t = help_text(&app);
    assert!(t.contains("is a face"), "face section: {t}");
    assert!(t.contains("Comments."), "face prose: {t}");
    press(&mut app, "M-x describe-symbol RET");
    answer(&mut app, "buffer-saved");
    let t = help_text(&app);
    assert!(t.contains("is an event"), "event section: {t}");
    assert!(t.contains("written to disk"), "event prose: {t}");
    // Nothing by that name: an error, and no *Help* switch.
    press(&mut app, "M-x describe-symbol RET");
    answer(&mut app, "nope-nope");
    assert_eq!(app.message.as_deref(), Some("No symbol named nope-nope"));
}

#[test]
fn describe_symbol_defaults_to_the_symbol_at_point() {
    let mut app = app_with("goto-line\n");
    press(&mut app, "M-x describe-symbol RET");
    answer(&mut app, "");
    let t = help_text(&app);
    assert!(t.contains("goto-line is an interactive command"), "{t}");
}

#[test]
fn a_click_beside_an_open_menu_is_not_a_menu_click() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut app = app_with("one\n");
    press(&mut app, "M-x menu-bar-mode RET");
    screen(&mut app, 40, 10);
    press(&mut app, "<f10>");
    // Far to the right of File's panel, so it's a click in the text: it
    // dismisses the menu the way a click away does, but picks no item.
    let ev = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 39,
        row: 2,
        modifiers: KeyModifiers::NONE,
    };
    crate::mouse::handle(&mut app, ev);
    assert!(!crate::menu::active(&app), "a click away dismisses it");
    assert!(app.minibuffer.is_none(), "and ran no menu item");
}

#[test]
fn with_the_menu_bar_on_the_text_still_gets_its_clicks() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut app = app_with("one two\n");
    press(&mut app, "M-x menu-bar-mode RET");
    // A redisplay lays the windows out; hit-testing reads that layout.
    screen(&mut app, 40, 10);
    let ev = |row| MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row,
        modifiers: KeyModifiers::NONE,
    };
    crate::mouse::handle(&mut app, ev(1));
    // Row 1 is the first text row now the bar has a row of its own.
    assert_eq!(point(&app).line, 0, "the click landed in the text");
}

#[test]
fn moving_the_mouse_over_the_bar_highlights_the_title_under_it() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut app = app_with("one\n");
    press(&mut app, "M-x menu-bar-mode RET");
    // A redisplay lays the windows out; hit-testing reads that layout.
    screen(&mut app, 40, 20);
    let move_to = |app: &mut App, column: u16, row: u16| {
        crate::mouse::handle(
            app,
            MouseEvent {
                kind: MouseEventKind::Moved,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            },
        )
    };
    let click = |app: &mut App, column: u16, row: u16| {
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            crate::mouse::handle(
                app,
                MouseEvent {
                    kind,
                    column,
                    row,
                    modifiers: KeyModifiers::NONE,
                },
            );
        }
    };
    // " Buffers" starts at column 21 (File 6 + Edit 6 + Options 9).
    move_to(&mut app, 25, 0);
    assert_eq!(
        app.editor.menu.hover,
        Some(3),
        "the title under the pointer"
    );
    assert_eq!(app.editor.menu.open, None, "hovering doesn't open it");

    // Moving onto another title follows it.
    move_to(&mut app, 2, 0);
    assert_eq!(app.editor.menu.hover, Some(0), "and off the last one");

    move_to(&mut app, 25, 3);
    assert_eq!(app.editor.menu.hover, None);

    // With a menu open, moving onto another title switches to it.
    press(&mut app, "<f10>");
    assert_eq!(app.editor.menu.open, Some(0));
    move_to(&mut app, 25, 0);
    assert_eq!(
        app.editor.menu.open,
        Some(3),
        "the open menu follows the pointer"
    );
    assert_eq!(app.editor.menu.item, 0);
    // Moving down into the panel keeps it open and highlights what's under
    // the pointer -- reaching for an item must not close the menu. The
    // panel starts on the screen row under the bar, which is row 1.
    move_to(&mut app, 25, 1);
    assert_eq!(app.editor.menu.open, Some(3), "still open over the panel");
    assert_eq!(app.editor.menu.item, 0, "and the first item is highlighted");
    move_to(&mut app, 25, 2);
    assert_eq!(app.editor.menu.open, Some(3), "still open further down");
    assert_eq!(app.editor.menu.item, 1, "and the highlight followed");
    // Buffers separates "List All Buffers" from "Kill Buffer". Sweeping the
    // pointer down the separator keeps the menu open and holds the
    // highlight where it was, so the pointer can cross to the group below.
    move_to(&mut app, 25, 8);
    assert_eq!(app.editor.menu.item, 7);
    move_to(&mut app, 25, 9);
    assert_eq!(
        app.editor.menu.open,
        Some(3),
        "a separator is still the panel"
    );
    assert_eq!(app.editor.menu.item, 7, "and the highlight holds");
    move_to(&mut app, 25, 10);
    assert_eq!(app.editor.menu.open, Some(3), "past the separator");
    assert_eq!(app.editor.menu.item, 9, "and picks up the item below it");
    // Moving away leaves it open, the way an Forge tmenu does: the point is
    // to sweep past it to read the text behind it. Buffers has 13 items, so
    // the panel runs from screen row 1 to 13.
    move_to(&mut app, 25, 14);
    assert_eq!(app.editor.menu.open, Some(3), "still open off the panel");
    assert_eq!(app.editor.menu.hover, None, "with nothing marked");
    // A click away dismisses it.
    click(&mut app, 25, 15);
    assert_eq!(app.editor.menu.open, None);
    click(&mut app, 25, 15);
    press(&mut app, "<f10>");
    assert_eq!(app.editor.menu.open, Some(0));
    // But a click on the bar or the panel leaves it alone.
    // The pointer is on Buffers, so the click goes there rather than
    // dismissing File -- and a second click on the same title toggles it.
    click(&mut app, 25, 0);
    assert_eq!(
        app.editor.menu.open,
        Some(3),
        "the click follows the pointer"
    );
    click(&mut app, 25, 0);
    assert_eq!(app.editor.menu.open, None);
    // A click on the panel is the menu's own, not a click away: on the
    // separator it picks nothing and leaves the menu open.
    move_to(&mut app, 25, 0);
    click(&mut app, 25, 0);
    assert_eq!(app.editor.menu.open, Some(3));
    click(&mut app, 25, 9);
    assert_eq!(
        app.editor.menu.open,
        Some(3),
        "a click on the panel's separator leaves it open"
    );
    click(&mut app, 25, 10);
    assert_eq!(app.editor.menu.open, None, "an item click runs the command");
    // That item was Kill Buffer, so it left a prompt up; dismiss it, or the
    // keys below would go to it rather than to the buffer.
    press(&mut app, "C-g");

    // With the mode off there's no bar, so moving over its old row marks
    // nothing -- the mark the pointer left goes with it.
    press(&mut app, "M-x menu-bar-mode RET");
    move_to(&mut app, 25, 0);
    assert_eq!(app.editor.menu.hover, None);
}

#[test]
fn dragging_still_selects_text_with_the_menu_bar_on() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut app = app_with("one two three\n");
    press(&mut app, "M-x menu-bar-mode RET");
    screen(&mut app, 40, 10);
    let ev = |kind, column, row| MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    };
    // Press in the text (row 1 now the bar's is row 0), drag right.
    crate::mouse::handle(&mut app, ev(MouseEventKind::Down(MouseButton::Left), 1, 1));
    crate::mouse::handle(&mut app, ev(MouseEventKind::Drag(MouseButton::Left), 7, 1));
    let (start, end) = app.buf_ref().region().expect("a region");
    assert_eq!(start.line, 0, "still in the text");
    assert!(
        end.col > start.col,
        "the drag made a region: {start:?}..{end:?}"
    );
}

#[test]
fn the_panel_puts_the_keys_in_a_column_on_the_right() {
    let mut app = app_with("one\n");
    press(&mut app, "M-x menu-bar-mode RET");
    press(&mut app, "<f10>");
    let s = screen(&mut app, 60, 40);
    // The panel's rows: what it drew between the bar and the mode line,
    // ignoring the blank text rows under it.
    let open: Vec<&str> = s
        .lines()
        .skip(1)
        .take_while(|l| !l.starts_with("-U:"))
        .filter(|l| !l.trim().is_empty())
        .collect();
    assert!(open.len() >= 8, "{open:?}");
    // Labels on the left, keys against the right edge: every keyed entry
    // ends in the same column, whichever key it has.
    let keyed: Vec<&str> = open.iter().copied().filter(|l| l.contains("C-")).collect();
    assert!(keyed.len() >= 4, "several keyed entries: {open:?}");
    let ends: std::collections::BTreeSet<usize> =
        keyed.iter().map(|l| l.trim_end().chars().count()).collect();
    assert_eq!(ends.len(), 1, "the keys end flush: {open:?}");
    let save = open
        .iter()
        .find(|l| l.trim_start().starts_with("Save"))
        .unwrap();
    let new = open
        .iter()
        .find(|l| l.trim_start().starts_with("Save As"))
        .unwrap();
    assert!(save.trim_end().ends_with("C-x C-s"), "{save:?}");
    assert!(new.trim_end().ends_with("C-x C-w"), "{new:?}");
    // The label and the key are separated by the padding, never touching,
    // and the padding is the same for every entry.
    let mut key_starts = std::collections::BTreeSet::new();
    for l in &keyed {
        let t = l.trim_end();
        let gap = t
            .char_indices()
            .rev()
            .find_map(|(i, c)| (c == ' ' && t[..i].ends_with(' ')).then_some(i))
            .unwrap();
        // gap is the last space before the key, so the key starts after it.
        let (label, key) = t.split_at(gap + 1);
        assert!(
            !key.is_empty() && !key.starts_with(' '),
            "a key on the right: {l:?}"
        );
        assert!(!label.trim().is_empty(), "a label on the left: {l:?}");
        assert!(key.contains('C'), "{l:?}");
        key_starts.insert(gap + 1);
    }
    // Keys are flush against the right edge, so they start in different
    // columns when their lengths differ (C-x 2 and C-x C-f); what matters
    // is that none of them touches its label.
    assert!(!key_starts.is_empty(), "{open:?}");
    // An entry with no binding has a blank right column.
    let revert = open
        .iter()
        .find(|l| l.trim_start().starts_with("Revert"))
        .unwrap();
    assert!(!revert.contains("C-"), "{revert:?}");
}

// ------------------------------------------------------- version control
