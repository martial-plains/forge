// SPDX-License-Identifier: MIT OR Apache-2.0
//! Faces a theme restyles: `forge.theme.set("comment", { fg = "#6a9955" })`.
//!
//! An override is per-field: a field it leaves unset keeps the
//! frontend's own style, so light and dark mode keep working and a theme
//! only says what it means. Both frontends apply the table on top of
//! their built-in styles, so the terminal and the macOS app agree.

use std::collections::BTreeMap;

/// A color: red, green, blue, each 0-255.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// As `#rrggbb`, for the FFI boundary and messages.
    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

/// One face's restyle: every field optional, unset keeps the default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FaceStyle {
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
}

impl FaceStyle {
    /// True when nothing is overridden: setting one of these resets the
    /// face, the way an empty style table does from Lua.
    pub fn is_empty(&self) -> bool {
        self == &FaceStyle::default()
    }
}

/// The standard sixteen, as most terminals draw them.
const NAMED: &[(&str, Rgb)] = &[
    ("black", Rgb(0, 0, 0)),
    ("red", Rgb(205, 49, 49)),
    ("green", Rgb(13, 188, 121)),
    ("yellow", Rgb(229, 229, 16)),
    ("blue", Rgb(36, 114, 200)),
    ("magenta", Rgb(188, 63, 188)),
    ("cyan", Rgb(17, 168, 205)),
    ("white", Rgb(229, 229, 229)),
    ("bright-black", Rgb(102, 102, 102)),
    ("bright-red", Rgb(241, 76, 76)),
    ("bright-green", Rgb(35, 209, 139)),
    ("bright-yellow", Rgb(245, 245, 67)),
    ("bright-blue", Rgb(59, 142, 234)),
    ("bright-magenta", Rgb(214, 112, 214)),
    ("bright-cyan", Rgb(41, 184, 219)),
    ("bright-white", Rgb(255, 255, 255)),
];

/// A color: `#rgb`, `#rrggbb`, or one of the sixteen names
/// (`black` .. `bright-white`).
pub fn parse_color(s: &str) -> Result<Rgb, String> {
    let bad =
        || format!("{s:?} isn't a color: use #rgb, #rrggbb or a name (black .. bright-white)");
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        if !hex.is_ascii() {
            return Err(bad());
        }
        let digits: Option<Vec<u8>> = match hex.len() {
            3 => hex
                .chars()
                .map(|c| c.to_digit(16).map(|d| (d * 17) as u8))
                .collect(),
            6 => (0..3)
                .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok())
                .collect(),
            _ => return Err(bad()),
        };
        return digits
            .filter(|d| d.len() == 3)
            .map(|d| Rgb(d[0], d[1], d[2]))
            .ok_or_else(bad);
    }
    if let Some((_, rgb)) = NAMED.iter().find(|(n, _)| n.eq_ignore_ascii_case(s)) {
        return Ok(*rgb);
    }
    Err(bad())
}

/// Every face a theme can restyle: the plain faces, the syntax faces,
/// and the decorations with no parameters. Dired's faces keep their own
/// scale, so they aren't listed.
pub fn all_faces() -> Vec<String> {
    // No "escape" here: control characters and string escapes share the
    // name, and it comes in with the syntax faces below. Setting it
    // restyles both.
    let mut names = vec![
        "default",
        "region",
        "match",
        "truncation",
        "mode-line",
        "mode-line-inactive",
        "border",
        "line-number",
        "line-number-current",
        "paren-match",
        "paren-mismatch",
        "trailing-whitespace",
        "whitespace",
        "fill-column-indicator",
        "fold",
        "popup",
        "popup-selected",
        "popup-annotation",
        "popup-margin",
        "popup-match",
        "popup-deprecated",
        "menu-bar",
        "menu-selected",
        "menu-disabled",
        "tab-bar",
        "tab-bar-selected",
        "tool-bar",
        "tool-bar-selected",
        "tool-bar-disabled",
        "error",
        "warning",
        "info",
        "spell-error",
        "highlight",
        "symbol-highlight",
        "grep-heading",
        "underline",
        "plain",
    ];
    names.extend(crate::syntax::SyntaxFace::ALL.iter().map(|f| f.name()));
    names.extend(crate::decoration::RainbowFace::names());
    names.extend(crate::decoration::VcFace::names());
    names.extend(crate::decoration::ModernFace::names());
    names.into_iter().map(String::from).collect()
}

