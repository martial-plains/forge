// SPDX-License-Identifier: MIT OR Apache-2.0
//! The tool bar and modifier bar, tied to the editor: whether their rows
//! show, what to draw, and the keys and clicks that use them.
//!
//! The bars themselves live in [`forge_core::toolbar`], so both frontends
//! get the same items in the same order; this is the half that needs an
//! `App`: reading the registry for the selected buffer's major mode,
//! running what a click picks, and the modifier keys waiting for the next
//! keypress.

use crate::app::App;
use forge_core::FrameId;
use forge_core::display::Span;
use forge_core::modes::{MODIFIER_BAR_MODE, TOOL_BAR_MODE};
use forge_core::toolbar::{self, Modifier, ModifierBarView, Position, Style, ToolBarView, Toolbar};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Is `tool-bar-mode` on? It's global, like `menu-bar-mode`: the bar
/// frames the window, not the text in it.
pub fn mode_on(app: &App) -> bool {
    app.editor.global_modes.enabled(TOOL_BAR_MODE)
}

/// Is `modifier-bar-mode` on? Also global.
pub fn modifier_mode_on(app: &App) -> bool {
    app.editor.global_modes.enabled(MODIFIER_BAR_MODE)
}

/// Can this frontend draw a tool bar? Only a GUI can; a terminal frame
/// has no tool bar has none with `-nw`.
pub fn available(app: &App) -> bool {
    app.graphical
}

/// Is the tool bar's row showing?
pub fn visible(app: &App) -> bool {
    available(app) && mode_on(app)
}

/// Is the modifier bar's row showing? It's part of the tool bar, so the
/// tool bar has to be on and visible too.
pub fn modifier_visible(app: &App) -> bool {
    available(app) && visible(app) && modifier_mode_on(app)
}

/// Turn `tool-bar-mode` on or off for the whole session. The modifier bar
/// is part of the tool bar, so hiding it also cancels the modifiers that
/// were waiting for the next key.
pub fn set_mode(app: &mut App, on: bool) {
    if on {
        app.editor.global_modes.enable(TOOL_BAR_MODE);
    } else {
        app.editor.global_modes.disable(TOOL_BAR_MODE);
        app.editor.tool_bar.hover = None;
        app.editor.tool_bar.modifier_hover = None;
        app.pending_modifiers.clear();
    }
}

/// Turn `modifier-bar-mode` on or off for the whole session.
pub fn set_modifier_mode(app: &mut App, on: bool) {
    if on {
        app.editor.global_modes.enable(MODIFIER_BAR_MODE);
    } else {
        app.editor.global_modes.disable(MODIFIER_BAR_MODE);
        app.editor.tool_bar.modifier_hover = None;
        app.pending_modifiers.clear();
    }
}

/// The style the tool bar draws with.
pub fn style(app: &App) -> Style {
    app.tool_bar_style
}

/// Where the tool bar goes.
pub fn position(app: &App) -> Position {
    app.tool_bar_position
}

/// The bar the selected buffer shows: its major mode's, or the default
/// one (always the default when `tool-bar-always-show-default` is set).
pub fn bar(app: &App) -> &Toolbar {
    bar_in(app, app.editor.windows.selected_frame_id())
}

/// The bar the selected buffer in `frame` shows, for a native frame that
/// isn't the editor's selected one.
pub fn bar_in(app: &App, frame: FrameId) -> &Toolbar {
    let mode = app
        .editor
        .windows
        .get(app.editor.windows.frame_selected_id(frame))
        .and_then(|w| app.editor.state.get_buffer(w.buffer))
        .map(|b| b.major_mode().to_string())
        .unwrap_or_default();
    app.editor
        .tool_bars
        .bar_for(&mode, app.tool_bar_always_show_default)
}

/// What to draw: the bar's row and its item positions. Nothing when the
/// mode is off.
pub fn view(app: &App) -> Option<ToolBarView> {
    if !visible(app) {
        return None;
    }
    let ctx = crate::menu::context(app);
    let frame = app.editor.windows.frame();
    // Show the icons `icon-preference` picks rather than the stored glyph.
    let mut shown = bar(app).clone();
    for item in &mut shown.items {
        if let Some(g) = crate::icon::toolbar_glyph(app, &item.command) {
            item.icon = g;
        }
    }
    Some(toolbar::view(
        app.editor.tool_bar,
        &shown,
        frame.width,
        style(app),
        &ctx,
    ))
}

/// The modifier bar's row, marking the modifiers already waiting for the
/// next key.
pub fn modifier_view(app: &App) -> Option<ModifierBarView> {
    if !modifier_visible(app) {
        return None;
    }
    let frame = app.editor.windows.frame();
    Some(toolbar::modifier_view(
        app.editor.tool_bar,
        &app.pending_modifiers,
        frame.width,
    ))
}

