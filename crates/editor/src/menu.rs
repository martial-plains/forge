// SPDX-License-Identifier: MIT OR Apache-2.0
//! The menu bar, tied to the editor: whether its row shows, what to draw,
//! and the keys and clicks that open and close it.
//!
//! The menus themselves live in [`forge_core::menu`], so the macOS app
//! gets the same ones from the same place; this is the half that needs an
//! `App`: reading the registry, running what a click picks, and the frame
//! layout.

use crate::app::App;
use forge_core::Lookup;
use forge_core::display::{Face, Span};
use forge_core::keymap::{PRIORITY_TRANSIENT, TransientOpts};
use forge_core::menu::{self, MenuBarView, Registry};
use forge_core::modes::MENU_BAR_MODE;
use ratatui::crossterm::event::{KeyEvent, KeyModifiers};

/// Is the menu bar's row showing? It is per-buffer, like every other minor
/// mode, and the selected window's buffer decides.
pub fn visible(app: &App) -> bool {
    app.editor.minor_mode_enabled(MENU_BAR_MODE)
}

/// Is any menu open? A menu open means the bar wants the keyboard.
pub fn active(app: &App) -> bool {
    app.editor.menu.open.is_some() || context_open(app)
}

/// Is the context menu (right-click) the one open?
pub fn context_open(app: &App) -> bool {
    app.editor.context_menu.is_open()
}

/// The menu state and menus the keys and the pointer act on: the context
/// menu's while it is open, the bar's otherwise. Every navigation
/// function goes through this, which is how one set of keys serves both.
fn parts(app: &mut App) -> (&mut menu::MenuBar, &Registry) {
    let e = &mut app.editor;
    if e.context_menu.is_open() {
        let forge_core::context_menu::ContextMenu { bar, registry, .. } = &mut e.context_menu;
        (bar, &*registry)
    } else {
        (&mut e.menu, &e.menus)
    }
}

/// Read-only twin of [`parts`].
fn parts_ref(app: &App) -> (menu::MenuBar, &Registry) {
    let e = &app.editor;
    if e.context_menu.is_open() {
        (e.context_menu.bar, &e.context_menu.registry)
    } else {
        (e.menu, &e.menus)
    }
}

/// The transient map that is up while a menu is open, so the menu's keys
/// are looked up like any other key and can be rebound.
pub const TRANSIENT: &str = "menu";

/// Keep the transient map in step with the menu: up while a menu is open,
/// down when the last one closes. Pushing a name that is already up just
/// makes it the outermost copy, so every open and close path can call this.
fn sync_transient(app: &mut App) {
    if active(app) {
        app.editor
            .push_transient(TRANSIENT, TransientOpts::default());
    } else {
        app.editor.pop_transient(TRANSIENT);
    }
}

/// The menus as the editor has them.
pub fn menus(app: &App) -> &Registry {
    &app.editor.menus
}

/// What the menus need to know about each command, from the editor: whether
/// it exists and what keys run it. Shared with the macOS menu bar so both
/// say the same thing.
pub fn context<'a>(app: &'a App) -> menu::Context<'a> {
    // The active keymaps, so the shortcut shown is the one that would
    // actually run here: a shadowed or remapped binding, and the buffer's
    // own keymap, count.
    let maps = app.editor.active_keymaps_now();
    let editor = &app.editor;
    menu::Context {
        exists: Box::new(move |c: &str| {
            editor.commands.contains(c)
                || c.starts_with(forge_core::context_menu::YANK_PREFIX)
                || c.starts_with(menu::SELECT_BUFFER)
        }),
        // Just the shortest binding, the way a menu bar shows one; the
        // rest are in `where-is`.
        keys: Box::new(move |c: &str| maps.where_is(c).into_iter().take(1).collect()),
        state: state(app),
    }
}

/// The editor state a menu item's condition is answered from. Taken once
/// per draw, so the panel isn't a mix of answers from different moments.
/// The menu state, for the mode line's menus.
pub fn state_for_mode_line(app: &App) -> menu::MenuState<'_> {
    state(app)
}

/// Bring a popup that was just filled into view.
pub(crate) fn after_popup(app: &mut App) {
    if context_open(app) {
        sync_transient(app);
    }
}

