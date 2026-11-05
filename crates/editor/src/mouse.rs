// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mouse support (xterm-mouse-mode): the wheel scrolls the window under
//! the pointer, a click moves point there, a drag selects a region.
//!
//! While Forge captures the mouse, the terminal's own click-and-drag text
//! selection is off; most terminals still select with Shift held down.
//! `M-x xterm-mouse-mode` (or `forge.opt.mouse = false` in init.rhai) turns it off.

use crate::app::App;
use forge_core::width::col_at_display;
use forge_core::{Pos, WindowId};
use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

/// Lines per wheel notch.
pub const WHEEL_LINES: usize = 3;

/// Columns scrolled when clicking beside the horizontal scroll bar's thumb.
pub const HSCROLL_STEP: usize = 4;

/// Where a screen cell is.
pub(crate) enum Hit {
    /// A text cell of a window: (window, buffer position).
    Text(WindowId, Pos),
    /// A window's mode line, at this column from its left edge.
    ModeLine(WindowId, u16),
    /// A vertical scroll bar at this text row (from the window's top).
    ScrollBarV(WindowId, usize),
    /// A horizontal scroll bar at this text column (after the gutter).
    ScrollBarH(WindowId, usize),
    /// The menu bar's row, at this column.
    MenuBar(u16),
    /// The tool bar's row, at this column.
    ToolBar(u16),
    /// The modifier bar's row, at this column.
    ModifierBar(u16),
    /// The tab bar's row, at this column.
    TabBar(u16),
    Nothing,
}

