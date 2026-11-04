// SPDX-License-Identifier: MIT OR Apache-2.0
//! Icons: `icon-preference`, plus icons drawn by the host system.
//!
//! Every icon has a name (`undo`, `save`, …) and can be drawn four ways,
//! the types Forge knows:
//!
//! * `image`  — a picture. Forge ships none of its own: the picture is the
//!   system icon of the platform in use (`system`), or a file the user
//!   names in an override.
//! * `emoji`  — a colourful emoji.
//! * `symbol` — a monochrome one-cell symbol.
//! * `text`   — plain ASCII.
//!
//! `icon-preference` lists the types in order; the first one the frontend
//! can show wins. `system` is accepted too, as an alias that asks for the
//! native icon set of [`Platform`]: SF Symbols on macOS, Segoe Fluent on
//! Windows, freedesktop names on Linux.
//!
//! Each icon can be overridden with `forge.opt.icon_overrides` (see
//! [`Prefs::parse_override`]); themes do the same through that table.

use std::collections::BTreeMap;

/// How an icon can be drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconType {
    Image,
    Emoji,
    Symbol,
    Text,
    /// The native icon of the chosen platform.
    System,
}

impl IconType {
    pub const ALL: [IconType; 5] = [
        Self::System,
        Self::Image,
        Self::Emoji,
        Self::Symbol,
        Self::Text,
    ];

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "image" => Some(Self::Image),
            "emoji" => Some(Self::Emoji),
            "symbol" => Some(Self::Symbol),
            "text" => Some(Self::Text),
            "system" | "native" => Some(Self::System),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Emoji => "emoji",
            Self::Symbol => "symbol",
            Self::Text => "text",
            Self::System => "system",
        }
    }
}

/// Whose native icons `system` means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    MacOs,
    Windows,
    Linux,
}

impl Platform {
    pub const ALL: [Platform; 3] = [Self::MacOs, Self::Windows, Self::Linux];

    /// The platform Forge was built for. `ios` shares the macOS set.
    pub fn current() -> Self {
        if cfg!(any(target_os = "macos", target_os = "ios")) {
            Self::MacOs
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }

    /// `auto` is not a platform; callers treat it as [`Platform::current`].
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "macos" | "mac" | "darwin" | "apple" | "ios" => Some(Self::MacOs),
            "windows" | "win" | "win32" => Some(Self::Windows),
            "linux" | "freedesktop" | "gnome" | "kde" | "xdg" => Some(Self::Linux),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::MacOs => "macos",
            Self::Windows => "windows",
            Self::Linux => "linux",
        }
    }

    /// The icon set the names come from.
    pub fn set(self) -> &'static str {
        match self {
            Self::MacOs => "SF Symbols",
            Self::Windows => "Segoe Fluent Icons",
            Self::Linux => "freedesktop icon theme",
        }
    }
}

/// One built-in icon.
#[derive(Debug, Clone, Copy)]
pub struct Builtin {
    pub name: &'static str,
    /// Commands whose tool-bar button uses this icon.
    pub commands: &'static [&'static str],
    pub text: &'static str,
    pub symbol: &'static str,
    pub emoji: &'static str,
    /// SF Symbols name.
    pub macos: &'static str,
    /// Segoe Fluent Icons name (the glyph's documented name).
    pub windows: &'static str,
    /// freedesktop.org icon-naming-spec name.
    pub linux: &'static str,
}

macro_rules! icon {
    ($name:expr, [$($cmd:expr),*], $text:expr, $sym:expr, $emoji:expr,
     $mac:expr, $win:expr, $lin:expr) => {
        Builtin {
            name: $name,
            commands: &[$($cmd),*],
            text: $text,
            symbol: $sym,
            emoji: $emoji,
            macos: $mac,
            windows: $win,
            linux: $lin,
        }
    };
}

