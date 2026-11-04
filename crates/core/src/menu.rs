// SPDX-License-Identifier: MIT OR Apache-2.0
//! The menu bar: the menus, the registry a plugin adds its own to, the one
//! that's open, and the row it draws.
//!
//! The bar is a row at the top of the frame with a menu per title (File,
//! Edit, ...). Opening one draws its items in a panel under it. An item
//! runs a command, so the menus are a way to discover what Forge can do
//! rather than a set of features of their own.
//!
//! [`Registry`] is the whole list: the six built-in menus, plus whatever
//! plugins have added. Both frontends read it, so the in-editor bar and
//! the macOS app's native menu bar always offer the same things, and a
//! plugin that defines a menu gets it in both.

use crate::display::{Face, Span};
use crate::window::Rect;

/// When an item can be chosen, and when it shows a tick: the editor state
/// it asks about, the way a menu item's `:enable` and `:style toggle` do
///.
///
/// Built-in conditions only, for the reason a conditional layer's are
/// (see [`crate::keymap::Condition`]): an item must be answerable from
/// one snapshot of the editor, taken when the menu is drawn, because the
/// terminal redraws the open panel on every keypress and the macOS menu
/// bar asks again each time a menu opens. A predicate a plugin supplied
/// would run every time. What a plugin can express is the menu titles and
/// items themselves, and any of these conditions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum When {
    /// Always, which is what an item says when it has no condition.
    Always,
    /// Never, which is what an item says when it has nothing to tick.
    #[default]
    Never,
    /// `(buffer-modified-p)`: this buffer has unsaved changes. What greys
    /// Save out when there's nothing to save.
    BufferModified,
    /// The other way round: nothing has changed since the file was read.
    BufferUnmodified,
    /// A buffer nothing can be written to: a file with no write permission,
    /// or one opened read-only.
    ReadOnly,
    /// A buffer that can be written.
    Writable,
    /// More than one window, so a menu item that works across windows has
    /// something to work across.
    SeveralWindows,
    /// A region is selected, so the region commands have one to work on.
    Region,
    /// The buffer's major mode is this one, or derives from it (for a
    /// plugin's own modes).
    MajorMode(String),
    /// This minor mode is on in the buffer.
    MinorMode(String),
    /// The buffer's `name` option is a flag and it's on: `hl-line` and the
    /// rest, set by the mode that toggles it.
    BufferFlag(String),
    /// The buffer's `name` option is exactly `value`: `wrap` is `char`.
    BufferOption(String, String),
    /// The editor's own display setting `name` is on, for the ones that
    /// aren't buffer-local (`show-paren-mode`).
    Flag(String),
    Not(Box<When>),
    All(Vec<When>),
    Any(Vec<When>),
}

impl When {
    /// Does this hold in the given state?
    pub fn holds(&self, state: &MenuState) -> bool {
        match self {
            When::Always => true,
            When::Never => false,
            When::BufferModified => state.modified,
            When::BufferUnmodified => !state.modified,
            When::ReadOnly => state.read_only,
            When::Writable => !state.read_only,
            When::SeveralWindows => state.windows > 1,
            When::Region => state.region,
            When::MajorMode(m) => state.major_modes.iter().any(|on| on == m),
            When::MinorMode(m) => state.minor_modes.iter().any(|on| on == m),
            When::BufferFlag(name) => (state.option)(name).as_deref() == Some("true"),
            When::BufferOption(name, value) => {
                (state.option)(name).as_deref() == Some(value.as_str())
            }
            When::Flag(name) => (state.flag)(name),
            When::Not(c) => !c.holds(state),
            When::All(cs) => cs.iter().all(|c| c.holds(state)),
            When::Any(cs) => cs.iter().any(|c| c.holds(state)),
        }
    }
}

/// The editor state a menu's conditions are answered from.
///
/// One of these is taken when the menu is drawn, so every item in it is
/// asking about the same editor, and no item's answer depends on which
/// item was asked first.
pub struct MenuState<'a> {
    /// Does this buffer have unsaved changes?
    pub modified: bool,
    /// Is it read-only?
    pub read_only: bool,
    /// How many windows are open.
    pub windows: usize,
    /// Is there a region selected in the current buffer?
    pub region: bool,
    /// The current buffer's major mode, empty for none.
    pub major_mode: String,
    /// The current buffer's major mode and the modes it derives from.
    pub major_modes: Vec<String>,
    /// The minor modes on in the current buffer.
    pub minor_modes: Vec<String>,
    /// The current buffer's options, by name. Only the ones a menu asks
    /// about are worth reading.
    pub option: OptionFor<'a>,
    /// The editor's own display settings, by name, for the ones that
    /// aren't buffer-local.
    pub flag: FlagFor<'a>,
}

/// One buffer's options, by name: the value of `name`, or None when the
/// buffer hasn't set it.
pub type OptionFor<'a> = Box<dyn Fn(&str) -> Option<String> + 'a>;

/// One of the editor's own display settings, by name, on or off.
pub type FlagFor<'a> = Box<dyn Fn(&str) -> bool + 'a>;

impl Default for MenuState<'_> {
    fn default() -> Self {
        Self {
            modified: false,
            read_only: false,
            windows: 1,
            region: false,
            major_mode: String::new(),
            major_modes: Vec::new(),
            minor_modes: Vec::new(),
            option: Box::new(|_| None),
            flag: Box::new(|_| false),
        }
    }
}

impl<'a> MenuState<'a> {
    /// Everything the editor itself knows: the selected buffer, the
    /// windows, and its modes.
    ///
    /// The display settings that aren't buffer-local live in the frontend
    /// crate (they're part of what it draws), so they start out false here;
    /// a frontend that has them passes them to [`MenuState::with_flags`].
    pub fn of(editor: &'a crate::app::Editor) -> MenuState<'a> {
        let buf = editor.state.active_buffer();
        let region = buf.is_some_and(|b| b.active_region().is_some());
        let modified = buf.is_some_and(|b| b.is_modified());
        let read_only = buf.is_some_and(|b| b.read_only());
        let major_mode = buf.map(|b| b.major_mode().to_string()).unwrap_or_default();
        let major_modes = editor
            .modes
            .lineage(&major_mode)
            .map(str::to_string)
            .collect();
        let minor_modes = buf.map(|b| b.minor_modes().to_vec()).unwrap_or_default();
        MenuState {
            modified,
            read_only,
            windows: editor.windows.count(),
            region,
            major_mode,
            major_modes,
            minor_modes,
            option: Box::new(|name| {
                editor
                    .state
                    .active_buffer()
                    .and_then(|b| b.option(name).map(|s| s.to_string()))
            }),
            flag: Box::new(|_| false),
        }
    }

    /// One buffer's options, looked up by name.
    pub fn with_options(mut self, option: impl Fn(&str) -> Option<String> + 'a) -> Self {
        self.option = Box::new(option);
        self
    }

    /// The editor's display settings, looked up by name.
    pub fn with_flags(mut self, flag: impl Fn(&str) -> bool + 'a) -> Self {
        self.flag = Box::new(flag);
        self
    }
}

/// One entry in a menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub label: String,
    /// The command it runs, empty for a separator.
    pub command: String,
    /// The keys to show for this item, instead of the ones bound to its
    /// command: one binding, the way menus show one, when the
    /// command has several (Undo shows C-x u here, not C-/, C-_ and
    /// C-x u). None shows whatever is bound.
    pub shown_keys: Option<String>,
    /// The submenu this item opens, empty for a plain item. Submenus
    /// nest one level: their items are always leaves, which is also all
    /// the WIT plugin API and the UniFFI boundary can carry.
    pub submenu: Vec<Item>,
    /// When it can be chosen. Anything else is drawn
    /// greyed out rather than hidden: an item that appears and disappears
    /// moves the ones below it while the pointer is on the menu.
    pub enabled: When,
    /// When it shows a tick, for the
    /// items that turn a display option on and off.
    pub checked: When,
    /// The plugin that added it; None for a built-in. Unloading that
    /// plugin takes its items out again.
    pub owner: Option<String>,
}

impl Item {
    pub fn new(label: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            command: command.into(),
            shown_keys: None,
            submenu: Vec::new(),
            enabled: When::Always,
            checked: When::Never,
            owner: None,
        }
    }

    /// A submenu header: a label that opens `items` rather than running
    /// anything itself.
    pub fn parent(label: impl Into<String>, items: Vec<Item>) -> Self {
        Self {
            label: label.into(),
            submenu: items,
            ..Self::new("", "")
        }
    }

    /// A separator: a rule between groups of items.
    pub fn separator() -> Self {
        Self::new("", "")
    }

    /// Is this a separator rather than something to run?
    pub fn is_separator(&self) -> bool {
        self.command.is_empty() && self.submenu.is_empty()
    }

    /// Does this open a submenu rather than running a command?
    pub fn has_submenu(&self) -> bool {
        !self.submenu.is_empty()
    }

    /// The item, greyed out unless `when` holds.
    pub fn enabled(mut self, when: When) -> Self {
        self.enabled = when;
        self
    }

    /// The item, showing `keys` for its shortcut instead of the ones
    /// bound to its command.
    pub fn shown(mut self, keys: &str) -> Self {
        self.shown_keys = Some(keys.into());
        self
    }

    /// The item, ticked while `when` holds.
    pub fn checked(mut self, when: When) -> Self {
        self.checked = when;
        self
    }

    /// The same item, owned by `owner`.
    pub fn by(mut self, owner: &str) -> Self {
        self.owner = Some(owner.to_string());
        self
    }
}

/// A title and its items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Menu {
    pub title: String,
    /// The plugin that defined the whole menu (rather than adding items to
    /// a built-in one).
    pub owner: Option<String>,
    /// When the menu shows at all, the way a mode's `easy-menu-define`
    /// menu only appears while that mode is on.
    pub when: When,
    pub items: Vec<Item>,
}

impl Menu {
    /// Does this menu show in `state`?
    pub fn visible(&self, state: &MenuState) -> bool {
        self.when.holds(state)
    }
}

/// Take back what `owner` added to one menu: its items, the ones in
/// its submenus, and a submenu header it made that nothing is left in.
/// A header that still holds someone else's items stays, so they don't
/// lose their home.
fn strip_owner(menu: &mut Menu, owner: &str) {
    menu.items.retain(|it| it.owner.as_deref() != Some(owner));
    for it in &mut menu.items {
        it.submenu.retain(|c| c.owner.as_deref() != Some(owner));
    }
    menu.items.retain(|it| {
        !(it.has_submenu() && it.submenu.is_empty() && it.owner.as_deref() == Some(owner))
    });
}

/// The menus in the bar, in the order they show.
///
/// The built-in ones first, then any a plugin defined.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registry {
    menus: Vec<Menu>,
}

/// The prefix of the commands the Buffers menu's live list runs:
/// `select-buffer:NAME` switches to the buffer called NAME.
pub const SELECT_BUFFER: &str = "select-buffer:";

/// How many buffers the Buffers menu lists, as `buffers-menu-max-size`.
pub const BUFFERS_MENU_MAX: usize = 20;

impl Registry {
    /// Replace the Buffers menu's live list of open buffers: what comes
    /// before its first static item (Next Buffer). Forge rebuilds this
    /// each time the menu opens; Forge's items can't be computed on the
    /// fly, so the editor calls this when one is about to open.
    pub fn set_buffers(&mut self, names: &[String]) {
        let Some(menu) = self
            .menus
            .iter_mut()
            .find(|m| m.title == "Buffers" && m.owner.is_none())
        else {
            return;
        };
        let Some(first) = menu.items.iter().position(|i| i.command == "next-buffer") else {
            return;
        };
        let mut live: Vec<Item> = names
            .iter()
            .take(BUFFERS_MENU_MAX)
            .map(|n| Item::new(n.clone(), format!("{SELECT_BUFFER}{n}")))
            .collect();
        if !live.is_empty() {
            live.push(Item::separator());
        }
        menu.items.splice(0..first, live);
    }