/// The tool bar's row as spans, for the terminal frontend.
pub fn row(app: &App) -> Vec<Span> {
    view(app).map_or_else(Vec::new, |v| v.row)
}

/// The modifier bar's row as spans.
pub fn modifier_row(app: &App) -> Vec<Span> {
    modifier_view(app).map_or_else(Vec::new, |v| v.row)
}

/// One row when the tool bar is at the top, zero otherwise.
pub fn top_height(app: &App) -> u16 {
    u16::from(visible(app) && !position(app).is_bottom())
}

/// One row when the tool bar is at the bottom, zero otherwise.
pub fn bottom_height(app: &App) -> u16 {
    u16::from(visible(app) && position(app).is_bottom())
}

/// One row when the modifier bar is showing.
pub fn modifier_height(app: &App) -> u16 {
    u16::from(modifier_visible(app))
}

/// The screen row the top tool bar is on: below the menu bar and above
/// the modifier bar and tabs, all of which `menu::frame_areas_with` puts
/// there.
pub fn bar_row(app: &App) -> u16 {
    app.editor
        .windows
        .frame()
        .y
        .saturating_sub(1 + u16::from(crate::tabs::visible(app)) + modifier_height(app))
}

/// The screen row the modifier bar is on: just above the tab bar, below
/// the menu bar and a top tool bar.
pub fn modifier_bar_row(app: &App) -> u16 {
    app.editor
        .windows
        .frame()
        .y
        .saturating_sub(1 + u16::from(crate::tabs::visible(app)))
}

/// The screen row a bottom tool bar is on: below the windows and the
/// vertical completion list, above the echo area.
pub fn bottom_bar_row(app: &App) -> u16 {
    let frame = app.editor.windows.frame();
    frame
        .y
        .saturating_add(frame.height)
        .saturating_add(crate::minibuffer::vertical_height(app))
}

/// The pointer moved to `column` on the frame. If it is on the tool bar
/// (top or bottom, decided by `on_bar`), mark the item under it; otherwise
/// clear the mark. Returns whether the pointer was on the bar's row.
pub fn hover(app: &mut App, column: u16, on_bar: bool) -> bool {
    if !visible(app) {
        app.editor.tool_bar.hover = None;
        return false;
    }
    let hover = if on_bar {
        view(app).and_then(|v| v.item_at(column))
    } else {
        None
    };
    app.editor.tool_bar.hover = hover;
    on_bar
}

/// The pointer moved to `column`: mark the modifier button under it, or
/// clear the mark. Returns whether the pointer was on the modifier row.
pub fn hover_modifier(app: &mut App, column: u16, on_bar: bool) -> bool {
    if !modifier_visible(app) {
        app.editor.tool_bar.modifier_hover = None;
        return false;
    }
    let hover = if on_bar {
        modifier_view(app)
            .and_then(|v| v.button_at(column))
            .map(Modifier::index)
    } else {
        None
    };
    app.editor.tool_bar.modifier_hover = hover;
    on_bar
}

/// A click at `column` on the tool bar: run the item under it, if it can
/// be chosen. False when the click missed every item (a separator, empty
/// space, or a disabled button).
pub fn click(app: &mut App, column: u16) -> bool {
    match view(app).and_then(|v| v.item_at(column)) {
        Some(i) => run_item(app, i),
        None => false,
    }
}

/// Run the item at `index` in the selected bar, if it can be chosen. A
/// native tool bar has its own buttons, so it names the item rather than
/// a column.
pub fn run_item(app: &mut App, index: usize) -> bool {
    if !visible(app) {
        return false;
    }
    let Some(item) = bar(app).items.get(index).cloned() else {
        return false;
    };
    let enabled = {
        let ctx = crate::menu::context(app);
        (ctx.exists)(&item.command) && item.enabled.holds(&ctx.state)
    };
    if !enabled {
        return false;
    }
    crate::commands::execute(app, &item.command);
    true
}

/// A click at `column` on the modifier bar: add the button's modifier to
/// the ones waiting for the next key, or take it back if it is already
/// waiting. False when the click missed every button.
pub fn click_modifier(app: &mut App, column: u16) -> bool {
    let Some(modifier) = modifier_view(app).and_then(|v| v.button_at(column)) else {
        return false;
    };
    toggle_modifier(app, modifier);
    true
}

/// Add `modifier` to the pending set, or remove it.
pub fn toggle_modifier(app: &mut App, modifier: Modifier) {
    if !modifier_visible(app) {
        return;
    }
    if let Some(i) = app.pending_modifiers.iter().position(|m| *m == modifier) {
        app.pending_modifiers.remove(i);
    } else {
        app.pending_modifiers.push(modifier);
    }
}

