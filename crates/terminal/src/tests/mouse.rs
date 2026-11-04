// SPDX-License-Identifier: MIT OR Apache-2.0
//! mouse tests.

use super::*;

#[test]
fn mouse_wheel_scrolls_the_window_under_the_pointer() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let ev = |kind, column, row| MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    };
    let body: String = (1..=100).map(|i| format!("line {i}\n")).collect();
    let mut app = app_with(&body);
    screen(&mut app, 60, 24);

    crate::mouse::handle(&mut app, ev(MouseEventKind::ScrollDown, 5, 5));
    crate::mouse::handle(&mut app, ev(MouseEventKind::ScrollDown, 5, 5));
    assert_eq!(app.win().scroll_top, 6);
    assert_eq!(point(&app), Pos::new(6, 0), "point stays inside the window");
    let s = screen(&mut app, 60, 24);
    assert!(s.lines().next().unwrap().starts_with("line 7"), "{s}");
    crate::mouse::handle(&mut app, ev(MouseEventKind::ScrollUp, 5, 5));
    assert_eq!(app.win().scroll_top, 3);

    // In a split, the wheel scrolls the window under the pointer only.
    press(&mut app, "C-x 2");
    screen(&mut app, 60, 24);
    let lower = app.editor.windows.nth_from_selected(1);
    crate::mouse::handle(&mut app, ev(MouseEventKind::ScrollDown, 5, 18));
    assert_eq!(app.editor.windows.get(lower).unwrap().scroll_top, 6);
    assert_eq!(app.win().scroll_top, 3, "the selected window didn't move");
    assert_ne!(
        app.editor.windows.selected_id(),
        lower,
        "scrolling doesn't select"
    );

    // Clicking selects the window and moves point; dragging makes a region.
    crate::mouse::handle(&mut app, ev(MouseEventKind::Down(MouseButton::Left), 2, 13));
    assert_eq!(app.editor.windows.selected_id(), lower);
    assert_eq!(point(&app), Pos::new(7, 2));
    crate::mouse::handle(&mut app, ev(MouseEventKind::Drag(MouseButton::Left), 4, 14));
    crate::mouse::handle(&mut app, ev(MouseEventKind::Up(MouseButton::Left), 4, 14));
    assert_eq!(
        app.buf_ref().active_region(),
        Some((Pos::new(7, 2), Pos::new(8, 4)))
    );

    press(&mut app, "M-x xterm-mouse-mode RET");
    assert!(!app.mouse);
}

#[test]
fn clicks_land_on_indented_text() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let ev = |kind, column, row| MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    };
    let mut app = app_with("* A\nbody\n");
    // Org indent mode, as #+STARTUP: indent sets it: "body" draws two
    // cells right, but stays buffer columns 0..4.
    app.buf().set_major_mode(forge_core::org::MODE);
    app.buf().org_display.indent = true;
    let s = screen(&mut app, 40, 12);
    let lines: Vec<&str> = s.lines().collect();
    let row = lines
        .iter()
        .position(|r| r.contains("body"))
        .expect("body row");
    let col = lines[row].find('d').expect("the d in body") as u16;
    crate::mouse::handle(
        &mut app,
        ev(MouseEventKind::Down(MouseButton::Left), col, row as u16),
    );
    crate::mouse::handle(
        &mut app,
        ev(MouseEventKind::Up(MouseButton::Left), col, row as u16),
    );
    assert_eq!(point(&app), Pos::new(1, 2));
}

#[test]
fn clicks_skip_folded_lines() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let ev = |kind, column, row| MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    };
    let mut app = app_with("* A\nhidden1\nhidden2\n* B\n");
    app.buf().fold_lines(1, 2);
    let s = screen(&mut app, 40, 12);
    let lines: Vec<&str> = s.lines().collect();
    // "* B" draws right under "* A": the hidden lines take no rows.
    let row = lines
        .iter()
        .position(|r| r.contains("* B"))
        .expect("* B row");
    assert_eq!(
        row,
        lines.iter().position(|r| r.contains("* A")).unwrap() + 1
    );
    let col = lines[row].find("* B").expect("heading") as u16;
    crate::mouse::handle(
        &mut app,
        ev(MouseEventKind::Down(MouseButton::Left), col, row as u16),
    );
    crate::mouse::handle(
        &mut app,
        ev(MouseEventKind::Up(MouseButton::Left), col, row as u16),
    );
    // On the visible heading — not hidden1, as scroll_top + row says.
    assert_eq!(point(&app), Pos::new(3, 0));
}

#[test]
fn scrolls_step_through_shown_rows() {
    use ratatui::crossterm::event::{MouseEvent, MouseEventKind};
    let ev = |kind, column, row| MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    };
    let body: String = (0..30).map(|i| format!("line {i}\n")).collect();
    let mut app = app_with(&body);
    app.buf().fold_lines(5, 19);
    screen(&mut app, 40, 12);
    // A wheel notch moves three shown rows, landing on visible lines.
    crate::mouse::handle(&mut app, ev(MouseEventKind::ScrollDown, 5, 5));
    assert_eq!(app.win().scroll_top, 3);
    // The next notch skips the whole fold at once, not into it.
    crate::mouse::handle(&mut app, ev(MouseEventKind::ScrollDown, 5, 5));
    assert_eq!(app.win().scroll_top, 21);
    assert!(!app.buf_ref().line_hidden(app.win().scroll_top));
    // A screenful from there stays on visible lines too.
    press(&mut app, "C-v");
    assert!(!app.buf_ref().line_hidden(app.win().scroll_top));
}

#[test]
fn clicking_a_far_visible_line_does_not_scroll() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let ev = |kind, column, row| MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    };
    let body: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    let mut app = app_with(&body);
    // Fold lines 2..16 away: buffer line 19 shows a few rows down, and
    // its number is far past the window's height.
    app.buf().fold_lines(1, 15);
    app.buf().set_point(Pos::new(18, 0));
    let s = screen(&mut app, 40, 12);
    assert_eq!(app.win().scroll_top, 0, "point was already shown");
    assert!(s.lines().any(|r| r.contains("line 1")), "{s}");
    // Clicking it keeps point and place: nothing shifts.
    let lines: Vec<&str> = s.lines().collect();
    let row = lines
        .iter()
        .position(|r| r.contains("line 19"))
        .expect("line 19 row");
    let col = lines[row].find("line 19").expect("line 19 col") as u16;
    crate::mouse::handle(
        &mut app,
        ev(MouseEventKind::Down(MouseButton::Left), col, row as u16),
    );
    crate::mouse::handle(
        &mut app,
        ev(MouseEventKind::Up(MouseButton::Left), col, row as u16),
    );
    assert_eq!(point(&app), Pos::new(18, 0));
    assert_eq!(app.win().scroll_top, 0);
}