    /// The items of the menu titled `title` as one flat list of leaves
    /// (submenu children in place of their headers), for places that show
    /// a menu inside another.
    pub fn leaves_of(&self, title: &str) -> Vec<Item> {
        let Some(m) = self.menus.iter().find(|m| m.title == title) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for it in &m.items {
            if it.has_submenu() {
                out.extend(it.submenu.iter().cloned());
            } else {
                out.push(it.clone());
            }
        }
        out
    }

    /// A registry of one built-in menu: what the context menu is.
    pub fn single(title: &str, items: Vec<Item>) -> Self {
        Registry {
            menus: vec![Menu {
                title: title.into(),
                owner: None,
                when: When::Always,
                items,
            }],
        }
    }

    pub fn len(&self) -> usize {
        self.menus.len()
    }

    pub fn is_empty(&self) -> bool {
        self.menus.is_empty()
    }

    pub fn menu(&self, index: usize) -> Option<&Menu> {
        self.menus.get(index)
    }

    pub fn menus(&self) -> &[Menu] {
        &self.menus
    }

    /// The index of the menu called `title`.
    pub fn index_of(&self, title: &str) -> Option<usize> {
        self.menus.iter().position(|m| m.title == title)
    }

    /// The indexes of the menus that show in `state`, in bar order.
    pub fn visible_indices(&self, state: &MenuState) -> Vec<usize> {
        self.menus
            .iter()
            .enumerate()
            .filter(|(_, m)| m.visible(state))
            .map(|(i, _)| i)
            .collect()
    }

    /// Add a whole menu, after the built-in ones. Fails if this plugin
    /// already has one by that name, or if the title is taken.
    pub fn define(&mut self, title: &str, items: Vec<Item>, owner: &str) -> Result<(), String> {
        self.define_when(title, items, owner, When::Always)
    }

    /// Add a whole menu that only shows while `when` holds. Fails under
    /// the same conditions as [`Registry::define`].
    pub fn define_when(
        &mut self,
        title: &str,
        items: Vec<Item>,
        owner: &str,
        when: When,
    ) -> Result<(), String> {
        if title.is_empty() {
            return Err("a menu needs a title".into());
        }
        // A plugin may define several menus (a Bases menu for each of its
        // modes, say), and one plugin's menus may share a title when they
        // show under different conditions: only one shows at a time.
        if self.menus.iter().any(|m| m.title == title && (m.owner.as_deref() != Some(owner) || m.when == when)) {
            return Err(format!("there is already a {title} menu"));
        }
        self.menus.push(Menu {
            title: title.to_string(),
            owner: Some(owner.to_string()),
            when,
            items: items.into_iter().map(|i| i.by(owner)).collect(),
        });
        Ok(())
    }

    /// Add items to a menu someone else defined — a built-in one, or
    /// another plugin's — after the ones already there.
    pub fn extend(&mut self, title: &str, items: Vec<Item>, owner: &str) -> Result<(), String> {
        let Some(i) = self.index_of(title) else {
            return Err(format!("there is no {title} menu"));
        };
        if self.menus[i].owner.as_deref() == Some(owner) {
            return Err(format!(
                "{owner} defined {title} itself; define items on another menu"
            ));
        }
        self.menus[i]
            .items
            .extend(items.into_iter().map(|i| i.by(owner)));
        Ok(())
    }

    /// Add items to the submenu called `label` in the menu called
    /// `title`, after the ones already there. A missing label becomes a
    /// submenu header owned by `owner`; a label on a plain item fails,
    /// since opening it would hide its command.
    pub fn extend_submenu(
        &mut self,
        menu: &str,
        label: &str,
        items: Vec<Item>,
        owner: &str,
    ) -> Result<(), String> {
        let Some(i) = self.index_of(menu) else {
            return Err(format!("there is no {menu} menu"));
        };
        if label.is_empty() {
            return Err("a submenu needs a label".into());
        }
        let items: Vec<Item> = items.into_iter().map(|it| it.by(owner)).collect();
        let m = &mut self.menus[i];
        match m.items.iter().position(|it| it.label == label) {
            Some(j) if m.items[j].has_submenu() => m.items[j].submenu.extend(items),
            Some(_) => {
                return Err(format!(
                    "{label} in {} runs a command; it can't also open a submenu",
                    m.title
                ));
            }
            None => m.items.push(Item::parent(label, items).by(owner)),
        }
        Ok(())
    }

    /// Take back what `owner` added to `title`: the whole menu if it
    /// defined it, otherwise just its items — including the ones in its
    /// submenus, and a submenu header it made that nothing is left in.
    pub fn remove(&mut self, title: &str, owner: &str) -> Result<(), String> {
        let Some(i) = self.index_of(title) else {
            return Err(format!("there is no {title} menu"));
        };
        if self.menus[i].owner.as_deref() == Some(owner) {
            self.menus.remove(i);
            return Ok(());
        }
        strip_owner(&mut self.menus[i], owner);
        Ok(())
    }

    /// Forget everything `owner` added: its menu, and its items in the
    /// others (a plugin being unloaded) — including the ones in their
    /// submenus, and a submenu header it made that nothing is left in.
    pub fn remove_plugin(&mut self, owner: &str) {
        self.menus.retain(|m| m.owner.as_deref() != Some(owner));
        for m in &mut self.menus {
            strip_owner(m, owner);
        }
    }