/// Apply the pending modifiers to the next key and clear them.
///
/// Forge combines them (C-M-a): Meta and Alt both mean ALT, Super is
/// SUPER and Hyper is HYPER. Shift on a character uppercases it instead
/// of adding SHIFT, so S-a is A; on any other key it stays SHIFT.
pub fn apply_pending(app: &mut App, key: &mut KeyEvent) {
    if app.pending_modifiers.is_empty() {
        return;
    }
    let pending = std::mem::take(&mut app.pending_modifiers);
    let mut modifiers = key.modifiers;
    let mut shift = false;
    for modifier in pending {
        match modifier {
            Modifier::Control => modifiers |= KeyModifiers::CONTROL,
            Modifier::Shift => shift = true,
            Modifier::Meta | Modifier::Alt => modifiers |= KeyModifiers::ALT,
            Modifier::Super => modifiers |= KeyModifiers::SUPER,
            Modifier::Hyper => modifiers |= KeyModifiers::HYPER,
        }
    }
    if shift {
        if let KeyCode::Char(c) = key.code {
            let upper: String = c.to_uppercase().collect();
            key.code = KeyCode::Char(upper.chars().next().unwrap_or(c));
        } else {
            modifiers |= KeyModifiers::SHIFT;
        }
    }
    key.modifiers = modifiers;
}

/// The modifier keys waiting for the next key, for the mode line or a
/// plugin that wants to show them.
pub fn pending(app: &App) -> &[Modifier] {
    &app.pending_modifiers
}

/// `tool-bar-mode` and `modifier-bar-mode`, and the commands that click a
/// modifier button from the keyboard.
pub fn execute(app: &mut App, cmd: &str) -> bool {
    match cmd {
        "tool-bar-mode" => {
            if !available(app) {
                app.msg("Tool bar is only available in graphical frames");
                return true;
            }
            let on = if app.has_arg() { true } else { !mode_on(app) };
            set_mode(app, on);
            app.msg(format!(
                "Tool Bar mode {}",
                if on { "enabled" } else { "disabled" }
            ));
        }
        "modifier-bar-mode" => {
            if !available(app) {
                app.msg("Modifier bar is only available in graphical frames");
                return true;
            }
            let on = if app.has_arg() {
                true
            } else {
                !modifier_mode_on(app)
            };
            if on && !mode_on(app) {
                app.msg("Modifier bar is part of the tool bar; enable tool-bar-mode first");
                return true;
            }
            set_modifier_mode(app, on);
            app.msg(format!(
                "Modifier Bar mode {}",
                if on { "enabled" } else { "disabled" }
            ));
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::Editor;

    fn app() -> App {
        let mut app = App::new(Editor::new());
        app.graphical = true;
        let id = app.editor.create_buffer("*scratch*");
        app.editor.show_buffer(id);
        app
    }

    #[test]
    fn toolbar_mode_is_off_until_turned_on() {
        let mut app = app();
        assert!(!visible(&app));
        assert!(view(&app).is_none());
        set_mode(&mut app, true);
        assert!(visible(&app));
        assert!(view(&app).is_some());
        set_mode(&mut app, false);
        assert!(!visible(&app));
    }

    #[test]
    fn terminal_frames_have_no_tool_bar() {
        let mut app = app();
        app.graphical = false;
        set_mode(&mut app, true);
        set_modifier_mode(&mut app, true);
        assert!(!visible(&app));
        assert!(view(&app).is_none());
        assert!(!modifier_visible(&app));
        assert!(modifier_view(&app).is_none());
    }

    #[test]
    fn the_modifier_bar_needs_the_tool_bar() {
        let mut app = app();
        set_mode(&mut app, false);
        execute(&mut app, "modifier-bar-mode");
        assert!(!modifier_mode_on(&app));
        assert!(!modifier_visible(&app));

        set_mode(&mut app, true);
        execute(&mut app, "modifier-bar-mode");
        assert!(modifier_mode_on(&app));
        assert!(modifier_visible(&app));

        // Hiding the tool bar hides the modifier bar with it, and the
        // modifiers waiting for the next key go with it.
        app.pending_modifiers = vec![Modifier::Control];
        set_mode(&mut app, false);
        assert!(app.pending_modifiers.is_empty());
        assert!(!modifier_visible(&app));
    }

    #[test]
    fn pending_modifiers_apply_to_one_key_then_clear() {
        let mut app = app();
        app.pending_modifiers = vec![Modifier::Control, Modifier::Meta];
        let mut key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        apply_pending(&mut app, &mut key);
        assert_eq!(key.modifiers, KeyModifiers::CONTROL | KeyModifiers::ALT);
        assert!(app.pending_modifiers.is_empty());

        app.pending_modifiers = vec![Modifier::Shift];
        let mut key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        apply_pending(&mut app, &mut key);
        assert_eq!(key.code, KeyCode::Char('A'));
        assert_eq!(key.modifiers, KeyModifiers::NONE);

        app.pending_modifiers = vec![Modifier::Shift];
        let mut key = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
        apply_pending(&mut app, &mut key);
        assert_eq!(key.modifiers, KeyModifiers::SHIFT);
    }
}
