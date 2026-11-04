// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tabs: named window configurations within a frame, and the tab bar that
//! shows them.
//!
//! Each frame holds several tabs; a tab is a whole window configuration
//! (see `forge_core::window`). C-x t 2 makes one, C-x t 0 closes one, C-x t
//! o switches between them, and the rest of the C-x t map renames, moves,
//! selects and undoes them. The bar itself is a row at the top of the
//! frame (below the menu bar), which the terminal paints from [`row`] and
//! the macOS app draws as a native strip.

use crate::app::App;
use crate::minibuffer::Purpose;
use forge_core::display::{Face, Span};
use forge_core::modes::TAB_BAR_MODE;
use forge_core::{FrameId, TabId};
use std::collections::HashMap;

pub fn execute(app: &mut App, cmd: &str) -> bool {
    match cmd {
        "tab-bar-mode" => {
            let on = if app.has_arg() { true } else { !mode_on(app) };
            set_mode(app, on);
            app.msg(format!(
                "Tab Bar mode {}",
                if on { "enabled" } else { "disabled" }
            ));
        }
        "toggle-frame-tab-bar" => toggle_frame(app),
        "tab-bar-new-tab" | "tab-new" => new_tab_public(app),
        "tab-bar-close-tab" | "tab-close" => close_tab(app),
        "tab-bar-close-other-tabs" | "tab-close-other" => close_others(app),
        "tab-bar-undo-close-tab" | "tab-undo" => undo_close(app),
        "tab-bar-move-tab" | "tab-move" => move_tab(app),
        "tab-bar-rename-tab" | "tab-rename" => rename_tab(app),
        "tab-bar-next-tab" | "tab-next" => app.editor.other_tab(app.count() as isize),
        "tab-bar-previous-tab" | "tab-previous" => app.editor.other_tab(-(app.count() as isize)),
        "tab-bar-select-tab" | "tab-select" => select_tab(app),
        "tab-switch" => switch_prompt(app),
        "tab-list-tabs" => list_tabs(app),
        "switch-to-buffer-other-tab" => {
            let default = app
                .other_buffer()
                .and_then(|id| app.editor.state.get_buffer(id))
                .map(|b| b.name().to_string());
            let prompt = match &default {
                Some(d) => format!("Switch to buffer in other tab (default {d}): "),
                None => "Switch to buffer in other tab: ".to_string(),
            };
            app.prompt(
                prompt,
                Purpose::SwitchBuffer {
                    default,
                    other_window: false,
                    other_frame: false,
                    other_tab: true,
                },
            );
        }
        "find-file-other-tab" => {
            let input = crate::minibuffer::initial_find_file_input(app);
            app.prompt(
                "Find file in other tab: ",
                Purpose::FindFile {
                    other_window: false,
                    other_frame: false,
                    other_tab: true,
                },
            );
            if let Some(mb) = app.minibuffer.as_mut() {
                mb.input = input;
            }
        }
        "dired-other-tab" => {
            let dir = crate::dired::start_dir(app);
            app.prompt(
                "Dired in other tab (directory): ",
                Purpose::Dired(crate::dired::DiredAction::OpenOtherTab),
            );
            if let Some(mb) = app.minibuffer.as_mut() {
                mb.input = format!("{}/", dir.display().to_string().trim_end_matches('/'));
            }
        }
        _ => return false,
    }
    true
}

/// Is `tab-bar-mode` on? It's global, like `menu-bar-mode`: the bar frames
/// the window.
pub fn mode_on(app: &App) -> bool {
    app.editor.global_modes.enabled(TAB_BAR_MODE)
}

/// Turn it on or off for the whole session.
pub fn set_mode(app: &mut App, on: bool) {
    if on {
        app.editor.global_modes.enable(TAB_BAR_MODE);
    } else {
        app.editor.global_modes.disable(TAB_BAR_MODE);
    }
}

/// Is the tab bar's row showing on the selected frame?
///
/// `toggle-frame-tab-bar` forces it on or off for one frame; otherwise the
/// global mode decides, and a frame with a single tab keeps its row hidden
///, so the bar only appears once there is
/// something to switch between.
pub fn visible(app: &App) -> bool {
    visible_in(app, app.editor.windows.selected_frame_id())
}

pub fn visible_in(app: &App, frame: FrameId) -> bool {
    if let Some(on) = app.tab_bar_override.get(&frame) {
        return *on;
    }
    mode_on(app) && app.editor.windows.frame_tab_count(frame) > 1
}

