// SPDX-License-Identifier: MIT OR Apache-2.0
//! The mode line's clickable parts, as in `mode-line-modified`,
//! `mode-line-buffer-identification`, `mode-line-position`,
//! `mode-line-modes` and friends:
//!
//! | part            | mouse-1                  | mouse-2       | mouse-3                 |
//! |-----------------|--------------------------|---------------|-------------------------|
//! | `U:` / input    | describe input method    | toggle it     | choose an input method  |
//! | `**` / `%%`     | toggle read-only         |               |                         |
//! | buffer name     | previous buffer          |               | next buffer             |
//! | position        | line/column menu         |               | line/column menu        |
//! | version control | VC menu                  |               | VC menu                 |
//! | major mode      | major mode menu          | describe mode | minor modes menu        |
//! | minor mode      | its menu                 | describe mode | its menu                |
//! | `Def`           | end the macro            |               | macro menu              |
//! | `Compiling`     | compilation menu         |               | compilation menu        |

use forge_core::WindowId;
use forge_core::display::{ModeLineHit, ModeLineTarget};
use forge_core::menu::{Item, When};

use crate::app::App;

pub const COMMANDS: &[forge_docs::CommandDoc] = &[
    forge_docs::CommandDoc::new("column-number-mode", "Show the column in the mode line.")
        .body("On by default. forge.opt.column_number_mode sets it for good; clicking the position in the mode line offers it too."),
    forge_docs::CommandDoc::new("line-number-mode", "Show the line number in the mode line.")
        .body("On by default. forge.opt.line_number_mode sets it for good."),
    forge_docs::CommandDoc::line("mode-line-next-buffer", "Switch to the next buffer (mouse-3 on the buffer name)."),
    forge_docs::CommandDoc::line("mode-line-previous-buffer", "Switch to the previous buffer (mouse-1 on the buffer name)."),
    forge_docs::CommandDoc::new("size-indication-mode", "Show the buffer's size in the mode line.")
        .body("Off by default. forge.opt.size_indication_mode sets it for good."),
];

pub fn execute(app: &mut App, cmd: &str) -> bool {
    let arg = app.has_arg();
    let flip = |on: &mut bool| *on = if arg { true } else { !*on };
    match cmd {
        "line-number-mode" => flip(&mut app.line_number_mode),
        "column-number-mode" => flip(&mut app.column_number_mode),
        "size-indication-mode" => flip(&mut app.size_indication_mode),
        "mode-line-next-buffer" => return crate::commands::execute(app, "next-buffer"),
        "mode-line-previous-buffer" => return crate::commands::execute(app, "previous-buffer"),
        _ => return false,
    }
    true
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    Left,
    Middle,
    Right,
}

/// The flags the position menu ticks.
pub fn flag(app: &App, name: &str) -> Option<bool> {
    match name {
        "line-number-mode" => Some(app.line_number_mode),
        "column-number-mode" => Some(app.column_number_mode),
        "size-indication-mode" => Some(app.size_indication_mode),
        _ => None,
    }
}

/// A click on window `id`'s mode line at `column` (cells from its left
/// edge), shown at frame cell (`x`, `y`).
pub fn click(app: &mut App, id: WindowId, column: u16, button: Button, x: u16, y: u16) {
    app.select_window(id);
    let target = app
        .editor
        .windows
        .get(id)
        .and_then(|w| ModeLineHit::at(&w.mode_line_hits, column).cloned());
    let Some(target) = target else { return };
    let run = |app: &mut App, cmd: &str| {
        crate::commands::execute(app, cmd);
    };
    use Button::*;
    use ModeLineTarget as T;
    match (&target, button) {
        (T::Mule, Left) => run(app, "describe-input-method"),
        (T::Mule, Middle) => run(app, "toggle-input-method"),
        (T::Mule, Right) => run(app, "set-input-method"),
        (T::Modified, Left) => run(app, "read-only-mode"),
        (T::BufferName, Left) => run(app, "mode-line-previous-buffer"),
        (T::BufferName, Right) => run(app, "mode-line-next-buffer"),
        (T::Percent | T::Size | T::Line | T::Column, Left | Right) => {
            popup(app, position_menu(), x, y)
        }
        (T::Vc, Left | Right) => popup(app, vc_menu(), x, y),
        (T::MajorMode, Left) => {
            let items = major_menu(app);
            popup(app, items, x, y)
        }
        (T::MajorMode, Middle) => run(app, "describe-mode"),
        (T::MajorMode, Right) => {
            let items = minor_menu(app, None);
            popup(app, items, x, y)
        }
        (T::MinorMode(m), Left | Right) => {
            let items = minor_menu(app, Some(m));
            popup(app, items, x, y)
        }
        (T::MinorMode(_), Middle) => run(app, "describe-mode"),
        (T::Extra(w), Left) if w == "Def" => run(app, "end-kbd-macro"),
        (T::Extra(w), Left | Right) if w == "Def" => popup(app, macro_menu(), x, y),
        (T::Extra(w), Left | Right) if w == "Compiling" => popup(app, compile_menu(), x, y),
        _ => {}
    }
}

fn l(label: &str, cmd: &str) -> Item {
    Item::new(label, cmd)
}

fn position_menu() -> Vec<Item> {
    vec![
        l("Line Number", "line-number-mode").checked(When::Flag("line-number-mode".into())),
        l("Column Number", "column-number-mode").checked(When::Flag("column-number-mode".into())),
        l("Size Indication", "size-indication-mode")
            .checked(When::Flag("size-indication-mode".into())),
        Item::separator(),
        l("Go to Line...", "goto-line"),
        l("What Cursor Position", "what-cursor-position"),
        l("Recenter", "recenter-top-bottom"),
        l("Count Words", "count-words"),
    ]
}