fn state(app: &App) -> menu::MenuState<'_> {
    menu::MenuState::of(&app.editor).with_flags(|name| match name {
        "show-paren" => app.show_paren,
        "electric-pair" => app.electric_pair,
        "menu-bar-mode" => mode_on(app),
        "context-menu-mode" => context_mode_on(app),
        "tool-bar-mode" => crate::toolbar::mode_on(app),
        "tab-bar-mode" => crate::tabs::mode_on(app),
        "tooltip-mode" => app.tooltip_mode,
        "scroll-bar-mode" => app.scroll_bar,
        "horizontal-scroll-bar-mode" => app.horizontal_scroll_bar,
        other => crate::mode_line::flag(app, other).unwrap_or(false),
    })
}

/// The indexes of the menus the keys and pointer may visit: the context
/// menu's only menu while it is open, otherwise the bar's menus that the
/// current buffer's modes say should show.
fn visible_indices(app: &App) -> Vec<usize> {
    if context_open(app) {
        (0..app.editor.context_menu.registry.len()).collect()
    } else {
        app.editor.menus.visible_indices(&state(app))
    }
}

/// What to draw: the bar's row, and the open menu's panel. Nothing at all
/// when the mode is off, so a frontend can skip the work.
pub fn view(app: &App) -> Option<MenuBarView> {
    if context_open(app) {
        return context_view(app);
    }
    if !visible(app) {
        return None;
    }
    let ctx = context(app);
    Some(menu::view(app.editor.menu, menus(app), width(app), &ctx))
}

/// The context menu's panels, placed where it was opened and kept on the
/// frame: slid left near the right edge and up near the bottom, a
/// submenu flipping to the parent's left when there's no room for it on
/// the right. Columns and rows are relative to the frame, like the bar's.
/// The bar's own row and titles in the result are not meant to be drawn.
pub fn context_view(app: &App) -> Option<MenuBarView> {
    let cm = &app.editor.context_menu;
    if !cm.is_open() {
        return None;
    }
    let ctx = context(app);
    let frame = app.editor.windows.frame();
    let mut v = menu::view(cm.bar, &cm.registry, frame.width.max(1), &ctx);
    let popup = v.popup.as_mut()?;
    let pw = menu::panel_width(&popup.rows);
    popup.column = cm.column.min(frame.width.saturating_sub(pw));
    popup.row = cm
        .row
        .min(frame.height.saturating_sub(popup.rows.len() as u16));
    let (pcol, prow) = (popup.column, popup.row);
    if let Some(sub) = v.subpopup.as_mut() {
        let sw = menu::panel_width(&sub.rows);
        sub.row =
            (prow + cm.bar.item as u16).min(frame.height.saturating_sub(sub.rows.len() as u16));
        sub.column = if pcol + pw + 1 + sw <= frame.width {
            pcol + pw + 1
        } else {
            pcol.saturating_sub(sw + 1)
        };
    }
    Some(v)
}

/// The bar's row as spans.
pub fn row(app: &App) -> Vec<Span> {
    view(app).map_or_else(Vec::new, |v| v.row)
}

/// The open panel: its rows (text, face) and the column it starts at.
pub fn panel(app: &App) -> Option<(u16, Vec<(String, Face)>)> {
    if context_open(app) || !visible(app) {
        return None;
    }
    let ctx = context(app);
    let rows = menu::panel_rows(app.editor.menu, menus(app), &ctx);
    let v = menu::view(app.editor.menu, menus(app), width(app), &ctx);
    Some((
        v.popup.as_ref()?.column,
        rows.into_iter().map(|r| (r.text, r.face)).collect(),
    ))
}

/// The open submenu's panel: its rows (text, face), the column it
/// starts at, and the frame row its first row is on. The terminal draws
/// it like the panel, cascading from its parent item.
pub fn subpanel(app: &App) -> Option<(u16, u16, Vec<(String, Face)>)> {
    if context_open(app) || !visible(app) {
        return None;
    }
    let ctx = context(app);
    let v = menu::view(app.editor.menu, menus(app), width(app), &ctx);
    let p = v.subpopup?;
    Some((
        p.column,
        p.row,
        p.rows.into_iter().map(|r| (r.text, r.face)).collect(),
    ))
}