/// toggle-frame-tab-bar: force the bar on or off for the selected frame,
/// irrespective of the global mode.
fn toggle_frame(app: &mut App) {
    let frame = app.editor.windows.selected_frame_id();
    let now = visible_in(app, frame);
    app.tab_bar_override.insert(frame, !now);
    app.msg(format!(
        "Tab Bar {} on this frame",
        if now { "hidden" } else { "shown" }
    ));
}

pub fn new_tab_public(app: &mut App) {
    let id = app.editor.make_tab();
    let name = display_name(app, id);
    app.msg(format!("Created tab {name}"));
}

fn close_tab(app: &mut App) {
    let ids = app.editor.windows.tab_ids();
    let id = if app.has_arg() {
        match nth_arg(app, &ids) {
            Some(id) => id,
            None => return app.error("No such tab"),
        }
    } else {
        app.editor.windows.selected_tab_id()
    };
    match app.editor.delete_tab(id) {
        Ok(()) => app.msg("Deleted tab"),
        Err(e) => app.error(e),
    }
}

fn close_others(app: &mut App) {
    let keep = app.editor.windows.selected_tab_id();
    let others: Vec<TabId> = app
        .editor
        .windows
        .tab_ids()
        .into_iter()
        .filter(|id| *id != keep)
        .collect();
    match delete_tabs(app, &others) {
        Ok(()) => app.msg("Deleted the other tabs"),
        Err(e) => app.error(e),
    }
}

/// Delete the given tabs, stopping at the first one that fails: a tab that
/// is left behind is not the same as a tab that was deleted, and the error
/// that stopped the run is the one worth reporting.
fn delete_tabs(app: &mut App, ids: &[TabId]) -> Result<(), String> {
    for id in ids {
        app.editor.delete_tab(*id)?;
    }
    Ok(())
}

fn undo_close(app: &mut App) {
    let arg = app.count().max(1) as usize;
    match app.editor.undelete_tab(arg) {
        Some((_, id)) => {
            let name = display_name(app, id);
            app.msg(format!("Restored tab {name}"));
        }
        None => app.error("No tab was restored"),
    }
}

fn move_tab(app: &mut App) {
    app.editor.move_tab(app.count() as isize);
}

fn rename_tab(app: &mut App) {
    let id = app.editor.windows.selected_tab_id();
    let default = display_name(app, id);
    app.prompt(
        "Rename tab (empty for the default): ",
        Purpose::RenameTab { default },
    );
}

fn select_tab(app: &mut App) {
    if !app.has_arg() {
        return switch_prompt(app);
    }
    let ids = app.editor.windows.tab_ids();
    match nth_arg(app, &ids) {
        Some(id) => app.editor.select_tab(id),
        None => app.error("No such tab"),
    }
}

/// The tab a numeric prefix names: 1 is the first, 9 the last, and a raw
/// C-u (4) the fourth.
fn nth_arg(app: &App, ids: &[TabId]) -> Option<TabId> {
    let n = app.count();
    let index = if n == 9 {
        ids.len().saturating_sub(1)
    } else if n >= 1 {
        (n - 1) as usize
    } else {
        return None;
    };
    ids.get(index).copied()
}

fn switch_prompt(app: &mut App) {
    let candidates: Vec<String> = app
        .editor
        .windows
        .tab_ids()
        .into_iter()
        .map(|id| display_name(app, id))
        .collect();
    app.prompt(
        "Switch to tab (name): ",
        Purpose::Choose {
            what: crate::minibuffer::Choice::SelectTab,
            candidates,
        },
    );
}

fn list_tabs(app: &mut App) {
    let names: Vec<String> = app
        .editor
        .windows
        .tab_ids()
        .into_iter()
        .map(|id| display_name(app, id))
        .collect();
    app.msg(format!("{} tab(s): {}", names.len(), names.join(", ")));
}

/// The tab's name: what `tab-rename` gave it, or the buffers it shows
///.
pub fn display_name(app: &App, id: TabId) -> String {
    display_name_in(app, app.editor.windows.selected_frame_id(), id)
}

/// The same as [`display_name`], for any frame.
pub fn display_name_in(app: &App, frame: FrameId, id: TabId) -> String {
    if let Some(custom) = app.editor.windows.frame_custom_tab_name(frame, id) {
        return custom;
    }
    let names: Vec<String> = app
        .editor
        .windows
        .frame_tab_window_buffers(frame, id)
        .into_iter()
        .filter_map(|b| app.editor.state.get_buffer(b).map(|b| b.name().to_string()))
        .collect();
    if names.is_empty() {
        "???".to_string()
    } else {
        names.join(" ")
    }
}