/// The built-in icons, sorted by name.
pub const BUILTIN: &[Builtin] = &[
    icon!(
        "back",
        [
            "info-last",
            "webkit-back",
            "few-back",
            "help-prev-link",
            "image-previous-file"
        ],
        "<",
        "←",
        "⬅️",
        "chevron.left",
        "Back",
        "go-previous"
    ),
    icon!(
        "bookmark-add",
        ["few-add-bookmark"],
        "+*",
        "★",
        "⭐",
        "star",
        "FavoriteStar",
        "bookmark-new"
    ),
    icon!(
        "bookmarks",
        ["few-list-bookmarks"],
        "bm",
        "▤",
        "📚",
        "book",
        "FavoriteList",
        "user-bookmarks"
    ),
    icon!(
        "checkbox",
        ["org-toggle-checkbox"],
        "[x]",
        "☑",
        "☑️",
        "checkmark.square",
        "CheckboxComposite",
        "checkbox-checked"
    ),
    icon!(
        "close",
        [
            "kill-this-buffer",
            "org-agenda-quit",
            "info-quit",
            "quit-window"
        ],
        "x",
        "✕",
        "❌",
        "xmark",
        "Cancel",
        "window-close"
    ),
    icon!(
        "copy",
        ["kill-ring-save", "dired-do-copy", "webkit-copy-url"],
        "cp",
        "⧉",
        "📋",
        "doc.on.doc",
        "Copy",
        "edit-copy"
    ),
    icon!(
        "cut",
        ["kill-region"],
        "x",
        "✂",
        "✂️",
        "scissors",
        "Cut",
        "edit-cut"
    ),
    icon!(
        "deadline",
        ["org-deadline"],
        "!",
        "⚐",
        "🚩",
        "flag",
        "Flag",
        "flag-red"
    ),
    icon!(
        "delete",
        ["dired-do-delete", "package-delete", "diff-hunk-kill"],
        "del",
        "⌫",
        "🗑️",
        "trash",
        "Delete",
        "edit-delete"
    ),
    icon!(
        "dired",
        ["dired"],
        "ls",
        "▤",
        "🗂️",
        "folder",
        "Folder",
        "folder"
    ),
    icon!(
        "download",
        ["few-download", "package-install"],
        "dl",
        "⤓",
        "⬇️",
        "arrow.down.circle",
        "Download",
        "document-save-as"
    ),
    icon!(
        "flag",
        ["dired-flag-file-deletion"],
        "D",
        "⚑",
        "🏴",
        "flag.fill",
        "Flag",
        "emblem-important"
    ),
    icon!(
        "history",
        ["webkit-browse-history", "few-list-history", "vc-print-log"],
        "hist",
        "◷",
        "🕘",
        "clock.arrow.circlepath",
        "History",
        "document-open-recent"
    ),
    icon!(
        "home",
        ["webkit-root-url"],
        "home",
        "⌂",
        "🏠",
        "house",
        "Home",
        "go-home"
    ),
    icon!(
        "inspect",
        ["webkit-show-element"],
        "dev",
        "⌥",
        "🔧",
        "hammer",
        "DeveloperTools",
        "applications-development"
    ),
    icon!(
        "link",
        ["org-insert-link"],
        "ln",
        "⛓",
        "🔗",
        "link",
        "Link",
        "insert-link"
    ),
    icon!(
        "mark",
        ["dired-mark", "vc-dir-mark"],
        "*",
        "✓",
        "✅",
        "checkmark",
        "Accept",
        "emblem-default"
    ),
    icon!(
        "menu",
        ["info-menu"],
        "=",
        "☰",
        "📖",
        "list.bullet",
        "GlobalNavButton",
        "open-menu"
    ),
    icon!(
        "new-window",
        ["webkit-new", "term"],
        "+",
        "⊞",
        "🪟",
        "macwindow.badge.plus",
        "NewWindow",
        "window-new"
    ),
    icon!(
        "next",
        [
            "info-next",
            "webkit-forward",
            "few-forward",
            "help-next-link",
            "image-next-file"
        ],
        ">",
        "→",
        "➡️",
        "chevron.right",
        "Forward",
        "go-next"
    ),
    icon!(
        "open",
        [
            "find-file",
            "webkit-browse-url",
            "few-browse-url",
            "vc-dir-find-file"
        ],
        "o",
        "⊞",
        "📂",
        "folder.badge.plus",
        "OpenFile",
        "document-open"
    ),
    icon!(
        "open-link",
        ["org-open-at-point"],
        "->",
        "↗",
        "↗️",
        "arrow.up.right.square",
        "OpenInNewWindow",
        "go-jump"
    ),
    icon!(
        "paste",
        ["yank"],
        "pa",
        "⎘",
        "📌",
        "doc.on.clipboard",
        "Paste",
        "edit-paste"
    ),
    icon!(
        "refresh",
        [
            "revert-buffer",
            "webkit-reload",
            "few-reload",
            "vc-dir-refresh",
            "org-agenda-redo",
            "package-refresh-contents",
            "recompile"
        ],
        "r",
        "↻",
        "🔄",
        "arrow.clockwise",
        "Refresh",
        "view-refresh"
    ),
    icon!(
        "rename",
        ["dired-do-rename"],
        "mv",
        "✎",
        "✏️",
        "pencil",
        "Rename",
        "document-edit"
    ),
    icon!(
        "save",
        ["save-buffer"],
        "S",
        "▣",
        "💾",
        "square.and.arrow.down",
        "Save",
        "document-save"
    ),
    icon!(
        "schedule",
        ["org-schedule"],
        "@",
        "◷",
        "🗓️",
        "calendar",
        "Calendar",
        "appointment-new"
    ),
    icon!(
        "search",
        [
            "isearch-forward",
            "org-tags-sparse-tree",
            "webkit-find",
            "few-find",
            "grep",
            "info-search",
            "org-agenda-filter"
        ],
        "?",
        "⌕",
        "🔍",
        "magnifyingglass",
        "Find",
        "edit-find"
    ),
    icon!(
        "stop",
        ["webkit-stop", "kill-compilation", "term-kill-subjob"],
        "stop",
        "■",
        "⏹️",
        "stop.fill",
        "Stop",
        "process-stop"
    ),
    icon!(
        "store",
        ["org-store-link"],
        "st",
        "⎘",
        "🔖",
        "bookmark",
        "Bookmarks",
        "bookmark-new"
    ),
    icon!(
        "tags",
        ["org-set-tags-command"],
        "#",
        "⌗",
        "🏷️",
        "tag",
        "Tag",
        "tag"
    ),
    icon!(
        "toc",
        ["info-toc"],
        "toc",
        "≡",
        "📑",
        "list.dash",
        "List",
        "view-list"
    ),
    icon!(
        "top",
        ["info-top"],
        "^",
        "⇤",
        "⏫",
        "arrow.up.to.line",
        "Up",
        "go-top"
    ),
    icon!(
        "tree",
        ["dired-toggle-tree"],
        "tr",
        "⌥",
        "🌲",
        "list.bullet.indent",
        "ShowResults",
        "view-list-tree"
    ),
    icon!(
        "undo",
        ["undo"],
        "<-",
        "↶",
        "↩️",
        "arrow.uturn.backward",
        "Undo",
        "edit-undo"
    ),
    icon!(
        "unmark",
        ["dired-unmark", "vc-dir-unmark"],
        "o",
        "○",
        "⬜",
        "circle",
        "Checkbox",
        "emblem-unreadable"
    ),
    icon!(
        "up",
        ["dired-up-directory", "info-up", "webkit-up-url"],
        "^",
        "↑",
        "⬆️",
        "arrow.up",
        "Up",
        "go-up"
    ),
    icon!(
        "zoom-in",
        ["webkit-zoom-in", "image-increase-size"],
        "+",
        "⊕",
        "🔍",
        "plus.magnifyingglass",
        "ZoomIn",
        "zoom-in"
    ),
    icon!(
        "zoom-out",
        ["webkit-zoom-out", "image-decrease-size"],
        "-",
        "⊖",
        "🔎",
        "minus.magnifyingglass",
        "ZoomOut",
        "zoom-out"
    ),
    icon!(
        "zoom-reset",
        ["webkit-zoom-reset", "image-transform-reset-to-original"],
        "100",
        "◎",
        "🎯",
        "1.magnifyingglass",
        "FullScreen",
        "zoom-original"
    ),
];