/// The context menu's panel and submenu, for drawing: each is the
/// frame-relative column and row of its first cell, and its rows.
pub fn context_panels(app: &App) -> Vec<(u16, u16, Vec<(String, Face)>)> {
    let Some(v) = context_view(app) else {
        return Vec::new();
    };
    [v.popup, v.subpopup]
        .into_iter()
        .flatten()
        .map(|p| {
            (
                p.column,
                p.row,
                p.rows.into_iter().map(|r| (r.text, r.face)).collect(),
            )
        })
        .collect()
}

/// The frame's width: the window it's in, which is the whole screen.
fn width(app: &App) -> u16 {
    app.editor.windows.frame().width
}

/// The screen row the bar is on: the one above the windows, since
/// `menu::frame_areas_with` puts it there, and above the tool bar, modifier
/// bar and tab bar when they show too. Both frontends go through this, so
/// hit-testing and drawing agree on which row it is.
pub fn bar_row(app: &App) -> u16 {
    let tab = u16::from(crate::tabs::visible(app));
    app.editor.windows.frame().y.saturating_sub(
        1 + tab + crate::toolbar::modifier_height(app) + crate::toolbar::top_height(app),
    )
}

/// Turn `menu-bar-mode` on or off, closing any open menu either way.
///
/// It's a global mode: the bar frames the window, so opening a file
/// shouldn't take it away.
pub fn set_mode(app: &mut App, on: bool) {
    if on {
        app.editor.global_modes.enable(MENU_BAR_MODE);
    } else {
        app.editor.global_modes.disable(MENU_BAR_MODE);
        close(app);
    }
}

/// Is `menu-bar-mode` on? It's global, so this doesn't depend on which
/// buffer is selected.
pub fn mode_on(app: &App) -> bool {
    visible(app)
}

/// Open the menu bar with the keyboard, or move on to the next one.
pub fn open_next(app: &mut App) {
    if !visible(app) {
        app.msg("menu-bar-mode is off (M-x menu-bar-mode to turn it on)");
        return;
    }
    app.editor.refresh_menus();
    let visible = visible_indices(app);
    let (state, r) = parts(app);
    state.right_visible(r, &visible);
    sync_transient(app);
}

/// The pointer moved to `column` on the frame's row `row`, which is the
/// bar's own row when `on_bar`.
///
/// `column` and `row` are relative to the frame. The bar sits on a row of
/// its own above the frame, so its row and the panel's first row are both
/// row 0 in here: the caller says which one the pointer is on.
///
/// Returns whether it was over the menu bar (the row, or the panel under
/// an open one), so a caller knows if it should keep looking for
/// something else under the pointer.
pub fn hover(app: &mut App, column: u16, row: u16, on_bar: bool) -> bool {
    if !visible(app) && !context_open(app) {
        // The bar is gone, so nothing is under the pointer to mark.
        app.editor.menu.hover = None;
        return false;
    }
    // The layout is only needed to place the pointer, so compute it and let
    // the borrow go before changing any state.
    let (title, hit) = {
        let Some(v) = view(app) else {
            return false;
        };
        let on_bar = on_bar && !context_open(app);
        let title = on_bar.then(|| menu::title_at(&v.titles, column)).flatten();
        // Over an open panel, mark the item under the pointer. A
        // separator holds the highlight where it was.
        let hit = match &v.popup {
            Some(_) if title.is_none() => panel_hit(&v, column, row as usize),
            _ => None,
        };
        (title, hit)
    };
    if title.is_some() && title != app.editor.menu.open {
        app.editor.refresh_menus();
    }
    let visible = visible_indices(app);
    let (state, r) = parts(app);
    state.hover_visible(title, r, &visible);
    match hit {
        Some((true, row)) => state.hover_sub(row, r),
        Some((false, row)) => state.hover_item(row, r),
        None => {}
    }
    true
}

