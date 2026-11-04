// SPDX-License-Identifier: MIT OR Apache-2.0
//! What is under the pointer: whether a click would do something there, the
//! stretch of text that would be clicked, and the help text Forge's
//! `tooltip-mode` shows when the pointer rests on it (`help-echo`).

use crate::app::App;
use crate::mouse::{Hit, hit};
use forge_core::{Pos, WindowId};
use forge_docs::CommandDoc;

pub const COMMANDS: &[CommandDoc] = &[CommandDoc::line(
    "tooltip-mode",
    "Toggle tooltips: help text shown when the pointer rests on something.",
)];

/// `tooltip-mode`.
pub fn execute(app: &mut App, cmd: &str) -> bool {
    if cmd != "tooltip-mode" {
        return false;
    }
    app.tooltip_mode = if app.has_arg() {
        app.count() > 0
    } else {
        !app.tooltip_mode
    };
    app.msg(format!(
        "Tooltip mode {}",
        if app.tooltip_mode {
            "enabled"
        } else {
            "disabled"
        }
    ));
    true
}

/// A clickable stretch of text under the pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hover {
    /// Frame row, and the screen columns it covers (end exclusive).
    pub row: u16,
    pub start: u16,
    pub end: u16,
    /// The help text; empty when `tooltip-mode` is off.
    pub tooltip: String,
}

/// The clickable thing at screen cell (`x`, `y`), if there is one.
pub fn at(app: &App, x: u16, y: u16) -> Option<Hover> {
    let Hit::Text(id, p) = hit(app, x, y) else {
        return None;
    };
    let w = app.editor.windows.get(id)?;
    let b = app.editor.state.get_buffer(w.buffer)?;
    // A picture isn't a link to the pointer.
    if on_picture(app, id, y, b) {
        return None;
    }
    let (line, a, z, tip) = target(app, b, p)?;
    let (start, end) = screen_span(app, x, y, id, line, a, z);
    Some(Hover {
        row: y,
        start,
        end,
        tooltip: if app.tooltip_mode { tip } else { String::new() },
    })
}

/// Whether screen row `y` of window `id` is one of the blank rows a
/// picture is drawn over.
fn on_picture(app: &App, id: WindowId, y: u16, b: &forge_core::Buffer) -> bool {
    let frame = app.editor.windows.frame();
    let Some((_, r)) = app
        .editor
        .windows
        .layout(frame)
        .into_iter()
        .find(|(i, _)| *i == id)
    else {
        return false;
    };
    let Some(w) = app.editor.windows.get(id) else {
        return false;
    };
    let Some(row) = (y.saturating_sub(r.y) as usize).checked_sub(w.header_rows) else {
        return false;
    };
    picture_row(
        &w.rows,
        row,
        |l| b.line_len(l),
        |l| b.inline_image(l).is_some(),
    )
}

/// The window's rows are (line, first column); a picture's rows follow the
/// line's last text row and start at its end.
fn picture_row(
    rows: &[(usize, usize)],
    row: usize,
    len: impl Fn(usize) -> usize,
    has_picture: impl Fn(usize) -> bool,
) -> bool {
    let Some(&(line, from)) = rows.get(row) else {
        return false;
    };
    row > 0 && rows[row - 1].0 == line && from >= len(line) && has_picture(line)
}

