// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Info-style manual browser (C-h i, and the `manual` alias).
//!
//! The buffer is `*Manual*` in [`MODE`]. Its text is one node of the
//! manual, rendered as Info renders it: a `File: forge.org,  Node: ...`
//! header line, the underlined title, the body, and a `* Menu:` of the
//! node's children. Navigation (n/p/u/t/...), the history (l/r) and the
//! minibuffer commands (g/m/s/i, C-h R) are Info's, so the manual is read
//! the way a manual is. The node tree comes from `forge_docs`, so
//! the browser and the HTML/Texinfo exporters show the same document.

use forge_core::{LinkSpan, LinkTarget};
use forge_docs::render::{InfoTarget, render_info};

use crate::app::App;

/// The manual buffer's major mode.
pub const MODE: &str = "info-mode";

/// The manual buffer's name.
pub const BUFFER: &str = "*Manual*";

/// Where the browser is: the node being read and how the reader got here.
#[derive(Debug, Default)]
pub struct InfoState {
    /// Index of the node being shown, or `None` before the first visit.
    pub node: Option<usize>,
    /// Nodes left behind by `n`/`u`/... , oldest first, for `l`.
    pub history: Vec<usize>,
    /// Nodes undone by `l`, for `r`.
    pub forward: Vec<usize>,
}

/// Every node's title, in document order, for completion (g, i).
pub fn node_titles() -> Vec<String> {
    forge_docs::the_manual()
        .nodes
        .iter()
        .map(|n| n.title.clone())
        .collect()
}

/// The child nodes of the node being read, for `m`'s completion.
pub fn menu_candidates(app: &App) -> Vec<String> {
    let manual = forge_docs::the_manual();
    match app.info.node {
        Some(i) if i < manual.nodes.len() => manual
            .children(i)
            .into_iter()
            .map(|c| manual.nodes[c].title.clone())
            .collect(),
        _ => Vec::new(),
    }
}

/// The manual's index terms (its `#+CINDEX:` lines), for `i`'s completion.
pub fn index_terms() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for n in &forge_docs::the_manual().nodes {
        for t in &n.index_terms {
            if !out.iter().any(|x: &String| x.eq_ignore_ascii_case(t)) {
                out.push(t.clone());
            }
        }
    }
    out.sort();
    out
}

/// C-h i / `info` / `manual`: open the manual at the node being read, or
/// at Top the first time.
pub fn open(app: &mut App) {
    if app.editor.find_buffer(BUFFER).is_some() && app.info.node.is_some() {
        let i = app.info.node.expect("checked");
        show_node(app, i, false);
    } else if let Some(top) = top_index() {
        app.info.history.clear();
        app.info.forward.clear();
        show_node(app, top, false);
    } else {
        app.error("The manual has no Top node");
    }
}

/// g / `info-goto-node`: go to a node by its title.
pub fn goto(app: &mut App, title: &str) {
    match forge_docs::the_manual().index_of(title) {
        Some(i) => show_node(app, i, true),
        None => app.error(format!("No manual node named {title}")),
    }
}

/// `info-next`: the next sibling, or the next node in document order.
pub fn next(app: &mut App) {
    let manual = forge_docs::the_manual();
    let i = match app.info.node {
        Some(i) => i,
        None => return open(app),
    };
    let target = manual
        .next_sibling(i)
        .or_else(|| manual.next_in_order(i))
        .unwrap_or(i);
    if target == i {
        app.msg("At the last node");
    } else {
        show_node(app, target, true);
    }
}

/// `info-prev`: the previous sibling, or the previous node in order.
pub fn prev(app: &mut App) {
    let manual = forge_docs::the_manual();
    let i = match app.info.node {
        Some(i) => i,
        None => return open(app),
    };
    let target = manual
        .prev_sibling(i)
        .or_else(|| manual.prev_in_order(i))
        .unwrap_or(i);
    if target == i {
        app.msg("At the first node");
    } else {
        show_node(app, target, true);
    }
}

/// u / `info-up`: the node this one is a section of.
pub fn up(app: &mut App) {
    match app.info.node.and_then(|i| forge_docs::the_manual().up(i)) {
        Some(i) => show_node(app, i, true),
        None => app.msg("At the top of the manual"),
    }
}

