// SPDX-License-Identifier: MIT OR Apache-2.0
//! windows tests.

use super::*;

#[test]
fn windows_split_select_and_keep_their_own_point() {
    let body: String = (1..=60).map(|i| format!("line {i}\n")).collect();
    let mut app = app_with(&body);
    press(&mut app, "C-x 2");
    assert_eq!(mode_lines(&mut app, 60, 24).len(), 2);
    press(&mut app, "M-> C-x o");
    assert_eq!(
        point(&app),
        Pos::new(0, 0),
        "the other window kept its own point"
    );
    press(&mut app, "C-x o");
    assert_eq!(point(&app), Pos::new(60, 0));

    // Typing at the top in one window keeps the other window on its line.
    press(&mut app, "C-x o new RET C-x o");
    assert_eq!(point(&app), Pos::new(61, 0));

    // Side by side, then back to one.
    press(&mut app, "C-x 3");
    let s = screen(&mut app, 60, 24);
    assert!(s.contains('│'), "{s}");
    assert_eq!(
        mode_lines(&mut app, 60, 24).len(),
        2,
        "the right window's mode line starts mid-row"
    );
    press(&mut app, "C-x 1");
    assert_eq!(app.editor.windows.count(), 1);
    press(&mut app, "C-x 0");
    assert!(
        app.message
            .as_deref()
            .unwrap()
            .contains("only window")
    );

    // C-x 4 b opens a buffer in another window; C-M-v scrolls it.
    press(&mut app, "C-x 4 b *other* RET");
    assert_eq!(app.editor.windows.count(), 2);
    assert_eq!(app.buf_ref().name(), "*other*");
    press(&mut app, "C-x 1 C-x b RET M-< C-x 2");
    screen(&mut app, 60, 24);
    press(&mut app, "C-M-v");
    let lower = app.editor.windows.nth_from_selected(1);
    assert!(app.editor.windows.get(lower).unwrap().scroll_top > 0);
    assert_eq!(
        point(&app).line,
        0,
        "scrolling the other window doesn't move this one"
    );

    screen(&mut app, 60, 24);
    let h = app.win().height;
    press(&mut app, "C-u 3 C-x ^");
    screen(&mut app, 60, 24);
    assert_eq!(app.win().height, h + 3);
    press(&mut app, "C-x +");
    screen(&mut app, 60, 24);
    assert_eq!(app.win().height, h);
}

#[test]
fn frames_have_their_own_windows_and_share_buffers() {
    let body: String = (1..=40).map(|i| format!("line {i}\n")).collect();
    let mut app = app_with(&body);
    let base = app.editor.windows.selected_frame_id();
    press(&mut app, "M->");
    assert_eq!(point(&app), Pos::new(40, 0));

    // C-x 5 2: a new frame, selected, showing the same buffer.
    press(&mut app, "C-x 5 2");
    assert_eq!(app.editor.windows.frame_count(), 2);
    let new = app.editor.windows.selected_frame_id();
    assert_ne!(new, base);
    assert_eq!(app.buf_ref().name(), "*scratch*", "frames share buffers");
    assert_eq!(
        point(&app),
        Pos::new(40, 0),
        "the new frame starts where point was"
    );

    // Point is per frame: this one moves to the top.
    press(&mut app, "M-<");
    assert_eq!(point(&app), Pos::new(0, 0));
    press(&mut app, "C-x 5 o");
    assert_eq!(app.editor.windows.selected_frame_id(), base);
    assert_eq!(
        point(&app),
        Pos::new(40, 0),
        "the other frame kept its own point"
    );

    // Each frame has its own window tree.
    press(&mut app, "C-x 5 o");
    press(&mut app, "C-x 2");
    screen(&mut app, 60, 24);
    assert_eq!(app.editor.windows.count(), 2);
    press(&mut app, "C-x 5 o");
    assert_eq!(
        app.editor.windows.count(),
        1,
        "the other frame kept its layout"
    );
    assert_eq!(app.editor.windows.frame_count(), 2);
    let s = screen(&mut app, 60, 24);
    let mode_lines = s.lines().filter(|l| l.starts_with("-U:")).count();
    assert_eq!(
        mode_lines, 1,
        "one frame at a time: the single-window frame is the one drawn: {s}"
    );
    assert!(s.contains("line 40") || s.contains("line 41"), "{s}");

    // C-x 5 0: delete the selected frame, leaving the other intact.
    press(&mut app, "C-x 5 0");
    assert_eq!(app.editor.windows.frame_count(), 1);
    assert_eq!(app.editor.windows.selected_frame_id(), new);
    assert_eq!(
        app.editor.windows.count(),
        2,
        "the survivor keeps its split"
    );

    // C-x 5 u brings it back: the same frame, selected, with its windows.
    press(&mut app, "C-x 5 u");
    assert_eq!(app.editor.windows.frame_count(), 2);
    assert_eq!(app.editor.windows.selected_frame_id(), base);
    assert_eq!(app.editor.windows.count(), 1, "frame 1 came back as it was");

    // Delete two frames, then reach the older deletion with a numeric
    // prefix: 1 is the most recently deleted, so C-u 2 gets the one before.
    press(&mut app, "C-x 5 2");
    let older = app.editor.windows.selected_frame_id();
    press(&mut app, "C-x 5 2");
    let newer = app.editor.windows.selected_frame_id();
    press(&mut app, "C-x 5 0"); // deletes `newer`
    press(&mut app, "C-x 5 0"); // deletes the frame that took its place
    assert_eq!(app.editor.windows.frame_count(), 2);
    press(&mut app, "C-u 2 C-x 5 u");
    assert_eq!(app.editor.windows.selected_frame_id(), newer);
    assert_eq!(app.editor.windows.frame_count(), 3);
    assert_ne!(older, newer);
}