/// Is this cell inside the open menu's panel? `column` and `row` are
/// relative to the frame, like [`hover`], and the bar's row never counts:
/// it is not part of the panel.
pub fn over_panel_at(app: &App, column: u16, row: u16, on_bar: bool) -> bool {
    if on_bar && !context_open(app) {
        return false;
    }
    let Some(v) = view(app) else {
        return false;
    };
    over_panel(&v, column, row)
}

/// Is this cell inside the open menu's panel?
///
/// Any row counts, separators included: they're part of the panel, so
/// sweeping the pointer down its edge must not close the menu.
fn over_panel(v: &menu::MenuBarView, column: u16, row: u16) -> bool {
    panel_hit(v, column, row as usize).is_some()
}

/// The panel under (`column`, `row`): the subpanel when it covers the
/// cell (it's drawn on top), else the parent panel. Returns which panel
/// and the row within it.
fn panel_hit(v: &menu::MenuBarView, column: u16, row: usize) -> Option<(bool, usize)> {
    if let Some(p) = &v.subpopup
        && let Some(r) = hit_panel(p, column, row)
    {
        return Some((true, r));
    }
    if let Some(p) = &v.popup
        && let Some(r) = hit_panel(p, column, row)
    {
        return Some((false, r));
    }
    None
}

/// The row of `popup` under (`column`, `row`), relative to the panel's
/// first row. Separators count: they're part of the panel.
fn hit_panel(popup: &menu::MenuPopup, column: u16, row: usize) -> Option<usize> {
    let width = popup
        .rows
        .iter()
        .map(|r| r.text.chars().count() as u16)
        .max()
        .unwrap_or(0);
    let rel = row.checked_sub(popup.row as usize)?;
    (column >= popup.column && column < popup.column + width && rel < popup.rows.len())
        .then_some(rel)
}

/// Close whatever is open, if anything.
pub fn close(app: &mut App) {
    app.editor.menu.close();
    app.editor.context_menu.close();
    app.editor.pop_transient(TRANSIENT);
}

/// Move the highlight within the open menu.
pub fn move_item(app: &mut App, by: isize) {
    let (state, r) = parts(app);
    state.move_item(by, r);
}

/// Move the highlight within the open submenu.
pub fn move_sub(app: &mut App, by: isize) {
    let (state, r) = parts(app);
    state.move_sub(by, r);
}

/// Switch to the neighbouring menu.
pub fn move_menu(app: &mut App, right: bool) {
    let visible = visible_indices(app);
    let (state, r) = parts(app);
    if right {
        state.right_visible(r, &visible);
    } else {
        state.left_visible(r, &visible);
    }
}

/// Run the highlighted item's command, and close the menu. False if
/// nothing is highlighted (a separator, or a submenu header), so the
/// caller can leave the menu open.
pub fn choose(app: &mut App) -> bool {
    let command = {
        let (state, r) = parts_ref(app);
        if state.sub.is_some() {
            state.sub_selected(r).map(|i| i.command.clone())
        } else {
            state.selected(r).map(|i| i.command.clone())
        }
    };
    let Some(command) = command else {
        return false;
    };
    close(app);
    crate::commands::execute(app, &command);
    true
}

/// A click on the bar's row at `column`: opens that menu, or toggles it
/// closed if it was already open. False if the click missed every title.
pub fn click_bar(app: &mut App, column: u16) -> bool {
    if !visible(app) || context_open(app) {
        return false;
    }
    // The titles don't depend on the context (a separator can't be a
    // title), so borrow only what the click needs and let it go.
    let hit = {
        let ctx = context(app);
        let v = menu::view(app.editor.menu, menus(app), width(app), &ctx);
        menu::title_at(&v.titles, column)
    };
    match hit {
        Some(m) => {
            app.editor.refresh_menus();
            let visible = visible_indices(app);
            let (state, r) = parts(app);
            state.toggle_visible(m, r, &visible);
            // A click on the open menu's title toggles it closed again.
            sync_transient(app);
            true
        }
        None => false,
    }
}

