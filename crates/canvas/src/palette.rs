// SPDX-License-Identifier: MIT OR Apache-2.0
//! The colours a canvas is drawn in, so a board or a graph looks like the
//! rest of Forge: dark or light, in the user's theme.
//!
//! A plugin (or Forge itself) builds scenes from named roles (`card`, `line`,
//! `soft` ...) instead of hex codes. The roles come from the host
//! (`theme.palette` in the plugin API) and are made current before a scene
//! is built with [`Palette::set_current`]; code that builds scenes asks for
//! [`Palette::current`]. The default is the dark palette, so nothing changes
//! for code that never sets one.

use std::cell::RefCell;

use crate::Color;

/// Roles, each a `#rrggbb` colour.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    /// Dark rather than light: what the colours below are tuned for.
    pub dark: bool,
    /// The page behind everything.
    pub bg: String,
    /// A well set into the page (a Kanban lane).
    pub sunken: String,
    /// A card or panel on the page.
    pub card: String,
    /// A small chip on a card (a tag, a count).
    pub chip: String,
    /// Borders and outlines.
    pub line: String,
    /// Links between things, quiet until one is picked.
    pub faint: String,
    /// Links and dots that something else has taken the light from.
    pub fainter: String,
    /// Main text.
    pub fg: String,
    /// Titles and the picked thing's outline: the strongest contrast.
    pub strong: String,
    /// Secondary text.
    pub soft: String,
    /// Faint text: placeholders, labels, things that aren't there.
    pub dim: String,
    /// A tag.
    pub tag: String,
    /// Eight colours that tell things apart (folders, lanes).
    pub accents: [String; 8],
}

fn s(x: &str) -> String {
    x.to_string()
}

impl Palette {
    /// The dark palette (Forge's own look).
    pub fn dark() -> Palette {
        Palette {
            dark: true,
            bg: s("#1e1e2e"),
            sunken: s("#181825"),
            card: s("#313244"),
            chip: s("#45475a"),
            line: s("#585b70"),
            faint: s("#3b3e51"),
            fainter: s("#262837"),
            fg: s("#cdd6f4"),
            strong: s("#ffffff"),
            soft: s("#a6adc8"),
            dim: s("#7f849c"),
            tag: s("#b4befe"),
            accents: ["#89b4fa", "#f9e2af", "#a6e3a1", "#f38ba8", "#cba6f7", "#fab387", "#94e2d5", "#f5c2e7"].map(s),
        }
    }

    /// The light palette.
    pub fn light() -> Palette {
        Palette {
            dark: false,
            bg: s("#eff1f5"),
            sunken: s("#e6e9ef"),
            card: s("#ffffff"),
            chip: s("#dce0e8"),
            line: s("#bcc0cc"),
            faint: s("#ccd0da"),
            fainter: s("#e1e4ec"),
            fg: s("#4c4f69"),
            strong: s("#000000"),
            soft: s("#6c6f85"),
            dim: s("#8c8fa1"),
            tag: s("#7287fd"),
            accents: ["#1e66f5", "#df8e1d", "#40a02b", "#d20f39", "#8839ef", "#fe640b", "#179299", "#ea76cb"].map(s),
        }
    }

    /// A palette for a theme that says what its page and text look like (and
    /// maybe the colours it gives code): the surfaces are blended from those
    /// two, so cards and lines sit right on any background. `accents` fill
    /// the eight slots in order, the defaults for the mode filling the rest.
    pub fn derive(bg: Color, fg: Color, accents: &[Option<Color>]) -> Palette {
        let dark = luminance(bg) < 0.5;
        let base = if dark { Palette::dark() } else { Palette::light() };
        let mix = |t: f32| mix(bg, fg, t);
        let black = Color::new(0, 0, 0);
        let p = Palette {
            bg: bg.hex_rgb(),
            sunken: if dark { self::mix(bg, black, 0.25) } else { self::mix(bg, black, 0.04) }.hex_rgb(),
            card: mix(0.12).hex_rgb(),
            chip: mix(0.2).hex_rgb(),
            line: mix(0.3).hex_rgb(),
            faint: mix(0.15).hex_rgb(),
            fainter: mix(0.07).hex_rgb(),
            fg: fg.hex_rgb(),
            strong: if dark { s("#ffffff") } else { s("#000000") },
            soft: mix(0.75).hex_rgb(),
            dim: mix(0.5).hex_rgb(),
            ..base
        };
        p.with_accents(accents)
    }