/// Every tab of the selected frame: (what to show, whether it's current).
pub fn tabs(app: &App) -> Vec<(TabId, String, bool)> {
    tabs_in(app, app.editor.windows.selected_frame_id())
}

/// Every tab of any frame: (what to show, whether it's current). The
/// macOS app draws one bar per frame, so it needs a frame that isn't the
/// selected one.
pub fn tabs_in(app: &App, frame: FrameId) -> Vec<(TabId, String, bool)> {
    let selected = app.editor.windows.frame_selected_tab_id(frame);
    app.editor
        .windows
        .frame_tab_ids(frame)
        .into_iter()
        .map(|id| (id, display_name_in(app, frame, id), id == selected))
        .collect()
}

/// select-tab-by-name (tab-switch answered): make the first tab with this
/// name the current one.
pub fn select_by_name(app: &mut App, name: &str) {
    let found = app
        .editor
        .windows
        .tab_ids()
        .into_iter()
        .find(|id| display_name(app, *id) == name);
    match found {
        Some(id) => {
            app.editor.select_tab(id);
            app.msg(format!("Selected tab {name}"));
        }
        None => app.error(format!("No tab named {name}")),
    }
}

/// tab-rename answered.
pub fn rename_selected(app: &mut App, name: &str) {
    let id = app.editor.windows.selected_tab_id();
    app.editor.rename_tab(id, name);
    if name.is_empty() {
        app.msg("Tab renamed to its default name");
    } else {
        app.msg(format!("Tab renamed to {name}"));
    }
}

/// The bar's row as spans, for the terminal frontend.
pub fn row(app: &App) -> Vec<Span> {
    view(app).map_or_else(Vec::new, |v| v.row)
}

/// The bar's row and where each tab sits in it, for drawing and clicks.
pub fn view(app: &App) -> Option<TabStripView> {
    if !visible(app) {
        return None;
    }
    let width = app.editor.windows.frame().width;
    let mut row: Vec<Span> = Vec::new();
    let mut hits: Vec<(TabId, u16, u16)> = Vec::new();
    let mut closes: Vec<(TabId, u16)> = Vec::new();
    for (id, name, selected) in tabs(app) {
        let label = format!(" {name} × ");
        let w = label.chars().count() as u16;
        let column: u16 = row
            .iter()
            .map(|s: &Span| s.text.chars().count() as u16)
            .sum();
        if column.saturating_add(w) > width {
            break;
        }
        hits.push((id, column, w));
        closes.push((id, column + w - 2));
        row.push(Span {
            text: label,
            face: if selected {
                Face::TabBarSelected
            } else {
                Face::TabBar
            },
        });
    }
    let used: u16 = row.iter().map(|s| s.text.chars().count() as u16).sum();
    if used < width {
        row.push(Span {
            text: " ".repeat((width - used) as usize),
            face: Face::TabBar,
        });
    }
    Some(TabStripView {
        row,
        tabs: hits,
        closes,
    })
}

/// The tab bar's row and its tabs' columns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TabStripView {
    pub row: Vec<Span>,
    /// (tab, first column, width) of each tab in the row.
    pub tabs: Vec<(TabId, u16, u16)>,
    pub closes: Vec<(TabId, u16)>,
}

/// Which tab a click at `column` (frame-relative) hits.
pub fn tab_at(app: &App, column: u16) -> Option<TabId> {
    let v = view(app)?;
    v.closes
        .iter()
        .find(|(_, x)| column == *x || (*x > 0 && column == *x + 1))
        .map(|(id, _)| *id)
        .or_else(|| {
            v.tabs
                .iter()
                .find(|(_, x, w)| column >= *x && column < x + w)
                .map(|(id, _, _)| *id)
        })
}

/// The screen row the bar is on: the one just above the windows, below the
/// menu bar, tool bar and modifier bar when they are showing. Both
/// frontends go through this, so hit-testing and drawing agree on which row
/// it is.
pub fn bar_row(app: &App) -> u16 {
    app.editor
        .windows
        .frame()
        .y
        .saturating_sub(1 + crate::toolbar::modifier_height(app))
}

/// per-frame tab-bar overrides the app carries (toggle-frame-tab-bar).
pub type Overrides = HashMap<FrameId, bool>;