pub(crate) fn hit(app: &App, x: u16, y: u16) -> Hit {
    let frame = app.editor.windows.frame();
    let on_frame = x >= frame.x && x < frame.x + frame.width;
    // The bar's row sits above the windows, so it's found before them: the
    // windows' area already starts below it (see menu::frame_areas_with).
    if crate::menu::visible(app) && y == crate::menu::bar_row(app) && on_frame {
        return Hit::MenuBar(x - frame.x);
    }
    // The tool bar is below the menu bar and above the modifier bar and
    // tabs, unless its position puts it at the bottom.
    let tool_bottom = crate::toolbar::position(app).is_bottom();
    if crate::toolbar::visible(app) && !tool_bottom && y == crate::toolbar::bar_row(app) && on_frame
    {
        return Hit::ToolBar(x - frame.x);
    }
    if crate::toolbar::modifier_visible(app)
        && y == crate::toolbar::modifier_bar_row(app)
        && on_frame
    {
        return Hit::ModifierBar(x - frame.x);
    }
    // The tab bar's row sits between the modifier bar and the windows.
    if crate::tabs::visible(app) && y == crate::tabs::bar_row(app) && on_frame {
        return Hit::TabBar(x - frame.x);
    }
    // A bottom tool bar is below the windows, not part of them.
    if crate::toolbar::visible(app)
        && tool_bottom
        && y == crate::toolbar::bottom_bar_row(app)
        && on_frame
    {
        return Hit::ToolBar(x - frame.x);
    }
    for (id, r) in app.editor.windows.layout(frame) {
        if x < r.x || x >= r.x + r.width || y < r.y || y >= r.y + r.height {
            continue;
        }
        if y == r.y + r.height - 1 {
            return Hit::ModeLine(id, x - r.x);
        }
        let w = app.editor.windows.get(id).expect("laid out");
        // The column/row a scroll bar reserved belongs to the bar, not the
        // text under it. The bars are found before the text branch below.
        let rows_total = (w.height + w.header_rows) as u16;
        let border = r.x + r.width < frame.x + frame.width;
        let full_w = r.width.saturating_sub(u16::from(border));
        if w.scroll_bar.is_some() {
            let bx = r.x + full_w.saturating_sub(1);
            if x == bx && y >= r.y && y < r.y + rows_total {
                return Hit::ScrollBarV(id, (y - r.y) as usize);
            }
        }
        if w.horizontal_scroll_bar.is_some() {
            let by = r.y + rows_total;
            let gx = r.x + w.gutter as u16;
            if y == by && x >= gx && x < r.x + full_w {
                return Hit::ScrollBarH(id, (x - gx) as usize);
            }
        }
        let Some(b) = app.editor.state.get_buffer(w.buffer) else {
            return Hit::Nothing;
        };
        let row = (y - r.y) as usize;
        // A header line above the text isn't text.
        if row < w.header_rows {
            return Hit::Nothing;
        }
        let row = row - w.header_rows;
        // The rows of the last redisplay say which line each screen row
        // shows (if the window hasn't scrolled since): folds hide lines,
        // wrapped lines share them, so arithmetic from scroll_top is
        // wrong whenever either is in play.
        let fresh = w.rows.first().is_some_and(|r| r.0 == w.scroll_top);
        let (line, from) = match w.rows.get(row) {
            Some(&(line, from)) if fresh => (line, from),
            None if fresh => (b.line_count() - 1, 0),
            _ => {
                let extra = if w.scroll_y_px > 0 { 1usize } else { 0 };
                ((w.scroll_top + row + extra).min(b.line_count() - 1), 0)
            }
        };
        // The rows a click uses are laid out from the shown text, not the
        // source line. Descriptive links hide their markup there, so map
        // the clicked display column back through the same line to get the
        // buffer column (and so the link a click landed on). Other Org
        // stand-ins (tables, entities, previews) have no such map.
        let raw = b.line(line);
        let desc = if b.major_mode() == forge_core::org::MODE
            && (b.point().line == line || b.org_display_line(line).is_none())
        {
            // Links stay collapsed under point (the display never expands
            // them), so the map back to columns never leaves them whole.
            b.org_descriptive_line(line, None)
        } else {
            None
        };
        let shown = desc.as_ref().map_or(raw.as_str(), |(t, _)| t.as_str());
        let chunk: String = shown.chars().skip(from).collect();
        // Clicks on line numbers go to the start of the line.
        let x = (x - r.x) as usize;
        let displayed = if x < w.gutter {
            from
        } else {
            // Org indent mode prefixes virtual spaces (first visual row
            // only): they aren't buffer columns.
            let indent = if from == 0 {
                forge_core::org_display::org_indent_width(b, line, app.editor.org_indent_per_level)
            } else {
                0
            };
            let mut off = (w.hscroll + x - w.gutter).saturating_sub(indent);
            // Org num mode inserts the section number after the stars (first
            // visual row only): skip past it when mapping back to columns.
            if from == 0
                && b.org_display.num
                && let (Some(n), Some(num)) = (
                    b.org_level(line),
                    forge_core::org_display::org_line_number(b, line),
                )
            {
                let at = (n + 1).min(b.line_len(line));
                if off > at {
                    off = off.saturating_sub(num.chars().count());
                }
            }
            from + col_at_display(&chunk, off)
        };
        // Past the shown text is the end of the real line, not the same
        // column of the longer source.
        let col = desc.as_ref().map_or(displayed, |(_, src)| {
            src.get(displayed)
                .copied()
                .unwrap_or_else(|| raw.chars().count())
        });
        return Hit::Text(id, Pos::new(line, col));
    }
    Hit::Nothing
}

/// The pointer moved: mark the menu title, tool bar item, and modifier
/// button under it, if any.
fn hover_bars(app: &mut App, ev: &MouseEvent) {
    let frame = app.editor.windows.frame();
    let on_frame = ev.column >= frame.x && ev.column < frame.x + frame.width;
    // Saturating: a report above the frame (or left of it) is "not the
    // bar", not a panic.
    let x = ev.column.saturating_sub(frame.x);
    let y = ev.row.saturating_sub(frame.y);
    let on_menu = crate::menu::visible(app) && on_frame && ev.row == crate::menu::bar_row(app);
    crate::menu::hover(app, x, y, on_menu);
    let tool_bottom = crate::toolbar::position(app).is_bottom();
    let on_tool = crate::toolbar::visible(app)
        && on_frame
        && if tool_bottom {
            ev.row == crate::toolbar::bottom_bar_row(app)
        } else {
            ev.row == crate::toolbar::bar_row(app)
        };
    crate::toolbar::hover(app, x, on_tool);
    let on_modifier = crate::toolbar::modifier_visible(app)
        && on_frame
        && ev.row == crate::toolbar::modifier_bar_row(app);
    crate::toolbar::hover_modifier(app, x, on_modifier);
}