    /// This palette with some accent slots replaced (`None` keeps a slot).
    pub fn with_accents(mut self, accents: &[Option<Color>]) -> Palette {
        for (slot, c) in self.accents.iter_mut().zip(accents) {
            if let Some(c) = c {
                *slot = c.hex_rgb();
            }
        }
        self
    }

    /// The roles as `(name, #rrggbb)` pairs, for the plugin API.
    pub fn to_pairs(&self) -> Vec<(String, String)> {
        let mut v = vec![
            ("mode".to_string(), s(if self.dark { "dark" } else { "light" })),
            ("bg".into(), self.bg.clone()),
            ("sunken".into(), self.sunken.clone()),
            ("card".into(), self.card.clone()),
            ("chip".into(), self.chip.clone()),
            ("line".into(), self.line.clone()),
            ("faint".into(), self.faint.clone()),
            ("fainter".into(), self.fainter.clone()),
            ("fg".into(), self.fg.clone()),
            ("strong".into(), self.strong.clone()),
            ("soft".into(), self.soft.clone()),
            ("dim".into(), self.dim.clone()),
            ("tag".into(), self.tag.clone()),
        ];
        v.extend(self.accents.iter().enumerate().map(|(i, c)| (format!("accent{i}"), c.clone())));
        v
    }

    /// The inverse of [`to_pairs`](Self::to_pairs); a role the pairs leave
    /// out keeps the dark (or, for a `mode` of light, the light) default.
    pub fn from_pairs(pairs: &[(String, String)]) -> Palette {
        let light = pairs.iter().any(|(k, v)| k == "mode" && v == "light");
        let mut p = if light { Palette::light() } else { Palette::dark() };
        for (k, v) in pairs {
            if Color::parse(v).is_none() {
                continue;
            }
            let slot = match k.as_str() {
                "bg" => &mut p.bg,
                "sunken" => &mut p.sunken,
                "card" => &mut p.card,
                "chip" => &mut p.chip,
                "line" => &mut p.line,
                "faint" => &mut p.faint,
                "fainter" => &mut p.fainter,
                "fg" => &mut p.fg,
                "strong" => &mut p.strong,
                "soft" => &mut p.soft,
                "dim" => &mut p.dim,
                "tag" => &mut p.tag,
                other => match other.strip_prefix("accent").and_then(|n| n.parse::<usize>().ok()).and_then(|n| p.accents.get_mut(n)) {
                    Some(a) => a,
                    None => continue,
                },
            };
            *slot = v.clone();
        }
        p
    }

    /// Every role as its own name (`@card`, `@accent2` ...): a scene built in
    /// this palette says which role a colour is, and the colour is picked
    /// when the scene is drawn ([`Scene::resolve`](crate::Scene::resolve)).
    /// That keeps a scene valid when the theme or the system's light and dark
    /// mode changes, with no plugin involved.
    pub fn tokens() -> Palette {
        let t = |n: &str| format!("@{n}");
        Palette {
            dark: true,
            bg: t("bg"),
            sunken: t("sunken"),
            card: t("card"),
            chip: t("chip"),
            line: t("line"),
            faint: t("faint"),
            fainter: t("fainter"),
            fg: t("fg"),
            strong: t("strong"),
            soft: t("soft"),
            dim: t("dim"),
            tag: t("tag"),
            accents: std::array::from_fn(|i| t(&format!("accent{i}"))),
        }
    }

    /// The colour of a role named by a token (`@card`), or None when `c` is
    /// not a token or names no role.
    pub fn lookup(&self, c: &str) -> Option<&str> {
        Some(match c.strip_prefix('@')? {
            "bg" => &self.bg,
            "sunken" => &self.sunken,
            "card" => &self.card,
            "chip" => &self.chip,
            "line" => &self.line,
            "faint" => &self.faint,
            "fainter" => &self.fainter,
            "fg" => &self.fg,
            "strong" => &self.strong,
            "soft" => &self.soft,
            "dim" => &self.dim,
            "tag" => &self.tag,
            other => self.accents.get(other.strip_prefix("accent")?.parse::<usize>().ok()?)?,
        })
    }

    /// The accent for slot `i`, wrapping round.
    pub fn accent(&self, i: usize) -> &str {
        &self.accents[i % self.accents.len()]
    }

    /// The palette scenes are being built in.
    pub fn current() -> Palette {
        CURRENT.with(|c| c.borrow().clone())
    }