/// t / `info-top`: the Top node.
pub fn top(app: &mut App) {
    match top_index() {
        Some(i) => show_node(app, i, true),
        None => app.error("The manual has no Top node"),
    }
}

/// `]` / `info-forward-node`: the next node in document order.
pub fn forward_node(app: &mut App) {
    step(app, |m, i| m.next_in_order(i), "At the last node");
}

/// `[` / `info-backward-node`: the previous node in document order.
pub fn backward_node(app: &mut App) {
    step(app, |m, i| m.prev_in_order(i), "At the first node");
}

/// l / `info-last`: back to the node before the last move.
pub fn last(app: &mut App) {
    match app.info.history.pop() {
        Some(i) => {
            if let Some(cur) = app.info.node.take() {
                app.info.forward.push(cur);
            }
            show_node(app, i, false);
        }
        None => app.msg("No previous node"),
    }
}

/// r / `info-history-forward`: forward again after `l`.
pub fn history_forward(app: &mut App) {
    match app.info.forward.pop() {
        Some(i) => {
            if let Some(cur) = app.info.node.take() {
                app.info.history.push(cur);
            }
            show_node(app, i, false);
        }
        None => app.msg("No node to go forward to"),
    }
}

/// RET / `<mouse-1>` in the manual: follow the link at point.
pub fn follow(app: &mut App) {
    let off = app.buf_ref().point_offset();
    let target = app.buf_ref().link_at(off).map(|l| l.target.clone());
    match target {
        Some(LinkTarget::Node(title)) => goto(app, &title),
        Some(LinkTarget::Symbol(name)) => crate::help::describe_symbol(app, &name),
        Some(_) => app.msg("That link opens outside the manual"),
        None => app.msg("There is no link at point"),
    }
}

/// TAB in the manual: the next link.
pub fn next_ref(app: &mut App) {
    if !app.buf().goto_link(true) {
        app.msg("No next link");
    }
}

/// S-TAB in the manual: the previous link.
pub fn prev_ref(app: &mut App) {
    if !app.buf().goto_link(false) {
        app.msg("No previous link");
    }
}

/// m's answer: one of the current node's children.
pub fn menu(app: &mut App, title: &str) {
    let manual = forge_docs::the_manual();
    let ok = match app.info.node {
        Some(i) if i < manual.nodes.len() => manual
            .children(i)
            .into_iter()
            .any(|c| manual.nodes[c].title == title),
        _ => false,
    };
    if ok {
        goto(app, title);
    } else {
        app.error(format!("{title} is not on this node's menu"));
    }
}

/// T / `info-toc`: the whole manual as one menu of links.
pub fn toc(app: &mut App) {
    let manual = forge_docs::the_manual();
    let mut out = String::from("Table of Contents\n\n");
    let mut links = Vec::new();
    for n in &manual.nodes {
        out.push_str(&"  ".repeat(usize::from(n.level.saturating_sub(1))));
        let start = out.chars().count();
        out.push_str(&n.title);
        let end = out.chars().count();
        links.push(LinkSpan {
            start,
            end,
            target: LinkTarget::Node(n.title.clone()),
            face: None,
        });
        let summary = n.summary();
        if summary.is_empty() {
            out.push('\n');
        } else {
            out.push_str(&format!(":  {summary}\n"));
        }
    }
    app.show_special_buffer(BUFFER, MODE, &out);
    app.buf().set_links(links);
}

/// s's answer: a regexp to find in the manual, first match forward.
pub fn search(app: &mut App, pattern: &str) {
    if pattern.is_empty() {
        return;
    }
    let re = regex::Regex::new(&format!("(?i){pattern}")).ok();
    let manual = forge_docs::the_manual();
    let start = app.info.node.unwrap_or(0);
    let n = manual.nodes.len();
    for k in 0..n {
        let i = (start + k) % n;
        let (text, _) = render_info(manual, i);
        let hit = match &re {
            Some(re) => re.find(&text).map(|m| m.start()),
            None => text.to_lowercase().find(&pattern.to_lowercase()),
        };
        if let Some(byte_off) = hit {
            let char_off = text[..byte_off].chars().count();
            show_node(app, i, true);
            app.buf().set_offset(char_off);
            let more = (k + 1..n).any(|k2| {
                let j = (start + k2) % n;
                let (t2, _) = render_info(manual, j);
                match &re {
                    Some(re) => re.is_match(&t2),
                    None => t2.to_lowercase().contains(&pattern.to_lowercase()),
                }
            });
            if more {
                app.msg("Match; s again for the next one");
            } else {
                app.msg("Match");
            }
            return;
        }
    }
    app.error(format!("Search failed: {pattern}"));
}