    /// The built-in menus: own six (File, Edit, Options, Buffers,
    /// Tools, Help), with what Forge adds to them. Items are the ones
    /// menus have that Forge has a command for; the rest are in
    /// The roadmap. Forge's former Go, View and Packages menus are now Edit >
    /// Go To, Options > Show/Hide and Options > Manage Packages.
    pub fn builtin() -> Self {
        let leaf = |label: &str, command: &str| builtin_item(label, command);
        let sep = Item::separator;
        let sub = |label: &str, items: Vec<Item>| Item::parent(label, items);
        let menu = |title: &str, items: Vec<Item>| Menu {
            title: title.into(),
            owner: None,
            when: When::Always,
            items,
        };
        let mut registry = Registry {
            menus: vec![
                // File menu (`menu-bar-file-menu`), without what Forge
                // has no command for: Insert File, Recover Session, tabs and
                // printing (the roadmap lists them).
                menu(
                    "File",
                    vec![
                        leaf("Open File...", "find-file"),
                        leaf("Open File In Project...", "project-find-file"),
                        leaf("Open Directory...", "dired"),
                        leaf("Open File with Picker...", "find-file-dialog"),
                        leaf("Open Folder with Picker...", "find-directory-dialog"),
                        leaf("Insert File...", "insert-file").shown("C-x i"),
                        leaf("Open Recent", "recentf-open"),
                        leaf("Close", "kill-this-buffer"),
                        leaf("New Tab", "tab-new"),
                        leaf("Close Tab", "tab-close"),
                        sep(),
                        leaf("Save", "save-buffer").shown("C-x C-s"),
                        leaf("Save As...", "write-file").shown("C-x C-w"),
                        leaf("Save As with Picker...", "write-file-dialog"),
                        leaf("Revert Buffer", "revert-buffer"),
                        sep(),
                        leaf("New Window Below", "split-window-below"),
                        leaf("New Window on Right", "split-window-right"),
                        leaf("Remove Other Windows", "delete-other-windows"),
                        sep(),
                        leaf("New Frame", "make-frame").shown("C-x 5 2"),
                        leaf("Delete Frame", "delete-frame").shown("C-x 5 0"),
                        leaf("Undelete Frame", "undelete-frame").shown("C-x 5 u"),
                        sep(),
                        leaf("Quit", "save-buffers-kill-terminal"),
                    ],
                ),
                // Edit menu. Forge's submenus nest one level, so
                // Search, Incremental Search, Replace, Go To and Bookmarks
                // open inline. Each item shows one binding (see
                // `shown_keys`), the way menus show one.
                menu(
                    "Edit",
                    vec![
                        leaf("Undo", "undo").shown("C-x u"),
                        leaf("Redo", "undo-redo").shown("C-M-_"),
                        sep(),
                        leaf("Cut", "kill-region").shown("C-w"),
                        leaf("Copy", "kill-ring-save").shown("M-w"),
                        leaf("Paste", "yank").shown("C-y"),
                        leaf("Select and Paste", "select-and-paste"),
                        leaf("Clear", "delete-active-region"),
                        leaf("Select All", "mark-whole-buffer").shown("C-x h"),
                        sep(),
                        sub(
                            "Search",
                            vec![leaf("Search in Project Files...", "project-find-regexp")],
                        ),
                        sub(
                            "Incremental Search",
                            vec![
                                leaf("Forward String...", "isearch-forward").shown("C-s"),
                                leaf("Backward String...", "isearch-backward").shown("C-r"),
                            ],
                        ),
                        sub(
                            "Replace",
                            vec![
                                leaf("Replace String...", "query-replace").shown("M-%"),
                                leaf("Replace Regexp...", "query-replace-regexp").shown("C-M-%"),
                            ],
                        ),
                        sub(
                            "Go To",
                            vec![
                                leaf("Goto Line...", "goto-line").shown("M-g g"),
                                leaf("Goto Buffer Position...", "goto-char"),
                                leaf("Goto Beginning of Buffer", "beginning-of-buffer")
                                    .shown("M-<"),
                                leaf("Goto End of Buffer", "end-of-buffer").shown("M->"),
                                sep(),
                                leaf("Find Definition...", "lsp-find-definition"),
                                leaf("Find References", "lsp-references"),
                                leaf("Find Apropos...", "xref-find-apropos"),
                                leaf("Back", "xref-go-back"),
                                leaf("Forward", "xref-go-forward"),
                                sep(),
                                leaf("Next Error", "next-error"),
                                leaf("Previous Error", "previous-error"),
                                sep(),
                                leaf("Exchange Point and Mark", "exchange-point-and-mark"),
                                leaf("Occur...", "occur"),
                                leaf("Imenu", "imenu"),
                            ],
                        ),
                        sub(
                            "Bookmarks",
                            vec![
                                leaf("Set Bookmark", "bookmark-set").shown("C-x r m"),
                                leaf("Jump to Bookmark", "bookmark-jump").shown("C-x r b"),
                                leaf("Delete Bookmark", "bookmark-delete"),
                                leaf("Edit Bookmark List", "list-bookmarks"),
                            ],
                        ),
                        sep(),
                        sub(
                            "Text Editing",
                            vec![
                                leaf("Fill Paragraph", "fill-paragraph").shown("M-q"),
                                leaf("Fill Region", "fill-region"),
                                leaf("Unfill Paragraph", "unfill-paragraph"),
                                leaf("Center Line", "center-line"),
                                sep(),
                                leaf("Upcase Word", "upcase-word").shown("M-u"),
                                leaf("Downcase Word", "downcase-word").shown("M-l"),
                                leaf("Capitalize Word", "capitalize-word").shown("M-c"),
                                sep(),
                                leaf("Transpose Characters", "transpose-chars").shown("C-t"),
                                leaf("Transpose Words", "transpose-words").shown("M-t"),
                                leaf("Transpose Lines", "transpose-lines").shown("C-x C-t"),
                                leaf("Join Line", "delete-indentation").shown("M-^"),
                                leaf("Delete Trailing Whitespace", "delete-trailing-whitespace"),
                                leaf("Comment/Uncomment", "comment-dwim").shown("M-;"),
                                leaf("Indent Region", "indent-region").shown("C-M-\\"),
                            ],
                        ),
                        sub(
                            "Rectangles",
                            vec![
                                leaf("Kill Rectangle", "kill-rectangle").shown("C-x r k"),
                                leaf("Copy Rectangle", "copy-rectangle-as-kill").shown("M-w"),
                                leaf("Yank Rectangle", "yank-rectangle").shown("C-x r y"),
                                leaf("Delete Rectangle", "delete-rectangle").shown("C-x r d"),
                                leaf("Open Rectangle", "open-rectangle").shown("C-x r o"),
                                leaf("Clear Rectangle", "clear-rectangle").shown("C-x r c"),
                                leaf("Replace Rectangle...", "string-rectangle").shown("C-x r t"),
                                leaf("Number Lines", "rectangle-number-lines"),
                            ],
                        ),
                        sub(
                            "Registers",
                            vec![
                                leaf("Copy to Register...", "copy-to-register").shown("C-x r s"),
                                leaf("Insert Register...", "insert-register").shown("C-x r i"),
                                leaf("Save Point to Register...", "point-to-register")
                                    .shown("C-x r SPC"),
                                leaf("Jump to Register...", "jump-to-register").shown("C-x r j"),
                                leaf("List Registers", "list-registers"),
                            ],
                        ),
                        leaf("Fill", "fill-paragraph").shown("M-q"),
                        leaf("Execute Command", "execute-extended-command").shown("M-x"),
                    ],
                ),
                // Options menu (`menu-bar-options-menu`). View's
                // toggles are Show/Hide, and the package and plugin
                // managers sit where Forge puts "Manage Forge Packages".
                menu(
                    "Options",
                    vec![
                        sub(
                            "Line Wrapping in This Buffer",
                            vec![
                                leaf("Truncate Long Lines", "toggle-truncate-lines"),
                                leaf("Word Wrap (Visual Line mode)", "visual-line-mode"),
                            ],
                        ),
                        sep(),
                        sub(
                            "Show/Hide",
                            vec![
                                leaf("Context Menus", "context-menu-mode"),
                                leaf("Menu Bar", "menu-bar-mode"),
                                leaf("Tool Bar", "tool-bar-mode"),
                                leaf("Tab Bar", "tab-bar-mode"),
                                leaf("Tooltips", "tooltip-mode"),
                                leaf("Scroll Bar", "scroll-bar-mode"),
                                leaf("Horizontal Scroll Bar", "horizontal-scroll-bar-mode"),
                                leaf("Line Numbers for All Lines", "display-line-numbers-mode"),
                                leaf(
                                    "Fill Column Indicator",
                                    "display-fill-column-indicator-mode",
                                ),
                                leaf("Highlight Current Line", "hl-line-mode"),
                                leaf("Matching Parens", "show-paren-mode"),
                                leaf("Trailing Whitespace", "whitespace-mode"),
                                leaf("Line Number in Mode Line", "line-number-mode"),
                                leaf("Column Number in Mode Line", "column-number-mode"),
                                leaf("Size in Mode Line", "size-indication-mode"),
                                leaf("Modifier Bar", "modifier-bar-mode"),
                            ],
                        ),
                        leaf("Auto Fill in This Buffer", "auto-fill-mode"),
                        sub(
                            "Multilingual Environment",
                            vec![
                                leaf("Select Input Method...", "set-input-method"),
                                leaf("Describe Input Method...", "describe-input-method"),
                                leaf("List Input Methods", "list-input-methods"),
                            ],
                        ),
                        sep(),
                        sub(
                            "Text Size",
                            vec![
                                leaf("Increase", "text-scale-increase").shown("C-x C-+"),
                                leaf("Decrease", "text-scale-decrease").shown("C-x C--"),
                                leaf("Reset", "text-scale-reset").shown("C-x C-0"),
                            ],
                        ),
                        sub(
                            "Icons",
                            vec![
                                leaf("Customize Icon...", "customize-icon"),
                                leaf("Describe Icon...", "describe-icon"),
                                leaf("List Icons", "icon-list"),
                                leaf("Icon Platform...", "icon-set-platform"),
                            ],
                        ),
                        leaf("Customize Themes", "list-themes"),
                        leaf("Reload init.rhai", "reload-init-file"),
                        sep(),
                        sub(
                            "Manage Packages",
                            vec![
                                leaf("List Packages", "list-packages"),
                                leaf("Install Package", "package-install"),
                                leaf("Delete Package", "package-delete"),
                                leaf("Refresh Contents", "package-refresh-contents"),
                                leaf("Upgrade All", "package-vc-upgrade-all"),
                            ],
                        ),
                        sub(
                            "Manage Plugins",
                            vec![
                                leaf("List Plugins", "list-plugins"),
                                leaf("Load Plugin", "load-plugin"),
                                leaf("Reload Plugin", "reload-plugin"),
                                leaf("Unload Plugin", "unload-plugin"),
                                leaf("Revoke Permissions", "revoke-plugin-permissions"),
                            ],
                        ),
                    ],
                ),
                // Forge lists the open buffers here, then these. A buffer
                // list changes while the menu is open and an item runs a
                // command with no argument, so Forge has Select Named Buffer
                // instead.
                menu(
                    "Buffers",
                    vec![
                        leaf("Next Buffer", "next-buffer").shown("C-x <right>"),
                        leaf("Previous Buffer", "previous-buffer").shown("C-x <left>"),
                        leaf("Select Named Buffer...", "switch-to-buffer").shown("C-x b"),
                        leaf(
                            "Select Named Buffer in Other Window...",
                            "switch-to-buffer-other-window",
                        ),
                        leaf("List All Buffers", "list-buffers").shown("C-x C-b"),
                        sep(),
                        leaf("Kill Buffer...", "kill-buffer").shown("C-x k"),
                        leaf("Recover Auto-Save", "recover-this-file"),
                        sep(),
                        sub(
                            "Window",
                            vec![
                                leaf("Other Window", "other-window").shown("C-x o"),
                                leaf("Balance Windows", "balance-windows"),
                                leaf("Enlarge Window", "enlarge-window"),
                                leaf("Shrink Window", "shrink-window"),
                                leaf("Recenter", "recenter-top-bottom"),
                            ],
                        ),
                    ],
                ),
                // Tools menu, plus Forge's own tools (compilation
                // results, macros, and the two browsers).
                menu(
                    "Tools",
                    vec![
                        leaf("Search Files (Grep)...", "grep"),
                        leaf("Recursive Grep...", "rgrep"),
                        sub(
                            "Shell Commands",
                            vec![
                                leaf("Shell Command...", "shell-command"),
                                leaf("Async Shell Command...", "async-shell-command"),
                            ],
                        ),
                        leaf("Compile...", "compile"),
                        sub(
                            "Compilation",
                            vec![
                                leaf("Recompile", "recompile"),
                                leaf("Next Error", "compilation-next-error"),
                                leaf("Previous Error", "compilation-previous-error"),
                                leaf("Go to Error", "compile-goto-error"),
                                leaf("Kill Compilation", "kill-compilation"),
                            ],
                        ),
                        sub(
                            "Project",
                            vec![
                                leaf("Open File...", "project-find-file"),
                                leaf("Compile...", "project-compile"),
                                leaf("Find Regexp...", "project-find-regexp"),
                            ],
                        ),
                        sub(
                            "Version Control",
                            vec![
                                leaf("Show Status (VC Directory)", "vc-dir"),
                                leaf("Next Action (Check In/Out)", "vc-next-action"),
                                leaf("Register", "vc-register"),
                                sep(),
                                leaf("Compare with Base Version", "vc-diff"),
                                leaf("Compare Whole Project", "vc-root-diff"),
                                leaf("Show History", "vc-print-log"),
                                leaf("Show Project History", "vc-print-root-log"),
                                leaf("Show Incoming Changes", "vc-log-incoming"),
                                leaf("Show Outgoing Changes", "vc-log-outgoing"),
                                leaf("Annotate", "vc-annotate"),
                                leaf("Show Region History", "vc-region-history"),
                                sep(),
                                leaf("Revert File", "vc-revert"),
                                leaf("Pull", "vc-pull"),
                                leaf("Push", "vc-push"),
                                leaf("Update", "vc-update"),
                                leaf("Merge Branch...", "vc-merge"),
                                sep(),
                                leaf("Create Branch...", "vc-create-branch"),
                                leaf("Switch Branch...", "vc-switch-branch"),
                                leaf("List Branches", "vc-list-branches"),
                                leaf("Create Tag...", "vc-create-tag"),
                                leaf("Retrieve Tag...", "vc-retrieve-tag"),
                                sep(),
                                leaf("Ignore File", "vc-ignore"),
                                leaf("Rename File...", "vc-rename-file"),
                                leaf("Delete File", "vc-delete-file"),
                                leaf("Refresh State", "vc-refresh-state"),
                            ],
                        ),
                        sub(
                            "Spell Checking",
                            vec![
                                leaf("Spell-Check Buffer", "spell-check-buffer"),
                                leaf("Correct Word", "spell-correct-word"),
                                leaf("Continuous Spell Checking", "spell-check-mode"),
                            ],
                        ),
                        sep(),
                        leaf("Complete at Point", "completion-at-point"),
                        leaf("Start Macro", "start-kbd-macro"),
                        leaf("Call Last Macro", "kmacro-end-and-call-macro"),
                        leaf("List Registers", "list-registers"),
                        leaf("Count Words", "count-words"),
                        sep(),
                        leaf("Terminal (term)", "term"),
                        leaf("Emoji", "emoji-insert"),
                        leaf("Browse the Web (webkit)...", "webkit"),
                        leaf("Text Browser (few)...", "few"),
                    ],
                ),
                // Help menu keeps the Describe commands in a submenu.
                menu(
                    "Help",
                    vec![
                        sub(
                            "Search Documentation",
                            vec![
                                leaf("Find Commands by Name...", "apropos-command").shown("C-h a"),
                                leaf("Search Documentation Strings...", "apropos-documentation")
                                    .shown("C-h d"),
                            ],
                        ),
                        sub(
                            "Describe",
                            vec![
                                leaf("Describe Buffer Modes", "describe-mode").shown("C-h m"),
                                leaf("Describe Key or Mouse Operation...", "describe-key")
                                    .shown("C-h k"),
                                leaf("Describe Command...", "describe-command"),
                                leaf("Describe Function...", "describe-function").shown("C-h f"),
                                leaf("Describe Variable...", "describe-variable").shown("C-h v"),
                                leaf("Describe Face...", "describe-face"),
                                leaf("Describe Symbol...", "describe-symbol").shown("C-h o"),
                                leaf("Where Is...", "where-is").shown("C-h w"),
                                leaf("Show Recent Inputs", "view-lossage").shown("C-h l"),
                                leaf("Describe Key Briefly...", "describe-key-briefly"),
                                leaf("Describe Plugin...", "describe-plugin"),
                                leaf("Describe Icon...", "describe-icon"),
                                leaf("View Echo Area Messages", "view-echo-area-messages")
                                    .shown("C-h e"),
                                leaf("What Cursor Position", "what-cursor-position"),
                                leaf("List Key Bindings", "describe-bindings").shown("C-h b"),
                            ],
                        ),
                        sep(),
                        leaf("Tutorial", "help-with-tutorial").shown("C-h t"),
                        leaf("Read the Manual", "info").shown("C-h i"),
                        leaf("Describe Input Method...", "describe-input-method").shown("C-h I"),
                        leaf("Describe Character...", "describe-char"),
                        leaf("What's New", "view-news").shown("C-h n"),
                        sep(),
                        leaf("Help for Help", "help-for-help").shown("C-h ?"),
                    ],
                ),
            ],
        };
        // Mode menus sit between Tools and Help, as do.
        let help = registry.menus.len() - 1;
        registry
            .menus
            .splice(help..help, crate::mode_menus::menus());
        for m in &mut registry.menus {
            apply_conditions(&mut m.items);
        }
        registry
    }
}

/// Give the items of the mode menus the same conditions the core menus
/// carry (greyed out without a region, ticked while a mode is on).
fn apply_conditions(items: &mut [Item]) {
    for it in items {
        if it.has_submenu() {
            apply_conditions(&mut it.submenu);
        } else if !it.is_separator() && it.enabled == When::Always && it.checked == When::Never {
            let b = builtin_item(&it.label, &it.command);
            it.enabled = b.enabled;
            it.checked = b.checked;
        }
    }
}

