// SPDX-License-Identifier: MIT OR Apache-2.0
//! Kill ring linked to the OS clipboard and primary selection.

use super::*;
use forge_core::clipboard::SelectionTarget;

fn clipboard(app: &App) -> Option<String> {
    app.clipboard_provider.get_text(SelectionTarget::Clipboard)
}

fn primary(app: &App) -> Option<String> {
    app.clipboard_provider.get_text(SelectionTarget::Primary)
}

fn set_clipboard(app: &mut App, text: &str) {
    app.clipboard_provider
        .set_text(SelectionTarget::Clipboard, text);
}

fn set_primary(app: &mut App, text: &str) {
    app.clipboard_provider
        .set_text(SelectionTarget::Primary, text);
}

#[test]
fn kill_exports_to_clipboard_and_yank_reads_foreign_clipboard() {
    let mut app = app_with("one\ntwo");
    press(&mut app, "C-k");
    assert_eq!(app.editor.kill_ring.current(), Some("one"));
    assert_eq!(clipboard(&app).as_deref(), Some("one"));
    // Another program copies something; C-y must yank that instead.
    set_clipboard(&mut app, "from os");
    press(&mut app, "C-y");
    assert_eq!(text(&app), "from os\ntwo");
}

#[test]
fn select_enable_clipboard_off_disables_the_round_trip() {
    let mut app = app_with("one\ntwo");
    app.select_enable_clipboard = false;
    press(&mut app, "C-k");
    assert_eq!(app.editor.kill_ring.current(), Some("one"));
    assert_eq!(clipboard(&app), None);
    // The foreign clipboard is ignored; C-y pastes the kill ring.
    set_clipboard(&mut app, "from os");
    press(&mut app, "C-y");
    assert_eq!(text(&app), "one\ntwo");
}

#[test]
fn save_interprogram_paste_before_kill_keeps_foreign_clipboard() {
    let mut app = app_with("one\ntwo");
    set_clipboard(&mut app, "foreign");
    app.save_interprogram_paste_before_kill = forge_core::SaveInterprogramPasteBeforeKill::Always;
    press(&mut app, "C-k");
    // The foreign text was pushed onto the ring before our kill replaced it.
    assert_eq!(app.editor.kill_ring.current(), Some("one"));
    assert_eq!(app.editor.kill_ring.get(1), Some("foreign"));
    assert_eq!(clipboard(&app).as_deref(), Some("one"));
}

#[test]
fn yank_pop_change_selection_saves_the_new_yank_to_clipboard() {
    let mut app = app_with("one\ntwo\n");
    press(&mut app, "C-k");
    press(&mut app, "C-n C-k");
    assert_eq!(app.editor.kill_ring.current(), Some("two"));
    press(&mut app, "C-e C-y");
    assert_eq!(clipboard(&app).as_deref(), Some("two"));
    app.yank_pop_change_selection = true;
    press(&mut app, "M-y");
    assert_eq!(clipboard(&app).as_deref(), Some("one"));
}

#[test]
fn select_enable_primary_round_trips_the_primary_buffer() {
    let mut app = app_with("one\ntwo");
    app.select_enable_primary = true;
    press(&mut app, "C-k");
    assert_eq!(primary(&app).as_deref(), Some("one"));
    set_primary(&mut app, "from primary");
    press(&mut app, "C-y");
    assert_eq!(text(&app), "from primary\ntwo");
}

#[test]
fn nul_bytes_are_sanitized_before_the_selection_buffers() {
    let mut app = app_with("");
    app.clipboard_provider
        .set_text(SelectionTarget::Clipboard, "a\0b");
    assert_eq!(clipboard(&app).as_deref(), Some("a\\0b"));
}

#[test]
fn save_clipboard_on_exit_obeys_the_option() {
    let mut app = app_with("one\ntwo");
    press(&mut app, "C-k");
    assert_eq!(clipboard(&app).as_deref(), Some("one"));
    // Another app replaces the clipboard after Forge's last kill.
    set_clipboard(&mut app, "foreign");
    // Off: Forge leaves the foreign clipboard alone.
    app.x_select_enable_clipboard_manager = false;
    app.save_clipboard_on_exit();
    assert_eq!(clipboard(&app).as_deref(), Some("foreign"));
    // On: the kill ring's current entry is put back.
    app.x_select_enable_clipboard_manager = true;
    app.save_clipboard_on_exit();
    assert_eq!(clipboard(&app).as_deref(), Some("one"));
}
