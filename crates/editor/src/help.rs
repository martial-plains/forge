// SPDX-License-Identifier: MIT OR Apache-2.0
//! The C-h commands: describe-key, describe-function, describe-symbol,
//! where-is, describe-bindings, describe-mode.

use crate::app::App;
use forge_core::{ActiveKeymaps, LinkSpan, LinkTarget, Lookup, Scope};

pub use forge_core::display::mode_display_name;

/// Is `c` part of a command name (the characters command names are made of)?
fn is_name_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'
}

/// The link layer for a help page: a link for every command name that
/// appears as a word in `text`, plus one for `manual` (the node the
/// documented command belongs to), when it appears.
fn links_in(app: &App, text: &str, manual: Option<&str>) -> Vec<LinkSpan> {
    let chars: Vec<char> = text.chars().collect();
    let mut links = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !is_name_char(chars[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && is_name_char(chars[i]) {
            i += 1;
        }
        let word: String = chars[start..i].iter().collect();
        if app.editor.commands.contains(&word) {
            links.push(LinkSpan::new(start, i, LinkTarget::Symbol(word)));
        }
    }
    if let Some(node) = manual {
        let n: Vec<char> = node.chars().collect();
        if !n.is_empty() {
            for j in 0..=chars.len().saturating_sub(n.len()) {
                if chars[j..j + n.len()] == n[..]
                    && (j == 0 || chars[j - 1].is_whitespace())
                    && (j + n.len() == chars.len() || chars[j + n.len()].is_whitespace())
                {
                    links.push(LinkSpan::new(
                        j,
                        j + n.len(),
                        LinkTarget::Node(node.to_string()),
                    ));
                    break;
                }
            }
        }
    }
    links
}

/// Show a help page, filling in its link layer. `manual` is the node the
/// page's subject belongs to, if any.
fn show(app: &mut App, text: &str, manual: Option<&str>) {
    let links = links_in(app, text, manual);
    app.show_help(text, links);
}

/// The manual node the command's doc points at, if any.
fn manual_node(app: &App, command: &str) -> Option<String> {
    app.docs
        .get(forge_docs::Kind::Command, command)
        .and_then(|d| d.manual.as_ref())
        .map(|n| n.0.clone())
}

pub fn doc_of(app: &App, command: &str) -> String {
    if let Some(d) = app.docs.get(forge_docs::Kind::Command, command) {
        let mut out = forge_docs::render_text(d);
        match &d.source {
            forge_docs::Source::Plugin(p) => out.push_str(&format!("\n\nProvided by plugin {p}.")),
            forge_docs::Source::Config => out.push_str("\n\nDefined in init.rhai."),
            forge_docs::Source::Builtin | forge_docs::Source::Manual => {}
        }
        return out;
    }
    match app.editor.commands.get(command) {
        Some(c) => match &c.source {
            forge_core::CommandSource::Plugin(p) => format!("{}\n\nProvided by plugin {p}.", c.doc),
            forge_core::CommandSource::Config => format!("{}\n\nDefined in init.rhai.", c.doc),
            forge_core::CommandSource::Builtin => c.doc.clone(),
        },
        None => "Not documented (this command is not defined).".to_string(),
    }
}

/// The first line of a command's documentation.
pub fn first_line(doc: &str) -> String {
    doc.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Where a command is in the menus: "File > Save", one per place.
pub fn menu_paths(app: &App, command: &str) -> Vec<String> {
    let mut out = Vec::new();
    for m in app.editor.menus.menus() {
        for it in &m.items {
            if it.command == command {
                out.push(format!("{} > {}", m.title, it.label));
            }
            for c in &it.submenu {
                if c.command == command {
                    out.push(format!("{} > {} > {}", m.title, it.label, c.label));
                }
            }
        }
    }
    out
}

/// Every key bound to `command` in the active keymaps, with the keymap
/// each is in: `("C-x C-s", "global-map")`.
fn keys_by_map(app: &App, command: &str) -> Vec<(String, String)> {
    let maps = app.editor.active_keymaps_now();
    let minors = app.buf_ref().minor_modes().to_vec();
    let mut out = Vec::new();
    for i in 0..maps.maps.len() {
        for (keys, cmd) in maps.maps[i].map.bindings() {
            if cmd == command && maps.is_effective(i, keys) {
                let scope = match &maps.maps[i].scope {
                    Scope::Global => "global-map".to_string(),
                    Scope::Mode(m) if minors.contains(m) => format!("{m}-map"),
                    Scope::Mode(m) => format!("{m}-map"),
                    other => other.label(),
                };
                out.push((keys.to_string(), scope));
            }
        }
    }
    // Modes that are not on here still bind it: say where, so a Dired
    // command read about from elsewhere does not look unbound.
    let on: Vec<String> = maps
        .maps
        .iter()
        .filter_map(|m| match &m.scope {
            Scope::Mode(n) => Some(n.clone()),
            _ => None,
        })
        .collect();
    for mode in app.editor.keymaps.mode_names() {
        if on.iter().any(|n| n == mode) {
            continue;
        }
        if let Some(km) = app.editor.keymaps.get(&Scope::Mode(mode.to_string())) {
            for (keys, cmd) in km.bindings() {
                if cmd == command {
                    out.push((keys.to_string(), format!("{mode}-map, when in {mode}")));
                }
            }
        }
    }
    out
}

/// Commands whose names start like this one's, for "Related commands".
fn related(app: &App, command: &str) -> Vec<String> {
    let parts: Vec<&str> = command.split('-').collect();
    // The longest prefix of whole words that at least two others share,
    // never just one short word like "describe" alone for huge families.
    for n in (1..parts.len()).rev() {
        let prefix = format!("{}-", parts[..n].join("-"));
        let mut hits: Vec<String> = app
            .editor
            .commands
            .names()
            .filter(|c| c.starts_with(&prefix) && *c != command)
            .map(String::from)
            .collect();
        if hits.len() >= 2 {
            hits.sort();
            hits.truncate(8);
            return hits;
        }
    }
    Vec::new()
}

/// The sections shared by C-h f and C-h k: where it is, how to run it,
/// what it does, and what is like it.
fn command_sections(app: &App, command: &str) -> String {
    let mut t = String::new();
    let keys = keys_by_map(app, command);
    t.push_str("Key bindings:\n");
    if keys.is_empty() {
        t.push_str(&format!(
            "  Not bound to any key. Run it with M-x {command} RET.\n"
        ));
    } else {
        for (k, map) in &keys {
            t.push_str(&format!("  {k:<18} ({map})\n"));
        }
    }
    let maps = app.editor.active_keymaps_now();
    if let Some(to) = maps.remap(command) {
        t.push_str(&format!(
            "  Remapped to {to}, so its keys run {to} instead.\n"
        ));
    }
    let from = maps.remapped_from(command);
    if !from.is_empty() {
        t.push_str(&format!(
            "  Runs instead of {} (remapped).\n",
            from.join(", ")
        ));
    }
    drop(maps);
    let menus = menu_paths(app, command);
    if !menus.is_empty() {
        t.push_str("\nMenu:\n");
        for m in menus {
            t.push_str(&format!("  {m}\n"));
        }
    }
    t.push_str("\nDocumentation:\n");
    for l in doc_of(app, command).lines() {
        if l.is_empty() {
            t.push('\n');
        } else {
            t.push_str(&format!("  {l}\n"));
        }
    }
    let rel = related(app, command);
    if !rel.is_empty() {
        t.push_str("\nRelated commands:\n");
        for r in rel {
            let doc = app
                .editor
                .commands
                .get(&r)
                .map(|c| first_line(&c.doc))
                .unwrap_or_default();
            t.push_str(&format!("  {r:<30} {doc}\n"));
        }
    }
    if let Some(d) = app.docs.get(forge_docs::Kind::Command, command) {
        if !d.see_also.is_empty() {
            t.push_str("\nSee also:\n");
            for r in &d.see_also {
                let doc = app
                    .editor
                    .commands
                    .get(&r.name)
                    .map(|c| first_line(&c.doc))
                    .unwrap_or_default();
                t.push_str(&format!("  {:<30} {doc}\n", r.name));
            }
        }
        if let Some(node) = &d.manual {
            t.push_str(&format!("\nManual:\n  {node}\n"));
        }
    }
    t
}

/// How a command got here, for the first line.
fn provenance(app: &App, command: &str) -> String {
    match app.editor.commands.get(command).map(|c| &c.source) {
        Some(forge_core::CommandSource::Plugin(p)) => format!("provided by the {p} plugin"),
        Some(forge_core::CommandSource::Config) => "defined in init.rhai".to_string(),
        _ => "built into Forge".to_string(),
    }
}

/// C-h k: show the command a key sequence runs.
pub fn describe_key(app: &mut App, keys: &str, lookup: &Lookup) {
    let (text, command) = match lookup {
        Lookup::Command {
            command,
            scope,
            remapped_from,
        } => {
            let mut t = format!(
                "{keys} runs the command {command} (found in {})",
                scope.label()
            );
            if let Some(from) = remapped_from {
                t.push_str(&format!(",\nwhich was remapped from {from}"));
            }
            t.push_str(&format!(", {}.\n\n", provenance(app, command)));
            t.push_str(&command_sections(app, command));
            (t, command.as_str())
        }
        _ => (
            format!(
                "{keys} runs the command self-insert-command (the default for printable keys).\n\n{}",
                command_sections(app, "self-insert-command")
            ),
            "self-insert-command",
        ),
    };
    let manual = manual_node(app, command);
    show(app, &text, manual.as_deref());
}

/// C-h f: documentation and keys for one command.
pub fn describe_function(app: &mut App, command: &str) {
    let text = function_text(app, command);
    let manual = manual_node(app, command);
    show(app, &text, manual.as_deref());
}

/// The C-h f text, for describe-symbol to reuse.
fn function_text(app: &App, command: &str) -> String {
    format!(
        "{command} is an interactive command, {}.\n\n{}",
        provenance(app, command),
        command_sections(app, command)
    )
}

/// C-h w: one line for the echo area.
pub fn where_is_message(app: &App, command: &str) -> String {
    let maps = app.editor.active_keymaps_now();
    if let Some(to) = maps.remap(command) {
        let keys = maps.where_is(to);
        return if keys.is_empty() {
            format!("{command} is remapped to {to}, which is not on any key")
        } else {
            format!(
                "{command} is remapped to {to}, which is on {}",
                keys.join(", ")
            )
        };
    }
    let keys = maps.where_is(command);
    if keys.is_empty() {
        format!("{command} is not on any key")
    } else {
        format!("{command} is on {}", keys.join(", "))
    }
}

/// Heading for one keymap in C-h b / C-h m. Ends in a colon, which is
/// how help-mode knows it is a heading.
fn section_title(scope: &Scope, minors: &[String]) -> String {
    let t = match scope {
        Scope::Global => "Global bindings (global-map)".to_string(),
        Scope::Buffer(_) => "Buffer-local bindings (local-set-key)".to_string(),
        Scope::Mode(m) if minors.contains(m) => format!("Minor mode bindings ({m}-map)"),
        Scope::Mode(m) => format!("Major mode bindings ({m}-map)"),
        Scope::Transient(name) => format!("Transient bindings ({name})"),
        Scope::Prompt(name) => format!("Prompt bindings ({name})"),
        Scope::Layer(name) => format!("Conditional layer ({name})"),
        Scope::Region(name) => format!("Region ({name})"),
        Scope::Project(root) => format!("Project ({})", root.display()),
    };
    format!("{t}:")
}

/// The effective (not shadowed) bindings and remaps of active map `index`,
/// as (keys, command) pairs.
fn map_entries(maps: &ActiveKeymaps, index: usize) -> Vec<(String, String)> {
    let map = maps.maps[index].map;
    let mut out = Vec::new();
    for (keys, cmd) in map.bindings() {
        if maps.is_effective(index, keys) {
            out.push((keys.to_string(), cmd.to_string()));
        }
    }
    for (from, to) in map.remaps() {
        out.push((format!("[remap {from}]"), to.to_string()));
    }
    out
}

/// A table of one map's bindings: key, command, and (with `docs`) the
/// first line of what the command does.
fn map_listing(app: &App, maps: &ActiveKeymaps, index: usize, docs: bool) -> String {
    let mut text = String::new();
    for (keys, cmd) in map_entries(maps, index) {
        if docs {
            let doc = app
                .editor
                .commands
                .get(&cmd)
                .map(|c| first_line(&c.doc))
                .unwrap_or_default();
            let doc = if doc.chars().count() > 60 {
                format!("{}...", doc.chars().take(57).collect::<String>())
            } else {
                doc
            };
            text.push_str(&format!("  {keys:<16} {cmd:<28} {doc}\n"));
        } else {
            text.push_str(&format!("  {keys:<18} {cmd}\n"));
        }
    }
    text
}

/// C-h b: every active binding, one section per keymap, highest priority first.
pub fn describe_bindings(app: &mut App) {
    let b = app.buf_ref();
    let (name, major, minors) = (
        b.name().to_string(),
        b.major_mode().to_string(),
        b.minor_modes().to_vec(),
    );
    let maps = app.editor.active_keymaps_now();

    let mut body = String::new();
    let mut total = 0;
    let mut sections = 0;
    for i in 0..maps.maps.len() {
        let n = map_entries(&maps, i).len();
        if n == 0 {
            continue;
        }
        total += n;
        sections += 1;
        let mut title = section_title(&maps.maps[i].scope, &minors);
        // A layer's bindings are only there while its condition holds, so
        // say which condition: otherwise the section reads as if it were
        // always on.
        if let forge_core::Scope::Layer(name) = &maps.maps[i].scope
            && let Some(when) = app.editor.keymaps.layer_condition(name)
        {
            title = format!("{}, when {}:", title.trim_end_matches(':'), when.label());
        }
        body.push_str(&format!("\n{title}\n"));
        body.push_str("  key                command\n  ---                -------\n");
        body.push_str(&map_listing(app, &maps, i, false));
    }
    let mut text = format!(
        "Key bindings in {name} ({})\n\n{total} bindings in {sections} keymaps are active here, highest priority first.\nShadowed bindings are left out.\n\nTips:\n  C-h k KEY       what a key does          C-h f COMMAND   what a command does\n  C-h w COMMAND   which keys run it        C-h m           this buffer's modes and keys\n  C-x C-h         list the keys after a prefix (C-h works after any prefix key)\n",
        mode_display_name(&major)
    );
    text.push_str(&body);
    text.push_str(
        "\nNotes:\n  Any other printable key inserts itself.\n  ESC works as a Meta prefix: ESC f is the same as M-f.\n  C-c followed by a letter is reserved for your own bindings: put\n  forge.keymap.set(...) in init.rhai, or try M-x global-set-key.",
    );
    drop(maps);
    show(app, &text, None);
}

/// What a built-in mode is for.
/// C-h m: the buffer's modes, what they are for, and the keys each adds.
pub fn describe_mode(app: &mut App) {
    let b = app.buf_ref();
    let (major, minors) = (b.major_mode().to_string(), b.minor_modes().to_vec());
    let maps = app.editor.active_keymaps_now();

    let mut text = format!(
        "{} mode ({major}) is the major mode of this buffer.\n",
        mode_display_name(&major)
    );
    let describe = |app: &App, name: &str| -> String {
        let mut t = String::new();
        if let Some(d) = app.docs.get(forge_docs::Kind::Mode, name) {
            let rendered = forge_docs::render_text(d);
            if !rendered.is_empty() {
                t.push_str(&format!("\n{rendered}\n"));
            }
        }
        if let Some(def) = app.editor.modes.get(name) {
            if let Some(parent) = &def.parent {
                t.push_str(&format!(
                    "  Derived from {parent}, so its keys apply too.\n"
                ));
            }
            if !def.extensions.is_empty() {
                let e: Vec<String> = def.extensions.iter().map(|e| format!("*.{e}")).collect();
                t.push_str(&format!("  Opens {} files.\n", e.join(" ")));
            }
            if let Some(owner) = &def.owner {
                t.push_str(&format!("  Provided by the {owner} plugin.\n"));
            }
        }
        t
    };
    text.push_str(&describe(app, &major));
    text.push_str("\nMinor modes:\n");
    if minors.is_empty() {
        text.push_str("  None are enabled in this buffer.\n");
    }
    for m in &minors {
        let first = app
            .docs
            .get(forge_docs::Kind::Mode, m)
            .map(|d| forge_docs::render::summary_text(d).to_string())
            .unwrap_or_else(|| "A minor mode.".to_string());
        text.push_str(&format!("  {m:<28} {first}\n"));
    }
    for (i, m) in maps.maps.iter().enumerate() {
        if matches!(m.scope, Scope::Global) {
            continue;
        }
        let listing = map_listing(app, &maps, i, true);
        if !listing.is_empty() {
            text.push_str(&format!(
                "\n{}\n{listing}",
                section_title(&m.scope, &minors)
            ));
        }
    }
    if maps.maps.len() == 1 {
        text.push_str("\nThis mode adds no key bindings; the global ones apply (C-h b).\n");
    } else {
        text.push_str("\nThe global keys apply too: C-h b lists them.\n");
    }
    drop(maps);
    show(app, &text, None);
}

/// C-h C-h / C-h ?: what the C-h keys are.
pub fn help_for_help(app: &mut App) {
    let text = "Help commands (type C-h and then one of these):\n\n\
Find out about a key or command:\n  \
C-h k   describe-key        what a key does, and its command's documentation\n  \
C-h c   describe-key-briefly   the same in one line\n  \
C-h f   describe-function   a command: keys, menu entry, documentation\n  \
C-h w   where-is            which keys run a command\n  \
C-h o   describe-symbol     a command, option, mode or face\n  \
C-h v   describe-variable   an option (same as C-h o)\n  \
C-h a   apropos-command     commands whose name matches\n  \
C-h d   apropos-documentation   commands whose documentation matches\n\n\
Find out about the buffer:\n  \
C-h m   describe-mode       its modes, what they are for and their keys\n  \
C-h b   describe-bindings   every key active here\n  \
C-h l   view-lossage        the last keys you typed\n  \
C-h n   view-news           the newest release notes\n\n\
After a prefix key (C-x, C-c, M-g...), C-h lists the keys that can follow it.\n\
In the menu bar (F10), each item shows its key; M-x runs any command by name.\n";
    show(app, text, None);
}

/// C-h l: the last keys typed.
pub fn view_lossage(app: &mut App) {
    let keys: Vec<&str> = app.lossage.iter().map(String::as_str).collect();
    let text = format!(
        "Recent input (oldest first, last {} keys):\n\n{}\n",
        keys.len(),
        keys.join(" ")
    );
    show(app, &text, None);
}

/// C-h n: the newest release's section of NEWS.org, with its links live.
pub fn view_news(app: &mut App) {
    match forge_docs::newest_release() {
        Some(release) => {
            let text = format!(
                "Forge NEWS — {}\n\n{}",
                release.title,
                forge_docs::render_body(&release.body)
            );
            show(app, &text, None);
        }
        None => app.msg("No NEWS file is available for this build".to_string()),
    }
}

/// C-h a / C-h d: commands whose name (or documentation) matches every
/// word of `query`, or the regexp.
pub fn apropos(app: &mut App, query: &str, docs: bool) {
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    let re = regex::Regex::new(&format!("(?i){query}")).ok();
    let mut hits: Vec<(String, String)> = Vec::new();
    // The registry indexes answer the word-list form of the query; the
    // regexp form (which an index cannot express) falls back to a scan of
    // the candidate text. Both may contribute, so a query that is both a
    // word list and a valid regexp still finds everything either would.
    let indexed: std::collections::BTreeSet<String> = if docs {
        app.docs
            .apropos(query)
            .into_iter()
            .map(|d| d.name.clone())
            .collect()
    } else {
        app.docs
            .apropos_names(query)
            .into_iter()
            .map(|d| d.name.clone())
            .collect()
    };
    // The registry is the source of truth for command docs; this only
    // mirrors it into the flat listing the *Help* buffer shows.
    for d in app.docs.commands() {
        let name = d.name.as_str();
        let hay = if docs {
            format!("{}\n{}", d.summary, forge_docs::body_text(d))
        } else {
            name.to_string()
        };
        let hay_lower = hay.to_lowercase();
        let word_hit =
            !docs && !words.is_empty() && words.iter().all(|w| hay_lower.contains(w.as_str()));
        let re_hit = re.as_ref().is_some_and(|r| r.is_match(&hay));
        if indexed.contains(name) || word_hit || re_hit {
            let summary = if docs {
                first_line(&hay)
            } else {
                d.summary.clone()
            };
            hits.push((name.to_string(), summary));
        }
    }
    hits.sort();
    if hits.is_empty() {
        app.error(format!("No commands match {query}"));
        return;
    }
    let maps = app.editor.active_keymaps_now();
    let mut text = format!(
        "Commands whose {} matches \"{query}\" ({} found):\n\n",
        if docs { "documentation" } else { "name" },
        hits.len()
    );
    for (name, doc) in &hits {
        let keys = maps.where_is(name);
        let k = if keys.is_empty() {
            String::new()
        } else {
            format!("  ({})", keys.join(", "))
        };
        text.push_str(&format!("{name}{k}\n  {doc}\n"));
    }
    text.push_str("\nType C-h f and a name for the whole documentation.\n");
    drop(maps);
    show(app, &text, None);
}

/// A buffer option: what it means, and what it takes.
const OPTION_DOCS: &[(&str, &str, &str)] = &[
    (
        "cursor",
        "How the cursor looks in this buffer.",
        "box, bar, hbar",
    ),
    (
        "fill-column",
        "The column M-q fills to.",
        "a number of columns",
    ),
    (
        "electric-quote",
        "Electric Quote mode in this buffer: ` and ' type curved quotes.",
        "true or false",
    ),
    (
        "display-fill-column-indicator",
        "A line at the fill column.",
        "true or false",
    ),
    (
        "display-fill-column-indicator-column",
        "Where the fill column line goes.",
        "t (the fill column), a number, or nil (no line)",
    ),
    (
        "display-fill-column-indicator-character",
        "The character the fill column line is drawn with.",
        "one character, or nil (no line)",
    ),
    (
        "hl-line",
        "Highlight the line point is on.",
        "true or false",
    ),
    ("indent-tabs", "Indent with tabs.", "true or false"),
    (
        "indent-width",
        "Columns per indentation level.",
        "a number of columns",
    ),
    (
        "insert-final-newline",
        "End the file with a newline on save.",
        "true or false",
    ),
    (
        "line-numbers",
        "Line numbers here.",
        "off, absolute or relative",
    ),
    (
        "read-only",
        "Commands may not change the text; the plugin that owns the buffer still can.",
        "true or false",
    ),
    (
        "show-trailing-whitespace",
        "Blanks at the ends of lines stand out.",
        "true or false",
    ),
    (
        "trim-trailing-whitespace",
        "Delete trailing blanks on save.",
        "true or false",
    ),
    (
        "whitespace",
        "Draw spaces and tabs visibly.",
        "true or false",
    ),
    (
        "wrap",
        "What long lines do: cut off, wrap anywhere, or at words.",
        "off, char or word",
    ),
];

/// A forge.opt setting: what it means, and what it takes.
const OPT_DOCS: &[(&str, &str, &str)] = &[
    (
        "prefix_help_delay",
        "Seconds to wait after a prefix key before listing the keys that can follow it.",
        "0 or more",
    ),
    (
        "mouse",
        "The wheel scrolls, a click moves point, a drag selects.",
        "true or false",
    ),
    (
        "plugin_auto_reload",
        "Reload a plugin by itself when its file is rebuilt.",
        "true or false",
    ),
    (
        "package_index",
        "Where package indexes live.",
        "a path or URL, or a list of them",
    ),
    (
        "no_littering",
        "Keep auto-saves and backups in the state directory's var/ instead of next to visited files (backup_directory still wins).",
        "true or false",
    ),
    (
        "sentence_end",
        "A regexp that matches the end of a sentence and the whitespace after it.",
        "a regexp",
    ),
    (
        "page_delimiter",
        "A regexp for the start of a line that divides pages.",
        "a regexp",
    ),
    (
        "paragraph_start",
        "A regexp matching lines that start or separate paragraphs.",
        "a regexp",
    ),
    (
        "paragraph_separate",
        "A regexp matching lines that separate paragraphs.",
        "a regexp",
    ),
    (
        "electric_indent",
        "RET indents the new line; a closing bracket re-indents its line.",
        "true or false",
    ),
    (
        "line_numbers",
        "Line numbers in buffers that don't choose their own.",
        "off, absolute or relative",
    ),
    (
        "mark_ring_max",
        "How many marks a buffer's mark ring keeps (C-u C-SPC walks back through them).",
        "a number",
    ),
    (
        "set_mark_command_repeat_pop",
        "A plain C-SPC right after C-u C-SPC pops the mark ring again.",
        "true or false",
    ),
    (
        "wrap",
        "What long lines do in buffers that don't choose.",
        "off, char or word",
    ),
    (
        "editorconfig",
        ".editorconfig files set indentation and end-of-line behavior.",
        "true or false",
    ),
    ("electric_pair", "( inserts a matching ).", "true or false"),
    (
        "electric_quote",
        "` and ' type curved quotes; `` and '' type curved double quotes.",
        "true or false",
    ),
    (
        "electric_quote_chars",
        "The four curved quotes Electric Quote mode types: left single, right single, left double, right double.",
        "four characters",
    ),
    (
        "electric_quote_replace_double",
        "Typing \" also types a curved double quote (open or close by context).",
        "true or false",
    ),
    (
        "electric_quote_paragraph",
        "Electric Quote mode converts quotes in text paragraphs.",
        "true or false",
    ),
    (
        "electric_quote_comment",
        "Electric Quote mode converts quotes in comments.",
        "true or false",
    ),
    (
        "electric_quote_string",
        "Electric Quote mode converts quotes in strings.",
        "true or false",
    ),
    (
        "fill_column_indicator",
        "A line at the fill column in every buffer.",
        "true or false",
    ),
    (
        "hl_line",
        "Highlight the current line everywhere.",
        "true or false",
    ),
    (
        "context_menu",
        "Right-click opens a context menu (context-menu-mode).",
        "true or false",
    ),
    (
        "menu_bar",
        "Draw the menu bar (menu-bar-mode).",
        "true or false",
    ),
    (
        "modifier_bar",
        "Draw the modifier bar in graphical frames (modifier-bar-mode); it's part of the tool bar, so tool-bar-mode must be on.",
        "true or false",
    ),
    (
        "tool_bar",
        "Draw the tool bar in graphical frames (tool-bar-mode).",
        "true or false",
    ),
    (
        "tool_bar_always_show_default",
        "Keep the default tool bar in major modes that have their own.",
        "true or false",
    ),
    (
        "line_number_mode",
        "Show the line number in the mode line (line-number-mode).",
        "true or false",
    ),
    (
        "column_number_mode",
        "Show the column in the mode line (column-number-mode).",
        "true or false",
    ),
    (
        "size_indication_mode",
        "Show the buffer's size in the mode line (size-indication-mode).",
        "true or false",
    ),
    (
        "icon_overrides",
        "How individual icons look, as a table of icon name to TYPE:VALUE (what customize-icon sets).",
        "symbol:⟲, emoji:↩️, text:undo, image:/path/undo.png or system:arrow.uturn.backward",
    ),
    (
        "icon_platform",
        "Whose native icons the system icon type uses.",
        "auto, macos, windows or linux",
    ),
    (
        "icon_preference",
        "Icon types to try, in order; the first the display supports is used.",
        "a list of system, image, emoji, symbol and text, e.g. \"system emoji symbol text\"",
    ),
    (
        "tool_bar_position",
        "Where the tool bar goes: top, bottom, left or right.",
        "top, bottom, left or right",
    ),
    (
        "tool_bar_style",
        "How much of each tool bar item to draw.",
        "image, text, both, both-horiz or text-image-horiz",
    ),
    (
        "recentf",
        "Remember recently opened files.",
        "true or false",
    ),
    (
        "save_place",
        "Files open where you left them.",
        "true or false",
    ),
    (
        "savehist",
        "Keep minibuffer history between sessions.",
        "true or false",
    ),
    (
        "sentence_end_double_space",
        "A sentence ends only after two spaces or a line break, not one.",
        "true or false",
    ),
    (
        "sentence_end_without_period",
        "A sentence can end without a period, at a line break.",
        "true or false",
    ),
    (
        "show_paren",
        "Highlight matching brackets.",
        "true or false",
    ),
    (
        "show_trailing_whitespace",
        "Blanks at line ends stand out everywhere.",
        "true or false",
    ),
    (
        "mouse_drag_copy_region",
        "Dragging a region with the mouse copies it to the kill ring and selection.",
        "true or false",
    ),
    (
        "save_interprogram_paste_before_kill",
        "Save foreign clipboard text to the kill ring before a kill replaces it.",
        "true, false or a character limit",
    ),
    (
        "select_enable_clipboard",
        "Kill and yank commands make use of the system clipboard.",
        "true or false",
    ),
    (
        "select_enable_primary",
        "Kill and yank commands make use of the primary selection.",
        "true or false",
    ),
    (
        "x_select_enable_clipboard_manager",
        "Transfer clipboard data to an X11 clipboard manager on exit.",
        "true or false",
    ),
    (
        "yank_pop_change_selection",
        "Rotating the kill ring with M-y saves the new yank to the clipboard/selection.",
        "true or false",
    ),
];

/// What init.rhai set an option to (None: left at the default).
fn opt_value(app: &App, name: &str) -> Option<String> {
    let st = app.rhai.as_ref().map(|l| l.settings())?;
    match name {
        "prefix_help_delay" => st.prefix_help_delay.map(|n| n.to_string()),
        "mouse" => st.mouse.map(|b| b.to_string()),
        "plugin_auto_reload" => st.plugin_auto_reload.map(|b| b.to_string()),
        "package_index" => st.package_index.map(|l| l.join(", ")),
        "electric_indent" => st.electric_indent.map(|b| b.to_string()),
        "line_numbers" => st.line_numbers.map(|l| l.name().to_string()),
        "mark_ring_max" => st.numbers.get(name).map(|n| n.to_string()),
        "save_interprogram_paste_before_kill" => {
            st.save_interprogram_paste_before_kill.map(|s| match s {
                forge_core::SaveInterprogramPasteBeforeKill::Always => "true".to_string(),
                forge_core::SaveInterprogramPasteBeforeKill::Off => "false".to_string(),
                forge_core::SaveInterprogramPasteBeforeKill::Limit(n) => n.to_string(),
            })
        }
        "wrap" => st.wrap.map(|w| w.name().to_string()),
        "tool_bar_position" | "tool_bar_style" | "icon_platform" | "icon_preference" => {
            st.strings.get(name).and_then(|v| v.first()).cloned()
        }
        _ => st.flags.get(name).map(|b| b.to_string()),
    }
}

/// Every name describe-symbol completes: commands, buffer options,
/// forge.opt settings, modes and faces.
pub fn symbol_candidates(app: &App) -> Vec<String> {
    use std::collections::BTreeSet;
    let mut names = BTreeSet::new();
    names.extend(crate::commands::visible_names(app));
    names.extend(
        forge_core::decoration::OPTIONS
            .iter()
            .map(|s| s.to_string()),
    );
    names.extend(forge_core::rhai::OPTIONS.iter().map(|s| s.to_string()));
    names.extend(forge_core::rhai::FLAGS.iter().map(|s| s.to_string()));
    names.extend(app.editor.modes.names(forge_core::modes::ModeKind::Major));
    names.extend(app.editor.modes.names(forge_core::modes::ModeKind::Minor));
    names.extend(forge_core::theme::all_faces());
    names.extend(
        forge_core::events::EVENT_NAMES
            .iter()
            .map(|s| s.to_string()),
    );
    names.into_iter().collect()
}

/// The symbol around point, for describe-symbol's default: letters,
/// digits and symbol characters.
pub fn symbol_at_point(app: &App) -> Option<String> {
    let b = app.buf_ref();
    let line = b.line(b.point().line);
    let chars: Vec<char> = line.chars().collect();
    let mut col = b.point().col.min(chars.len().saturating_sub(1));
    if chars.is_empty() {
        return None;
    }
    let is_symbol = |c: char| c.is_alphanumeric() || "-_?!+*/<>=.".contains(c);
    if !is_symbol(chars[col]) {
        // On either edge of one, take the side that has a symbol.
        if col > 0 && is_symbol(chars[col - 1]) {
            col -= 1;
        } else if col + 1 < chars.len() && is_symbol(chars[col + 1]) {
            col += 1;
        } else {
            return None;
        }
    }
    let mut start = col;
    while start > 0 && is_symbol(chars[start - 1]) {
        start -= 1;
    }
    let mut end = col;
    while end + 1 < chars.len() && is_symbol(chars[end + 1]) {
        end += 1;
    }
    let word: String = chars[start..=end].iter().collect();
    if word.is_empty() {
        return None;
    }
    Some(word)
}

/// C-h o: everything known about one name — its command, option,
/// setting, mode or face sections, whichever match. Like Forge's
/// describe-symbol, with the symbol at point as the default.
pub fn describe_symbol(app: &mut App, name: &str) {
    let mut sections = Vec::new();
    if app.editor.commands.contains(name) {
        sections.push(function_text(app, name));
    }
    if let Some((_, doc, values)) = OPTION_DOCS.iter().find(|(n, _, _)| *n == name) {
        let value = app
            .buf_ref()
            .option(name)
            .unwrap_or_else(|| "unset".to_string());
        sections.push(format!(
            "{name} is a buffer option, currently {value} here.\n\nDocumentation:\n  {doc}\n\nValues:\n  {values}\n\nSetting it:\n  In Rhai, per buffer: buffer:set_option(\"{name}\", ...)"
        ));
    }
    if let Some((_, doc, values)) = OPT_DOCS.iter().find(|(n, _, _)| *n == name) {
        let value = opt_value(app, name).unwrap_or_else(|| "default".to_string());
        sections.push(format!(
            "forge.opt.{name} is {value}.\n\nDocumentation:\n  {doc}\n\nValues:\n  {values}\n\nSetting it:\n  In init.rhai: forge.opt.{name} = ..."
        ));
    }
    if let Some(def) = app.editor.modes.get(name) {
        let kind = match def.kind {
            forge_core::modes::ModeKind::Major => "major",
            forge_core::modes::ModeKind::Minor => "minor",
        };
        let mut text = format!("{name} is a {kind} mode");
        let enabled = match def.kind {
            forge_core::modes::ModeKind::Major => app.buf_ref().major_mode() == name,
            forge_core::modes::ModeKind::Minor => {
                app.buf_ref().minor_modes().iter().any(|m| m == name)
            }
        };
        if enabled {
            text.push_str(", on in this buffer");
        } else {
            text.push_str(", not on in this buffer");
        }
        text.push('.');
        if let Some(d) = app.docs.get(forge_docs::Kind::Mode, name) {
            let body = forge_docs::render_text(d);
            let body = body.trim_end();
            if !body.is_empty() {
                text.push('\n');
                text.push_str(body);
            }
        }
        if let Some(parent) = &def.parent {
            text.push_str(&format!("\nIt derives from {parent}."));
        }
        if !def.extensions.is_empty() {
            text.push_str(&format!("\nIt opens {} files.", def.extensions.join(", ")));
        }
        if let Some(owner) = &def.owner {
            text.push_str(&format!("\nDefined by plugin {owner}."));
        }
        sections.push(text);
    }
    if forge_core::theme::all_faces().iter().any(|n| n == name) {
        let style = app.editor.theme.get(name).map(|s| {
            let mut parts = Vec::new();
            if let Some(c) = s.fg {
                parts.push(format!("foreground {}", c.hex()));
            }
            if let Some(c) = s.bg {
                parts.push(format!("background {}", c.hex()));
            }
            if let Some(b) = s.bold {
                parts.push(format!("bold {}", if b { "on" } else { "off" }));
            }
            if let Some(b) = s.italic {
                parts.push(format!("italic {}", if b { "on" } else { "off" }));
            }
            if let Some(b) = s.underline {
                parts.push(format!("underline {}", if b { "on" } else { "off" }));
            }
            parts.join(", ")
        });
        let what = app
            .docs
            .get(forge_docs::Kind::Face, name)
            .map(forge_docs::render::summary_text)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "A face.".to_string());
        let style = match style {
            Some(style) => format!("restyled to {style} by forge.theme.set"),
            None => "in the frontend's own style".to_string(),
        };
        sections.push(format!(
            "{name} is a face, {style}.\n{what}\nRestyle it with forge.theme.set (see forge.theme.faces())."
        ));
    }
    if let Some(d) = app.docs.get(forge_docs::Kind::Event, name) {
        sections.push(format!(
            "{name} is an event: {}\nSubscribe to it from a plugin with handler.subscribe({name}) or from init.rhai with forge.on(\"{name}\", fn).",
            forge_docs::render::summary_text(d)
        ));
    }
    if sections.is_empty() {
        app.error(format!("No symbol named {name}"));
    } else {
        show(app, &sections.join("\n\n"), None);
    }
}

/// RET / `<mouse-1>` in a help buffer: follow the link at point.
pub fn follow_link(app: &mut App) {
    let off = app.buf_ref().point_offset();
    let target = app.buf_ref().link_at(off).map(|l| l.target.clone());
    match target {
        Some(LinkTarget::Symbol(name)) => describe_symbol(app, &name),
        Some(LinkTarget::Node(node)) => {
            let text = app
                .docs
                .get(forge_docs::Kind::Node, &node)
                .map(forge_docs::render_text);
            match text {
                Some(text) if !text.is_empty() => show(app, &text, Some(&node)),
                _ => app.msg(format!("No manual node named {node}")),
            }
        }
        Some(LinkTarget::Key(keys)) => {
            let seq: Vec<String> = keys.split_whitespace().map(str::to_string).collect();
            let lookup = app.editor.active_keymaps().lookup(&seq);
            describe_key(app, &keys, &lookup);
        }
        Some(LinkTarget::Url(url)) => app.msg(format!("Open {url} in a browser")),
        Some(LinkTarget::Source { file, line }) => app.msg(format!("Defined at {file}:{line}")),
        None => app.msg("There is no link at point".to_string()),
    }
}

/// TAB in a help buffer: move to the next link.
pub fn next_link(app: &mut App) {
    if !app.buf().goto_link(true) {
        app.msg("No next link".to_string());
    }
}

/// S-TAB in a help buffer: move to the previous link.
pub fn previous_link(app: &mut App) {
    if !app.buf().goto_link(false) {
        app.msg("No previous link".to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::{Buffer, Editor};

    fn app() -> App {
        let mut app = App::new(Editor::default());
        let id = app.editor.add_buffer(Buffer::new("*scratch*"));
        app.editor.show_buffer(id);
        app
    }

    fn target_names(app: &App) -> Vec<String> {
        app.buf_ref()
            .links()
            .iter()
            .filter_map(|l| match &l.target {
                LinkTarget::Symbol(n) => Some(n.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_help_page_links_the_commands_it_mentions() {
        let mut a = app();
        show(
            &mut a,
            "save-buffer is a command; find-file visits a file. built-in is not a command.\n",
            None,
        );
        let names = target_names(&a);
        assert!(names.iter().any(|n| n == "save-buffer"), "{names:?}");
        assert!(names.iter().any(|n| n == "find-file"), "{names:?}");
        assert!(!names.iter().any(|n| n == "built-in"), "{names:?}");
    }

    #[test]
    fn links_land_on_the_command_text() {
        let mut a = app();
        show(&mut a, "Open a file with find-file.\n", None);
        let l = a.buf_ref().links().first().expect("a link").clone();
        let chars: Vec<char> = "Open a file with find-file.\n".chars().collect();
        assert_eq!(
            chars[l.start..l.end].iter().collect::<String>(),
            "find-file"
        );
    }

    #[test]
    fn following_a_symbol_link_describes_it() {
        let mut a = app();
        show(&mut a, "See find-file for opening files.\n", None);
        let start = a.buf_ref().link_at(0).map(|_| 0).unwrap_or_else(|| {
            a.buf_ref()
                .links()
                .iter()
                .find(|l| matches!(&l.target, LinkTarget::Symbol(n) if n == "find-file"))
                .map(|l| l.start)
                .expect("find-file link")
        });
        a.buf().set_offset(start);
        follow_link(&mut a);
        assert!(
            target_names(&a).iter().any(|n| n == "find-file"),
            "the new page mentions find-file"
        );
    }

    #[test]
    fn tab_walks_from_link_to_link() {
        let mut a = app();
        show(
            &mut a,
            "save-buffer then find-file then revert-buffer.\n",
            None,
        );
        let mut starts: Vec<usize> = a.buf_ref().links().iter().map(|l| l.start).collect();
        starts.sort_unstable();
        assert!(starts.len() >= 3, "{starts:?}");
        // Point starts at the first link, so TAB moves to the second.
        a.buf().set_offset(starts[0]);
        next_link(&mut a);
        assert_eq!(a.buf_ref().point_offset(), starts[1]);
        next_link(&mut a);
        assert_eq!(a.buf_ref().point_offset(), starts[2]);
        previous_link(&mut a);
        assert_eq!(a.buf_ref().point_offset(), starts[1]);
    }
}
