// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tool bars: the row of buttons at the top of the frame and the
//! modifier bar that turns the next key into C-, M-, S-, H- or s-.
//!
//! A tool bar is a list of items, each naming a command. The built-in
//! bar is default one, and a major mode can have its own bar in
//! [`Registry`]. Both frontends read the same view, so an item is in the
//! same place and greyed out for the same reason wherever it is drawn.

use std::collections::BTreeMap;

use crate::display::{Face, Span};
use crate::menu::{Context, When};
use crate::width::line_width;

/// How much of an item to draw: its icon, its label, or both.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Style {
    /// The icon only.
    Image,
    /// The label only. This is the default in a terminal, where icons
    /// are not guaranteed to be one cell wide.
    #[default]
    Text,
    /// The icon above the label.
    Both,
    /// The icon beside the label.
    BothHorizontal,
    /// The label beside the icon.
    TextImageHorizontal,
}

impl Style {
    /// The style named by `tool-bar-style`.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "image" => Some(Self::Image),
            "text" => Some(Self::Text),
            "both" => Some(Self::Both),
            "both-horiz" | "both-horizontal" => Some(Self::BothHorizontal),
            "text-image-horiz" | "text-image-horizontal" => Some(Self::TextImageHorizontal),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Text => "text",
            Self::Both => "both",
            Self::BothHorizontal => "both-horiz",
            Self::TextImageHorizontal => "text-image-horiz",
        }
    }
}

/// Where the tool bar goes. The terminal draws top and bottom; left and
/// right fall back to the top, since a vertical strip would change the
/// width every window gets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Position {
    #[default]
    Top,
    Bottom,
    Left,
    Right,
}

impl Position {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "top" => Some(Self::Top),
            "bottom" => Some(Self::Bottom),
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Bottom => "bottom",
            Self::Left => "left",
            Self::Right => "right",
        }
    }

    pub fn is_bottom(self) -> bool {
        self == Self::Bottom
    }
}

/// A button in the modifier bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Modifier {
    Control,
    Shift,
    Meta,
    Alt,
    Super,
    Hyper,
}

impl Modifier {
    /// Every button, in order.
    pub const ALL: [Self; 6] = [
        Self::Control,
        Self::Shift,
        Self::Meta,
        Self::Alt,
        Self::Super,
        Self::Hyper,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Control => "control",
            Self::Shift => "shift",
            Self::Meta => "meta",
            Self::Alt => "alt",
            Self::Super => "super",
            Self::Hyper => "hyper",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "control" | "ctrl" | "c" => Some(Self::Control),
            "shift" | "s" => Some(Self::Shift),
            "meta" | "m" => Some(Self::Meta),
            "alt" | "a" => Some(Self::Alt),
            "super" | "command" | "cmd" => Some(Self::Super),
            "hyper" | "h" => Some(Self::Hyper),
            _ => None,
        }
    }

    /// The label on its button: the key's proper name, not Forge's
    /// one-letter prefix.
    pub fn label(self) -> &'static str {
        match self {
            Self::Control => "Control",
            Self::Shift => "Shift",
            Self::Meta => "Meta",
            Self::Alt => "Alt",
            Self::Super => "Super",
            Self::Hyper => "Hyper",
        }
    }

    /// The prefix Forge shows for it.
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Control => "C-",
            Self::Shift => "S-",
            Self::Meta => "M-",
            Self::Alt => "A-",
            Self::Super => "s-",
            Self::Hyper => "H-",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|m| *m == self).unwrap_or(0)
    }
}

/// One button, or a separator when `command` is empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub label: String,
    /// A single-cell icon, used by the image styles.
    pub icon: String,
    pub command: String,
    /// When the item can be chosen.
    pub enabled: When,
    /// The plugin that added it; None for a built-in.
    pub owner: Option<String>,
}

impl Item {
    pub fn new(label: &str, icon: &str, command: &str) -> Self {
        Self {
            label: label.into(),
            icon: icon.into(),
            command: command.into(),
            enabled: When::Always,
            owner: None,
        }
    }

    pub fn separator() -> Self {
        Self {
            label: String::new(),
            icon: String::new(),
            command: String::new(),
            enabled: When::Never,
            owner: None,
        }
    }

    pub fn enabled(mut self, when: When) -> Self {
        self.enabled = when;
        self
    }

    pub fn by(mut self, owner: &str) -> Self {
        self.owner = Some(owner.into());
        self
    }

