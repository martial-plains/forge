// SPDX-License-Identifier: MIT OR Apache-2.0
//! Icons: `icon-preference`, `describe-icon` and `customize-icon`,
//! plus icons drawn by the host system (SF Symbols, Material, Segoe
//! Fluent, freedesktop). The registry lives in `forge_core::icons`.

use forge_core::icons::{self, IconType, Platform, Prefs, Resolved, Support};

use crate::app::App;
use crate::org_cmds::{Ask, choose};

pub const COMMANDS: &[forge_docs::CommandDoc] = &[
    forge_docs::CommandDoc::new("customize-icon", "Change how one icon looks.")
        .body("Asks for an icon, then for TYPE:VALUE, where TYPE is symbol, emoji, text, image or system: symbol:⟲, emoji:↩️, text:undo, image:/path/undo.png or system:arrow.uturn.backward (a name from the native icon set of forge.opt.icon_platform). Lasts for the session; put the same pair in forge.opt.icon_overrides in init.rhai to keep it. An empty answer restores the default."),
    forge_docs::CommandDoc::new("describe-icon", "Show every form an icon has, and which one is showing.")
        .body("Lists the text, symbol and emoji forms and the native icon of macOS, Windows and Linux, then says what forge.opt.icon_preference and forge.opt.icon_platform make Forge show."),
    forge_docs::CommandDoc::line("icon-list", "List all icons and what each shows now."),
    forge_docs::CommandDoc::line("icon-set-platform", "Choose whose native icons `system` means (macos, windows, linux, auto)."),
];

/// What the running frontend can draw.
pub fn support(app: &App) -> Support {
    if app.graphical {
        Support::GRAPHICAL
    } else {
        Support::TERMINAL
    }
}

/// Read `icon_preference`, `icon_platform` and `icon_overrides`.
pub fn apply_settings(app: &mut App) {
    let mut prefs = Prefs::default();
    let mut errors = Vec::new();
    if let Some(v) = crate::org_flow::setting_string(app, "icon_preference") {
        match Prefs::parse_preference(&v) {
            Ok(p) => prefs.preference = p,
            Err(e) => errors.push(format!("forge.opt.icon_preference: {e}")),
        }
    }
    if let Some(v) = crate::org_flow::setting_string(app, "icon_platform") {
        if !v.trim().is_empty() && !v.trim().eq_ignore_ascii_case("auto") {
            match Platform::parse(&v) {
                Some(p) => prefs.platform = Some(p),
                None => errors.push(format!(
                    "forge.opt.icon_platform must be auto, macos, windows or linux, not {v:?}"
                )),
            }
        }
    }
    let maps = app
        .rhai
        .as_ref()
        .and_then(|l| l.settings().maps.get("icon_overrides").cloned());
    for (name, value) in maps.into_iter().flatten() {
        match Prefs::parse_override(&value) {
            Ok(o) => {
                prefs.overrides.insert(name, o);
            }
            Err(e) => errors.push(format!("forge.opt.icon_overrides[{name:?}]: {e}")),
        }
    }
    app.icons = prefs;
    for e in errors {
        app.error(e);
    }
}

/// The text a tool-bar item shows as its icon in the terminal.
pub fn toolbar_glyph(app: &App, command: &str) -> Option<String> {
    let b = icons::for_command(command)?;
    app.icons.glyph(b.name, support(app))
}

/// For the native frontends: the system icon a command's button should
/// use, as `(platform, name)`, when the preference reaches `system`/`image`
/// before any glyph type; otherwise None and the button draws its glyph.
pub fn toolbar_system(app: &App, command: &str) -> Option<(Platform, String)> {
    let b = icons::for_command(command)?;
    match app.icons.resolve(b.name, support(app))? {
        Resolved::System { platform, name } => Some((platform, name)),
        _ => None,
    }
}

/// A picture file the user chose for a command's button.
pub fn toolbar_file(app: &App, command: &str) -> Option<String> {
    let b = icons::for_command(command)?;
    match app.icons.resolve(b.name, support(app))? {
        Resolved::File(p) => Some(p),
        _ => None,
    }
}

/// The glyph a native button falls back to when it can't draw the
/// resolved icon (a missing symbol, an unreadable file): the first glyph
/// type in the preference, else the built-in symbol.
pub fn toolbar_fallback(app: &App, command: &str) -> String {
    let Some(b) = icons::for_command(command) else {
        return String::new();
    };
    let mut p = app.icons.clone();
    p.preference
        .retain(|k| !matches!(k, IconType::System | IconType::Image));
    p.preference.push(IconType::Symbol);
    p.glyph(b.name, support(app))
        .unwrap_or_else(|| b.symbol.to_string())
}

fn names() -> Vec<String> {
    icons::BUILTIN.iter().map(|b| b.name.to_string()).collect()
}

pub fn execute(app: &mut App, cmd: &str) -> bool {
    match cmd {
        "describe-icon" => choose(app, "Describe icon: ", Ask::IconDescribe, names()),
        "customize-icon" => choose(app, "Customize icon: ", Ask::IconCustomize, names()),
        "icon-set-platform" => choose(
            app,
            "Icon platform: ",
            Ask::IconPlatform,
            ["auto", "macos", "windows", "linux"]
                .map(String::from)
                .to_vec(),
        ),
        "icon-list" => list(app),
        _ => return false,
    }
    true
}