/// A click while a menu is open: picks the item under it and runs it.
/// A click on a submenu header opens its submenu instead; a click on a
/// submenu's item runs it. False if the click missed every panel, so the
/// caller can pass it on.
///
/// `column` and `row` are relative to the frame, like [`hover`], so the
/// panel's first row is row 0.
pub fn click_item(app: &mut App, column: u16, row: u16) -> bool {
    let Some(v) = view(app) else {
        return false;
    };
    match panel_hit(&v, column, row as usize) {
        // In the submenu: run the child under the click.
        Some((true, r)) => {
            let index = v
                .subpopup
                .as_ref()
                .and_then(|p| menu::subitem_at(p, r as u16));
            let Some(index) = index else {
                return false;
            };
            parts(app).0.sub = Some(index);
            choose(app)
        }
        // In the menu: a header opens its submenu, anything else runs.
        Some((false, r)) => {
            let index = v.popup.as_ref().and_then(|p| menu::item_at(p, r as u16));
            let Some(index) = index else {
                return false;
            };
            parts(app).0.item = index;
            let on_parent = {
                let (state, r) = parts_ref(app);
                state.on_parent(r)
            };
            if on_parent {
                let (state, menus) = parts(app);
                state.open_submenu(menus);
                true
            } else {
                choose(app)
            }
        }
        // Beside every panel is a click in the text, and belongs to
        // whatever is under it.
        None => false,
    }
}