    pub fn is_separator(&self) -> bool {
        self.command.is_empty()
    }

    /// The text the item shows in `style`.
    pub fn text(&self, style: Style) -> String {
        let icon = if self.icon.is_empty() {
            &self.label
        } else {
            &self.icon
        };
        match style {
            Style::Image => format!(" {icon} "),
            Style::Text => format!(" {} ", self.label),
            Style::Both => {
                if self.icon.is_empty() {
                    format!(" {} ", self.label)
                } else {
                    format!(" {} {} ", self.icon, self.label)
                }
            }
            Style::BothHorizontal => {
                if self.icon.is_empty() {
                    format!(" {} ", self.label)
                } else {
                    format!(" {} {} ", self.icon, self.label)
                }
            }
            Style::TextImageHorizontal => {
                if self.icon.is_empty() {
                    format!(" {} ", self.label)
                } else {
                    format!(" {} {} ", self.label, self.icon)
                }
            }
        }
    }
}

/// A row of items.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Toolbar {
    pub items: Vec<Item>,
}

impl Toolbar {
    pub fn new(items: Vec<Item>) -> Self {
        Self { items }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn push(&mut self, item: Item) {
        self.items.push(item);
    }

    pub fn extend(&mut self, items: impl IntoIterator<Item = Item>) {
        self.items.extend(items);
    }

    /// Take back what `owner` added.
    pub fn remove_owner(&mut self, owner: &str) {
        self.items.retain(|i| i.owner.as_deref() != Some(owner));
    }
}

/// The default bar and the per-major-mode bars.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registry {
    default: Toolbar,
    modes: BTreeMap<String, Toolbar>,
}

impl Registry {
    /// The built-in tool bars.
    pub fn builtin() -> Self {
        let mut modes = BTreeMap::new();
        modes.insert(
            "dired-mode".into(),
            Toolbar::new(vec![
                Item::new("Up", "↑", "dired-up-directory"),
                Item::new("Tree", "⌥", "dired-toggle-tree"),
                Item::separator(),
                Item::new("Mark", "✓", "dired-mark"),
                Item::new("Unmark", "○", "dired-unmark"),
                Item::new("Flag", "⚑", "dired-flag-file-deletion"),
                Item::separator(),
                Item::new("Delete", "⌫", "dired-do-delete"),
                Item::new("Copy", "⧉", "dired-do-copy"),
                Item::new("Rename", "✎", "dired-do-rename"),
                Item::separator(),
                Item::new("Revert", "↻", "revert-buffer"),
            ]),
        );
        modes.insert(
            "org-mode".into(),
            Toolbar::new(vec![
                Item::new("Tags", "⌗", "org-set-tags-command"),
                Item::new("Sparse", "⌕", "org-tags-sparse-tree"),
                Item::separator(),
                Item::new("Schedule", "◷", "org-schedule"),
                Item::new("Deadline", "⚐", "org-deadline"),
                Item::separator(),
                Item::new("Open", "↗", "org-open-at-point"),
                Item::new("Checkbox", "☑", "org-toggle-checkbox"),
                Item::separator(),
                Item::new("Link", "⛓", "org-insert-link"),
                Item::new("Store", "⎘", "org-store-link"),
            ]),
        );
        modes.insert(
            "info-mode".into(),
            Toolbar::new(vec![
                Item::new("Menu", "☰", "info-menu"),
                Item::new("TOC", "≡", "info-toc"),
                Item::separator(),
                Item::new("Back", "←", "info-last"),
                Item::new("Next", "→", "info-next"),
                Item::new("Up", "↑", "info-up"),
                Item::new("Top", "⇤", "info-top"),
            ]),
        );
        for (mode, bar) in crate::mode_menus::bars() {
            modes.insert(mode.to_string(), bar);
        }
        Self {
            default: Toolbar::new(vec![
                Item::new("Open", "⊞", "find-file"),
                Item::new("Dired", "▤", "dired"),
                Item::separator(),
                Item::new("Close", "✕", "kill-this-buffer"),
                Item::new("Save", "▣", "save-buffer")
                    .enabled(When::All(vec![When::BufferModified, When::Writable])),
                Item::separator(),
                Item::new("Undo", "↶", "undo"),
                Item::new("Cut", "✂", "kill-region").enabled(When::Region),
                Item::new("Copy", "⧉", "kill-ring-save").enabled(When::Region),
                Item::new("Paste", "⎘", "yank"),
                Item::separator(),
                Item::new("Search", "⌕", "isearch-forward"),
            ]),
            modes,
        }
    }

