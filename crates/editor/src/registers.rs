// SPDX-License-Identifier: MIT OR Apache-2.0
//! The C-x r prefix: rectangles and registers.
//!
//! Registers are named by one character and hold text, a rectangle, a
//! position, or a number. Positions are kept as (offset, buffer version)
//! and carried across later edits by the buffer's edit journal, so
//! C-x r j still lands on the same text after you've edited above it.

use crate::app::App;
use crate::minibuffer::Purpose;
use forge_core::registers::Register;
use forge_core::{Pos, rect};

/// What to do once the register name has been typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegAction {
    CopyText { delete: bool },
    CopyRectangle { delete: bool },
    Insert { point_after: bool },
    SavePoint,
    Jump,
    SetNumber(i64),
    Increment(i64),
}

impl RegAction {
    fn prompt(self) -> &'static str {
        match self {
            Self::CopyText { .. } => "Copy to register: ",
            Self::CopyRectangle { .. } => "Copy rectangle to register: ",
            Self::Insert { .. } => "Insert register: ",
            Self::SavePoint => "Point to register: ",
            Self::Jump => "Jump to register: ",
            Self::SetNumber(_) => "Number to register: ",
            Self::Increment(_) => "Increment register: ",
        }
    }
}

fn describe(r: &Register) -> String {
    match r {
        Register::Text(t) => {
            let one_line: String = t
                .chars()
                .take(60)
                .map(|c| if c == '\n' { '⏎' } else { c })
                .collect();
            format!(
                "text: {one_line}{}",
                if t.chars().count() > 60 { "…" } else { "" }
            )
        }
        Register::Rectangle(lines) => format!(
            "rectangle, {} lines: {}",
            lines.len(),
            lines.first().map_or("", |l| l.as_str())
        ),
        Register::Position { offset, .. } => format!("position {offset}"),
        Register::Number(n) => format!("number {n}"),
    }
}

/// The region as (start, end), or an error in the echo area.
fn region(app: &mut App) -> Option<(Pos, Pos)> {
    let r = app.buf_ref().region();
    if r.is_none() {
        app.error("The mark is not set now, so there is no region");
    }
    r
}

/// Corners of the rectangle: (mark, point), unordered.
fn corners(app: &mut App) -> Option<(Pos, Pos)> {
    match app.buf_ref().mark() {
        Some(m) => Some((m, app.buf_ref().point())),
        None => {
            app.error("The mark is not set now, so there is no region");
            None
        }
    }
}

pub fn execute(app: &mut App, cmd: &str) -> bool {
    let ask = |app: &mut App, a: RegAction| app.prompt(a.prompt(), Purpose::Register(a));
    match cmd {
        // ------------------------------------------------ rectangles
        "kill-rectangle" | "delete-rectangle" => {
            if let Some((a, b)) = corners(app) {
                let lines = rect::delete(app.buf(), a, b);
                if cmd == "kill-rectangle" {
                    app.editor.kill_ring.push(lines.join("\n"));
                    app.killed_rectangle = lines;
                }
            }
        }
        "copy-rectangle-as-kill" => {
            if let Some((a, b)) = corners(app) {
                let lines = rect::extract(app.buf_ref(), a, b);
                app.editor.kill_ring.push(lines.join("\n"));
                app.killed_rectangle = lines;
                app.buf().deactivate_mark();
                app.msg("Rectangle saved");
            }
        }
        "yank-rectangle" => {
            if app.killed_rectangle.is_empty() {
                app.error("No rectangle has been killed");
            } else {
                let lines = app.killed_rectangle.clone();
                let at = app.buf_ref().point();
                app.buf().push_mark(at, false);
                rect::insert(app.buf(), at, &lines);
            }
        }
        "open-rectangle" => {
            if let Some((a, b)) = corners(app) {
                rect::open(app.buf(), a, b);
            }
        }
        "clear-rectangle" => {
            if let Some((a, b)) = corners(app) {
                rect::clear(app.buf(), a, b);
            }
        }
        "string-rectangle" => {
            if corners(app).is_some() {
                app.prompt("String rectangle: ", Purpose::StringRectangle);
            }
        }
        "rectangle-number-lines" => {
            if let Some((a, b)) = corners(app) {
                let first = if app.has_arg() { app.count() } else { 1 };
                rect::number_lines(app.buf(), a, b, first);
            }
        }

        // ------------------------------------------------- registers
        "copy-to-register" => {
            let delete = app.has_arg();
            if region(app).is_some() {
                ask(app, RegAction::CopyText { delete });
            }
        }
        "copy-rectangle-to-register" => {
            let delete = app.has_arg();
            if corners(app).is_some() {
                ask(app, RegAction::CopyRectangle { delete });
            }
        }
        "insert-register" => {
            let point_after = app.has_arg();
            ask(app, RegAction::Insert { point_after });
        }
        "point-to-register" => ask(app, RegAction::SavePoint),
        "jump-to-register" => ask(app, RegAction::Jump),
        "number-to-register" => {
            let n = if app.has_arg() { app.count() } else { 0 };
            ask(app, RegAction::SetNumber(n));
        }
        "increment-register" => {
            let n = app.count();
            ask(app, RegAction::Increment(n));
        }
        "list-registers" => {
            let mut text = String::from("Registers\n\n");
            if app.editor.registers.is_empty() {
                text.push_str("No registers are set. C-x r s copies text into one.\n");
            }
            for (c, r) in &app.editor.registers {
                text.push_str(&format!("{c}  {}\n", describe(r)));
            }
            app.show_special_buffer("*Registers*", "special-mode", &text);
        }
        _ => return false,
    }
    true
}