/// What a click at `p` would act on: the line, the character columns of the
/// stretch to highlight, and its help text.
fn target(app: &App, b: &forge_core::Buffer, p: Pos) -> Option<(usize, usize, usize, String)> {
    let text = b.line(p.line);
    if b.major_mode() == forge_core::org::MODE {
        if let Some(l) = b.org_link_at_point(p) {
            return Some((p.line, l.start, l.end, format!("LINK: {}", l.target)));
        }
        if forge_core::org::heading_level(&text).is_none()
            && let Some((indent, bullet, Some(_))) = forge_core::org::list_item(&text)
        {
            let a = indent + bullet.chars().count() + 1;
            if p.col >= a && p.col < a + 3 {
                return Some((p.line, a, a + 3, "mouse-1: toggle checkbox".to_string()));
            }
        }
        return None;
    }
    if b.major_mode() == crate::org_agenda_ui::MODE {
        let st = app.org.agenda.as_ref()?;
        let entry = st
            .rendered
            .refs
            .get(p.line)
            .copied()
            .flatten()
            .and_then(|i| st.rendered.entries.get(i))?;
        let from = text.chars().take_while(|c| c.is_whitespace()).count();
        let to = text.trim_end().chars().count();
        if p.col < from || p.col >= to {
            return None;
        }
        let file = st
            .ids
            .get(entry.source)
            .and_then(|id| app.editor.state.get_buffer(*id))
            .map(|b| b.name().to_string())
            .unwrap_or_default();
        return Some((
            p.line,
            from,
            to,
            format!("mouse-1, RET: jump to the entry in {file}"),
        ));
    }
    // Help, Info and other buffers with links of their own.
    let off = b.pos_to_offset(p);
    let span = b.link_at(off)?;
    let (s, e) = (b.offset_to_pos(span.start), b.offset_to_pos(span.end));
    (s.line == p.line && e.line == p.line).then(|| {
        (
            p.line,
            s.col,
            e.col,
            "mouse-1, RET: follow this link".to_string(),
        )
    })
}

/// The screen columns, around column `x`, whose cells show buffer columns
/// `a..z` of `line`. Found by asking what each neighbouring cell shows, so
/// collapsed links and wide characters come out right.
fn screen_span(
    app: &App,
    x: u16,
    y: u16,
    id: WindowId,
    line: usize,
    a: usize,
    z: usize,
) -> (u16, u16) {
    let inside = |cx: u16| matches!(hit(app, cx, y), Hit::Text(i, p) if i == id && p.line == line && p.col >= a && p.col < z);
    let (mut s, mut e) = (x, x);
    let mut guard = 0;
    while s > 0 && inside(s - 1) && guard < 400 {
        s -= 1;
        guard += 1;
    }
    guard = 0;
    while e < u16::MAX - 1 && inside(e + 1) && guard < 400 {
        e += 1;
        guard += 1;
    }
    (s, e + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::display::{DisplayOptions, redisplay};
    use forge_core::{Buffer, Editor, Rect};

    fn org_app(text: &str) -> App {
        let mut app = App::new(Editor::default());
        let mut b = Buffer::new("t.org");
        b.set_contents(text);
        b.set_major_mode("org-mode");
        let id = app.editor.add_buffer(b);
        app.editor.show_buffer(id);
        crate::menu::set_mode(&mut app, false);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        redisplay(&mut app.editor, area, &DisplayOptions::default());
        app
    }

    #[test]
    fn org_links_and_checkboxes_say_they_are_clickable() {
        let mut app = org_app("see [[https://a.b/c][the site]] now\n- [ ] todo\n");
        let frame = app.editor.windows.frame();
        let (x0, y0) = (frame.x, frame.y);
        // "the site" is shown from column 4 (the markup is collapsed).
        let on_link = at(&app, x0 + 6, y0).expect("over the link");
        assert_eq!(on_link.tooltip, "LINK: https://a.b/c");
        assert_eq!((on_link.end - on_link.start) as usize, "the site".len());
        assert!(at(&app, x0, y0).is_none(), "plain text is not clickable");
        let on_box = at(&app, x0 + 3, y0 + 1).expect("over the checkbox");
        assert_eq!(on_box.tooltip, "mouse-1: toggle checkbox");
        app.tooltip_mode = false;
        assert_eq!(at(&app, x0 + 6, y0).unwrap().tooltip, "", "tooltips off");
    }

    #[test]
    fn picture_rows_are_not_links() {
        // Line 3 is 10 long and wraps once; then two picture rows.
        let rows = [(2, 0), (3, 0), (3, 6), (3, 10), (3, 10), (4, 0)];
        let len = |_| 10;
        let pic = |l| l == 3;
        let hit = |r| picture_row(&rows, r, len, pic);
        assert!(!hit(0) && !hit(1) && !hit(2));
        assert!(hit(3) && hit(4));
        assert!(!hit(5) && !hit(9));
        // A line with no picture: an empty second row is just text.
        assert!(!picture_row(&rows, 3, len, |_| false));
    }
}