/// One of the built-in menu's items, with the conditions it carries.
///
/// Only the commands whose answer changes with the editor get one: Save
/// and Revert are greyed out while there's nothing to save, the region
/// commands while no region is selected, and the View menu's items show
/// which of them are on. The rest are always there.
fn builtin_item(label: &str, command: &str) -> Item {
    let item = Item::new(label, command);
    let modified = When::BufferModified;
    let region = When::Region;
    match command {
        // Reverting throws unsaved changes away, so it's for a buffer
        // that has them; saving is that plus a buffer that can be written.
        "save-buffer" => item.enabled(When::All(vec![modified, When::Writable])),
        "revert-buffer" => item.enabled(modified),
        "kill-region"
        | "copy-to-register"
        | "string-rectangle"
        | "indent-region"
        | "kill-ring-save"
        | "delete-region"
        | "delete-active-region" => item.enabled(region),
        "menu-bar-mode" => item.checked(When::Flag("menu-bar-mode".into())),
        "context-menu-mode" => item.checked(When::Flag("context-menu-mode".into())),
        "visual-line-mode" => item.checked(When::BufferOption("wrap".into(), "word".into())),
        "display-line-numbers-mode" => item.checked(When::Any(vec![
            When::BufferOption("line-numbers".into(), "absolute".into()),
            When::BufferOption("line-numbers".into(), "relative".into()),
        ])),
        "display-fill-column-indicator-mode" => {
            item.checked(When::BufferFlag("display-fill-column-indicator".into()))
        }
        "hl-line-mode" => item.checked(When::BufferFlag("hl-line".into())),
        "whitespace-mode" => item.checked(When::BufferFlag("whitespace".into())),
        "show-paren-mode" => item.checked(When::Flag("show-paren".into())),
        "tool-bar-mode" => item.checked(When::Flag("tool-bar-mode".into())),
        "tab-bar-mode" => item.checked(When::Flag("tab-bar-mode".into())),
        "tooltip-mode" => item.checked(When::Flag("tooltip-mode".into())),
        "scroll-bar-mode" => item.checked(When::Flag("scroll-bar-mode".into())),
        "horizontal-scroll-bar-mode" => {
            item.checked(When::Flag("horizontal-scroll-bar-mode".into()))
        }
        "auto-fill-mode" => item.checked(When::MinorMode("auto-fill-mode".into())),
        "spell-check-mode" => item.checked(When::MinorMode("spell-check-mode".into())),
        "toggle-truncate-lines" => item.checked(When::BufferOption("wrap".into(), "off".into())),
        _ => item,
    }
}

/// Looks up the keys bound to a command, shortest first. Empty for one
/// with no binding.
pub type KeysFor<'a> = Box<dyn Fn(&str) -> Vec<String> + 'a>;

/// What a menu item needs to know about its command: whether it exists,
/// what keys run it, and what the editor looks like right now for the
/// item's [`When`] to ask about.
///
/// One of these is built by each frontend from the editor, so the terminal
/// row and the macOS menu bar say the same things about the same command.
pub struct Context<'a> {
    /// Is the command defined (a plugin unloaded, say)?
    pub exists: Box<dyn Fn(&str) -> bool + 'a>,
    /// The keys bound to it.
    pub keys: KeysFor<'a>,
    /// The editor state this item's conditions are answered from.
    pub state: MenuState<'a>,
}

impl<'a> Context<'a> {
    /// A context where nothing is defined and nothing is bound: for tests,
    /// and for drawing before the editor's keymap is known.
    pub fn none() -> Self {
        Self {
            exists: Box::new(|_| false),
            keys: Box::new(|_| Vec::new()),
            state: MenuState::default(),
        }
    }

    /// A context where every command exists and nothing is bound.
    pub fn all_exist() -> Self {
        Self {
            exists: Box::new(|_| true),
            keys: Box::new(|_| Vec::new()),
            state: MenuState::default(),
        }
    }

    /// Can this item be chosen: its command is defined and its condition
    /// holds. Both frontends ask this the same way, so an item is greyed
    /// in one if and only if it is greyed in the other.
    pub fn enabled(&self, item: &Item) -> bool {
        !item.is_separator() && (self.exists)(&item.command) && item.enabled.holds(&self.state)
    }

    /// Should this item show a tick?
    pub fn checked(&self, item: &Item) -> bool {
        !item.is_separator() && item.checked.holds(&self.state)
    }
}

/// One row of the open menu's panel: its text (already padded to the
/// panel's width), its face, and the item it stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelRow {
    pub text: String,
    pub face: Face,
    /// The item's index in the menu, or None for a separator.
    pub item: Option<usize>,
}

/// The open menu's panel: one row per item, every row the same width.
///
/// menu panel is a solid block: a space either side of the label,
/// and the highlighted entry filled across the whole panel rather than
/// just its own text. So the rows are padded here, once, and a frontend
/// draws them as they come.
pub fn panel_rows(state: MenuBar, menus: &Registry, ctx: &Context) -> Vec<PanelRow> {
    let Some(menu) = state
        .open
        .and_then(|m| menus.menu(m))
        .filter(|m| m.visible(&ctx.state))
    else {
        return Vec::new();
    };
    item_rows(&menu.items, Some(state.item), ctx)
}

/// The open submenu's rows, if the highlighted item opens one.
pub fn subpanel_rows(state: MenuBar, menus: &Registry, ctx: &Context) -> Vec<PanelRow> {
    let Some(items) = state
        .open
        .and_then(|m| menus.menu(m))
        .filter(|menu| menu.visible(&ctx.state))
        .and_then(|menu| menu.items.get(state.item))
        .filter(|it| it.has_submenu())
        .map(|it| &it.submenu)
    else {
        return Vec::new();
    };
    // A submenu left open while its parent moved away belongs to the
    // wrong item: draw nothing rather than someone else's children.
    if state.sub.is_none() {
        return Vec::new();
    }
    item_rows(items, state.sub, ctx)
}

/// One row per item: what the item is called on the left, the keys that
/// run it against the right edge. An item with no binding has nothing in
/// the second column, and the label still lines up with the ones that
/// do. `highlight` is the row drawn selected, if any.
fn item_rows(items: &[Item], highlight: Option<usize>, ctx: &Context) -> Vec<PanelRow> {
    // Two columns, the way a menu is laid out.
    let cells: Vec<(String, String)> = items
        .iter()
        .map(|it| {
            if it.is_separator() {
                (String::new(), String::new())
            } else {
                // The tick in the left column, where a menu puts it, so
                // the labels of the ticked items and the rest still line
                // up.
                let label = if ctx.checked(it) {
                    format!("\u{2713} {}", it.label)
                } else {
                    format!("  {}", it.label)
                };
                // A submenu header shows where it leads, the way a
                // native menu's chevron does, rather than any keys.
                let keys = if it.has_submenu() {
                    ">".to_string()
                } else {
                    it.shown_keys
                        .clone()
                        .unwrap_or_else(|| (ctx.keys)(&it.command).join(", "))
                };
                (label, keys)
            }
        })
        .collect();
    let label_w = cells
        .iter()
        .map(|(l, _)| l.chars().count())
        .max()
        .unwrap_or(0);
    let keys_w = cells
        .iter()
        .map(|(_, k)| k.chars().count())
        .max()
        .unwrap_or(0);
    // A space each side, and two between the columns so the keys don't run
    // into a long label.
    let gap = if keys_w > 0 { 2 } else { 0 };
    let width = 1 + label_w + gap + keys_w + 1;
    items
        .iter()
        .zip(cells)
        .enumerate()
        .map(|(i, (it, (label, keys)))| {
            let text = if it.is_separator() {
                // A rule across the panel separates groups.
                format!(" {}", "\u{2500}".repeat(width - 1))
            } else {
                // Label out to the left column, keys flush against the
                // right, then the trailing space that every row has.
                let mut t = format!(" {label}{}", " ".repeat(label_w - label.chars().count()));
                if keys_w > 0 {
                    t.push_str(&" ".repeat(gap + keys_w - keys.chars().count()));
                    t.push_str(&keys);
                }
                t.push_str(&" ".repeat(width - t.chars().count()));
                t
            };
            let face = if it.is_separator() {
                Face::MenuBar
            } else {
                match (Some(i) == highlight, ctx.enabled(it)) {
                    (true, _) => Face::MenuSelected,
                    (false, true) => Face::MenuBar,
                    (false, false) => Face::MenuDisabled,
                }
            };
            PanelRow {
                text,
                face,
                item: (!it.is_separator()).then_some(i),
            }
        })
        .collect()
}

/// The panel's width in columns.
pub fn panel_width(rows: &[PanelRow]) -> u16 {
    rows.iter()
        .map(|r| r.text.chars().count() as u16)
        .max()
        .unwrap_or(0)
}

/// Which menu is open, which item in it, and where the pointer is.
///
/// `open` and `hover` are indexes into the [`Registry`]. While a menu is
/// open the item highlighted there is `item`; `sub` is the item
/// highlighted in that item's submenu, when one is open.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MenuBar {
    pub open: Option<usize>,
    pub item: usize,
    /// The title the pointer is over, if any. Highlighted so you can see
    /// which menu a click would open. With a menu open, moving onto
    /// another title switches to it, the way menus do.
    pub hover: Option<usize>,
    /// The open submenu's highlighted item: an index into the open
    /// item's `submenu`. None when no submenu is open.
    pub sub: Option<usize>,
}

impl MenuBar {
    /// Open the menu at `menu` (toggling it closed if it's already open).
    pub fn toggle(&mut self, menu: usize, menus: &Registry) {
        let visible: Vec<usize> = (0..menus.len()).collect();
        self.toggle_visible(menu, menus, &visible);
    }

    /// Open the menu at `menu` when it is in `visible` (toggling it closed
    /// if it's already open).
    pub fn toggle_visible(&mut self, menu: usize, menus: &Registry, visible: &[usize]) {
        if menu >= menus.len() || !visible.contains(&menu) {
            return;
        }
        self.open = if self.open == Some(menu) {
            None
        } else {
            Some(menu)
        };
        self.item = 0;
        self.hover = Some(menu);
        self.sub = None;
        self.skip_separators(1, menus);
    }

    /// Open the first menu, or the next one after the open one (right).
    pub fn right(&mut self, menus: &Registry) {
        let visible: Vec<usize> = (0..menus.len()).collect();
        self.right_visible(menus, &visible);
    }

    /// Open the next menu that is in `visible` (right).
    pub fn right_visible(&mut self, menus: &Registry, visible: &[usize]) {
        if visible.is_empty() {
            return;
        }
        let next = match self.open.and_then(|m| visible.iter().position(|&v| v == m)) {
            Some(p) => visible[(p + 1) % visible.len()],
            None => visible[0],
        };
        self.open = Some(next);
        self.hover = Some(next);
        self.item = 0;
        self.sub = None;
        self.skip_separators(1, menus);
    }

    /// The previous menu (left).
    pub fn left(&mut self, menus: &Registry) {
        let visible: Vec<usize> = (0..menus.len()).collect();
        self.left_visible(menus, &visible);
    }

    /// The previous menu that is in `visible` (left).
    pub fn left_visible(&mut self, menus: &Registry, visible: &[usize]) {
        if visible.is_empty() {
            return;
        }
        let next = match self.open.and_then(|m| visible.iter().position(|&v| v == m)) {
            Some(0) | None => *visible.last().expect("visible is not empty"),
            Some(p) => visible[p - 1],
        };
        self.open = Some(next);
        self.hover = Some(next);
        self.item = 0;
        self.sub = None;
        self.skip_separators(1, menus);
    }

    pub fn close(&mut self) {
        self.open = None;
        self.item = 0;
        self.hover = None;
        self.sub = None;
    }

