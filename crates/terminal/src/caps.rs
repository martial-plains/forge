// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the terminal Forge is running in can display.
//!
//! A terminal ignores escape sequences it doesn't know, but not always
//! politely. Underline color is SGR 58, sent as `ESC [ 58 ; 5 ; N m`; a
//! terminal that has never heard of 58 reads the `5` and the `N` as
//! attributes of their own (blink, bold, italic...), so the text comes out
//! scrambled instead of getting a colored underline. Hence Forge asks
//! before using anything that only some terminals have.
//!
//! Asking the terminal itself can scramble the screen too, so this looks
//! at the environment a terminal sets up for its children (`TERM`,
//! `TERM_PROGRAM`, ...). A terminal that isn't recognized counts as "can't":
//! a wrong "can't" costs a plainer look, a wrong "can" costs a garbled
//! screen. When detection guesses wrong, say so with `FORGE_UNDERLINE_COLOR`.

/// Features of the terminal that Forge only uses when they're there.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TermCaps {
    /// SGR 58/59: an underline colored separately from its text.
    pub underline_color: bool,
    /// Inline pictures (the Kitty graphics protocol).
    pub graphics: GraphicsProto,
}

/// Inline picture protocols.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GraphicsProto {
    /// Plain `[image]` placeholders.
    #[default]
    Off,
    /// Kitty graphics (kitty, Ghostty, WezTerm): pictures draw over the
    /// blank rows the display reserves for them.
    Kitty,
}

/// `kitty` (or `1`, `true`, `yes`, `on`) forces pictures on, `off` (or
/// `0`, `false`, `no`) forces them off, in this variable. Detection
/// decides otherwise.
pub const GRAPHICS_ENV: &str = "FORGE_TERM_GRAPHICS";

/// `1`, `true`, `yes` or `on` (or `0`, `false`, `no`, `off`) in this
/// variable overrides what detection concluded about underline color.
pub const UNDERLINE_COLOR_ENV: &str = "FORGE_UNDERLINE_COLOR";

impl TermCaps {
    /// Look at this process's environment.
    pub fn detect() -> Self {
        Self::from_env(|name| std::env::var(name).ok())
    }

    /// Detect from any source of variables (tests pass their own).
    pub fn from_env(var: impl Fn(&str) -> Option<String>) -> Self {
        // A variable that's set but empty is the same as unset.
        let get = |name: &str| var(name).filter(|v| !v.is_empty());
        let forced = get(UNDERLINE_COLOR_ENV).map(|v| v.to_ascii_lowercase());
        let underline_color = match forced.as_deref() {
            Some("1" | "true" | "yes" | "on") => true,
            Some("0" | "false" | "no" | "off") => false,
            _ => detect_underline_color(&get),
        };
        let graphics = match get(GRAPHICS_ENV).map(|v| v.to_ascii_lowercase()).as_deref() {
            Some("kitty" | "1" | "true" | "yes" | "on") => GraphicsProto::Kitty,
            Some("off" | "0" | "false" | "no") => GraphicsProto::Off,
            _ => detect_graphics(&get),
        };
        Self {
            underline_color,
            graphics,
        }
    }
}

/// Do pictures via the Kitty protocol: kitty itself, Ghostty and
/// WezTerm speak it. A multiplexer is "can't" like above (tmux passes
/// graphics through, but only when told to, so force it there).
/// Terminal.app, iTerm2, foot and VTE stay placeholders for now.
fn detect_graphics(get: &dyn Fn(&str) -> Option<String>) -> GraphicsProto {
    let term = get("TERM").unwrap_or_default();
    if term.starts_with("tmux") || term.starts_with("screen") {
        return GraphicsProto::Off;
    }
    let program = get("TERM_PROGRAM").unwrap_or_default();
    match program.as_str() {
        "kitty" | "ghostty" | "WezTerm" => return GraphicsProto::Kitty,
        _ => {}
    }
    if term.contains("kitty") || term.contains("ghostty") || get("KITTY_WINDOW_ID").is_some() {
        return GraphicsProto::Kitty;
    }
    GraphicsProto::Off
}