pub fn builtin(name: &str) -> Option<&'static Builtin> {
    BUILTIN.iter().find(|b| b.name == name)
}

/// The icon a command's tool-bar button uses.
pub fn for_command(command: &str) -> Option<&'static Builtin> {
    BUILTIN.iter().find(|b| b.commands.contains(&command))
}

/// What an icon resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// Characters to draw with the editor's font (text, symbol or emoji).
    Glyph { kind: IconType, text: String },
    /// A picture on disk.
    File(String),
    /// A native icon of `platform`, named in its own icon set.
    System { platform: Platform, name: String },
}

/// What the frontend can show. The terminal has no pictures and often no
/// emoji; the macOS app has everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Support {
    pub image: bool,
    pub emoji: bool,
    pub symbol: bool,
    /// Whether the native icons of the current platform can be drawn.
    pub system: bool,
}

impl Support {
    pub const GRAPHICAL: Support = Support {
        image: true,
        emoji: true,
        symbol: true,
        system: true,
    };
    pub const TERMINAL: Support = Support {
        image: false,
        emoji: false,
        symbol: true,
        system: false,
    };
}

/// One customisation (`customize-icon`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Override {
    /// Use this text/symbol/emoji instead of the built-in one.
    Glyph(IconType, String),
    /// Use this picture file.
    File(String),
    /// Use this name from the platform's icon set.
    System(String),
}