/// A named theme: a description and the faces it restyles. Defined by
/// init.rhai, a file under `~/.config/forge/themes/`, or a plugin.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NamedTheme {
    pub description: String,
    /// `(face, style)`, already validated against [`all_faces`].
    pub faces: Vec<(String, FaceStyle)>,
}

/// The named themes and which one is applied. A theme is defined but not
/// applied until [`crate::app::Editor::load_theme`]; the applied restyles
/// live on the [`Theme`] table beside this.
#[derive(Debug, Clone, Default)]
pub struct Themes {
    /// name -> (owner, theme). The owner is a plugin's name, or
    /// `crate::rhai::SOURCE` for init.rhai and the theme files.
    entries: BTreeMap<String, (String, NamedTheme)>,
    /// The name of the applied theme, if any.
    current: Option<String>,
}

impl Themes {
    /// Define (or redefine) `name`, owned by `owner`. Its faces are
    /// checked now, so a typo is caught where the theme is written.
    pub fn define(&mut self, owner: &str, name: &str, theme: NamedTheme) -> Result<(), String> {
        if name.trim().is_empty() {
            return Err("a theme needs a name".into());
        }
        let known = all_faces();
        for (face, _) in &theme.faces {
            if !known.iter().any(|n| n == face) {
                return Err(format!(
                    "theme {name:?}: unknown face {face:?} (see forge.theme.faces())"
                ));
            }
        }
        self.entries
            .insert(name.to_string(), (owner.to_string(), theme));
        Ok(())
    }

    /// Forget every theme `owner` defined. True when the applied theme
    /// was one of them (the caller then clears the restyles).
    pub fn remove_owner(&mut self, owner: &str) -> bool {
        let removed_current = self
            .current
            .as_deref()
            .is_some_and(|c| self.entries.get(c).is_some_and(|(o, _)| o == owner));
        self.entries.retain(|_, (o, _)| o != owner);
        if removed_current {
            self.current = None;
        }
        removed_current
    }

    /// Forget one theme. True when it was the applied one.
    pub fn undefine(&mut self, name: &str) -> bool {
        let was_current = self.current.as_deref() == Some(name);
        self.entries.remove(name);
        if was_current {
            self.current = None;
        }
        was_current
    }

    pub fn get(&self, name: &str) -> Option<&NamedTheme> {
        self.entries.get(name).map(|(_, t)| t)
    }

    /// Whether `name` is defined and owned by `owner`.
    pub fn is_owned_by(&self, name: &str, owner: &str) -> bool {
        self.entries.get(name).is_some_and(|(o, _)| o == owner)
    }

    pub fn description(&self, name: &str) -> Option<&str> {
        self.entries.get(name).map(|(_, t)| t.description.as_str())
    }