pub fn open_in_new_tab(app: &mut App, path: &str) {
    new_tab_public(app);
    if !path.is_empty() {
        let _ = app.editor.open_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands;

    fn app() -> App {
        let mut editor = forge_core::Editor::new();
        editor.create_buffer("*scratch*");
        App::new(editor)
    }

    fn run(app: &mut App, cmd: &str) {
        assert!(commands::execute(app, cmd), "{cmd} ran");
    }

    #[test]
    fn tab_new_copies_the_config_and_the_bar_appears() {
        let mut app = app();
        assert!(!visible(&app), "one tab and the mode off");
        run(&mut app, "tab-bar-mode");
        assert!(mode_on(&app));
        assert!(!visible(&app), "a single tab keeps the row hidden");
        run(&mut app, "split-window-below");
        run(&mut app, "tab-new");
        assert_eq!(app.editor.windows.tab_count(), 2);
        assert!(visible(&app), "two tabs and the bar shows");
        // The copy has the same number of windows as the tab it came from.
        assert_eq!(app.editor.windows.count(), 2);
    }

    #[test]
    fn tab_close_and_undo_round_trip() {
        let mut app = app();
        run(&mut app, "tab-bar-mode");
        run(&mut app, "tab-new");
        assert_eq!(app.editor.windows.tab_count(), 2);
        let first = app.editor.windows.tab_ids()[0];
        app.editor.select_tab(first);
        run(&mut app, "tab-close");
        assert_eq!(app.editor.windows.tab_count(), 1);
        run(&mut app, "tab-undo");
        assert_eq!(app.editor.windows.tab_count(), 2);
    }

    #[test]
    fn a_failed_delete_stops_the_run_and_is_reported() {
        let mut app = app();
        run(&mut app, "tab-new");
        assert_eq!(app.editor.windows.tab_count(), 2);
        let real = app.editor.windows.tab_ids()[0];
        // A tab that is not there: deleting it fails, and the tab after it
        // is left alone rather than deleted behind a swallowed error.
        let err = delete_tabs(&mut app, &[TabId(999), real]).unwrap_err();
        assert_eq!(err, "No such tab");
        assert_eq!(app.editor.windows.tab_count(), 2, "the run stopped");
    }

    #[test]
    fn a_numeric_prefix_selects_a_tab() {
        let mut app = app();
        run(&mut app, "tab-bar-mode");
        run(&mut app, "tab-new");
        let ids = app.editor.windows.tab_ids();
        app.prefix_arg = Some(crate::app::PrefixArg::Raw(1));
        run(&mut app, "tab-select");
        assert_eq!(app.editor.windows.selected_tab_id(), ids[0]);
    }

    #[test]
    fn rename_gives_a_custom_name() {
        let mut app = app();
        run(&mut app, "tab-bar-mode");
        rename_selected(&mut app, "work");
        let id = app.editor.windows.selected_tab_id();
        assert_eq!(display_name(&app, id), "work");
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("forge-tabs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn other_tab_commands_ask_with_other_tab_set() {
        let mut app = app();
        run(&mut app, "find-file-other-tab");
        assert!(matches!(
            app.minibuffer.as_ref().map(|m| &m.purpose),
            Some(crate::minibuffer::Purpose::FindFile {
                other_tab: true,
                ..
            })
        ));
        app.minibuffer = None;
        run(&mut app, "switch-to-buffer-other-tab");
        assert!(matches!(
            app.minibuffer.as_ref().map(|m| &m.purpose),
            Some(crate::minibuffer::Purpose::SwitchBuffer {
                other_tab: true,
                ..
            })
        ));
        app.minibuffer = None;
        run(&mut app, "dired-other-tab");
        assert!(matches!(
            app.minibuffer.as_ref().map(|m| &m.purpose),
            Some(crate::minibuffer::Purpose::Dired(
                crate::dired::DiredAction::OpenOtherTab
            ))
        ));
    }

    #[test]
    fn find_file_other_tab_makes_a_tab_and_visits() {
        let mut app = app();
        let dir = temp_dir("find-file");
        let file = dir.join("hello.txt");
        std::fs::write(&file, "hi").unwrap();
        let before = app.editor.windows.tab_count();
        crate::minibuffer::visit_file_where(&mut app, &file, false, false, true);
        assert_eq!(app.editor.windows.tab_count(), before + 1);
        let opened = std::fs::canonicalize(&file).unwrap();
        assert_eq!(app.buf_ref().path(), Some(opened.as_path()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dired_other_tab_makes_a_tab() {
        let mut app = app();
        let dir = temp_dir("dired");
        let before = app.editor.windows.tab_count();
        crate::dired::answered(
            &mut app,
            crate::dired::DiredAction::OpenOtherTab,
            &dir.to_string_lossy(),
        );
        assert_eq!(app.editor.windows.tab_count(), before + 1);
        assert_eq!(app.buf_ref().major_mode(), crate::dired::MODE);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