#[derive(Debug, Clone)]
pub struct Prefs {
    pub preference: Vec<IconType>,
    /// None means [`Platform::current`].
    pub platform: Option<Platform>,
    pub overrides: BTreeMap<String, Override>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            preference: vec![
                IconType::Image,
                IconType::Emoji,
                IconType::Symbol,
                IconType::Text,
            ],
            platform: None,
            overrides: BTreeMap::new(),
        }
    }
}

impl Prefs {
    pub fn platform(&self) -> Platform {
        self.platform.unwrap_or_else(Platform::current)
    }

    /// `icon_preference`: type names separated by commas or spaces.
    pub fn parse_preference(s: &str) -> Result<Vec<IconType>, String> {
        let mut out = Vec::new();
        for word in s
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|w| !w.is_empty())
        {
            let kind = IconType::parse(word).ok_or_else(|| {
                format!("unknown icon type {word:?}; use system, image, emoji, symbol or text")
            })?;
            if !out.contains(&kind) {
                out.push(kind);
            }
        }
        if out.is_empty() {
            return Err("icon_preference needs at least one icon type".into());
        }
        Ok(out)
    }

    /// An override value: `symbol:↶`, `emoji:↩️`, `text:undo`,
    /// `image:/path/icon.png` or `system:arrow.uturn.backward`.
    pub fn parse_override(v: &str) -> Result<Override, String> {
        let (kind, rest) = v
            .split_once(':')
            .ok_or_else(|| format!("{v:?} is not TYPE:VALUE"))?;
        let rest = rest.trim();
        if rest.is_empty() {
            return Err(format!("{v:?} has no value"));
        }
        match IconType::parse(kind) {
            Some(IconType::Image) => Ok(Override::File(rest.into())),
            Some(IconType::System) => Ok(Override::System(rest.into())),
            Some(k) => Ok(Override::Glyph(k, rest.into())),
            None => Err(format!("unknown icon type {kind:?}")),
        }
    }

    /// Pick the icon to draw: the first type in the preference that the
    /// frontend supports and the icon has. An override of a type wins over
    /// the built-in one of that type.
    pub fn resolve(&self, name: &str, support: Support) -> Option<Resolved> {
        let base = builtin(name);
        let over = self.overrides.get(name);
        let platform = self.platform();
        for kind in &self.preference {
            let found = match kind {
                IconType::System | IconType::Image => {
                    let system = support.system && (*kind == IconType::System || support.image);
                    match over {
                        Some(Override::System(n)) if system => Some(Resolved::System {
                            platform,
                            name: n.clone(),
                        }),
                        Some(Override::File(p)) if support.image && *kind == IconType::Image => {
                            Some(Resolved::File(p.clone()))
                        }
                        _ if *kind == IconType::Image
                            && over.is_some_and(|o| matches!(o, Override::File(_))) =>
                        {
                            None
                        }
                        _ if system => base.map(|b| Resolved::System {
                            platform,
                            name: system_name(b, platform).to_string(),
                        }),
                        _ => None,
                    }
                }
                IconType::Emoji | IconType::Symbol | IconType::Text => {
                    let ok = match kind {
                        IconType::Emoji => support.emoji,
                        IconType::Symbol => support.symbol,
                        _ => true,
                    };
                    if !ok {
                        None
                    } else {
                        match over {
                            Some(Override::Glyph(k, t)) if k == kind => Some(Resolved::Glyph {
                                kind: *kind,
                                text: t.clone(),
                            }),
                            _ => base.map(|b| Resolved::Glyph {
                                kind: *kind,
                                text: match kind {
                                    IconType::Emoji => b.emoji,
                                    IconType::Symbol => b.symbol,
                                    _ => b.text,
                                }
                                .to_string(),
                            }),
                        }
                    }
                }
            };
            if found.is_some() {
                return found;
            }
        }
        None
    }

    /// The glyph for a one-cell display: what the terminal draws.
    pub fn glyph(&self, name: &str, support: Support) -> Option<String> {
        match self.resolve(name, support)? {
            Resolved::Glyph { text, .. } => Some(text),
            _ => None,
        }
    }
}

