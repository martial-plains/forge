// SPDX-License-Identifier: MIT OR Apache-2.0
//! The context menu (`context-menu-mode`): the menu a right-click, or
//! S-F10, pops up at the pointer, filled with what makes sense there.
//!
//! Forge builds it from `context-menu-functions`, a list of functions that
//! each add their part to the menu: undo and redo, the region commands, a
//! separator, the major mode's items, and so on. [`build`] does the same
//! with the builders named in [`DEFAULT_FUNCTIONS`], from a plain snapshot
//! of the editor ([`Input`]), so what a menu holds can be tested without a
//! terminal. The popup itself is the ordinary menu machinery in
//! [`crate::menu`]: a [`Registry`] with one menu in it, which the keys
//! and the pointer drive the way they drive the bar.
//!
//! the mode is global and off by default.
//!
//! `local` and `minor` show the menu a mode defines for itself: a menu in
//! the registry titled with the mode's name (`org-mode`) or its pretty
//! form (`Org`), which a plugin adds with `define_menu`. `global`,
//! `buffers`, `project` and `vc` are built from the bar's menus. `ffap`
//! isn't built (Forge has no file-at-point command to offer).

use crate::menu::{Item, MenuBar, Registry};

/// `context-menu-functions`' default 30 has it.
pub const DEFAULT_FUNCTIONS: &[&str] = &["undo", "region", "middle-separator", "local", "minor"];

/// The title of the one menu a context menu's registry holds.
pub const TITLE: &str = "Context Menu";

/// The prefix of the commands that paste kill-ring entry N (1 is the
/// newest) at the click: `context-menu-yank-3`.
pub const YANK_PREFIX: &str = "context-menu-yank-";

/// How many kill-ring entries "Paste from Kill Menu" offers.
pub const YANK_ENTRIES: usize = 20;

/// What the builders need to know about the editor, taken when the menu
/// is opened.
#[derive(Debug, Clone, Default)]
pub struct Input {
    pub read_only: bool,
    /// A region is active.
    pub region: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    /// The click (or point, for S-F10) is inside a string literal.
    pub in_string: bool,
    /// The kill ring, newest first.
    pub kill_ring: Vec<String>,
    /// Whether the system clipboard has something to paste. Forge pastes
    /// from the kill ring, so this only matters to a frontend that knows.
    pub clipboard: bool,
    /// The open buffers' names, the selected one first.
    pub buffers: Vec<String>,
    /// The buffer's major mode and minor modes, for `local` and `minor`.
    pub major_mode: String,
    pub minor_modes: Vec<String>,
    /// Every menu in the bar as (title, its leaf items), for `global`,
    /// `local`, `minor` and `project`.
    pub menus: Vec<(String, Vec<Item>)>,
    /// The buffer's file is under version control, for `vc`.
    pub under_vc: bool,
}

/// The context menu: its items as a registry the menu code can drive,
/// its state, and where it is on the frame.
#[derive(Debug, Clone, Default)]
pub struct ContextMenu {
    pub bar: MenuBar,
    pub registry: Registry,
    /// Frame-relative column and row of the popup's top-left cell.
    pub column: u16,
    pub row: u16,
    /// The buffer offset of the click, which Paste and Select act on.
    /// Kept after the menu closes, since the command it ran needs it.
    pub click: Option<usize>,
}

impl ContextMenu {
    /// Is it showing?
    pub fn is_open(&self) -> bool {
        self.bar.open.is_some()
    }

    /// Fill it for `input` and open it at (`column`, `row`).
    pub fn open(
        &mut self,
        functions: &[String],
        input: &Input,
        column: u16,
        row: u16,
        click: Option<usize>,
    ) {
        let items = build(functions, input);
        self.registry = Registry::default();
        self.bar = MenuBar::default();
        self.column = column;
        self.row = row;
        self.click = click;
        if items.is_empty() {
            return;
        }
        self.registry = Registry::single(TITLE, items);
        self.bar.toggle(0, &self.registry);
    }

    /// Open a popup of ready-made `items` (the mode line's menus) at
    /// (`column`, `row`).
    pub fn open_items(&mut self, items: Vec<Item>, column: u16, row: u16) {
        let items = tidy(items);
        self.registry = Registry::default();
        self.bar = MenuBar::default();
        self.column = column;
        self.row = row;
        self.click = None;
        if items.is_empty() {
            return;
        }
        self.registry = Registry::single(TITLE, items);
        self.bar.toggle(0, &self.registry);
    }

    pub fn close(&mut self) {
        self.bar.close();
    }
}