    /// Is this menu open?
    pub fn is_open(&self, menu: usize) -> bool {
        self.open == Some(menu)
    }

    /// The open item's submenu, if it opens one.
    fn open_submenu_items<'m>(&self, menus: &'m Registry) -> Option<&'m [Item]> {
        let menu = menus.menu(self.open?)?;
        let item = menu.items.get(self.item)?;
        (!item.submenu.is_empty()).then_some(&item.submenu)
    }

    /// Is the highlight on an item that opens a submenu?
    pub fn on_parent(&self, menus: &Registry) -> bool {
        self.open_submenu_items(menus).is_some()
    }

    /// Is a submenu open?
    pub fn sub_open(&self) -> bool {
        self.sub.is_some()
    }

    /// Open the highlighted item's submenu, if it has one. False for a
    /// plain item, so the caller can run it instead.
    pub fn open_submenu(&mut self, menus: &Registry) -> bool {
        let Some(items) = self.open_submenu_items(menus) else {
            return false;
        };
        let mut sub = 0;
        skip_in(items, &mut sub, 1);
        self.sub = Some(sub);
        true
    }

    /// Close the open submenu, if any, staying in its menu.
    pub fn close_submenu(&mut self) -> bool {
        self.sub.take().is_some()
    }

    /// Move the highlight within the open submenu.
    pub fn move_sub(&mut self, by: isize, menus: &Registry) {
        if self.sub.is_none() {
            return;
        }
        let Some(items) = self.open_submenu_items(menus) else {
            self.sub = None;
            return;
        };
        let mut sub = self.sub.unwrap_or(0);
        step_in(items, &mut sub, by);
        self.sub = Some(sub);
    }

    /// Highlight the first runnable item in the open submenu.
    pub fn first_sub(&mut self, menus: &Registry) {
        if let Some(items) = self.open_submenu_items(menus) {
            let mut sub = 0;
            skip_in(items, &mut sub, 1);
            self.sub = Some(sub);
        }
    }

    /// Highlight the last runnable item in the open submenu.
    pub fn last_sub(&mut self, menus: &Registry) {
        if let Some(items) = self.open_submenu_items(menus) {
            let mut sub = items.len().saturating_sub(1);
            skip_in(items, &mut sub, -1);
            self.sub = Some(sub);
        }
    }

    /// Highlight the submenu row at `row` (None for a separator, so the
    /// highlight stays where it was).
    pub fn hover_sub(&mut self, row: usize, menus: &Registry) {
        let Some(items) = self.open_submenu_items(menus) else {
            return;
        };
        if items.get(row).is_some_and(|i| !i.is_separator()) {
            self.sub = Some(row);
        }
    }

    /// The submenu item under the highlight, if it's runnable.
    pub fn sub_selected<'m>(&self, menus: &'m Registry) -> Option<&'m Item> {
        let items = self.open_submenu_items(menus)?;
        items.get(self.sub?).filter(|i| !i.is_separator())
    }

    /// The pointer moved: onto `menu`'s title, or off the bar (None).
    ///
    /// A menu that is open stays open wherever the pointer goes -- as in
    /// tmenu, sweeping past it to read the text behind it must not
    /// dismiss it. It closes on a click away, or from the keys.
    pub fn hover(&mut self, menu: Option<usize>, menus: &Registry) {
        let visible: Vec<usize> = (0..menus.len()).collect();
        self.hover_visible(menu, menus, &visible);
    }

    /// The pointer moved, considering only the menus in `visible`.
    pub fn hover_visible(&mut self, menu: Option<usize>, menus: &Registry, visible: &[usize]) {
        let menu = menu.filter(|m| visible.contains(m));
        match (self.open, menu) {
            (None, m) => self.hover = m,
            // A menu is open: following the pointer onto another title
            // switches to it.
            (Some(_), Some(m)) if Some(m) != self.open => {
                self.open = Some(m);
                self.item = 0;
                self.hover = Some(m);
                self.skip_separators(1, menus);
            }
            (Some(_), m) => self.hover = m,
        }
    }

    /// Highlight the item at `row` of the open panel (None for a
    /// separator, so the highlight stays where it was). A submenu left
    /// open follows the highlight: it shows the new item's children, or
    /// closes when the item opens none.
    pub fn hover_item(&mut self, row: usize, menus: &Registry) {
        let Some(menu) = self.open.and_then(|m| menus.menu(m)) else {
            return;
        };
        if menu.items.get(row).is_some_and(|i| !i.is_separator()) {
            self.item = row;
            if self.sub.is_some() {
                if menu.items[row].has_submenu() {
                    let mut sub = 0;
                    skip_in(&menu.items[row].submenu, &mut sub, 1);
                    self.sub = Some(sub);
                } else {
                    self.sub = None;
                }
            }
        }
    }

    /// Is this title under the pointer?
    pub fn is_hovered(&self, menu: usize) -> bool {
        self.hover == Some(menu)
    }

    /// Move the highlight, skipping separators and stopping at the ends
    /// (`by` is -1 or 1).
    pub fn move_item(&mut self, by: isize, menus: &Registry) {
        self.step(by, menus);
    }

    /// Highlight the first runnable item in the open menu.
    pub fn first_item(&mut self, menus: &Registry) {
        self.item = 0;
        self.skip_separators(1, menus);
    }

    /// Highlight the last runnable item in the open menu.
    pub fn last_item(&mut self, menus: &Registry) {
        self.item = self
            .open
            .and_then(|m| menus.menu(m))
            .map_or(0, |m| m.items.len().saturating_sub(1));
        self.skip_separators(-1, menus);
    }

    /// Move off a separator in one direction, for the first/last keys.
    fn skip_separators(&mut self, by: isize, menus: &Registry) {
        let Some(menu) = self.open.and_then(|m| menus.menu(m)) else {
            return;
        };
        if menu.items.is_empty() {
            return;
        }
        let mut i = self.item;
        skip_in(&menu.items, &mut i, by);
        self.item = i;
    }

    fn step(&mut self, by: isize, menus: &Registry) {
        let Some(menu) = self.open.and_then(|m| menus.menu(m)) else {
            return;
        };
        if menu.items.is_empty() {
            return;
        }
        let n = menu.items.len() as isize;
        let mut i = (self.item as isize).clamp(0, n - 1);
        for _ in 0..n {
            i = (i + by).clamp(0, n - 1);
            if !menu.items[i as usize].is_separator() {
                break;
            }
        }
        self.item = i as usize;
    }

    /// The item under the highlight, if it's runnable: a separator or
    /// a submenu header runs nothing itself.
    pub fn selected<'m>(&self, menus: &'m Registry) -> Option<&'m Item> {
        let menu = menus.menu(self.open?)?;
        menu.items
            .get(self.item)
            .filter(|i| !i.is_separator() && !i.has_submenu())
    }
}

/// Move `i` off a separator in one direction, stopping at the ends.
/// For the first/last keys, which land on whatever is already there.
fn skip_in(items: &[Item], i: &mut usize, by: isize) {
    if items.is_empty() {
        return;
    }
    let n = items.len() as isize;
    let mut j = (*i as isize).clamp(0, n - 1);
    for _ in 0..n {
        if !items[j as usize].is_separator() {
            break;
        }
        j = (j + by).clamp(0, n - 1);
    }
    *i = j as usize;
}

/// Move `i` one step in one direction, skipping separators and stopping
/// at the ends. For the previous/next keys, which always move.
fn step_in(items: &[Item], i: &mut usize, by: isize) {
    if items.is_empty() {
        return;
    }
    let n = items.len() as isize;
    let mut j = (*i as isize).clamp(0, n - 1);
    for _ in 0..n {
        j = (j + by).clamp(0, n - 1);
        if !items[j as usize].is_separator() {
            break;
        }
    }
    *i = j as usize;
}

/// One title in the bar, and where it sits: the column it starts at and
/// how wide it is, so a click can find it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuTitle {
    pub title: String,
    pub menu: usize,
    pub column: u16,
    pub width: u16,
}

/// The open panel: its rows and where it sits: the column it starts at
/// and the row its first row is on, both relative to the frame (the bar's
/// own row never counts). A submenu sits one column past its parent
/// panel's right edge, on its parent item's row, the way native menus
/// cascade.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuPopup {
    pub column: u16,
    pub row: u16,
    /// The same rows [`panel_rows`] produced, padded and with each item's
    /// index, so a click can tell a separator from an entry.
    pub rows: Vec<PanelRow>,
}

/// Everything needed to draw the bar and, when one is open, its panel.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MenuBarView {
    /// The bar's row, as spans: a title per menu, padded to the width.
    pub row: Vec<Span>,
    /// Where each title is, for clicks.
    pub titles: Vec<MenuTitle>,
    /// The open menu's panel.
    pub popup: Option<MenuPopup>,
    /// The open submenu's panel, if the highlighted item opens one.
    pub subpopup: Option<MenuPopup>,
}

/// The bar's row and any open panel, for `state`, `frame` columns wide.
///
/// `ctx` says which commands exist and what keys run them, which is what
/// the labels and the faces come from.
pub fn view(state: MenuBar, menus: &Registry, frame: u16, ctx: &Context) -> MenuBarView {
    let mut row: Vec<Span> = Vec::new();
    let mut titles = Vec::new();
    for menu in menus.visible_indices(&ctx.state) {
        let m = &menus.menus()[menu];
        let label = format!(" {} ", m.title);
        let width = label.chars().count() as u16;
        let column = used(&row);
        // A frame too narrow for the whole bar: stop where it ends.
        if column.saturating_add(width) > frame {
            break;
        }
        titles.push(MenuTitle {
            title: m.title.clone(),
            menu,
            column,
            width,
        });
        row.push(Span {
            text: label,
            face: if state.is_open(menu) || state.is_hovered(menu) {
                Face::MenuSelected
            } else {
                Face::MenuBar
            },
        });
    }
    // The rest of the row is blank, so a frontend can paint it solid.
    let used = used(&row);
    if used < frame {
        row.push(Span {
            text: " ".repeat((frame - used) as usize),
            face: Face::MenuBar,
        });
    }
    let rows = panel_rows(state, menus, ctx);
    let popup = state
        .open
        .filter(|menu| titles.iter().any(|t| t.menu == *menu))
        .map(|menu| MenuPopup {
            column: titles
                .iter()
                .find(|t| t.menu == menu)
                .map_or(0, |t| t.column),
            row: 0,
            rows,
        });
    // A submenu cascades from its parent item: one column past the
    // parent panel's right edge, on the parent's own row.
    let subpopup = match (&popup, state.sub) {
        (Some(popup), Some(_)) => {
            let rows = subpanel_rows(state, menus, ctx);
            if rows.is_empty() {
                None
            } else {
                Some(MenuPopup {
                    column: popup.column + panel_width(&popup.rows) + 1,
                    row: state.item as u16,
                    rows,
                })
            }
        }
        _ => None,
    };
    MenuBarView {
        row,
        titles,
        popup,
        subpopup,
    }
}

/// How many columns a row of spans covers.
fn used(row: &[Span]) -> u16 {
    row.iter().map(|s| s.text.chars().count() as u16).sum()
}

/// The rows to draw for the open menu, as spans, so a frontend paints the
/// highlighted and disabled ones in the right face.
pub fn popup_rows(state: MenuBar, menus: &Registry, ctx: &Context) -> Vec<(String, Face)> {
    panel_rows(state, menus, ctx)
        .into_iter()
        .map(|r| (r.text, r.face))
        .collect()
}

/// Which menu, if any, a click at column `column` hit.
pub fn title_at(titles: &[MenuTitle], column: u16) -> Option<usize> {
    titles
        .iter()
        .find(|t| column >= t.column && column < t.column + t.width)
        .map(|t| t.menu)
}

/// The item a click at `row` of the panel hits, `row` relative to the
/// panel's first row. None for a separator or past the end.
pub fn item_at(popup: &MenuPopup, row: u16) -> Option<usize> {
    popup.rows.get(row as usize)?.item
}