    /// Every theme name, sorted.
    pub fn names(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    pub fn set_current(&mut self, name: Option<String>) {
        self.current = name;
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The restyled faces, by name.
#[derive(Debug, Clone, Default)]
pub struct Theme {
    overrides: BTreeMap<String, FaceStyle>,
}

impl Theme {
    pub fn new() -> Self {
        Self::default()
    }

    /// Restyle `face`. An unknown face is an error (see [`all_faces`]);
    /// an empty style resets it.
    pub fn set(&mut self, face: &str, style: FaceStyle) -> Result<(), String> {
        if !all_faces().iter().any(|n| n == face) {
            return Err(format!("unknown face {face:?} (see forge.theme.faces())"));
        }
        if style.is_empty() {
            self.overrides.remove(face);
        } else {
            self.overrides.insert(face.to_string(), style);
        }
        Ok(())
    }

    /// The restyle for `face`, if any.
    pub fn get(&self, face: &str) -> Option<&FaceStyle> {
        self.overrides.get(face)
    }

    /// Forget one face's restyle.
    pub fn reset(&mut self, face: &str) {
        self.overrides.remove(face);
    }

    /// Forget every restyle.
    pub fn clear(&mut self) {
        self.overrides.clear();
    }

    /// Every restyle, by face name.
    pub fn entries(&self) -> Vec<(String, FaceStyle)> {
        self.overrides
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.overrides.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_parse() {
        assert_eq!(parse_color("#fff"), Ok(Rgb(255, 255, 255)));
        assert_eq!(parse_color("#000000"), Ok(Rgb(0, 0, 0)));
        assert_eq!(parse_color("#6a9955"), Ok(Rgb(0x6a, 0x99, 0x55)));
        assert_eq!(parse_color("red"), Ok(Rgb(205, 49, 49)));
        assert_eq!(parse_color("Bright-Blue"), Ok(Rgb(59, 142, 234)));
        assert!(parse_color("#12").is_err());
        assert!(parse_color("#xyz").is_err());
        assert!(parse_color("chartreuse").is_err());
    }

    #[test]
    fn set_get_reset() {
        let mut t = Theme::new();
        assert!(t.get("comment").is_none());
        t.set(
            "comment",
            FaceStyle {
                fg: Some(Rgb(1, 2, 3)),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(t.get("comment").unwrap().fg, Some(Rgb(1, 2, 3)));
        assert!(t.set("nope", FaceStyle::default()).is_err());
        t.reset("comment");
        assert!(t.get("comment").is_none());
        t.set(
            "comment",
            FaceStyle {
                bold: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        t.clear();
        assert!(t.is_empty());
    }

    #[test]
    fn named_themes_define_replace_and_remove() {
        let mut ts = Themes::default();
        let mk = |fg: u8| NamedTheme {
            description: "d".into(),
            faces: vec![(
                "comment".into(),
                FaceStyle {
                    fg: Some(Rgb(fg, 0, 0)),
                    ..Default::default()
                },
            )],
        };
        assert!(ts.define("lua", "solar", mk(1)).is_ok());
        assert!(ts.define("lua", "gruvbox", mk(2)).is_ok());
        assert_eq!(ts.names(), vec!["gruvbox", "solar"]);
        assert_eq!(ts.description("solar"), Some("d"));

        // A plugin redefining a name takes it over.
        ts.define("plug", "solar", mk(9)).unwrap();
        assert!(!ts.remove_owner("lua"), "nothing applied yet");
        assert_eq!(ts.names(), vec!["solar"], "lua's own gone, plugin's kept");
        assert_eq!(ts.get("solar").unwrap().faces[0].1.fg, Some(Rgb(9, 0, 0)));

        // Unknown faces and empty names are refused at define time.
        assert!(
            ts.define(
                "lua",
                "bad",
                NamedTheme {
                    description: String::new(),
                    faces: vec![("nope".into(), FaceStyle::default())],
                }
            )
            .is_err()
        );
        assert!(ts.define("lua", "  ", mk(3)).is_err());

        ts.set_current(Some("solar".into()));
        assert_eq!(ts.current(), Some("solar"));
        assert!(ts.undefine("solar"), "it was applied");
        assert_eq!(ts.current(), None);
        assert!(ts.is_empty());
    }

    #[test]
    fn every_name_is_real() {
        // Syntax names come from the grammar faces; the rest are the
        // plain and decoration faces both frontends draw.
        assert!(all_faces().contains(&"keyword".to_string()));
        assert!(all_faces().contains(&"mode-line".to_string()));
        assert!(all_faces().contains(&"spell-error".to_string()));
        assert!(all_faces().contains(&"rainbow-depth-9".to_string()));
        assert_eq!(
            all_faces().len(),
            all_faces()
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            "no duplicates"
        );
    }
}