pub fn system_name(b: &Builtin, p: Platform) -> &'static str {
    match p {
        Platform::MacOs => b.macos,
        Platform::Windows => b.windows,
        Platform::Linux => b.linux,
    }
}

/// `describe-icon`: everything known about one icon.
pub fn describe(prefs: &Prefs, name: &str, support: Support) -> Option<String> {
    let b = builtin(name);
    let over = prefs.overrides.get(name);
    if b.is_none() && over.is_none() {
        return None;
    }
    let mut out = format!("Icon `{name}`\n\n");
    if let Some(b) = b {
        if !b.commands.is_empty() {
            out.push_str(&format!("  Used by:  {}\n", b.commands.join(", ")));
        }
        out.push_str(&format!(
            "  text:     {}\n  symbol:   {}\n  emoji:    {}\n",
            b.text, b.symbol, b.emoji
        ));
        for p in Platform::ALL {
            out.push_str(&format!(
                "  {:<9} {} ({})\n",
                format!("{}:", p.name()),
                system_name(b, p),
                p.set()
            ));
        }
    }
    if let Some(o) = over {
        out.push_str(&format!("\n  Customized: {}\n", show_override(o)));
    }
    out.push_str(&format!(
        "\n  icon-preference: {}\n  platform: {}{}\n",
        prefs
            .preference
            .iter()
            .map(|k| k.name())
            .collect::<Vec<_>>()
            .join(", "),
        prefs.platform().name(),
        if prefs.platform.is_none() {
            " (auto)"
        } else {
            ""
        },
    ));
    match prefs.resolve(name, support) {
        Some(Resolved::Glyph { kind, text }) => {
            out.push_str(&format!("  Showing:  {} {text}\n", kind.name()))
        }
        Some(Resolved::File(p)) => out.push_str(&format!("  Showing:  image {p}\n")),
        Some(Resolved::System { platform, name }) => {
            out.push_str(&format!("  Showing:  {} {name}\n", platform.set()))
        }
        None => out.push_str("  Showing:  nothing\n"),
    }
    Some(out)
}