/// The submenu item a click at `row` of the subpanel hits, `row`
/// relative to the subpanel's first row. None for a separator or past
/// the end.
pub fn subitem_at(subpopup: &MenuPopup, row: u16) -> Option<usize> {
    subpopup.rows.get(row as usize)?.item
}

/// Where each area of the frame is, without a tool bar or modifier bar.
///
/// This is the old four-argument layout, kept for callers that don't know
/// about the newer bars. New code should use [`frame_areas_with`].
pub fn frame_areas(area: Rect, menu: bool, tab_bar: bool, list_h: u16) -> FrameAreas {
    frame_areas_with(area, menu, false, false, tab_bar, list_h, false)
}

/// Where each area of the frame is.
///
/// The menu bar takes a row at the top when `menu` is true; the tool bar
/// takes one below it (or above the echo area when `tool_bar_bottom`),
/// the modifier bar one below that, and the tab bar one below that
///. The completion list and the echo
/// area take one row each at the bottom, as before. Both frontends call
/// this, so a bar can't come out one row tall in the terminal and two rows
/// tall on macOS.
pub fn frame_areas_with(
    area: Rect,
    menu: bool,
    tool_bar: bool,
    modifier_bar: bool,
    tab_bar: bool,
    list_h: u16,
    tool_bar_bottom: bool,
) -> FrameAreas {
    // Never promise more rows than the frame has: the echo area keeps its
    // last row even on a frame too short for the bars, so a one-row frame
    // drops them rather than laying areas out past its bottom.
    let room = area.height.saturating_sub(1);
    let menu_h = u16::from(menu).min(room);
    let tool_h = u16::from(tool_bar).min(room.saturating_sub(menu_h));
    let top_tool_h = if tool_bar_bottom { 0 } else { tool_h };
    let modifier_h =
        u16::from(modifier_bar).min(room.saturating_sub(menu_h.saturating_add(top_tool_h)));
    let tab_h = u16::from(tab_bar)
        .min(room.saturating_sub(menu_h.saturating_add(top_tool_h).saturating_add(modifier_h)));
    let top = menu_h
        .saturating_add(top_tool_h)
        .saturating_add(modifier_h)
        .saturating_add(tab_h);
    let bottom_tool_h = if tool_bar_bottom { tool_h } else { 0 };
    // Leave a text row and the echo area, whatever the list asked for.
    let list_h = list_h.min(
        area.height
            .saturating_sub(top.saturating_add(2).saturating_add(bottom_tool_h)),
    );
    let windows_h = area.height.saturating_sub(
        top.saturating_add(list_h)
            .saturating_add(bottom_tool_h)
            .saturating_add(1),
    );
    let modifier_y = area.y + menu_h + top_tool_h;
    let tab_y = modifier_y + modifier_h;
    let list_y = area.y + top + windows_h;
    let bottom_tool_y = list_y + list_h;
    FrameAreas {
        menu: Rect {
            y: area.y,
            height: menu_h,
            ..area
        },
        tool_bar: Rect {
            y: if tool_bar_bottom {
                bottom_tool_y
            } else {
                area.y + menu_h
            },
            height: tool_h,
            ..area
        },
        modifier_bar: Rect {
            y: modifier_y,
            height: modifier_h,
            ..area
        },
        tab_bar: Rect {
            y: tab_y,
            height: tab_h,
            ..area
        },
        windows: Rect {
            y: area.y + top,
            height: windows_h,
            ..area
        },
        list: Rect {
            y: list_y,
            height: list_h,
            ..area
        },
        echo: Rect {
            y: bottom_tool_y + bottom_tool_h,
            height: area.height.min(1),
            ..area
        },
    }
}