fn vc_menu() -> Vec<Item> {
    vec![
        l("Next Action (Check In/Out)", "vc-next-action"),
        l("Compare with Base Version", "vc-diff"),
        l("Show History", "vc-print-log"),
        l("Annotate", "vc-annotate"),
        l("VC Directory", "vc-dir"),
        Item::separator(),
        l("Switch Branch...", "vc-switch-branch"),
        l("Pull", "vc-pull"),
        l("Push", "vc-push"),
        l("Refresh State", "vc-refresh-state"),
    ]
}

fn macro_menu() -> Vec<Item> {
    vec![
        l("End Macro Definition", "end-kbd-macro"),
        l("Call Last Macro", "kmacro-end-and-call-macro"),
    ]
}

fn compile_menu() -> Vec<Item> {
    vec![
        l("Kill Compilation", "kill-compilation"),
        l("Recompile", "recompile"),
        l("Next Error", "next-error"),
    ]
}

/// Mouse-1 on the major mode: describe it, then its own menu.
fn major_menu(app: &App) -> Vec<Item> {
    let mut items = vec![l("Describe Major Mode", "describe-mode"), Item::separator()];
    let mode = app.buf_ref().major_mode().to_string();
    let state = crate::menu::state_for_mode_line(app);
    for m in app.editor.menus.menus() {
        if m.owner.is_none() && m.when != When::Always && m.visible(&state) {
            for it in &m.items {
                items.push(it.clone());
            }
            items.push(Item::separator());
        }
    }
    let _ = mode;
    items.push(l("Show Minor Modes...", "describe-mode"));
    items
}

/// The minor modes of the buffer, each ticked while on, to turn on or off;
/// `only` is the one that was clicked.
fn minor_menu(app: &App, only: Option<&str>) -> Vec<Item> {
    let mut items = Vec::new();
    if let Some(m) = only {
        let pretty = forge_core::display::mode_display_name(m);
        items.push(l(&format!("Describe {pretty}"), "describe-mode"));
        if app.editor.commands.contains(m) {
            items.push(l(&format!("Turn Off {pretty}"), m).checked(When::MinorMode(m.to_string())));
        }
        return items;
    }
    for (label, cmd) in [
        ("Auto Fill", "auto-fill-mode"),
        ("Visual Line (Word Wrap)", "visual-line-mode"),
        ("Line Numbers", "display-line-numbers-mode"),
        ("Highlight Current Line", "hl-line-mode"),
        ("Whitespace", "whitespace-mode"),
        ("Electric Pair", "electric-pair-mode"),
        ("Read-Only", "read-only-mode"),
        ("Spell Checking", "spell-check-mode"),
    ] {
        items.push(l(label, cmd).checked(When::MinorMode(cmd.to_string())));
    }
    items.push(Item::separator());
    items.push(l("Describe Modes", "describe-mode"));
    items
}

pub(crate) fn popup(app: &mut App, items: Vec<Item>, x: u16, y: u16) {
    let frame = app.editor.windows.frame();
    crate::menu::close(app);
    app.editor
        .context_menu
        .open_items(items, x.saturating_sub(frame.x), y.saturating_sub(frame.y));
    crate::menu::after_popup(app);
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::Rect;
    use forge_core::display::{ModeLineTarget as T, redisplay};

    fn app() -> (App, WindowId) {
        let mut a = App::new(forge_core::Editor::default());
        let id = a.editor.add_buffer(forge_core::Buffer::new("notes.txt"));
        a.editor.show_buffer(id);
        let area = Rect {
            x: 0,
            y: 0,
            width: 70,
            height: 6,
        };
        let opts = crate::ui::display_options(&a);
        redisplay(&mut a.editor, area, &opts);
        let w = a.editor.windows.selected_id();
        (a, w)
    }

    fn column_of(a: &App, w: WindowId, t: &T) -> u16 {
        a.editor
            .windows
            .get(w)
            .unwrap()
            .mode_line_hits
            .iter()
            .find(|h| &h.target == t)
            .unwrap()
            .from
    }

    #[test]
    fn clicking_the_modified_flag_toggles_read_only() {
        let (mut a, w) = app();
        let col = column_of(&a, w, &T::Modified);
        click(&mut a, w, col, Button::Left, col, 5);
        assert!(a.buf_ref().read_only());
    }

    #[test]
    fn clicking_the_position_opens_a_menu_that_toggles_the_column() {
        let (mut a, w) = app();
        let col = column_of(&a, w, &T::Line);
        click(&mut a, w, col, Button::Left, col, 5);
        assert!(a.editor.context_menu.is_open());
        crate::menu::close(&mut a);
        execute(&mut a, "column-number-mode");
        assert!(!a.column_number_mode);
        let opts = crate::ui::display_options(&a);
        assert!(opts.mode_line_no_column);
    }

    #[test]
    fn the_major_and_minor_mode_names_open_menus() {
        let (mut a, w) = app();
        let col = column_of(&a, w, &T::MajorMode);
        click(&mut a, w, col, Button::Left, col, 5);
        assert!(a.editor.context_menu.is_open());
        crate::menu::close(&mut a);
        click(&mut a, w, col, Button::Right, col, 5);
        assert!(a.editor.context_menu.is_open());
    }
}