    pub fn default_bar(&self) -> &Toolbar {
        &self.default
    }

    pub fn default_bar_mut(&mut self) -> &mut Toolbar {
        &mut self.default
    }

    pub fn mode_bar(&self, mode: &str) -> Option<&Toolbar> {
        self.modes.get(mode)
    }

    /// The bar for `mode`, or the default bar when the mode has none or
    /// `always_default` is on.
    pub fn bar_for(&self, mode: &str, always_default: bool) -> &Toolbar {
        if always_default {
            return &self.default;
        }
        let mode = match mode {
            "webkit-edit-mode" => "webkit-mode",
            "occur-mode" => "grep-mode",
            "log-view-mode" | "vc-list-mode" | "vc-annotate-mode" => "vc-dir-mode",
            other => other,
        };
        self.modes.get(mode).unwrap_or(&self.default)
    }

    /// Define (or replace) a mode's bar.
    pub fn set_mode(&mut self, mode: &str, toolbar: Toolbar) {
        self.modes.insert(mode.into(), toolbar);
    }

    pub fn mode_names(&self) -> impl Iterator<Item = &str> {
        self.modes.keys().map(String::as_str)
    }

    /// Take back what `owner` added to every bar.
    pub fn remove_owner(&mut self, owner: &str) {
        self.default.remove_owner(owner);
        for toolbar in self.modes.values_mut() {
            toolbar.remove_owner(owner);
        }
    }
}

/// The tool bar's pointer state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToolBar {
    /// The item the pointer is over.
    pub hover: Option<usize>,
    /// The modifier button the pointer is over.
    pub modifier_hover: Option<usize>,
}

/// A drawn item: where it starts and how wide it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    pub item: usize,
    pub column: u16,
    pub width: u16,
}

/// One row of buttons.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolBarView {
    pub row: Vec<Span>,
    pub hits: Vec<Hit>,
}

impl ToolBarView {
    /// The item at a frame-relative column, if the column is on one.
    pub fn item_at(&self, column: u16) -> Option<usize> {
        self.hits
            .iter()
            .find(|h| column >= h.column && column < h.column.saturating_add(h.width))
            .map(|h| h.item)
    }
}

/// Draw a bar as one row, padded to `frame` cells.
pub fn view(
    state: ToolBar,
    toolbar: &Toolbar,
    frame: u16,
    style: Style,
    context: &Context<'_>,
) -> ToolBarView {
    let mut row = Vec::new();
    let mut hits = Vec::new();
    let mut used = 0u16;
    for (i, item) in toolbar.items.iter().enumerate() {
        if item.is_separator() {
            let text = " │ ";
            let width = line_width(text) as u16;
            if used.saturating_add(width) > frame {
                break;
            }
            used += width;
            row.push(Span {
                text: text.into(),
                face: Face::ToolBar,
            });
            continue;
        }
        let text = item.text(style);
        let width = line_width(&text) as u16;
        if used.saturating_add(width) > frame {
            break;
        }
        let enabled = (context.exists)(&item.command) && item.enabled.holds(&context.state);
        let face = if !enabled {
            Face::ToolBarDisabled
        } else if state.hover == Some(i) {
            Face::ToolBarSelected
        } else {
            Face::ToolBar
        };
        hits.push(Hit {
            item: i,
            column: used,
            width,
        });
        used += width;
        row.push(Span { text, face });
    }
    if used < frame {
        row.push(Span {
            text: " ".repeat(usize::from(frame - used)),
            face: Face::ToolBar,
        });
    }
    ToolBarView { row, hits }
}

/// A drawn modifier button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModifierHit {
    pub modifier: Modifier,
    pub column: u16,
    pub width: u16,
}

/// The modifier bar's row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModifierBarView {
    pub row: Vec<Span>,
    pub hits: Vec<ModifierHit>,
}

impl ModifierBarView {
    pub fn button_at(&self, column: u16) -> Option<Modifier> {
        self.hits
            .iter()
            .find(|h| column >= h.column && column < h.column.saturating_add(h.width))
            .map(|h| h.modifier)
    }
}