/// i's answer: an index term (a `#+CINDEX:` of some node).
pub fn index(app: &mut App, term: &str) {
    let term = term.trim();
    if term.is_empty() {
        return;
    }
    let manual = forge_docs::the_manual();
    let hit = manual
        .nodes
        .iter()
        .position(|n| n.index_terms.iter().any(|t| t.eq_ignore_ascii_case(term)));
    match hit.or_else(|| manual.index_of(term)) {
        Some(i) => show_node(app, i, true),
        None => app.error(format!("No index entry for {term}")),
    }
}

/// w's answer: a command; open the manual node that documents it, or
/// describe it the usual way when no node claims it.
pub fn lookup(app: &mut App, command: &str) {
    let command = command.trim();
    if command.is_empty() {
        return;
    }
    let manual = forge_docs::the_manual();
    let node = manual
        .nodes
        .iter()
        .find(|n| n.property_list("COMMANDS").contains(&command));
    match node {
        Some(n) => goto(app, &n.title.clone()),
        None => crate::help::describe_symbol(app, command),
    }
}

/// q in the manual: leave it, back to the buffer before it.
pub fn quit(app: &mut App) {
    match app.other_buffer() {
        Some(id) => app.switch_to(id),
        None => app.msg("No other buffer"),
    }
}

fn top_index() -> Option<usize> {
    forge_docs::the_manual()
        .nodes
        .iter()
        .position(|n| n.parent.is_none())
}

fn step(app: &mut App, dir: fn(&forge_docs::Manual, usize) -> Option<usize>, at_end: &str) {
    let manual = forge_docs::the_manual();
    let i = match app.info.node {
        Some(i) => i,
        None => return open(app),
    };
    match dir(manual, i) {
        Some(t) if t != i => show_node(app, t, true),
        _ => app.msg(at_end),
    }
}