/// The frame's areas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameAreas {
    /// The menu bar's row (height 0 when it isn't on).
    pub menu: Rect,
    /// The tool bar's row (height 0 when it isn't on).
    pub tool_bar: Rect,
    /// The modifier bar's row (height 0 when it isn't on).
    pub modifier_bar: Rect,
    /// The tab bar's row (height 0 when it isn't on), just below the menu
    /// bar and above the windows.
    pub tab_bar: Rect,
    pub windows: Rect,
    pub list: Rect,
    pub echo: Rect,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The six menus every buffer shows; the mode menus (Dired, Org, …)
    /// have their own tests in `mode_menus`.
    fn menus() -> Registry {
        let mut r = Registry::builtin();
        r.menus.retain(|m| m.when == When::Always);
        r
    }

    /// A frame wide enough for every title plus room to spare.
    const WIDE: u16 = 60;

    #[test]
    fn the_builtin_menus_are_there_in_order() {
        let r = menus();
        let titles: Vec<&str> = r.menus().iter().map(|m| m.title.as_str()).collect();
        assert_eq!(
            titles,
            ["File", "Edit", "Options", "Buffers", "Tools", "Help"]
        );
        assert!(r.menus().iter().all(|m| m.owner.is_none()));
        // Every non-separator item names a command.
        for m in r.menus() {
            for i in &m.items {
                assert!(i.owner.is_none(), "a built-in item has no owner");
            }
        }
    }

    #[test]
    fn the_file_menu_offers_the_frame_commands() {
        let r = menus();
        let file = r.menu(r.index_of("File").unwrap()).unwrap();
        let labels: Vec<&str> = file.items.iter().map(|i| i.label.as_str()).collect();
        let commands: Vec<&str> = file.items.iter().map(|i| i.command.as_str()).collect();
        for (label, command) in [
            ("New Frame", "make-frame"),
            ("Delete Frame", "delete-frame"),
            ("Undelete Frame", "undelete-frame"),
        ] {
            assert!(labels.contains(&label), "no {label} in the File menu");
            assert!(commands.contains(&command), "no {command} in the File menu");
        }
    }

    #[test]
    fn a_plugin_defines_a_menu_after_the_builtins() {
        let mut r = menus();
        let before = r.len();
        r.define(
            "Meow",
            vec![Item::new("Motion", "meow-insert"), Item::separator()],
            "meow",
        )
        .unwrap();
        assert_eq!(r.len(), before + 1);
        let i = r.index_of("Meow").unwrap();
        assert_eq!(r.menus()[i].owner.as_deref(), Some("meow"));
        // Its items are owned too, so unloading takes them.
        assert_eq!(r.menus()[i].items[0].owner.as_deref(), Some("meow"));
        // No duplicate titles, even from the same plugin; but a plugin may
        // define more than one menu.
        assert!(r.define("Meow", vec![], "meow").is_err());
        assert!(r.define("Meow Two", vec![], "meow").is_ok());
        assert!(r.define("File", vec![], "other").is_err());
        assert!(r.define("", vec![], "other").is_err());
        // A title that's taken, even by a built-in.
        assert!(r.define("Tools", vec![], "other").is_err());
        assert!(r.define("Mine", vec![], "other").is_ok());
    }

    #[test]
    fn a_menu_can_wait_for_its_major_mode() {
        let mut r = menus();
        r.define_when(
            "Markdown",
            vec![Item::new("Bold", "markdown-bold")],
            "markdown-mode",
            When::MajorMode("markdown-mode".into()),
        )
        .unwrap();
        let markdown = r.index_of("Markdown").unwrap();

        let plain = MenuState::default();
        let plain_visible = r.visible_indices(&plain);
        assert!(!plain_visible.contains(&markdown));
        let ctx = Context {
            state: plain,
            ..Context::all_exist()
        };
        let v = view(MenuBar::default(), &r, WIDE, &ctx);
        assert!(v.titles.iter().all(|t| t.title != "Markdown"));

        // gfm-mode derives from markdown-mode, so it shows there too.
        let derived = MenuState {
            major_modes: vec![
                "gfm-mode".into(),
                "markdown-mode".into(),
                "text-mode".into(),
            ],
            ..Default::default()
        };
        assert!(r.visible_indices(&derived).contains(&markdown));

        // Moving right from the last built-in menu wraps to the first,
        // skipping the one that isn't showing.
        let mut bar = MenuBar {
            open: Some(*plain_visible.last().unwrap()),
            ..Default::default()
        };
        bar.right_visible(&r, &plain_visible);
        assert_eq!(bar.open, Some(plain_visible[0]));
        assert_ne!(bar.open, Some(markdown));

        // And with its mode on, it is reachable again.
        let visible = r.visible_indices(&derived);
        bar.open = Some(*visible.first().unwrap());
        bar.left_visible(&r, &visible);
        assert_eq!(bar.open, Some(markdown));
    }

    #[test]
    fn a_plugin_can_extend_a_builtin_menu_and_take_it_back() {
        let mut r = menus();
        let before = r.menus()[0].items.len();
        r.extend(
            "File",
            vec![Item::new("Revert All", "revert-buffer")],
            "mine",
        )
        .unwrap();
        assert_eq!(r.len(), 6, "extending doesn't add a menu");
        assert_eq!(r.menus()[0].items.len(), before + 1);
        assert!(r.extend("Nope", vec![Item::new("x", "y")], "mine").is_err());
        r.define("Mine", vec![], "mine").unwrap();
        assert!(r.extend("Mine", vec![Item::new("x", "y")], "mine").is_err());

        r.remove("File", "mine").unwrap();
        assert_eq!(r.menus()[0].items.len(), before, "its items went away");
        assert!(r.remove("Nope", "mine").is_err());
        // Removing its own menu takes the whole thing.
        r.remove("Mine", "mine").unwrap();
        assert_eq!(r.index_of("Mine"), None);
    }

    #[test]
    fn unloading_a_plugin_takes_its_menu_and_its_items() {
        let mut r = menus();
        r.define("Meow", vec![Item::new("Motion", "meow-insert")], "meow")
            .unwrap();
        r.extend(
            "File",
            vec![Item::new("Revert All", "revert-buffer")],
            "meow",
        )
        .unwrap();
        assert!(r.index_of("Meow").is_some());
        assert!(
            r.menus()[0]
                .items
                .iter()
                .any(|i| i.owner.as_deref() == Some("meow"))
        );

        r.remove_plugin("meow");
        assert_eq!(r.index_of("Meow"), None, "its menu went away");
        assert!(
            !r.menus()[0]
                .items
                .iter()
                .any(|i| i.owner.as_deref() == Some("meow")),
            "and its items in the built-ins"
        );
        assert_eq!(r.len(), 6, "the built-ins are untouched");
        // Another plugin's menu is left alone.
        r.define("Other", vec![], "other").unwrap();
        r.remove_plugin("meow");
        assert!(r.index_of("Other").is_some());
    }

    #[test]
    fn a_plugin_can_extend_a_submenu_and_take_it_back() {
        let mut r = menus();
        // A missing label becomes a header the plugin owns.
        r.extend_submenu(
            "Edit",
            "Spell",
            vec![Item::new("Check", "spell-check")],
            "spell",
        )
        .unwrap();
        let edit = r.index_of("Edit").expect("Edit is in the bar");
        let spell = r
            .menu(edit)
            .unwrap()
            .items
            .iter()
            .find(|i| i.label == "Spell")
            .expect("Spell is in Edit");
        assert!(spell.has_submenu());
        assert_eq!(spell.owner.as_deref(), Some("spell"));
        assert_eq!(spell.submenu.len(), 1);
        // Appending to an existing submenu works, whoever owns it...
        r.extend_submenu(
            "Edit",
            "Incremental Search",
            vec![Item::new("Repeat", "isearch-repeat")],
            "spell",
        )
        .unwrap();
        let search = r
            .menu(edit)
            .unwrap()
            .items
            .iter()
            .find(|i| i.label == "Incremental Search")
            .unwrap();
        assert!(search.submenu.iter().any(|c| c.command == "isearch-repeat"));
        // ...but a plain item can't become one: opening it would hide
        // the command it runs.
        assert!(
            r.extend_submenu("Edit", "Undo", vec![Item::new("X", "y")], "spell")
                .is_err()
        );
        assert!(
            r.extend_submenu("Missing", "Spell", vec![], "spell")
                .is_err()
        );
        assert!(r.extend_submenu("Edit", "", vec![], "spell").is_err());
        // Unloading takes the items back; the header it made goes too,
        // while the built-in one keeps its own children.
        r.remove_plugin("spell");
        let edit_menu = &r.menu(edit).unwrap().items;
        assert!(
            edit_menu.iter().all(|i| i.label != "Spell"),
            "empty header went away"
        );
        let search = edit_menu
            .iter()
            .find(|i| i.label == "Incremental Search")
            .unwrap();
        assert!(search.submenu.iter().all(|c| c.command != "isearch-repeat"));
        assert_eq!(search.submenu.len(), 2, "its own children stayed");
        r.extend_submenu(
            "Edit",
            "Spell",
            vec![Item::new("Check", "spell-check")],
            "spell",
        )
        .unwrap();
        r.remove("Edit", "spell").unwrap();
        assert!(
            r.menu(edit)
                .unwrap()
                .items
                .iter()
                .all(|i| i.label != "Spell")
        );
    }

    #[test]
    fn a_plugin_menu_shows_in_the_bar() {
        let mut r = menus();
        r.define("Meow", vec![Item::new("Motion", "meow-insert")], "meow")
            .unwrap();
        let v = view(MenuBar::default(), &r, WIDE, &Context::all_exist());
        assert_eq!(v.titles.last().unwrap().title, "Meow");
        let text: String = v.row.iter().map(|s| s.text.as_str()).collect();
        assert!(text.contains(" Meow "), "{text:?}");
    }

    #[test]
    fn the_bar_has_one_span_per_menu_and_fills_the_row() {
        let r = menus();
        let v = view(MenuBar::default(), &r, WIDE, &Context::all_exist());
        assert_eq!(v.titles.len(), r.len(), "every menu has a title");
        assert_eq!(v.row.len(), r.len() + 1, "and a blank tail");
        let text: String = v.row.iter().map(|s| s.text.as_str()).collect();
        assert!(
            text.starts_with(" File  Edit  Options  Buffers "),
            "{text:?}"
        );
        assert_eq!(text.chars().count(), WIDE as usize);
        for t in &v.titles {
            assert_eq!(title_at(&v.titles, t.column), Some(t.menu));
            assert_eq!(title_at(&v.titles, t.column + t.width - 1), Some(t.menu));
            assert_eq!(
                &text[t.column as usize..(t.column + t.width) as usize],
                format!(" {} ", t.title)
            );
        }
        assert_eq!(title_at(&v.titles, WIDE - 1), None, "past the last title");
    }

    #[test]
    fn a_narrow_bar_shows_only_the_titles_that_fit() {
        let r = menus();
        let v = view(MenuBar::default(), &r, 3, &Context::all_exist());
        let text: String = v.row.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(text.chars().count(), 3, "still exactly one row");
        assert!(v.titles.is_empty(), "nothing to click");

        // File (6) and Edit (6) fit in 20 columns; Buffers (9) doesn't.
        let v = view(MenuBar::default(), &r, 20, &Context::all_exist());
        let text: String = v.row.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(text.chars().count(), 20);
        assert_eq!(
            v.titles
                .iter()
                .map(|t| t.title.as_str())
                .collect::<Vec<_>>(),
            ["File", "Edit"]
        );
    }

    #[test]
    fn opening_a_menu_draws_its_panel_under_its_title() {
        let r = menus();
        let mut state = MenuBar::default();
        state.toggle(1, &r);
        let v = view(state, &r, WIDE, &Context::all_exist());
        assert_eq!(v.popup.as_ref().map(|p| p.column), Some(v.titles[1].column));
        let rows = popup_rows(state, &r, &Context::all_exist());
        assert_eq!(rows.len(), r.menu(1).unwrap().items.len());
        assert_eq!(
            rows[0].1,
            Face::MenuSelected,
            "the first item is highlighted"
        );
        assert!(v.row[1].face == Face::MenuSelected, "and so is its title");
        state.toggle(1, &r);
        assert!(view(state, &r, WIDE, &Context::all_exist()).popup.is_none());
    }

    #[test]
    fn every_row_is_the_same_width_so_the_panel_is_a_block() {
        let r = menus();
        let mut state = MenuBar {
            open: Some(1),
            item: 3,
            hover: None,
            sub: None,
        };
        state.move_item(1, &r);
        let rows = panel_rows(state, &r, &Context::all_exist());
        let width = panel_width(&rows);
        assert!(
            rows.iter().all(|x| x.text.chars().count() as u16 == width),
            "every row is {width} wide: {:?}",
            rows.iter().map(|x| &x.text).collect::<Vec<_>>()
        );
        // A space either side of the label, so it isn't flush to the edge
        // and the highlight has something to fill. Separators are the rule.
        let items: Vec<&PanelRow> = rows.iter().filter(|x| x.item.is_some()).collect();
        assert!(items.iter().all(|x| x.text.starts_with(' ')));
        assert!(items.iter().all(|x| x.text.ends_with(' ')));
        let rules: Vec<&PanelRow> = rows.iter().filter(|x| x.item.is_none()).collect();
        assert!(
            !rules.is_empty()
                && rules
                    .iter()
                    .all(|x| x.text.trim().chars().all(|c| c == '\u{2500}'))
        );
        // The keys come before the label, in the same shape vertico's
        // marginalia uses.
        let keys = Context {
            exists: Box::new(|_| true),
            keys: Box::new(|c: &str| {
                if c == "yank" {
                    vec!["C-y".into()]
                } else {
                    Vec::new()
                }
            }),
            state: MenuState::default(),
        };
        let rows = panel_rows(state, &r, &keys);
        let width = panel_width(&rows);

        // Every row the same width again, now that the keys are a column.
        assert!(rows.iter().all(|x| x.text.chars().count() as u16 == width));
        // The label is on the left, the keys flush against the right edge.
        // "Paste" is yank, which the context above binds to C-y.
        // The two-space gutter is where a tick goes, so every label
        // starts at the same column whether it's ticked or not.
        let paste = rows.iter().find(|x| x.text.contains("Paste")).unwrap();
        assert!(paste.text.starts_with("   Paste"), "{:?}", paste.text);
        assert!(paste.text.trim_end().ends_with("C-y"), "{:?}", paste.text);
        // A row without a binding has a blank right column, but still the
        // same width, so the panel stays a block.
        let undo = rows.iter().find(|x| x.text.contains("Undo")).unwrap();
        assert!(!undo.text.trim_end().ends_with("C-y"), "{:?}", undo.text);
        assert_eq!(undo.text.chars().count() as u16, width);
        // And the keys all line up at the same column, at the panel's edge.
        let keyed: Vec<&PanelRow> = rows.iter().filter(|r| r.text.contains("C-y")).collect();
        assert!(keyed.iter().all(|r| r.text.chars().count() as u16 == width));
    }

    #[test]
    fn the_highlight_skips_separators_and_stops_at_the_ends() {
        let r = menus();
        let menu = 1; // Edit
        let items = &r.menu(menu).unwrap().items;
        let sep = items
            .iter()
            .position(Item::is_separator)
            .expect("a separator");
        let mut state = MenuBar {
            open: Some(menu),
            item: sep,
            hover: None,
            sub: None,
        };
        assert!(state.selected(&r).is_none(), "a separator runs nothing");
        state.move_item(-1, &r);
        assert_eq!(state.item, sep - 1);
        state.item = sep;
        state.move_item(1, &r);
        assert_eq!(state.item, sep + 1);

        let mut edit = MenuBar {
            open: Some(menu),
            item: 0,
            hover: None,
            sub: None,
        };
        edit.move_item(-1, &r);
        assert_eq!(edit.item, 0, "stops at the top");
        let last = items.len() - 1;
        edit.item = last;
        edit.move_item(1, &r);
        assert_eq!(edit.item, last, "stops at the bottom");
    }

    #[test]
    fn an_item_shows_one_binding_when_it_names_one() {
        let r = menus();
        let edit = r.index_of("Edit").expect("Edit is in the bar");
        let state = MenuBar {
            open: Some(edit),
            item: 0,
            hover: None,
            sub: None,
        };
        // Undo is bound to C-/, C-_ and C-x u; the menu shows one.
        let rows = panel_rows(state, &r, &Context::all_exist());
        let undo = rows.iter().find(|x| x.text.contains("Undo")).unwrap();
        assert!(undo.text.trim_end().ends_with("C-x u"), "{:?}", undo.text);
    }

    #[test]
    fn a_submenu_opens_moves_runs_and_closes() {
        let r = menus();
        let edit = r.index_of("Edit").expect("Edit is in the bar");
        let search = r
            .menu(edit)
            .unwrap()
            .items
            .iter()
            .position(|i| i.label == "Incremental Search")
            .expect("Incremental Search is in Edit");
        let mut state = MenuBar {
            open: Some(edit),
            item: search,
            hover: None,
            sub: None,
        };
        assert!(state.on_parent(&r));
        assert!(state.selected(&r).is_none(), "a header runs nothing");
        assert!(
            !MenuBar {
                open: Some(0),
                item: 0,
                hover: None,
                sub: None
            }
            .on_parent(&r)
        );
        assert!(state.open_submenu(&r));
        assert_eq!(state.sub, Some(0));
        assert_eq!(
            state.sub_selected(&r).map(|i| i.command.as_str()),
            Some("isearch-forward")
        );
        state.move_sub(1, &r);
        assert_eq!(state.sub, Some(1));
        state.move_sub(1, &r);
        assert_eq!(state.sub, Some(1), "stops at the bottom");
        state.move_sub(-1, &r);
        assert_eq!(state.sub, Some(0));
        state.first_sub(&r);
        assert_eq!(state.sub, Some(0));
        state.last_sub(&r);
        assert_eq!(state.sub, Some(1));
        // The rows show the children, highlighted in turn.
        let rows = subpanel_rows(state, &r, &Context::all_exist());
        assert_eq!(rows.len(), 2);
        assert!(rows[0].text.contains("Forward String"));
        assert!(rows[1].text.contains("Backward String"));
        assert_eq!(rows[1].face, Face::MenuSelected);
        assert!(rows.iter().all(|x| x.text.contains("C-")), "{rows:?}");
        // The parent panel marks the header with where it leads...
        let rows = panel_rows(state, &r, &Context::all_exist());
        let header = rows
            .iter()
            .find(|x| x.text.contains("Incremental Search"))
            .unwrap();
        assert!(header.text.trim_end().ends_with('>'), "{:?}", header.text);
        assert_eq!(
            header.face,
            Face::MenuSelected,
            "the open header stays highlighted"
        );
        // ...and the view cascades the submenu from the header's row.
        let v = view(state, &r, WIDE, &Context::all_exist());
        let parent = v.popup.expect("a parent panel");
        let sub = v.subpopup.expect("a submenu panel");
        assert_eq!(sub.row, search as u16, "on the header's row");
        assert_eq!(sub.column, parent.column + panel_width(&parent.rows) + 1);
        // Closing the submenu stays in the menu; switching menus drops it.
        assert!(state.close_submenu());
        assert_eq!(state.sub, None);
        assert!(!state.close_submenu(), "nothing left to close");
        assert!(subpanel_rows(state, &r, &Context::all_exist()).is_empty());
        assert!(state.open_submenu(&r));
        state.right(&r);
        assert_eq!(state.sub, None);
        assert_eq!(state.open, Some(edit + 1));
    }

    #[test]
    fn moving_between_menus_wraps_and_keeps_the_bar_open() {
        let r = menus();
        let mut state = MenuBar::default();
        state.right(&r);
        assert_eq!(state.open, Some(0));
        state.right(&r);
        assert_eq!(state.open, Some(1));
        state.left(&r);
        assert_eq!(state.open, Some(0));
        state.left(&r);
        assert_eq!(state.open, Some(r.len() - 1), "wraps to Help");
        state.close();
        assert_eq!(state.open, None);
        assert!(state.selected(&r).is_none());
    }

    #[test]
    fn hovering_marks_a_title_and_follows_it_between_menus() {
        let r = menus();
        let mut state = MenuBar::default();
        state.hover(Some(2), &r);
        assert!(state.is_hovered(2));
        assert_eq!(state.open, None, "hovering doesn't open it");
        let v = view(state, &r, WIDE, &Context::all_exist());
        assert!(v.row[2].face == Face::MenuSelected, "and it is highlighted");

        // With a menu open, moving onto another title switches to it.
        state.open = Some(0);
        state.hover(Some(3), &r);
        assert_eq!(state.open, Some(3));
        assert_eq!(state.item, 0, "the highlight starts at the top");
        // Leaving the bar leaves it open, the way a tmenu does, and takes
        // the mark off the bar with it.
        state.hover(None, &r);
        assert_eq!(state.open, Some(3), "still open off the bar");
        assert_eq!(state.hover, None, "with nothing marked");
        state.close();
        assert_eq!(state.open, None);
        assert_eq!(state.hover, None);
    }

    #[test]
    fn an_undefined_command_is_shown_disabled() {
        let r = menus();
        let item = r
            .menu(0)
            .unwrap()
            .items
            .iter()
            .position(|i| i.command == "project-find-file")
            .unwrap();
        let state = MenuBar {
            open: Some(0),
            item,
            hover: None,
            sub: None,
        };
        assert_eq!(
            state.selected(&r).map(|i| i.command.as_str()),
            Some("project-find-file")
        );
        // The highlighted item shows as highlighted either way.
        assert_eq!(
            popup_rows(state, &r, &Context::none())[item].1,
            Face::MenuSelected
        );
        // Some other item's face depends on whether it can run at all.
        assert_eq!(
            popup_rows(state, &r, &Context::none())[item - 1].1,
            Face::MenuDisabled
        );
        assert_eq!(
            popup_rows(state, &r, &Context::all_exist())[item - 1].1,
            Face::MenuBar
        );
        // The label is there to read either way.
        assert!(
            popup_rows(state, &r, &Context::none())[item]
                .0
                .contains("Open File In Project...")
        );
    }

    #[test]
    fn an_item_s_condition_greys_it_and_ticks_it() {
        let r = menus();
        let item = |command: &str| {
            r.menus()
                .iter()
                .flat_map(|m| m.items.iter())
                .flat_map(|i| std::iter::once(i).chain(i.submenu.iter()))
                .find(|i| i.command == command)
                .unwrap_or_else(|| panic!("{command} is in the bar"))
                .clone()
        };

        let unmodified = MenuState::default();
        let save = item("save-buffer");
        assert!(!save.enabled.holds(&unmodified));
        let modified = MenuState {
            modified: true,
            ..Default::default()
        };
        assert!(save.enabled.holds(&modified));

        // The region commands need a region.
        let kill = item("kill-region");
        assert!(!kill.enabled.holds(&unmodified));
        let region = MenuState {
            region: true,
            ..Default::default()
        };
        assert!(kill.enabled.holds(&region));

        // A display toggle ticks while its option is on, and its condition
        // can be "one of these values" rather than one.
        let numbers = item("display-line-numbers-mode");
        assert!(!numbers.checked.holds(&unmodified));
        let mut absolute = MenuState::default().with_options(|n| match n {
            "line-numbers" => Some("absolute".into()),
            _ => None,
        });
        assert!(numbers.checked.holds(&absolute));
        absolute = absolute.with_options(|n| match n {
            "line-numbers" => Some("relative".into()),
            _ => None,
        });
        assert!(numbers.checked.holds(&absolute));
        let hl = item("hl-line-mode");
        assert!(!hl.checked.holds(&absolute));
        let hl_on = MenuState::default().with_options(|n| match n {
            "hl-line" => Some("true".into()),
            _ => None,
        });
        assert!(hl.checked.holds(&hl_on));
        // Show-paren isn't buffer-local, so it comes from the editor's own
        // settings instead.
        let parens = item("show-paren-mode");
        assert!(!parens.checked.holds(&hl_on));
        assert!(
            parens
                .checked
                .holds(&hl_on.with_flags(|n| n == "show-paren"))
        );

        // And what the conditions say is what the panel draws.
        let file = MenuBar {
            open: Some(0),
            item: 0,
            hover: None,
            sub: None,
        };
        let mut ctx = Context::all_exist();
        ctx.state = modified;
        let rows = panel_rows(file, &r, &ctx);
        let save_row = rows
            .iter()
            .find(|x| x.text.contains("Save"))
            .expect("Save is in File");
        assert_eq!(save_row.face, Face::MenuBar);
        ctx.state = unmodified;
        let rows = panel_rows(file, &r, &ctx);
        let save_row = rows.iter().find(|x| x.text.contains("Save")).unwrap();
        assert_eq!(save_row.face, Face::MenuDisabled);

        // A ticked item shows the tick, and the labels still line up.
        let ctx = Context {
            exists: Box::new(|_| true),
            keys: Box::new(|_| Vec::new()),
            state: MenuState::default().with_options(|n| match n {
                "hl-line" => Some("true".into()),
                _ => None,
            }),
        };
        let options = r.index_of("Options").expect("Options is in the bar");
        let show_hide = r
            .menu(options)
            .unwrap()
            .items
            .iter()
            .position(|i| i.label == "Show/Hide")
            .expect("Show/Hide is in Options");
        let rows = subpanel_rows(
            MenuBar {
                open: Some(options),
                item: show_hide,
                hover: None,
                sub: Some(0),
            },
            &r,
            &ctx,
        );
        let hl_row = rows
            .iter()
            .find(|x| x.text.contains("Highlight Current Line"))
            .expect("it is in Show/Hide");
        assert!(hl_row.text.contains('\u{2713}'), "{:?}", hl_row.text);
        let parens_row = rows
            .iter()
            .find(|x| x.text.contains("Matching Parens"))
            .expect("it is in Show/Hide");
        assert!(!parens_row.text.contains('\u{2713}'));
        // Same gutter either way, so the labels don't jump.
        // Same gutter either way, so the labels don't jump: both start
        // after the panel's leading space and the two-wide tick column.
        let label_col = |t: &str| t.chars().position(|c| c != ' ' && c != '\u{2713}').unwrap();
        assert_eq!(label_col(&hl_row.text), 3, "{:?}", hl_row.text);
        assert_eq!(label_col(&parens_row.text), 3, "{:?}", parens_row.text);
        // Every row is still the same width.
        let width = panel_width(&rows);
        assert!(rows.iter().all(|x| x.text.chars().count() as u16 == width));
    }

    #[test]
    fn the_frame_gives_the_bar_a_row_when_asked() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        };
        let off = frame_areas(area, false, false, 0);
        assert_eq!(off.menu.height, 0);
        assert_eq!(off.tab_bar.height, 0);
        assert_eq!(off.windows.height, 9);
        assert_eq!(off.windows.y, 0);
        assert_eq!(off.echo.y, 9);

        let on = frame_areas(area, true, false, 0);
        assert_eq!(on.menu.height, 1);
        assert_eq!(on.menu.y, 0);
        assert_eq!(on.windows.y, 1);
        assert_eq!(on.windows.height, 8);
        assert_eq!(on.echo.y, 9, "the echo area is still the last row");
    }

    #[test]
    fn the_tab_bar_gets_its_row_below_the_menu_bar() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        };
        let both = frame_areas(area, true, true, 0);
        assert_eq!(both.menu.y, 0);
        assert_eq!(both.tab_bar.y, 1);
        assert_eq!(both.tab_bar.height, 1);
        assert_eq!(both.windows.y, 2, "windows start under both bars");
        assert_eq!(both.windows.height, 7);
        assert_eq!(both.echo.y, 9);
        // The tab bar alone sits where the menu bar alone would.
        let tab_only = frame_areas(area, false, true, 0);
        assert_eq!(tab_only.tab_bar.y, 0);
        assert_eq!(tab_only.windows.y, 1);
    }

    #[test]
    fn the_tool_bar_and_modifier_bar_get_their_rows() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        };
        let top = frame_areas_with(area, true, true, true, true, 0, false);
        assert_eq!(top.menu.y, 0);
        assert_eq!(top.tool_bar.y, 1);
        assert_eq!(top.modifier_bar.y, 2);
        assert_eq!(top.tab_bar.y, 3);
        assert_eq!(top.windows.y, 4);
        assert_eq!(top.windows.height, 5);
        assert_eq!(top.echo.y, 9);

        let bottom = frame_areas_with(area, true, true, true, true, 0, true);
        assert_eq!(bottom.menu.y, 0);
        assert_eq!(bottom.modifier_bar.y, 1);
        assert_eq!(bottom.tab_bar.y, 2);
        assert_eq!(bottom.windows.y, 3);
        assert_eq!(bottom.windows.height, 5);
        assert_eq!(bottom.tool_bar.y, 8);
        assert_eq!(bottom.echo.y, 9);
    }

    #[test]
    fn the_list_and_the_bar_share_the_frame_without_overlapping() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        };
        for list in [0, 1, 3, 8, 20] {
            let a = frame_areas(area, true, true, list);
            assert_eq!(a.menu.y + a.menu.height, a.tab_bar.y);
            assert_eq!(a.tab_bar.y + a.tab_bar.height, a.windows.y);
            assert_eq!(a.windows.y + a.windows.height, a.list.y);
            assert_eq!(a.list.y + a.list.height, a.echo.y);
            assert!(a.echo.y + a.echo.height <= area.y + area.height);
            assert!(a.windows.height >= 1, "list {list} left no text rows");
        }
    }

    #[test]
    fn a_tiny_frame_does_not_go_negative() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 1,
        };
        for h in 0..4 {
            let a = frame_areas(area, true, true, h);
            assert!(a.echo.y <= area.y + area.height, "h={h} {a:?}");
        }
    }
}