/// The register name was typed.
pub fn run(app: &mut App, action: RegAction, name: char) {
    match action {
        RegAction::CopyText { delete } => {
            let Some((a, b)) = region(app) else { return };
            let text = app.buf_ref().text_range(a, b);
            app.editor.registers.insert(name, Register::Text(text));
            if delete {
                app.buf().delete_range(a, b);
            } else {
                app.buf().deactivate_mark();
            }
            app.msg(format!("Copied to register {name}"));
        }
        RegAction::CopyRectangle { delete } => {
            let Some((a, b)) = corners(app) else { return };
            let lines = if delete {
                rect::delete(app.buf(), a, b)
            } else {
                rect::extract(app.buf_ref(), a, b)
            };
            app.editor
                .registers
                .insert(name, Register::Rectangle(lines));
            app.buf().deactivate_mark();
            app.msg(format!("Copied rectangle to register {name}"));
        }
        RegAction::Insert { point_after } => {
            let Some(r) = app.editor.registers.get(&name).cloned() else {
                return app.error(format!("Register {name} is empty"));
            };
            let start = app.buf_ref().point();
            match r {
                Register::Text(t) => app.buf().insert_str(&t),
                Register::Number(n) => app.buf().insert_str(&n.to_string()),
                Register::Rectangle(lines) => rect::insert(app.buf(), start, &lines),
                Register::Position { .. } => {
                    return app.error(format!("Register {name} holds a position, not text"));
                }
            }
            // : point before the text, mark after (C-u swaps them).
            let end = app.buf_ref().point();
            if point_after {
                app.buf().push_mark(start, false);
            } else {
                app.buf().push_mark(end, false);
                app.buf().set_point(start);
            }
        }
        RegAction::SavePoint => {
            let (buffer, b) = (app.active_id(), app.buf_ref());
            let reg = Register::Position {
                buffer,
                offset: b.point_offset(),
                version: b.version(),
            };
            app.editor.registers.insert(name, reg);
            app.msg(format!("Saved point in register {name}"));
        }
        RegAction::Jump => match app.editor.registers.get(&name).cloned() {
            Some(Register::Position {
                buffer,
                offset,
                version,
            }) => {
                if app.editor.state.get_buffer(buffer).is_none() {
                    return app.error("That register's buffer no longer exists");
                }
                app.switch_to(buffer);
                let o = app.buf_ref().transform_offset(offset, version);
                app.buf().set_offset(o);
            }
            Some(_) => app.error(format!("Register {name} doesn't contain a position")),
            None => app.error(format!("Register {name} is empty")),
        },
        RegAction::SetNumber(n) => {
            app.editor.registers.insert(name, Register::Number(n));
            app.msg(format!("Register {name} = {n}"));
        }
        RegAction::Increment(n) => match app.editor.registers.get_mut(&name) {
            Some(Register::Number(v)) => {
                *v += n;
                let v = *v;
                app.msg(format!("Register {name} = {v}"));
            }
            Some(Register::Text(t)) => {
                // : append the region to a text register.
                let mut t = t.clone();
                let Some((a, b)) = region(app) else { return };
                t.push_str(&app.buf_ref().text_range(a, b));
                app.editor.registers.insert(name, Register::Text(t));
                app.buf().deactivate_mark();
                app.msg(format!("Appended to register {name}"));
            }
            _ => app.error(format!("Register {name} doesn't contain a number or text")),
        },
    }
}