    /// Make `p` the palette scenes are built in (on this thread).
    pub fn set_current(p: Palette) {
        CURRENT.with(|c| *c.borrow_mut() = p);
    }
}

impl Default for Palette {
    fn default() -> Self {
        Palette::dark()
    }
}

thread_local! {
    static CURRENT: RefCell<Palette> = RefCell::new(Palette::tokens());
}

/// Relative brightness, 0 (black) to 1 (white).
pub fn luminance(c: Color) -> f32 {
    (0.2126 * f32::from(c.r) + 0.7152 * f32::from(c.g) + 0.0722 * f32::from(c.b)) / 255.0
}

/// `a` blended `t` of the way to `b`.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round().clamp(0.0, 255.0) as u8;
    Color::new(m(a.r, b.r), m(a.g, b.g), m(a.b, b.b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_round_trip_and_odd_ones_are_ignored() {
        for p in [Palette::dark(), Palette::light()] {
            assert_eq!(Palette::from_pairs(&p.to_pairs()), p);
        }
        let odd = vec![("fg".to_string(), "not a colour".to_string()), ("mystery".to_string(), "#123456".to_string()), ("accent99".to_string(), "#123456".to_string())];
        assert_eq!(Palette::from_pairs(&odd), Palette::dark());
    }

    #[test]
    fn a_themes_page_and_text_make_surfaces_that_sit_on_it() {
        let dark = Palette::derive(Color::new(0x28, 0x2c, 0x34), Color::new(0xab, 0xb2, 0xbf), &[Some(Color::new(1, 2, 3)), None]);
        assert!(dark.dark && dark.bg == "#282c34" && dark.fg == "#abb2bf" && dark.strong == "#ffffff");
        assert_eq!(dark.accents[0], "#010203");
        assert_eq!(dark.accents[1], Palette::dark().accents[1], "unset slots keep the default");
        // Cards are lighter than the page, lines lighter still, in a dark theme.
        let l = |h: &str| luminance(Color::parse(h).unwrap());
        assert!(l(&dark.sunken) < l(&dark.bg) && l(&dark.bg) < l(&dark.card) && l(&dark.card) < l(&dark.line) && l(&dark.line) < l(&dark.fg));
        let light = Palette::derive(Color::new(0xfa, 0xfa, 0xfa), Color::new(0x38, 0x3a, 0x42), &[]);
        assert!(!light.dark && light.strong == "#000000" && light.accents == Palette::light().accents);
        assert!(l(&light.fg) < l(&light.dim) && l(&light.dim) < l(&light.line) && l(&light.line) < l(&light.bg));
    }

    #[test]
    fn the_current_palette_names_roles_until_set() {
        assert_eq!(Palette::current(), Palette::tokens());
        assert_eq!(Palette::current().card, "@card");
        Palette::set_current(Palette::light());
        assert!(!Palette::current().dark);
        Palette::set_current(Palette::tokens());
    }

    #[test]
    fn roles_resolve_to_a_palettes_colours() {
        let (t, dark, light) = (Palette::tokens(), Palette::dark(), Palette::light());
        for (token, want_d, want_l) in [(&t.bg, &dark.bg, &light.bg), (&t.card, &dark.card, &light.card), (&t.accents[3], &dark.accents[3], &light.accents[3]), (&t.tag, &dark.tag, &light.tag)] {
            assert_eq!(dark.lookup(token), Some(want_d.as_str()));
            assert_eq!(light.lookup(token), Some(want_l.as_str()));
        }
        assert_eq!(dark.lookup("#ffffff"), None);
        assert_eq!(dark.lookup("@nonsense"), None);
        assert_eq!(dark.lookup("@accent99"), None);
        let mut scene = crate::Scene::new(10.0, 10.0).background(&t.bg);
        scene.ops.push(crate::Op::rect(0.0, 0.0, 5.0, 5.0).fill(&t.card).stroked(crate::Stroke::new(&t.line, 1.0)));
        scene.ops.push(crate::Op::text(0.0, 0.0, "x", 10.0, &t.fg));
        assert!(scene.has_roles());
        let lit = scene.resolved(&light);
        assert!(!lit.has_roles() && lit.background.as_deref() == Some(light.bg.as_str()));
        assert!(!scene.resolved(&dark).has_roles());
        // Nothing to resolve: not copied.
        assert!(matches!(lit.resolved(&dark), std::borrow::Cow::Borrowed(_)));
    }
}