/// Draw the modifier bar, marking the modifiers already waiting for the
/// next key.
pub fn modifier_view(state: ToolBar, pending: &[Modifier], frame: u16) -> ModifierBarView {
    let mut row = Vec::new();
    let mut hits = Vec::new();
    let mut used = 0u16;
    for modifier in Modifier::ALL {
        let text = format!(" {} ", modifier.label());
        let width = line_width(&text) as u16;
        if used.saturating_add(width) > frame {
            break;
        }
        let face = if pending.contains(&modifier) {
            Face::ToolBarSelected
        } else if state.modifier_hover == Some(modifier.index()) {
            Face::ToolBarSelected
        } else {
            Face::ToolBar
        };
        hits.push(ModifierHit {
            modifier,
            column: used,
            width,
        });
        used += width;
        row.push(Span { text, face });
    }
    if used < frame {
        row.push(Span {
            text: " ".repeat(usize::from(frame - used)),
            face: Face::ToolBar,
        });
    }
    ModifierBarView { row, hits }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_bar_starts_with_open_and_has_standard_commands() {
        let r = Registry::builtin();
        let commands: Vec<&str> = r
            .default_bar()
            .items
            .iter()
            .filter(|i| !i.is_separator())
            .map(|i| i.command.as_str())
            .collect();
        assert_eq!(commands[0], "find-file");
        assert!(commands.contains(&"save-buffer"));
        assert!(commands.contains(&"isearch-forward"));
    }

    #[test]
    fn builtin_bars_use_icons_and_separate_their_groups() {
        let r = Registry::builtin();
        let mut bars = vec![r.default_bar()];
        bars.extend(r.mode_names().filter_map(|name| r.mode_bar(name)));

        for bar in bars {
            assert!(
                !bar.items.first().unwrap().is_separator(),
                "a bar starts with a divider"
            );
            assert!(
                !bar.items.last().unwrap().is_separator(),
                "a bar ends with a divider"
            );
            assert!(
                bar.items
                    .windows(2)
                    .all(|w| !(w[0].is_separator() && w[1].is_separator())),
                "two dividers in a row"
            );
            assert!(
                bar.items.iter().any(Item::is_separator),
                "a built-in bar has no dividers"
            );
            for item in bar.items.iter().filter(|i| !i.is_separator()) {
                assert!(
                    !item.icon.chars().all(|c| c.is_ascii_alphanumeric()),
                    "{} uses text as its icon: {:?}",
                    item.command,
                    item.icon
                );
            }
        }
    }

    #[test]
    fn a_mode_bar_replaces_the_default_but_can_always_show_it() {
        let r = Registry::builtin();
        assert_eq!(
            r.bar_for("dired-mode", false).items[0].command,
            "dired-up-directory"
        );
        assert_eq!(r.bar_for("dired-mode", true).items[0].command, "find-file");
        assert_eq!(
            r.bar_for("fundamental-mode", false).items[0].command,
            "find-file"
        );
    }

    #[test]
    fn text_style_pads_each_item_and_skips_what_does_not_fit() {
        let bar = Toolbar::new(vec![
            Item::new("Open", "O", "find-file"),
            Item::new("Save", "S", "save-buffer"),
        ]);
        let ctx = Context::all_exist();
        let v = view(ToolBar::default(), &bar, 13, Style::Text, &ctx);
        assert_eq!(v.item_at(0), Some(0));
        assert_eq!(v.item_at(7), Some(1));
        assert_eq!(v.item_at(12), None);
        assert_eq!(
            v.row.iter().map(|s| s.text.as_str()).collect::<String>(),
            " Open  Save  "
        );
    }

    #[test]
    fn a_disabled_item_is_greyed() {
        let bar = Toolbar::new(vec![
            Item::new("Save", "S", "save-buffer")
                .enabled(When::All(vec![When::BufferModified, When::Writable])),
        ]);
        let ctx = Context {
            exists: Box::new(|_| true),
            keys: Box::new(|_| Vec::new()),
            state: crate::menu::MenuState::default(),
        };
        let v = view(ToolBar::default(), &bar, 10, Style::Text, &ctx);
        assert_eq!(v.row[0].face, Face::ToolBarDisabled);
    }

    #[test]
    fn modifier_buttons_combine_and_parse() {
        let v = modifier_view(ToolBar::default(), &[Modifier::Control, Modifier::Meta], 40);
        assert_eq!(v.button_at(0), Some(Modifier::Control));
        assert_eq!(v.row[0].face, Face::ToolBarSelected);
        assert_eq!(Modifier::parse("cmd"), Some(Modifier::Super));
        assert_eq!(Modifier::Meta.prefix(), "M-");
    }
}