/// Show a node in `*Manual*`, filling in its link layer. `record` is true
/// when the reader moved somewhere new, so the old node goes on the
/// history and the forward list is cleared; re-rendering in place (open,
/// l, r) leaves the history alone.
fn show_node(app: &mut App, index: usize, record: bool) {
    let manual = forge_docs::the_manual();
    if index >= manual.nodes.len() {
        app.error("No such manual node");
        return;
    }
    if record && Some(index) != app.info.node {
        if let Some(cur) = app.info.node {
            app.info.history.push(cur);
        }
        app.info.forward.clear();
    }
    let (text, links) = render_info(manual, index);
    let links = links
        .into_iter()
        .map(|l| LinkSpan {
            start: l.start,
            end: l.end,
            target: match l.target {
                InfoTarget::Node(n) => LinkTarget::Node(n),
                InfoTarget::Symbol(s) => LinkTarget::Symbol(s),
            },
            face: None,
        })
        .collect();
    app.info.node = Some(index);
    app.show_special_buffer(BUFFER, MODE, &text);
    app.buf().set_links(links);
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

    fn text(app: &App) -> String {
        let b = app.buf_ref();
        b.text_offsets(0, b.char_count())
    }

    fn here(app: &App) -> String {
        let manual = forge_docs::the_manual();
        app.info
            .node
            .map(|i| manual.nodes[i].title.clone())
            .unwrap_or_default()
    }

    #[test]
    fn open_shows_the_top_node_in_the_manual_buffer() {
        let mut a = app();
        a.run_command("info");
        assert!(a.editor.find_buffer(BUFFER).is_some());
        assert_eq!(a.buf_ref().major_mode(), MODE);
        assert_eq!(here(&a), "Top");
        let t = text(&a);
        assert!(t.starts_with("File: forge.org,  Node: Top"), "{t:?}");
        assert!(t.contains("* Menu:"));
        assert!(t.contains("How to read this manual"));
        assert!(!a.buf_ref().links().is_empty());
    }

    #[test]
    fn next_up_and_top_move_around_the_tree() {
        let mut a = app();
        let _ = &mut a;
        a.run_command("info");
        a.run_command("info-next");
        assert_eq!(here(&a), "How to read this manual");
        a.run_command("info-up");
        assert_eq!(here(&a), "Top");
        a.run_command("info-next");
        a.run_command("info-next");
        assert_eq!(here(&a), "Conventions");
        a.run_command("info-top");
        assert_eq!(here(&a), "Top");
    }

    #[test]
    fn forward_and_backward_walk_document_order() {
        let mut a = app();
        a.run_command("info");
        a.run_command("info-forward-node");
        assert_eq!(here(&a), "How to read this manual");
        a.run_command("info-backward-node");
        assert_eq!(here(&a), "Top");
    }

    #[test]
    fn goto_opens_a_node_by_name() {
        let mut a = app();
        goto(&mut a, "Getting started");
        assert_eq!(here(&a), "Getting started");
        assert!(text(&a).contains("Node: Getting started"));
        // A bad name is an error, not a panic, and leaves the node alone.
        goto(&mut a, "No Such Node");
        assert_eq!(here(&a), "Getting started");
    }

    #[test]
    fn the_history_walks_back_and_forward() {
        let mut a = app();
        a.run_command("info");
        goto(&mut a, "Getting started");
        goto(&mut a, "Keys and commands");
        assert_eq!(here(&a), "Keys and commands");
        last(&mut a);
        assert_eq!(here(&a), "Getting started");
        history_forward(&mut a);
        assert_eq!(here(&a), "Keys and commands");
    }

    #[test]
    fn the_menu_lists_the_child_nodes() {
        let mut a = app();
        a.run_command("info");
        let items = menu_candidates(&a);
        assert!(items.iter().any(|t| t == "Getting started"), "{items:?}");
        menu(&mut a, "Keys and commands");
        assert_eq!(here(&a), "Keys and commands");
        menu(&mut a, "Not a child");
        assert_eq!(here(&a), "Keys and commands");
    }

    #[test]
    fn following_a_link_at_point_opens_the_node() {
        let mut a = app();
        a.run_command("info");
        let link = a
            .buf_ref()
            .links()
            .iter()
            .find(|l| l.target == LinkTarget::Node("Getting started".into()))
            .cloned()
            .expect("a menu link to Getting started");
        a.buf().set_offset(link.start);
        follow(&mut a);
        assert_eq!(here(&a), "Getting started");
    }

    #[test]
    fn next_and_prev_reference_walk_the_links() {
        let mut a = app();
        a.run_command("info");
        a.buf().set_offset(0);
        next_ref(&mut a);
        let off = a.buf_ref().point_offset();
        assert!(a.buf_ref().link_at(off).is_some(), "TAB landed on a link");
        prev_ref(&mut a);
    }

    #[test]
    fn search_opens_the_matching_node() {
        let mut a = app();
        a.run_command("info");
        search(&mut a, "self-insert-command");
        assert_ne!(here(&a), "Top", "search moved off the Top node");
        assert!(text(&a).contains("self-insert-command"));
        let before = here(&a);
        search(&mut a, "this string cannot be in the manual zzzzz");
        assert_eq!(here(&a), before, "a failed search keeps the node");
    }

    #[test]
    fn toc_lists_every_node_with_a_link() {
        let mut a = app();
        toc(&mut a);
        let t = text(&a);
        assert!(t.starts_with("Table of Contents"));
        for node in forge_docs::the_manual().nodes.iter() {
            assert!(t.contains(&node.title), "TOC is missing {}", node.title);
        }
    }

    #[test]
    fn the_index_terms_open_their_node() {
        let terms = index_terms();
        assert!(!terms.is_empty(), "the manual declares some CINDEX terms");
        let mut a = app();
        index(&mut a, &terms[0]);
        assert!(a.info.node.is_some());
    }

    #[test]
    fn lookup_prefers_the_manual_node_for_a_command() {
        let mut a = app();
        // save-buffer is documented in the manual by a node property.
        lookup(&mut a, "save-buffer");
        assert!(a.editor.find_buffer(BUFFER).is_some());
    }

    #[test]
    fn quit_returns_to_the_buffer_before() {
        let mut a = app();
        a.run_command("info");
        quit(&mut a);
        assert_ne!(a.buf_ref().name(), BUFFER);
    }
}