fn window_at(app: &App, x: u16, y: u16) -> Option<WindowId> {
    match hit(app, x, y) {
        Hit::Text(id, _)
        | Hit::ModeLine(id, _)
        | Hit::ScrollBarV(id, _)
        | Hit::ScrollBarH(id, _) => Some(id),
        Hit::MenuBar(_) | Hit::ToolBar(_) | Hit::ModifierBar(_) | Hit::TabBar(_) | Hit::Nothing => {
            None
        }
    }
}

pub fn handle(app: &mut App, ev: MouseEvent) {
    app.pointer = (ev.column, ev.row);
    // Clicks and the wheel are input (they put off the idle auto-save);
    // the pointer merely moving is not.
    if !matches!(ev.kind, MouseEventKind::Moved) {
        app.note_input();
        if matches!(ev.kind, MouseEventKind::Down(MouseButton::Left))
            && crate::capf::click(app, ev.column, ev.row)
        {
            return;
        }
        // A click on a candidate in *Quail Completions* chooses it; any
        // other click ends an input method's translation where it stands.
        if app.im.translation.is_some()
            && matches!(
                ev.kind,
                MouseEventKind::Down(MouseButton::Left | MouseButton::Middle)
            )
            && let Hit::Text(id, p) = hit(app, ev.column, ev.row)
            && let Some(buffer) = app.editor.windows.get(id).map(|w| w.buffer)
            && crate::input_method::choose_at(app, buffer, p.line, p.col)
        {
            return;
        }
        crate::input_method::finish(app);
    }
    // A canvas takes the pointer over its picture.
    if app.minibuffer.is_none() && crate::canvas::mouse(app, &ev) {
        return;
    }
    let busy = app.minibuffer.is_some()
        || app.query.is_some()
        || app.key_reader.is_some()
        || app.quoted.is_some();
    match ev.kind {
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            // The wheel on the tab bar steps through the tabs, as Forge's
            // `tab-bar` does; elsewhere it scrolls the window under it.
            if let Hit::TabBar(_) = hit(app, ev.column, ev.row) {
                let n = if ev.kind == MouseEventKind::ScrollDown {
                    1
                } else {
                    -1
                };
                app.editor.select_tab(app.editor.windows.other_tab(n));
            } else if let Some(id) = window_at(app, ev.column, ev.row) {
                scroll(app, id, ev.kind == MouseEventKind::ScrollDown);
            }
        }
        // Clicks would pull point out from under a prompt; ignore them then.
        _ if busy => {}
        // The middle button moves point like the left one does, so a
        // <mouse-2> binding means something where the user clicked. It
        // doesn't drag a region.
        MouseEventKind::Down(MouseButton::Middle) => {
            // A middle click on a tab closes it, like tab bar.
            if let Hit::TabBar(column) = hit(app, ev.column, ev.row) {
                if let Some(id) = crate::tabs::tab_at(app, column) {
                    let _ = app.editor.delete_tab(id);
                }
                return;
            }
            if let Hit::ModeLine(id, col) = hit(app, ev.column, ev.row) {
                crate::mode_line::click(
                    app,
                    id,
                    col,
                    crate::mode_line::Button::Middle,
                    ev.column,
                    ev.row,
                );
                return;
            }
            if let Hit::Text(id, p) = hit(app, ev.column, ev.row) {
                app.select_window(id);
                let b = app.buf();
                b.deactivate_mark();
                b.set_point(p);
                app.message = None;
                app.dispatch_mouse(&crate::keymap::describe_mouse_button(MouseButton::Middle));
            }
        }
        // The right button opens the context menu (context-menu-mode), or
        // dismisses one that's open.
        MouseEventKind::Down(MouseButton::Right) => {
            let was_open = crate::menu::active(app);
            if was_open {
                crate::menu::close(app);
            }
            if let Hit::ModeLine(id, col) = hit(app, ev.column, ev.row) {
                crate::mode_line::click(
                    app,
                    id,
                    col,
                    crate::mode_line::Button::Right,
                    ev.column,
                    ev.row,
                );
                return;
            }
            // A mode that binds <mouse-3> (Bases) answers the right button
            // itself, with or without context-menu-mode.
            if let Hit::Text(id, p) = hit(app, ev.column, ev.row)
                && matches!(
                    app.editor
                        .active_keymaps()
                        .lookup(&["<mouse-3>".to_string()]),
                    forge_core::keymap::Lookup::Command { .. }
                )
            {
                app.select_window(id);
                let b = app.buf();
                b.deactivate_mark();
                b.set_point(p);
                app.dispatch_mouse("<mouse-3>");
                return;
            }
            if let Hit::Text(id, p) = hit(app, ev.column, ev.row)
                && crate::menu::context_mode_on(app)
            {
                app.select_window(id);
                let frame = app.editor.windows.frame();
                let off = app.buf_ref().pos_to_offset(p);
                crate::menu::open_context(
                    app,
                    ev.column.saturating_sub(frame.x),
                    ev.row.saturating_sub(frame.y),
                    Some(off),
                );
            }
        }
        MouseEventKind::Down(MouseButton::Left) => match hit(app, ev.column, ev.row) {
            // A click away from the open menu dismisses it, like any menu:
            // leaving it open just to look at the text would trap the next
            // click behind it.
            _ if crate::menu::active(app) && !over_open_menu(app, ev.column, ev.row) => {
                crate::menu::close(app);
            }
            Hit::Text(id, p) => {
                app.select_window(id);
                let b = app.buf();
                b.deactivate_mark();
                b.set_point(p);
                app.mouse_anchor = Some(app.buf_ref().point_offset());
                app.message = None;
            }
            Hit::ModeLine(id, col) => crate::mode_line::click(
                app,
                id,
                col,
                crate::mode_line::Button::Left,
                ev.column,
                ev.row,
            ),
            Hit::ScrollBarV(id, row) => scroll_bar_click(app, id, false, row),
            Hit::ScrollBarH(id, col) => scroll_bar_click(app, id, true, col),
            // Pressing a title opens that menu and leaves it for the keys.
            Hit::MenuBar(column) => {
                crate::menu::click_bar(app, column);
            }
            // A tool bar button runs its command; a modifier button waits
            // for the next key.
            Hit::ToolBar(column) => {
                crate::toolbar::click(app, column);
            }
            Hit::ModifierBar(column) => {
                crate::toolbar::click_modifier(app, column);
            }
            // A click on a tab selects it; a click on the empty space after
            // the last tab makes a new one, as tab bar does.
            Hit::TabBar(column) => {
                let v = crate::tabs::view(app);
                if let Some(v) = &v
                    && let Some((id, _)) = v
                        .closes
                        .iter()
                        .find(|(_, x)| *x == column || (*x > 0 && column == *x + 1))
                {
                    let _ = app.editor.delete_tab(*id);
                    return;
                }
                if let Some(id) = crate::tabs::tab_at(app, column) {
                    app.editor.select_tab(id);
                }
            }
            Hit::Nothing => {}
        },
        // Motion: highlight the menu bar's title under the pointer, and
        // follow it from one title to the next while a menu is open.
        MouseEventKind::Moved => hover_bars(app, &ev),
        MouseEventKind::Drag(MouseButton::Left) => {
            // Dragging past the bar still counts as hovering it.
            hover_bars(app, &ev);
            // A drag on a scroll bar's thumb scrolls, it doesn't select.
            if let Some((id, horizontal)) = app.scroll_bar_drag {
                scroll_bar_drag(app, id, horizontal, ev.column, ev.row);
                return;
            }
            let Some(anchor) = app.mouse_anchor else {
                return;
            };
            if let Hit::Text(id, p) = hit(app, ev.column, ev.row)
                && id == app.editor.windows.selected_id()
            {
                let a = app.buf_ref().offset_to_pos(anchor);
                app.buf().push_mark(a, true);
                app.buf().set_point(p);
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if app.scroll_bar_drag.take().is_some() {
                return;
            }
            let anchor = app.mouse_anchor.take();
            // A press that ended where it began was a click, not a drag. A
            // click in the text is dispatched as <mouse-1> below, which is
            // how a name in a Dired listing opens the file.
            // A click in an open menu picks an item; one on the bar's row
            // toggles its menu. Neither moves point, so they don't need the
            // anchor check a click in the text does.
            // A click in the panel picks an item. The bar's own row is not
            // part of the panel: a click there is a title click, below.
            let frame = app.editor.windows.frame();
            let (x, y, on_bar) = (
                ev.column.saturating_sub(frame.x),
                ev.row.saturating_sub(frame.y),
                crate::menu::visible(app) && ev.row == crate::menu::bar_row(app),
            );
            if !on_bar && crate::menu::active(app) && crate::menu::click_item(app, x, y) {
                return;
            }
            let clicked = anchor.is_some() && anchor == Some(app.buf_ref().point_offset());
            if !clicked
                && app.mouse_drag_copy_region
                && let Some((a, b)) = app.buf_ref().region()
            {
                let text = app.buf_ref().text_range(a, b);
                app.kill_ring_save(text);
            }
            if clicked && crate::menu::visible(app) && ev.row == crate::menu::bar_row(app) {
                crate::menu::click_bar(app, ev.column - app.editor.windows.frame().x);
                return;
            }
            // A click in the text is dispatched through the keymaps like
            // any other key, so the region map of whatever point landed on
            // can bind it: a name in a Dired listing is one of those.
            // Point has already moved, so <mouse-1> means "here".
            if clicked {
                app.dispatch_mouse("<mouse-1>");
            }
        }
        _ => {}
    }
}