pub fn show_override(o: &Override) -> String {
    match o {
        Override::Glyph(k, t) => format!("{}:{t}", k.name()),
        Override::File(p) => format!("image:{p}"),
        Override::System(n) => format!("system:{n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_and_commands_are_unique() {
        assert!(BUILTIN.windows(2).all(|w| w[0].name < w[1].name));
        let mut seen = std::collections::HashSet::new();
        for b in BUILTIN {
            for c in b.commands {
                assert!(seen.insert(*c), "{c} has two icons");
            }
            for s in [
                b.text, b.symbol, b.emoji, b.macos, b.windows, b.linux,
            ] {
                assert!(!s.is_empty(), "{} has an empty entry", b.name);
            }
        }
    }

    #[test]
    fn every_toolbar_icon_has_a_name() {
        for bar in [crate::toolbar::Registry::default().bar_for("", true)] {
            for item in &bar.items {
                if !item.is_separator() {
                    assert!(
                        for_command(&item.command).is_some(),
                        "{} has no icon",
                        item.command
                    );
                }
            }
        }
    }

    #[test]
    fn preference_picks_first_supported() {
        let mut p = Prefs::default();
        p.platform = Some(Platform::MacOs);
        assert_eq!(
            p.resolve("undo", Support::GRAPHICAL),
            Some(Resolved::System {
                platform: Platform::MacOs,
                name: "arrow.uturn.backward".into()
            })
        );
        // The terminal has no pictures or emoji, so the symbol wins.
        assert_eq!(p.glyph("undo", Support::TERMINAL).as_deref(), Some("↶"));
        p.preference = Prefs::parse_preference("emoji text").unwrap();
        assert_eq!(p.glyph("save", Support::GRAPHICAL).as_deref(), Some("💾"));
        assert_eq!(p.glyph("save", Support::TERMINAL).as_deref(), Some("S"));
    }

    #[test]
    fn each_platform_names_its_own_icons() {
        let mut p = Prefs::default();
        p.preference = vec![IconType::System];
        for (plat, want) in [
            (Platform::MacOs, "square.and.arrow.down"),
            (Platform::Windows, "Save"),
            (Platform::Linux, "document-save"),
        ] {
            p.platform = Some(plat);
            assert_eq!(
                p.resolve("save", Support::GRAPHICAL),
                Some(Resolved::System {
                    platform: plat,
                    name: want.into()
                })
            );
        }
        // Without system support, nothing in the list can be shown.
        assert_eq!(p.resolve("save", Support::TERMINAL), None);
    }

    #[test]
    fn overrides_replace_one_type() {
        let mut p = Prefs::default();
        p.overrides
            .insert("undo".into(), Prefs::parse_override("symbol:⟲").unwrap());
        p.preference = vec![IconType::Symbol];
        assert_eq!(p.glyph("undo", Support::TERMINAL).as_deref(), Some("⟲"));
        p.preference = vec![IconType::Text];
        assert_eq!(p.glyph("undo", Support::TERMINAL).as_deref(), Some("<-"));
        p.overrides.insert(
            "undo".into(),
            Prefs::parse_override("image:/tmp/u.png").unwrap(),
        );
        p.preference = vec![IconType::Image];
        assert_eq!(
            p.resolve("undo", Support::GRAPHICAL),
            Some(Resolved::File("/tmp/u.png".into()))
        );
        p.overrides.insert(
            "undo".into(),
            Prefs::parse_override("system:arrow.left").unwrap(),
        );
        p.preference = vec![IconType::System];
        assert!(
            matches!(p.resolve("undo", Support::GRAPHICAL), Some(Resolved::System { name, .. }) if name == "arrow.left")
        );
    }

    #[test]
    fn parsing_reports_mistakes() {
        assert!(Prefs::parse_preference("image, nope").is_err());
        assert!(Prefs::parse_preference("  ").is_err());
        assert!(Prefs::parse_override("symbol").is_err());
        assert!(Prefs::parse_override("blob:x").is_err());
        assert_eq!(Platform::parse("Darwin"), Some(Platform::MacOs));
        assert!(describe(&Prefs::default(), "nope", Support::GRAPHICAL).is_none());
        assert!(
            describe(&Prefs::default(), "undo", Support::GRAPHICAL)
                .unwrap()
                .contains("edit-undo")
        );
    }
}