/// Keys while a menu is open, and the one that opens the bar.
///
/// menu keys: C-n/C-p (Down/Up) move, C-f/C-b (Right/Left)
/// switch menus -- into a submenu on its header, back out of one --
/// TAB and RET choose, ESC or C-g closes. True if the key was used.
///
/// While a menu is open its transient map is up, so these keys are looked
/// up like any other key and can be rebound. A key the map doesn't bind
/// closes the menu and then does its normal job.
///
/// The key that opens the bar is a global binding on `menu-bar-open`, so
/// this only ever runs with a menu up.
pub fn handle_key(app: &mut App, key: KeyEvent) -> bool {
    if !active(app) {
        // The key that opens the bar (F10, M-`) is an ordinary global
        // binding, so nothing here has to recognize it: `handle_key` declines
        // and the editor dispatches it like any other key.
        return false;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let found = app.editor.active_keymaps_now().lookup_above(
        &[crate::minibuffer::describe(key, ctrl, alt)],
        PRIORITY_TRANSIENT,
    );
    match found {
        Lookup::Command { command, .. } => {
            app.run_command(&command);
            true
        }
        // Anything else closes the menu and then does what it says, the way
        // a menu bar behaves when you click away from it.
        _ => {
            close(app);
            false
        }
    }
}

// --------------------------------------------------------- menu key commands

/// ESC: close the submenu, or the menu when none is open.
pub fn cmd_close(app: &mut App) {
    if !parts(app).0.close_submenu() {
        close(app);
    }
}

/// C-g: close the menu, saying Quit the way keyboard-quit does.
pub fn cmd_quit(app: &mut App) {
    close(app);
    app.msg("Quit");
}

/// C-n / Down: highlight the next item, or the next submenu item when
/// a submenu is open.
pub fn cmd_next_item(app: &mut App) {
    if parts(app).0.sub.is_some() {
        move_sub(app, 1);
    } else {
        move_item(app, 1);
    }
}

/// C-p / Up: highlight the previous item, or the previous submenu item
/// when a submenu is open.
pub fn cmd_prev_item(app: &mut App) {
    if parts(app).0.sub.is_some() {
        move_sub(app, -1);
    } else {
        move_item(app, -1);
    }
}

/// C-f / Right: open the highlighted submenu, or the next menu when the
/// highlight opens none.
pub fn cmd_next_menu(app: &mut App) {
    let on_parent = {
        let (state, r) = parts_ref(app);
        state.sub.is_none() && state.on_parent(r)
    };
    if on_parent {
        let (state, r) = parts(app);
        state.open_submenu(r);
    } else {
        move_menu(app, true);
    }
}

/// C-b / Left: close the open submenu, or the previous menu when none
/// is open.
pub fn cmd_prev_menu(app: &mut App) {
    if !parts(app).0.close_submenu() {
        move_menu(app, false);
    }
}

/// `<home>`: highlight the first item, or the first submenu item when a
/// submenu is open.
pub fn cmd_first_item(app: &mut App) {
    let (state, r) = parts(app);
    if state.sub.is_some() {
        state.first_sub(r);
    } else {
        state.first_item(r);
    }
}

/// `<end>`: highlight the last item, or the last submenu item when a
/// submenu is open.
pub fn cmd_last_item(app: &mut App) {
    let (state, r) = parts(app);
    if state.sub.is_some() {
        state.last_sub(r);
    } else {
        state.last_item(r);
    }
}

/// TAB / RET: run the highlighted item's command. On a submenu header
/// it opens the submenu instead; in a submenu it runs the highlighted
/// child.
pub fn cmd_choose(app: &mut App) {
    if parts(app).0.sub.is_some() {
        if !choose(app) {
            app.msg("");
        }
        return;
    }
    let on_parent = {
        let (state, r) = parts_ref(app);
        state.on_parent(r)
    };
    if on_parent {
        let (state, r) = parts(app);
        state.open_submenu(r);
    } else if !choose(app) {
        // Nothing runnable under the highlight (a separator): RET leaves
        // the menu alone.
        app.msg("");
    }
}

// ------------------------------------------------------------ context menu

use forge_core::Pos;
use forge_core::context_menu::{self as cm, Input};
use forge_core::modes::CONTEXT_MENU_MODE;

/// Is `context-menu-mode` on? Global, like Forge's.
pub fn context_mode_on(app: &App) -> bool {
    app.editor.global_modes.enabled(CONTEXT_MENU_MODE)
}

/// Turn `context-menu-mode` on or off, closing an open context menu when
/// it goes off.
pub fn set_context_mode(app: &mut App, on: bool) {
    if on {
        app.editor.global_modes.enable(CONTEXT_MENU_MODE);
    } else {
        app.editor.global_modes.disable(CONTEXT_MENU_MODE);
        if context_open(app) {
            close(app);
        }
    }
}

/// Open the context menu at the frame cell (`column`, `row`), for a click
/// at buffer offset `click`. Does nothing when `context-menu-mode` is off.
///
/// Like right-click, a click outside the active region moves
/// point there and drops the region, so Paste and Select act where you
/// clicked; a click inside the region keeps it, so Cut and Copy have
/// something to work on.
pub fn open_context(app: &mut App, column: u16, row: u16, click: Option<usize>) -> bool {
    if !context_mode_on(app) {
        return false;
    }
    close(app);
    if let Some(off) = click {
        let b = app.buf();
        let inside = b.active_region().is_some_and(|(a, z)| {
            let (a, z) = (b.pos_to_offset(a), b.pos_to_offset(z));
            off >= a.min(z) && off <= a.max(z)
        });
        if !inside {
            b.deactivate_mark();
            b.set_offset(off);
        }
    }
    let input = context_input(app, click);
    let functions = app.editor.context_menu_functions.clone();
    app.editor
        .context_menu
        .open(&functions, &input, column, row, click);
    if !context_open(app) {
        app.msg("No context menu here");
        return false;
    }
    sync_transient(app);
    true
}

/// What the builders need, from the selected buffer.
fn context_input(app: &App, click: Option<usize>) -> Input {
    let b = app.buf_ref();
    let in_string = click.is_some_and(|off| {
        let p = b.offset_to_pos(off);
        cm::string_bounds(&b.line(p.line), p.col).is_some()
    });
    let menus = {
        let state = state(app);
        app.editor
            .menus
            .visible_indices(&state)
            .into_iter()
            .map(|i| &app.editor.menus.menus()[i])
            .filter(|m| m.title != cm::TITLE)
            .map(|m| (m.title.clone(), app.editor.menus.leaves_of(&m.title)))
            .collect()
    };
    Input {
        read_only: b.read_only(),
        region: b.active_region().is_some(),
        can_undo: b.can_undo(),
        can_redo: b.can_redo(),
        in_string,
        kill_ring: (0..app.editor.kill_ring.len())
            .filter_map(|i| app.editor.kill_ring.get(i).map(str::to_owned))
            .collect(),
        clipboard: app.select_enable_clipboard
            && app
                .clipboard_provider
                .get_text(forge_core::clipboard::SelectionTarget::Clipboard)
                .is_some_and(|t| !t.is_empty()),
        buffers: app.editor.menu_buffer_names(),
        major_mode: b.major_mode().to_string(),
        minor_modes: b.minor_modes().to_vec(),
        menus,
        under_vc: b
            .path()
            .is_some_and(|p| crate::vc::responsible(p).is_some()),
    }
}

/// `context-menu-open` (S-F10): open the menu at point, from the keyboard.
pub fn cmd_context_open(app: &mut App) {
    if !context_mode_on(app) {
        app.msg("context-menu-mode is off (M-x context-menu-mode to turn it on)");
        return;
    }
    let off = app.buf_ref().point_offset();
    let frame = app.editor.windows.frame();
    let (x, y) = point_cell(app);
    open_context(
        app,
        x.saturating_sub(frame.x),
        (y + 1).saturating_sub(frame.y),
        Some(off),
    );
}

/// The screen cell point is on, as best the window's last redisplay says:
/// exact for unwrapped text, the line's start when it wraps.
fn point_cell(app: &mut App) -> (u16, u16) {
    let id = app.editor.windows.selected_id();
    let frame = app.editor.windows.frame();
    let rect = app
        .editor
        .windows
        .layout(frame)
        .into_iter()
        .find(|(i, _)| *i == id)
        .map(|(_, r)| r);
    let Some(r) = rect else {
        return (frame.x, frame.y);
    };
    let p = app.buf_ref().point();
    let w = app.win_mut();
    let row = p.line.saturating_sub(w.scroll_top) + w.header_rows;
    let col = w.gutter + p.col.saturating_sub(w.hscroll);
    (
        (r.x + col as u16).min(r.x + r.width.saturating_sub(1)),
        (r.y + row as u16).min(r.y + r.height.saturating_sub(1)),
    )
}

/// `mouse-yank-at-click`: Paste. Point is already where the click was
/// (opening the menu put it there), so this is a yank.
pub fn cmd_yank_at_click(app: &mut App) {
    if let Some(off) = app.editor.context_menu.click {
        let b = app.buf();
        b.deactivate_mark();
        b.set_offset(off);
    }
    crate::commands::execute(app, "yank");
}

/// `context-menu-yank-N`: insert the N'th newest kill-ring entry at the
/// click, without reordering the ring.
pub fn cmd_yank_entry(app: &mut App, n: usize) {
    let Some(text) = n
        .checked_sub(1)
        .and_then(|i| app.editor.kill_ring.get(i).map(str::to_owned))
    else {
        app.error("No such kill-ring entry");
        return;
    };
    app.buf().deactivate_mark();
    if let Some(off) = app.editor.context_menu.click {
        app.buf().set_offset(off);
    }
    let start = app.buf_ref().point();
    app.buf().push_mark(start, false);
    app.buf().insert_str(&text);
}

/// Select > All / Line / String / None, and Clear.
pub fn cmd_select(app: &mut App, what: &str) {
    let click = app
        .editor
        .context_menu
        .click
        .unwrap_or_else(|| app.buf_ref().point_offset());
    match what {
        "all" => {
            let b = app.buf();
            let end = b.end();
            b.push_mark(end, true);
            b.beginning_of_buffer();
        }
        "none" => app.buf().deactivate_mark(),
        "line" | "string" => {
            let b = app.buf();
            let p = b.offset_to_pos(click);
            let len = b.line_len(p.line);
            let (from, to) = if what == "line" {
                // The line and its newline, like `line' thing.
                if p.line + 1 < b.line_count() {
                    (Pos::new(p.line, 0), Pos::new(p.line + 1, 0))
                } else {
                    (Pos::new(p.line, 0), Pos::new(p.line, len))
                }
            } else {
                match cm::string_bounds(&b.line(p.line), p.col) {
                    Some((a, z)) => (Pos::new(p.line, a), Pos::new(p.line, z)),
                    None => return,
                }
            };
            b.push_mark(from, true);
            b.set_point(to);
        }
        _ => {}
    }
}

/// Clear: delete the active region without touching the kill ring.
pub fn cmd_delete_active_region(app: &mut App) {
    if let Some((a, z)) = app.buf_ref().active_region() {
        app.buf().delete_range(a, z);
        app.buf().deactivate_mark();
    }
}
