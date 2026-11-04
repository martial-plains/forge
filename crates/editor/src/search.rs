// SPDX-License-Identifier: MIT OR Apache-2.0
//! The default web search engine, for webkit and few. Set it in init.rhai
//! (`search_engine` for both, `webkit_search_engine` and
//! `few_search_engine` for one) or with a command, for this session.
//! A value is the name of a known engine (`google`, `brave`, ...) or an
//! address with `%s` where the words go.

use crate::app::App;
use crate::org_cmds::{Ask, choose};
use forge_core::few::{self as engine, ENGINES};

pub const COMMANDS: &[forge_docs::CommandDoc] = &[
    forge_docs::CommandDoc::line(
        "set-search-engine",
        "Set the search engine for webkit and few (this session).",
    ),
    forge_docs::CommandDoc::line(
        "webkit-set-search-engine",
        "Webkit: set the search engine (this session).",
    ),
    forge_docs::CommandDoc::line(
        "few-set-search-engine",
        "Few: set the search engine (this session).",
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Webkit,
    Few,
}

/// What the commands have set. A value wins over init.rhai.
#[derive(Default)]
pub struct Engines {
    pub all: Option<String>,
    pub webkit: Option<String>,
    pub few: Option<String>,
}

fn setting(app: &App, key: &str) -> Option<String> {
    let s = app.rhai.as_ref()?.settings();
    s.strings
        .get(key)
        .and_then(|v| v.first())
        .filter(|v| !v.trim().is_empty())
        .cloned()
}

/// The engine in force for `t`, as written (a name or an address).
pub fn spec(app: &App, t: Target) -> Option<String> {
    let (own, key) = match t {
        Target::Webkit => (&app.search.webkit, "webkit_search_engine"),
        Target::Few => (&app.search.few, "few_search_engine"),
    };
    own.clone()
        .or_else(|| app.search.all.clone())
        .or_else(|| setting(app, key))
        .or_else(|| setting(app, "search_engine"))
}

/// The address to search for `q`.
pub fn url_for(app: &App, t: Target, q: &str) -> String {
    let text = t == Target::Few;
    match spec(app, t).and_then(|s| engine::engine_template(&s, text)) {
        Some(tmpl) => engine::search_with(&tmpl, q),
        None => match engine::engine_template("duckduckgo", text) {
            Some(tmpl) => engine::search_with(&tmpl, q),
            None => engine::search_url(q),
        },
    }
}

fn which(cmd: &str) -> u8 {
    match cmd {
        "webkit-set-search-engine" => 1,
        "few-set-search-engine" => 2,
        _ => 0,
    }
}

/// Run a search-engine command. False if `cmd` isn't one.
pub fn command(app: &mut App, cmd: &str) -> bool {
    if !COMMANDS.iter().any(|cd| cd.name() == cmd) {
        return false;
    }
    let mut names: Vec<String> = ENGINES.iter().map(|(n, ..)| n.to_string()).collect();
    names.push("default".to_string());
    let prompt = match which(cmd) {
        1 => "Webkit search engine (name, or address with %s): ",
        2 => "few search engine (name, or address with %s): ",
        _ => "Search engine for webkit and few (name, or address with %s): ",
    };
    choose(app, prompt, Ask::SearchEngine(which(cmd)), names);
    true
}

/// The prompt answered in `Ask::SearchEngine`.
pub fn answer(app: &mut App, a: &Ask, input: &str) -> bool {
    let Ask::SearchEngine(w) = a else {
        return false;
    };
    let t = input.trim();
    let value = if t.is_empty() || t.eq_ignore_ascii_case("default") {
        None
    } else if engine::engine_template(t, false).is_some() {
        Some(t.to_string())
    } else {
        app.error(format!("Not a search engine: {t}"));
        return true;
    };
    let slot = match w {
        1 => &mut app.search.webkit,
        2 => &mut app.search.few,
        _ => {
            app.search.webkit = None;
            app.search.few = None;
            &mut app.search.all
        }
    };
    *slot = value.clone();
    let what = match w {
        1 => "Webkit",
        2 => "few",
        _ => "Webkit and few",
    };
    app.msg(match value {
        Some(v) => format!("{what} search engine: {v}"),
        None => format!("{what} search engine: default"),
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_templates_and_defaults() {
        let mut app = App::new(forge_core::Editor::default());
        let wk = url_for(&app, Target::Webkit, "a b");
        assert_eq!(wk, "https://duckduckgo.com/?q=a+b");
        assert!(url_for(&app, Target::Few, "a b").contains("html.duckduckgo.com"));
        app.minibuffer = None;
        assert!(answer(&mut app, &Ask::SearchEngine(0), "brave"));
        assert!(url_for(&app, Target::Few, "x").starts_with("https://search.brave.com/"));
        // One target overrides the shared one.
        assert!(answer(
            &mut app,
            &Ask::SearchEngine(1),
            "https://s.example/?term=%s&x=1"
        ));
        assert_eq!(
            url_for(&app, Target::Webkit, "hi there"),
            "https://s.example/?term=hi+there&x=1"
        );
        assert!(url_for(&app, Target::Few, "x").starts_with("https://search.brave.com/"));
        // Nonsense changes nothing; "default" clears.
        assert!(answer(&mut app, &Ask::SearchEngine(2), "nosuch"));
        assert!(answer(&mut app, &Ask::SearchEngine(1), "default"));
        assert!(url_for(&app, Target::Webkit, "x").starts_with("https://search.brave.com/"));
        assert!(answer(&mut app, &Ask::SearchEngine(0), "default"));
        assert!(url_for(&app, Target::Webkit, "x").starts_with("https://duckduckgo.com/"));
    }
}
