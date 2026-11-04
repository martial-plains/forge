// SPDX-License-Identifier: MIT OR Apache-2.0
//! Window commands: C-x 2, C-x 3, C-x o, C-x 0, C-x 1, C-x ^, C-M-v, ...

use crate::app::App;
use forge_core::Direction;

pub fn execute(app: &mut App, cmd: &str) -> bool {
    match cmd {
        "split-window-below" => split(app, Direction::Rows),
        "split-window-right" => split(app, Direction::Columns),
        "other-window" => {
            if app.editor.windows.count() == 1 {
                app.msg("No other window to select");
            } else {
                let id = app.editor.windows.nth_from_selected(app.count() as isize);
                app.select_window(id);
            }
        }
        "delete-window" => {
            let id = app.editor.windows.selected_id();
            if let Err(e) = app.editor.delete_window(id) {
                app.error(e);
            }
        }
        "delete-other-windows" => app.editor.delete_other_windows(),
        "balance-windows" => app.editor.windows.balance(),
        "text-scale-increase" | "text-scale-decrease" | "text-scale-reset" => text_scale(app, cmd),
        "enlarge-window" => resize(app, Direction::Rows, app.count()),
        "shrink-window" => resize(app, Direction::Rows, -app.count()),
        "enlarge-window-horizontally" => resize(app, Direction::Columns, app.count()),
        "shrink-window-horizontally" => resize(app, Direction::Columns, -app.count()),
        "scroll-other-window" | "scroll-other-window-down" => {
            if app.editor.windows.count() == 1 {
                app.error("There is no other window");
                return true;
            }
            let here = app.editor.windows.selected_id();
            let other = app.editor.windows.nth_from_selected(1);
            app.editor.select_window(other);
            let down = cmd == "scroll-other-window-down";
            crate::commands::execute(
                app,
                if down {
                    "scroll-down-command"
                } else {
                    "scroll-up-command"
                },
            );
            app.editor.select_window(here);
        }
        _ => return false,
    }
    true
}

fn split(app: &mut App, dir: Direction) {
    if let Err(e) = app.editor.split_window(dir) {
        app.error(e);
    }
}

fn resize(app: &mut App, dir: Direction, delta: i64) {
    if let Err(e) = app.editor.windows.enlarge(dir, delta as isize) {
        app.error(e);
    }
}

/// For C-x 4 f and C-x 4 b: select another window, splitting if there
/// is only one.
pub fn pop_to_other_window(app: &mut App) {
    if app.editor.windows.count() == 1
        && let Err(e) = app.editor.split_window(Direction::Rows)
    {
        return app.error(e);
    }
    let other = app.editor.windows.nth_from_selected(1);
    app.select_window(other);
}

/// Scale the current buffer's text (C-x C-+, C-x C--, C-x C-0): each step
/// is 1.2 times larger or smaller. A numeric argument is the number of
/// steps. The GUI draws it; a terminal's cells cannot change size.
fn text_scale(app: &mut App, cmd: &str) {
    let n = app.count() as i32;
    let b = app.buf();
    b.text_scale = match cmd {
        "text-scale-increase" => b.text_scale + n,
        "text-scale-decrease" => b.text_scale - n,
        _ => 0,
    }
    .clamp(-8, 12);
    let scale = b.text_scale;
    if scale == 0 {
        app.msg("Text scale reset");
    } else {
        app.msg(format!("Text scale {scale:+}"));
    }
}
