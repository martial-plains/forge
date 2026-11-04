// SPDX-License-Identifier: MIT OR Apache-2.0
//! What every frontend needs to describe a screen: the display options
//! redisplay takes, the completion list's rows, and the echo area's text.
//!
//! Ratatui painting stays in the terminal frontend; this is the part both
//! it and the macOS app (crates/ffi) share, so the two can't describe a
//! screen differently.

use crate::app::App;
use forge_core::display::{DisplayOptions, Face, Span as CoreSpan};
use unicode_width::UnicodeWidthStr;

pub fn display_options(app: &App) -> DisplayOptions {
    DisplayOptions {
        highlight: app.highlight,
        mode_line_extra: app.mode_line_extra(),
        line_numbers: app.line_numbers,
        wrap: app.wrap,
        hl_line: app.hl_line,
        fill_column_indicator: app.fill_column_indicator,
        show_trailing_whitespace: app.show_trailing_whitespace,
        show_paren: app.show_paren,
        popup: crate::capf::popup(app),
        popup_info: crate::capf::popup_info(app),
        menu_bar: crate::menu::view(app),
        tool_bar: crate::toolbar::view(app),
        modifier_bar: crate::toolbar::modifier_view(app),
        scroll_bar: app.scroll_bar,
        horizontal_scroll_bar: app.horizontal_scroll_bar,
        mode_line_frame: false,
        mode_line_no_line: !app.line_number_mode,
        mode_line_no_column: !app.column_number_mode,
        mode_line_size: app.size_indication_mode,
        pixel_text: false,
        left_margin: crate::org_flow::setting_number(app, "left_margin_width")
            .map_or(0, |n| n.max(0.0) as usize),
        right_margin: crate::org_flow::setting_number(app, "right_margin_width")
            .map_or(0, |n| n.max(0.0) as usize),
    }
}

/// The vertical completion list's rows as spans, `width` cells each:
/// candidate, then its annotation (dimmed), the selected one highlighted.
pub fn vertical_rows(rows: &[(String, String, bool)], width: usize) -> Vec<Vec<CoreSpan>> {
    let text_w = rows
        .iter()
        .map(|r| r.0.width())
        .max()
        .unwrap_or(0)
        .min(width.saturating_sub(2) * 2 / 3);
    rows.iter()
        .map(|(text, note, selected)| {
            let face = if *selected {
                Face::PopupSelected
            } else {
                Face::Default
            };
            let mut t = forge_core::width::truncate(text, text_w);
            t.push_str(&" ".repeat(text_w.saturating_sub(t.width()) + 2));
            let room = width.saturating_sub(t.width());
            let mut n = forge_core::width::truncate(note, room);
            if *selected {
                n.push_str(&" ".repeat(room.saturating_sub(n.width())));
            }
            let note_face = if *selected {
                Face::PopupSelected
            } else {
                Face::LineNumber
            };
            vec![
                CoreSpan { text: t, face },
                CoreSpan {
                    text: n,
                    face: note_face,
                },
            ]
        })
        .collect()
}

/// The echo area's text, and the cursor's column in it when the cursor
/// belongs there (a prompt, or reading keys) rather than in a window.
/// Shared by every frontend.
pub fn echo_line(app: &App) -> (String, Option<usize>) {
    let mut cursor = None;
    let text = if let Some(mb) = &app.minibuffer {
        let text = mb.text();
        if mb.cursor_in_echo_area() {
            cursor = Some(mb.text_to_cursor().width());
        }
        format!("{text}{}", mb.note.as_deref().unwrap_or(""))
    } else {
        app.message.clone().unwrap_or_default()
    };
    let reading_keys = app.key_reader.is_some() || app.waiting_for_key();
    if cursor.is_none() && reading_keys {
        cursor = Some(app.message.as_deref().unwrap_or("").width());
    }
    (text, cursor)
}