#[cfg(test)]
mod buffers_menu_tests {
    use super::*;

    #[test]
    fn the_buffers_menu_lists_the_open_buffers_and_follows_them() {
        let mut r = Registry::builtin();
        let at = |r: &Registry| {
            r.menu(r.index_of("Buffers").unwrap())
                .unwrap()
                .items
                .iter()
                .map(|i| i.label.clone())
                .collect::<Vec<_>>()
        };
        let before = at(&r);
        assert_eq!(before[0], "Next Buffer");
        r.set_buffers(&["a.rs".into(), "b.rs".into()]);
        let now = at(&r);
        assert_eq!(
            &now[..3],
            ["a.rs", "b.rs", ""],
            "the list, then a separator"
        );
        assert_eq!(now[3], "Next Buffer");
        let item = &r.menu(r.index_of("Buffers").unwrap()).unwrap().items[1];
        assert_eq!(item.command, "select-buffer:b.rs");
        // A shorter list replaces it rather than adding to it.
        r.set_buffers(&["c.rs".into()]);
        assert_eq!(&at(&r)[..2], ["c.rs", ""]);
        r.set_buffers(&[]);
        assert_eq!(at(&r), before);
    }
}

#[cfg(test)]
mod per_mode_menu_tests {
    use super::*;

    #[test]
    fn a_plugin_may_define_a_menu_for_each_mode() {
        let mut r = Registry::default();
        let item = || vec![Item::new("A", "a")];
        r.define_when("Bases", item(), "bases", When::MajorMode("x".into())).unwrap();
        r.define_when("Bases", item(), "bases", When::MajorMode("y".into())).unwrap();
        assert!(r.define_when("Bases", item(), "bases", When::MajorMode("y".into())).is_err());
        assert!(r.define_when("Bases", item(), "other", When::MajorMode("z".into())).is_err());
    }
}