fn detect_underline_color(get: &dyn Fn(&str) -> Option<String>) -> bool {
    let term = get("TERM").unwrap_or_default();
    let program = get("TERM_PROGRAM").unwrap_or_default();

    // A multiplexer inherits the variables of the terminal it was first
    // started in, and decides for itself what gets through to the terminal
    // it's in now. Can't tell from in here; FORGE_UNDERLINE_COLOR can.
    if term.starts_with("tmux") || term.starts_with("screen") {
        return false;
    }

    match program.as_str() {
        // Terminal.app can't color an underline apart from its text.
        "Apple_Terminal" => return false,
        "WezTerm" | "ghostty" => return true,
        // iTerm2 learned SGR 58 in 3.4.
        "iTerm.app" => {
            return version_at_least(&get("TERM_PROGRAM_VERSION").unwrap_or_default(), (3, 4));
        }
        _ => {}
    }

    if term.contains("kitty") || get("KITTY_WINDOW_ID").is_some() {
        return true;
    }
    if term == "wezterm"
        || term.contains("ghostty")
        || term.starts_with("foot")
        || term.starts_with("alacritty")
    {
        return true;
    }
    // VTE (GNOME Terminal, Tilix, ...) since 0.52, which VTE_VERSION spells 5200.
    get("VTE_VERSION")
        .and_then(|v| v.parse::<u32>().ok())
        .is_some_and(|v| v >= 5200)
}