#[test]
fn killing_a_buffer_fixes_every_window_showing_it() {
    let mut app = app_with("scratch");
    press(&mut app, "C-x b doomed RET C-x 2 C-x k RET");
    for w in app.editor.windows.iter() {
        let name = app.editor.state.get_buffer(w.buffer).unwrap().name();
        assert_eq!(name, "*scratch*");
    }
}

#[test]
fn frame_commands_rename_select_and_list() {
    let mut app = app_with("hi");

    // M-x set-frame-name gives the selected frame a name of its own.
    press(&mut app, "M-x set-frame-name RET Boss RET");
    let id = app.editor.windows.selected_frame_id();
    assert_eq!(app.editor.windows.frame_name(id), Some("Boss"));

    // A terminal frame has no title bar, so its name leads the mode line.
    let lines = mode_lines(&mut app, 70, 6);
    assert!(lines[0].starts_with("-U:---Boss"), "{}", lines[0]);

    // A second frame keeps the default F<n> name.
    press(&mut app, "C-x 5 2");
    let second = app.editor.windows.selected_frame_id();
    assert_ne!(app.editor.windows.frame_name(second), Some("Boss"));
    let lines = mode_lines(&mut app, 70, 6);
    assert!(!lines[0].contains("Boss"), "{}", lines[0]);

    // M-x select-frame-by-name (TAB completes) selects by name.
    press(&mut app, "M-x select-frame-by-name RET Boss RET");
    assert_eq!(app.editor.windows.selected_frame_id(), id);

    // M-x frames-on-display names every frame on the one display.
    press(&mut app, "M-x frames-on-display RET");
    let msg = app.message.clone().expect("a message");
    assert!(msg.contains("Boss"), "{msg}");
    assert!(msg.contains("frame"), "{msg}");

    // M-x make-frame-on-display makes one on the default display...
    press(&mut app, "M-x make-frame-on-display RET RET");
    assert_eq!(app.editor.windows.frame_count(), 3);
    // ...but rejects a display that isn't ours.
    press(&mut app, "M-x make-frame-on-display RET nowhere RET");
    assert_eq!(app.editor.windows.frame_count(), 3);
    assert!(
        app.message
            .as_deref()
            .unwrap()
            .contains("No display named nowhere")
    );

    // A monitor name is never checked: the frame is made either way.
    press(&mut app, "M-x make-frame-on-monitor RET 42 RET");
    assert_eq!(app.editor.windows.frame_count(), 4);
}