/// Build the menu's items: each named builder in turn, then Forge's
/// cleanup (`context-menu-map`): no separator first, last or doubled.
pub fn build(functions: &[String], input: &Input) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    for f in functions {
        match f.as_str() {
            "undo" => undo(&mut items, input),
            "region" => region(&mut items, input),
            "middle-separator" => items.push(Item::separator()),
            "global" => global(&mut items, input),
            "local" => local(&mut items, input),
            "minor" => minor(&mut items, input),
            "buffers" => buffers(&mut items, input),
            "project" => project(&mut items, input),
            "vc" => vc(&mut items, input),
            // Not built (ffap, ...), but accepted so a
            // `context-menu-functions` from Forge keeps working.
            _ => {}
        }
    }
    tidy(items)
}

/// `context-menu-global`: the menu bar's own menus, as submenus.
fn global(items: &mut Vec<Item>, input: &Input) {
    items.push(Item::separator());
    for (title, leaves) in &input.menus {
        let leaves: Vec<Item> = leaves.to_vec();
        if !leaves.is_empty() {
            items.push(Item::parent(title.clone(), leaves));
        }
    }
}

/// How a mode is named in a menu title: `org-mode` is "Org".
fn pretty(mode: &str) -> String {
    mode.trim_end_matches("-mode")
        .split('-')
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The menu a mode defines for itself: one titled with the mode's name
/// (`org-mode`) or its pretty form (`Org`).
fn mode_menu<'a>(input: &'a Input, mode: &str) -> Option<&'a (String, Vec<Item>)> {
    let pretty = pretty(mode);
    input
        .menus
        .iter()
        .find(|(t, l)| !l.is_empty() && (t == mode || *t == pretty))
}

/// `context-menu-local`: the major mode's menu.
fn local(items: &mut Vec<Item>, input: &Input) {
    if let Some((title, leaves)) = mode_menu(input, &input.major_mode) {
        items.push(Item::parent(title.clone(), leaves.clone()));
    }
}

/// `context-menu-minor`: the menus of the minor modes that are on.
fn minor(items: &mut Vec<Item>, input: &Input) {
    for m in &input.minor_modes {
        if let Some((title, leaves)) = mode_menu(input, m) {
            items.push(Item::parent(title.clone(), leaves.clone()));
        }
    }
}

/// `context-menu-buffers`: the open buffers.
fn buffers(items: &mut Vec<Item>, input: &Input) {
    if input.buffers.is_empty() {
        return;
    }
    items.push(Item::separator());
    let kids = input
        .buffers
        .iter()
        .take(crate::menu::BUFFERS_MENU_MAX)
        .map(|n| Item::new(n.clone(), format!("{}{n}", crate::menu::SELECT_BUFFER)))
        .collect();
    items.push(Item::parent("Buffers", kids));
}