fn list(app: &mut App) {
    let sup = support(app);
    let mut out = format!(
        "icon-preference: {}\nplatform: {}{}\n\n",
        app.icons
            .preference
            .iter()
            .map(|k| k.name())
            .collect::<Vec<_>>()
            .join(", "),
        app.icons.platform().name(),
        if app.icons.platform.is_none() {
            " (auto)"
        } else {
            ""
        },
    );
    for b in icons::BUILTIN {
        let shown = match app.icons.resolve(b.name, sup) {
            Some(Resolved::Glyph { kind, text }) => format!("{} {text}", kind.name()),
            Some(Resolved::File(p)) => format!("image {p}"),
            Some(Resolved::System { name, .. }) => format!("system {name}"),
            None => "-".into(),
        };
        out.push_str(&format!("{:<12} {shown}\n", b.name));
    }
    app.show_special_buffer("*Icons*", "special-mode", &out);
}

pub fn answer(app: &mut App, a: &Ask, input: &str) -> bool {
    let t = input.trim();
    match a {
        Ask::IconDescribe => {
            match icons::describe(&app.icons, t, support(app)) {
                Some(text) => app.show_special_buffer("*Help*", "help-mode", &text),
                None => app.error(format!("No icon named {t}")),
            }
            true
        }
        Ask::IconCustomize => {
            if icons::builtin(t).is_none() {
                app.error(format!("No icon named {t}"));
            } else {
                let current = app
                    .icons
                    .overrides
                    .get(t)
                    .map(icons::show_override)
                    .unwrap_or_default();
                crate::compile::prompt_with(
                    app,
                    &format!("Icon {t} (TYPE:VALUE, empty for default): "),
                    crate::minibuffer::Purpose::OrgAsk(Ask::IconValue(t.to_string())),
                    &current,
                );
            }
            true
        }
        Ask::IconValue(name) => {
            if t.is_empty() {
                app.icons.overrides.remove(name);
                app.msg(format!("Icon {name} is back to its default"));
            } else {
                match Prefs::parse_override(t) {
                    Ok(o) => {
                        app.icons.overrides.insert(name.clone(), o);
                        app.msg(format!(
                            "Icon {name} is now {t} (put it in forge.opt.icon_overrides to keep it)"
                        ));
                    }
                    Err(e) => app.error(e),
                }
            }
            true
        }
        Ask::IconPlatform => {
            if t.eq_ignore_ascii_case("auto") {
                app.icons.platform = None;
            } else if let Some(p) = Platform::parse(t) {
                app.icons.platform = Some(p);
            } else {
                app.error(format!("Unknown platform {t}"));
            }
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_sorted() {
        assert!(COMMANDS.windows(2).all(|w| w[0].name() < w[1].name()));
    }
}

#[cfg(test)]
mod app_tests {
    use super::*;

    fn app() -> App {
        let mut a = App::new(forge_core::Editor::default());
        let id = a.editor.add_buffer(forge_core::Buffer::new("*scratch*"));
        a.editor.show_buffer(id);
        a
    }

    #[test]
    fn terminal_shows_a_symbol_and_the_app_a_native_icon() {
        let mut a = app();
        a.graphical = false;
        assert_eq!(toolbar_glyph(&a, "undo").as_deref(), Some("↶"));
        assert!(toolbar_system(&a, "undo").is_none());
        a.graphical = true;
        a.icons.platform = Some(Platform::MacOs);
        let (p, n) = toolbar_system(&a, "undo").unwrap();
        assert_eq!((p, n.as_str()), (Platform::MacOs, "arrow.uturn.backward"));
        // The preference after `image` is emoji, so that is the fallback.
        assert_eq!(toolbar_fallback(&a, "undo"), "↩\u{fe0f}");
        assert!(toolbar_system(&a, "no-such-command").is_none());
    }

    #[test]
    fn customize_and_describe_run() {
        let mut a = app();
        assert!(execute(&mut a, "icon-list"));
        assert!(answer(&mut a, &Ask::IconValue("undo".into()), "emoji:⏪"));
        assert!(a.icons.overrides.contains_key("undo"));
        assert!(answer(&mut a, &Ask::IconValue("undo".into()), ""));
        assert!(a.icons.overrides.is_empty());
        assert!(answer(&mut a, &Ask::IconPlatform, "windows"));
        assert_eq!(a.icons.platform(), Platform::Windows);
        assert!(answer(&mut a, &Ask::IconDescribe, "undo"));
        assert!(a.editor.find_buffer("*Help*").is_some());
    }
}

#[cfg(test)]
mod menu_commands {
    /// Plugins supply these; their items are greyed out without the plugin.
    const FROM_PLUGINS: &[&str] = &[
        "lsp-find-definition",
        "lsp-references",
        "xref-find-apropos",
        "xref-go-back",
        "xref-go-forward",
        "spell-check-buffer",
        "spell-correct-word",
        "spell-check-mode",
    ];

    #[test]
    fn every_mode_menu_and_bar_item_runs_a_real_command() {
        let a = crate::app::App::new(forge_core::Editor::default());
        let mut missing = Vec::new();
        let mut check = |c: &str| {
            if !c.is_empty() && !a.editor.commands.contains(c) && !FROM_PLUGINS.contains(&c) {
                missing.push(c.to_string());
            }
        };
        for m in forge_core::menu::Registry::builtin().menus() {
            for it in &m.items {
                check(&it.command);
                for c in &it.submenu {
                    check(&c.command);
                }
            }
        }
        for (_, b) in forge_core::mode_menus::bars() {
            for it in &b.items {
                check(&it.command);
            }
        }
        assert!(missing.is_empty(), "no such commands: {missing:?}");
    }
}