/// A press on a scroll bar: on the thumb it starts a drag; beside it the
/// window scrolls a screen (vertical) or a few columns (horizontal).
fn scroll_bar_click(app: &mut App, id: WindowId, horizontal: bool, pos: usize) {
    app.select_window(id);
    app.mouse_anchor = None;
    let thumb = if horizontal {
        app.win().horizontal_scroll_bar
    } else {
        app.win().scroll_bar
    }
    .unwrap_or((0, 0));
    let (start, len) = (thumb.0 as usize, thumb.1 as usize);
    if pos >= start && pos < start + len {
        app.scroll_bar_drag = Some((id, horizontal));
        return;
    }
    let back = pos < start;
    if horizontal {
        let hscroll = app.win().hscroll;
        let next = if back {
            hscroll.saturating_sub(HSCROLL_STEP)
        } else {
            hscroll + HSCROLL_STEP
        };
        app.scroll_to_column(next);
    } else {
        // above the thumb scrolls back; below, forward (like M-v / C-v).
        crate::commands::scroll_screen(app, !back);
    }
}

/// Drag a scroll bar's thumb: put its middle under the pointer and scroll
/// the selected window to match.
fn scroll_bar_drag(app: &mut App, id: WindowId, horizontal: bool, x: u16, y: u16) {
    if app.editor.windows.selected_id() != id {
        return;
    }
    let frame = app.editor.windows.frame();
    let Some((_, r)) = app
        .editor
        .windows
        .layout(frame)
        .into_iter()
        .find(|(i, _)| *i == id)
    else {
        return;
    };
    if horizontal {
        let Some((start, len)) = app.win().horizontal_scroll_bar else {
            return;
        };
        let _ = start;
        let text_w = app.win().width as u16;
        let (len, text_w) = (len as usize, text_w as usize);
        let gutter = app.win().gutter as u16;
        let col = x.saturating_sub(r.x + gutter) as usize;
        let room = text_w.saturating_sub(len).max(1);
        let desired = col.saturating_sub(len / 2).min(text_w.saturating_sub(len));
        let frac = desired as f64 / room as f64;
        let p = app.buf_ref().point();
        let text = app.buf_ref().line(p.line);
        let max_h = forge_core::width::line_width(&text).saturating_sub(text_w);
        if max_h == 0 {
            return;
        }
        app.scroll_to_column((frac * max_h as f64).round() as usize);
    } else {
        let Some((start, len)) = app.win().scroll_bar else {
            return;
        };
        let _ = start;
        let rows = (app.win().height + app.win().header_rows) as u16;
        let (len, rows) = (len as usize, rows as usize);
        let row = y.saturating_sub(r.y) as usize;
        let room = rows.saturating_sub(len).max(1);
        let desired = row.saturating_sub(len / 2).min(rows.saturating_sub(len));
        let frac = desired as f64 / room as f64;
        let last = app.buf_ref().line_count().saturating_sub(1);
        app.scroll_to_line((frac * last as f64).round() as usize);
    }
}