/// `context-menu-vc`: Tools > Version Control, for a file under version control.
fn vc(items: &mut Vec<Item>, input: &Input) {
    if !input.under_vc {
        return;
    }
    let kids: Vec<Item> = input
        .menus
        .iter()
        .find(|(t, _)| t == "Tools")
        .map(|(_, l)| {
            l.iter()
                .filter(|i| i.command.starts_with("vc-"))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if !kids.is_empty() {
        items.push(Item::separator());
        items.push(Item::parent("VC", kids));
    }
}

/// `context-menu-project`: Tools > Project, when the project commands exist.
fn project(items: &mut Vec<Item>, input: &Input) {
    let kids = input
        .menus
        .iter()
        .find(|(t, _)| t == "Tools")
        .map(|(_, l)| {
            l.iter()
                .filter(|i| i.command.starts_with("project-"))
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !kids.is_empty() {
        items.push(Item::separator());
        items.push(Item::parent("Project", kids));
    }
}

fn undo(items: &mut Vec<Item>, input: &Input) {
    items.push(Item::separator());
    if !input.read_only && input.can_undo {
        let label = if input.region {
            "Undo in Region"
        } else {
            "Undo"
        };
        items.push(Item::new(label, "undo").shown("C-x u"));
    }
    if !input.read_only && input.can_redo {
        items.push(Item::new("Redo", "undo-redo").shown("C-M-_"));
    }
}

fn region(items: &mut Vec<Item>, input: &Input) {
    items.push(Item::separator());
    let writable = !input.read_only;
    if input.region && writable {
        items.push(Item::new("Cut", "kill-region").shown("C-w"));
    }
    if input.region {
        items.push(Item::new("Copy", "kill-ring-save").shown("M-w"));
    }
    if (input.clipboard || !input.kill_ring.is_empty()) && writable {
        items.push(Item::new("Paste", "mouse-yank-at-click").shown("C-y"));
    }
    if input.kill_ring.len() > 1 && writable {
        let entries: Vec<Item> = input
            .kill_ring
            .iter()
            .take(YANK_ENTRIES)
            .enumerate()
            .map(|(i, text)| Item::new(kill_label(text), format!("{YANK_PREFIX}{}", i + 1)))
            .collect();
        items.push(Item::parent("Paste from Kill Menu", entries));
    }
    if input.region && writable {
        items.push(Item::new("Clear", "delete-active-region"));
    }
    let mut select = vec![Item::new("All", "context-menu-select-all")];
    if input.in_string {
        select.push(Item::new("String", "context-menu-select-string"));
    }
    select.push(Item::new("Line", "context-menu-select-line"));
    if input.region {
        select.push(Item::new("None", "context-menu-select-none"));
    }
    items.push(Item::parent("Select", select));
}

/// A kill-ring entry as a menu label: its first line, shortened the way
/// `yank-menu` does (20 characters and an ellipsis).
pub fn kill_label(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let line = line.trim();
    let n = line.chars().count();
    let mut label: String = line.chars().take(20).collect();
    if n > 20 {
        label.push('\u{2026}');
    }
    if label.is_empty() {
        "(blank)".into()
    } else {
        label
    }
}

/// Drop separators at either end and doubled ones, as `context-menu-map`
/// does after the builders have run.
fn tidy(items: Vec<Item>) -> Vec<Item> {
    let mut out: Vec<Item> = Vec::new();
    for it in items {
        if it.is_separator() && out.last().is_none_or(Item::is_separator) {
            continue;
        }
        out.push(it);
    }
    while out.last().is_some_and(Item::is_separator) {
        out.pop();
    }
    out
}

/// Is the character at `col` of `line` inside a double-quoted string?
/// Counts unescaped quotes before it on the line, which is enough for the
/// menu's "String" entry.
pub fn string_bounds(line: &str, col: usize) -> Option<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let mut open: Option<usize> = None;
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' if open.is_some() => i += 1,
            '"' => match open {
                None => open = Some(i),
                Some(start) => {
                    if col >= start && col <= i {
                        return Some((start, i + 1));
                    }
                    open = None;
                }
            },
            _ => {}
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fns() -> Vec<String> {
        DEFAULT_FUNCTIONS.iter().map(|s| s.to_string()).collect()
    }

    fn labels(items: &[Item]) -> Vec<&str> {
        items
            .iter()
            .map(|i| {
                if i.is_separator() {
                    "---"
                } else {
                    i.label.as_str()
                }
            })
            .collect()
    }

    #[test]
    fn nothing_to_do_leaves_only_select() {
        let items = build(&fns(), &Input::default());
        assert_eq!(labels(&items), ["Select"]);
        let select = &items[0];
        let kids: Vec<_> = select.submenu.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            kids,
            ["All", "Line"],
            "no String outside a string, no None without a region"
        );
    }

    #[test]
    fn undo_and_redo_follow_the_undo_history_and_read_only() {
        let both = Input {
            can_undo: true,
            can_redo: true,
            ..Default::default()
        };
        assert_eq!(
            labels(&build(&fns(), &both)),
            ["Undo", "Redo", "---", "Select"]
        );
        let ro = Input {
            read_only: true,
            ..both.clone()
        };
        assert_eq!(
            labels(&build(&fns(), &ro)),
            ["Select"],
            "nothing to change in a read-only buffer"
        );
        let region = Input {
            can_undo: true,
            region: true,
            ..Default::default()
        };
        assert_eq!(build(&fns(), &region)[0].label, "Undo in Region");
    }

    #[test]
    fn region_commands_need_a_region_and_a_writable_buffer() {
        let region = Input {
            region: true,
            ..Default::default()
        };
        let l = labels(&build(&fns(), &region)).join(",");
        assert_eq!(l, "Cut,Copy,Clear,Select");
        let ro = Input {
            region: true,
            read_only: true,
            ..Default::default()
        };
        assert_eq!(
            labels(&build(&fns(), &ro)).join(","),
            "Copy,Select",
            "Copy still works"
        );
    }

    #[test]
    fn paste_wants_a_kill_and_the_kill_menu_wants_two() {
        let one = Input {
            kill_ring: vec!["a".into()],
            ..Default::default()
        };
        assert_eq!(labels(&build(&fns(), &one)).join(","), "Paste,Select");
        let two = Input {
            kill_ring: vec![
                "newest\nline two".into(),
                "older text that is long enough to cut".into(),
            ],
            ..Default::default()
        };
        let items = build(&fns(), &two);
        let kill = items
            .iter()
            .find(|i| i.label == "Paste from Kill Menu")
            .unwrap();
        assert_eq!(kill.submenu.len(), 2);
        assert_eq!(kill.submenu[0].label, "newest", "first line only");
        assert_eq!(kill.submenu[0].command, "context-menu-yank-1");
        assert_eq!(
            kill.submenu[1].label, "older text that is l\u{2026}",
            "20 characters and an ellipsis"
        );
    }

    #[test]
    fn separators_are_tidied_like_context_menu_map_does() {
        // The default list puts separators between groups; none leads, trails or doubles.
        let input = Input {
            can_undo: true,
            region: true,
            kill_ring: vec!["x".into()],
            ..Default::default()
        };
        let items = build(&fns(), &input);
        assert!(!items.first().unwrap().is_separator());
        assert!(!items.last().unwrap().is_separator());
        assert!(
            items
                .windows(2)
                .all(|w| !(w[0].is_separator() && w[1].is_separator()))
        );
        assert_eq!(
            labels(&items).join(","),
            "Undo in Region,---,Cut,Copy,Paste,Clear,Select"
        );
    }

    #[test]
    fn the_function_list_decides_what_is_built() {
        let input = Input {
            can_undo: true,
            region: true,
            ..Default::default()
        };
        let only_undo = vec!["undo".to_string()];
        assert_eq!(labels(&build(&only_undo, &input)), ["Undo in Region"]);
        assert!(
            build(&["local".into(), "minor".into(), "nonsense".into()], &input).is_empty(),
            "no mode menus here"
        );
    }

    #[test]
    fn string_bounds_finds_the_quoted_text_around_a_column() {
        let line = r#"say "hi \" there" ok"#;
        assert_eq!(string_bounds(line, 7), Some((4, 17)));
        assert_eq!(
            string_bounds(line, 4),
            Some((4, 17)),
            "on the opening quote"
        );
        assert_eq!(string_bounds(line, 1), None);
        assert_eq!(string_bounds(line, 18), None);
    }

    #[test]
    fn opening_fills_the_registry_and_highlights_the_first_item() {
        let mut cm = ContextMenu::default();
        assert!(!cm.is_open());
        cm.open(
            &fns(),
            &Input {
                can_undo: true,
                ..Default::default()
            },
            3,
            4,
            Some(9),
        );
        assert!(cm.is_open());
        assert_eq!((cm.column, cm.row, cm.click), (3, 4, Some(9)));
        assert_eq!(cm.registry.menu(0).unwrap().items[0].command, "undo");
        cm.close();
        assert!(!cm.is_open());
        assert_eq!(
            cm.click,
            Some(9),
            "the click outlives the menu, for the command it ran"
        );
    }

    #[test]
    fn local_and_minor_show_the_menus_modes_define() {
        let org = vec![Item::new("Todo", "org-todo")];
        let meow = vec![Item::new("Motion", "meow-insert")];
        let input = Input {
            major_mode: "org-mode".into(),
            minor_modes: vec!["meow-mode".into(), "hl-mode".into()],
            menus: vec![("Org".into(), org), ("Meow".into(), meow)],
            ..Default::default()
        };
        let items = build(&fns(), &input);
        assert_eq!(labels(&items), ["Select", "---", "Org", "Meow"]);
    }

    #[test]
    fn buffers_global_and_project_build_from_the_bar() {
        let input = Input {
            buffers: vec!["a.rs".into(), "b.rs".into()],
            menus: vec![
                (
                    "Tools".into(),
                    vec![
                        Item::new("Open", "project-find-file"),
                        Item::new("Grep", "grep"),
                    ],
                ),
                ("Help".into(), vec![Item::new("Mode", "describe-mode")]),
            ],
            ..Default::default()
        };
        let f = |names: &[&str]| names.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let b = build(&f(&["buffers"]), &input);
        assert_eq!(b[0].label, "Buffers");
        assert_eq!(b[0].submenu[1].command, "select-buffer:b.rs");
        let p = build(&f(&["project"]), &input);
        assert_eq!(p[0].submenu.len(), 1, "only the project- commands");
        let g = build(&f(&["global"]), &input);
        assert_eq!(labels(&g), ["Tools", "Help"]);
    }

    #[test]
    fn vc_shows_the_version_control_commands_for_a_tracked_file() {
        let tools = vec![
            Item::new("Status", "vc-dir"),
            Item::new("Push", "vc-push"),
            Item::new("Grep", "grep"),
        ];
        let tracked = Input {
            under_vc: true,
            menus: vec![("Tools".into(), tools.clone())],
            ..Default::default()
        };
        let untracked = Input {
            menus: vec![("Tools".into(), tools)],
            ..Default::default()
        };
        let vc = vec!["vc".to_string()];
        let items = build(&vc, &tracked);
        assert_eq!(items[0].label, "VC");
        assert_eq!(items[0].submenu.len(), 2, "only the vc- commands");
        assert!(build(&vc, &untracked).is_empty());
    }
}