/// Is a `major.minor[.patch...]` version at least `min`? A version that
/// can't be read counts as new enough: the terminal named itself, so the
/// likelier mistake is an odd version string, not an ancient terminal.
fn version_at_least(version: &str, min: (u32, u32)) -> bool {
    let mut parts = version.split('.').map(|p| p.trim().parse::<u32>());
    match (parts.next(), parts.next()) {
        (Some(Ok(major)), Some(Ok(minor))) => (major, minor) >= min,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(vars: &[(&str, &str)]) -> TermCaps {
        TermCaps::from_env(|name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        })
    }

    #[test]
    fn kitty_terminals_get_graphics() {
        assert_eq!(
            caps(&[("TERM", "xterm-kitty")]).graphics,
            GraphicsProto::Kitty
        );
        assert_eq!(
            caps(&[("KITTY_WINDOW_ID", "3")]).graphics,
            GraphicsProto::Kitty
        );
        assert_eq!(
            caps(&[("TERM_PROGRAM", "ghostty")]).graphics,
            GraphicsProto::Kitty
        );
        assert_eq!(
            caps(&[("TERM_PROGRAM", "WezTerm")]).graphics,
            GraphicsProto::Kitty
        );
        assert_eq!(
            caps(&[("TERM_PROGRAM", "kitty")]).graphics,
            GraphicsProto::Kitty
        );
        assert_eq!(
            caps(&[("TERM", "xterm-ghostty")]).graphics,
            GraphicsProto::Kitty
        );
    }

    #[test]
    fn plain_terminals_keep_placeholders() {
        assert_eq!(caps(&[]).graphics, GraphicsProto::Off);
        assert_eq!(
            caps(&[("TERM", "xterm-256color")]).graphics,
            GraphicsProto::Off
        );
        assert_eq!(
            caps(&[("TERM_PROGRAM", "Apple_Terminal")]).graphics,
            GraphicsProto::Off
        );
        assert_eq!(
            caps(&[("TERM_PROGRAM", "iTerm.app")]).graphics,
            GraphicsProto::Off
        );
        assert_eq!(caps(&[("TERM", "foot")]).graphics, GraphicsProto::Off);
        // tmux passes graphics through, but only when told to.
        assert_eq!(
            caps(&[("TERM", "tmux-256color"), ("KITTY_WINDOW_ID", "1")]).graphics,
            GraphicsProto::Off
        );
    }

    #[test]
    fn the_graphics_override_wins() {
        for yes in ["kitty", "1", "true", "YES", "On"] {
            let vars = [("TERM_PROGRAM", "Apple_Terminal"), (GRAPHICS_ENV, yes)];
            assert_eq!(caps(&vars).graphics, GraphicsProto::Kitty, "{yes}");
        }
        for no in ["off", "0", "false", "No", "OFF"] {
            let vars = [("TERM", "xterm-kitty"), (GRAPHICS_ENV, no)];
            assert_eq!(caps(&vars).graphics, GraphicsProto::Off, "{no}");
        }
        assert_eq!(
            caps(&[("TERM", "xterm-kitty"), (GRAPHICS_ENV, "")]).graphics,
            GraphicsProto::Kitty
        );
        assert_eq!(
            caps(&[("TERM_PROGRAM", "Apple_Terminal"), (GRAPHICS_ENV, "maybe")]).graphics,
            GraphicsProto::Off
        );
    }

    #[test]
    fn unknown_terminals_get_no_underline_color() {
        assert!(!caps(&[]).underline_color);
        assert!(!caps(&[("TERM", "xterm-256color")]).underline_color);
        assert!(!caps(&[("TERM", "dumb")]).underline_color);
        assert!(!caps(&[("TERM", "linux")]).underline_color);
    }

    #[test]
    fn terminal_app_gets_none() {
        let vars = [
            ("TERM", "xterm-256color"),
            ("TERM_PROGRAM", "Apple_Terminal"),
        ];
        assert!(!caps(&vars).underline_color);
    }

    #[test]
    fn known_terminals_get_it() {
        assert!(caps(&[("TERM", "xterm-kitty")]).underline_color);
        assert!(caps(&[("TERM", "xterm-256color"), ("KITTY_WINDOW_ID", "3")]).underline_color);
        assert!(caps(&[("TERM", "xterm-256color"), ("TERM_PROGRAM", "WezTerm")]).underline_color);
        assert!(caps(&[("TERM", "xterm-256color"), ("TERM_PROGRAM", "ghostty")]).underline_color);
        assert!(caps(&[("TERM", "xterm-ghostty")]).underline_color);
        assert!(caps(&[("TERM", "foot")]).underline_color);
        assert!(caps(&[("TERM", "alacritty")]).underline_color);
    }

    #[test]
    fn iterm_needs_3_4() {
        let iterm = |v: &str| {
            caps(&[
                ("TERM", "xterm-256color"),
                ("TERM_PROGRAM", "iTerm.app"),
                ("TERM_PROGRAM_VERSION", v),
            ])
        };
        assert!(iterm("3.4.19").underline_color);
        assert!(iterm("3.5.0").underline_color);
        assert!(iterm("4.0.0").underline_color);
        assert!(!iterm("3.3.12").underline_color);
        assert!(!iterm("2.9.20").underline_color);
        // No readable version: trust the name.
        assert!(iterm("").underline_color);
        assert!(iterm("3.5beta").underline_color);
    }

    #[test]
    fn vte_needs_0_52() {
        assert!(caps(&[("TERM", "xterm-256color"), ("VTE_VERSION", "7600")]).underline_color);
        assert!(caps(&[("TERM", "xterm-256color"), ("VTE_VERSION", "5200")]).underline_color);
        assert!(!caps(&[("TERM", "xterm-256color"), ("VTE_VERSION", "4800")]).underline_color);
        assert!(!caps(&[("TERM", "xterm-256color"), ("VTE_VERSION", "junk")]).underline_color);
    }

    #[test]
    fn multiplexers_are_not_assumed() {
        // tmux in kitty: kitty's variables leak through, but tmux decides.
        let in_tmux = [
            ("TERM", "tmux-256color"),
            ("KITTY_WINDOW_ID", "1"),
            ("TERM_PROGRAM", "tmux"),
        ];
        assert!(!caps(&in_tmux).underline_color);
        assert!(!caps(&[("TERM", "screen-256color"), ("KITTY_WINDOW_ID", "1")]).underline_color);
    }

    #[test]
    fn the_override_wins_either_way() {
        let terminal_app = [("TERM_PROGRAM", "Apple_Terminal")];
        for yes in ["1", "true", "YES", "On"] {
            let vars = [terminal_app[0], (UNDERLINE_COLOR_ENV, yes)];
            assert!(caps(&vars).underline_color, "{yes}");
        }
        for no in ["0", "false", "No", "OFF"] {
            let vars = [("TERM", "xterm-kitty"), (UNDERLINE_COLOR_ENV, no)];
            assert!(!caps(&vars).underline_color, "{no}");
        }
        // Set but empty, or gibberish: detection decides.
        assert!(caps(&[("TERM", "xterm-kitty"), (UNDERLINE_COLOR_ENV, "")]).underline_color);
        assert!(caps(&[("TERM", "xterm-kitty"), (UNDERLINE_COLOR_ENV, "maybe")]).underline_color);
        assert!(!caps(&[terminal_app[0], (UNDERLINE_COLOR_ENV, "maybe")]).underline_color);
    }
}