/// Scroll a window without selecting it, keeping its point inside it.
fn scroll(app: &mut App, id: WindowId, down: bool) {
    let here = app.editor.windows.selected_id();
    if id == here {
        let top = app.win().scroll_top;
        // Step through shown rows: raw arithmetic stalls in folded text,
        // landing scroll_top inside a fold that snaps back.
        let next = if down {
            app.buf_ref().forward_visible_line(top, WHEEL_LINES)
        } else {
            app.buf_ref().backward_visible_line(top, WHEEL_LINES)
        };
        // The selected window can animate (smooth_scroll); a wheel over
        // any other window scrolls it at once.
        app.set_scroll_target(next);
        return;
    }
    app.editor.select_window(id);
    let top = app.win().scroll_top;
    let next = if down {
        app.buf_ref().forward_visible_line(top, WHEEL_LINES)
    } else {
        app.buf_ref().backward_visible_line(top, WHEEL_LINES)
    };
    app.win_mut().scroll_top = next;
    app.win_mut().scroll_y_px = 0;
    crate::commands::keep_point_in_window(app);
    app.editor.select_window(here);
}

/// Is this cell the menu bar's row, or part of the open panel? A click
/// there belongs to the menu, and leaves it open.
fn over_open_menu(app: &App, x: u16, y: u16) -> bool {
    let frame = app.editor.windows.frame();
    let on_bar = crate::menu::visible(app) && y == crate::menu::bar_row(app);
    crate::menu::over_panel_at(
        app,
        x.saturating_sub(frame.x),
        y.saturating_sub(frame.y),
        on_bar,
    ) || on_bar
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::display::{DisplayOptions, redisplay};
    use forge_core::{Buffer, Editor, Rect};
    use ratatui::crossterm::event::KeyModifiers;

    fn org_app(text: &str) -> App {
        let mut app = App::new(Editor::default());
        let id = app.editor.add_buffer(Buffer::new("*scratch*"));
        app.editor.show_buffer(id);
        app.buf().set_contents(text);
        app.buf().set_major_mode(forge_core::org::MODE);
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
    fn a_click_after_a_collapsed_link_lands_inside_it() {
        let mut app = org_app("see [[https://x.org][the site]] and here\n");
        handle(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 5,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
        );
        let p = app.buf_ref().point();
        assert_eq!(p.line, 0);
        assert!(app.buf_ref().org_link_at_point(p).is_some(), "{p:?}");

        // The link stays collapsed with point inside it, so a click beyond
        // the shown text lands at the end of the real line.
        handle(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 33,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
        );
        let p = app.buf_ref().point();
        assert_eq!(
            p,
            Pos::new(0, 40),
            "the click past the text maps to the end"
        );
    }
}
