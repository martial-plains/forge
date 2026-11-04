// SPDX-License-Identifier: MIT OR Apache-2.0
//! Lists of branches, tags and stashes, to act on what's at point.
//!
//! In `*vc-branches*`, RET switches to the branch, `l` shows its log, `m`
//! merges it, `r` renames it, `d` deletes it and `c` makes a new one. In
//! `*vc-tags*`, RET checks the tag out and `l` shows its log. In
//! `*vc-stashes*`, RET shows the stash, `a` applies it, `P` pops it and
//! `d` drops it. `g` reads the list again.

use super::{Ask, Confirm, Fileset, StashOp};
use crate::app::App;
use forge_core::decoration::{DecoFace, DecoSpec, VcFace};
use forge_core::{Buffer, BufferId, Pos};
use std::path::Path;

pub const MODE: &str = "vc-list-mode";
const DECO: &str = "vc-list";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    Branches,
    Tags,
    Stashes,
}

#[derive(Debug, Clone)]
pub struct ListView {
    pub fileset: Fileset,
    pub what: ListKind,
    /// (name, description), in display order; line i+2 is item i.
    pub items: Vec<(String, String)>,
}

const FIRST: usize = 2;

fn read(fs: &Fileset, what: ListKind) -> (String, Vec<(String, String)>) {
    let b = fs.backend();
    let root = crate::files::abbreviate(&fs.root);
    match what {
        ListKind::Branches => {
            let cur = b.current_branch(&fs.root);
            let mut items: Vec<(String, String)> = b
                .branches(&fs.root, true)
                .into_iter()
                .map(|n| {
                    let d = if Some(&n) == cur.as_ref() {
                        "current".to_string()
                    } else {
                        String::new()
                    };
                    (n, d)
                })
                .collect();
            items.sort_by_key(|(n, _)| (Some(n) != cur.as_ref(), n.contains('/'), n.clone()));
            (format!("Branches of {root}"), items)
        }
        ListKind::Tags => (
            format!("Tags of {root}"),
            b.tags(&fs.root)
                .into_iter()
                .map(|t| (t, String::new()))
                .collect(),
        ),
        ListKind::Stashes => (format!("Stashes of {root}"), b.stashes(&fs.root)),
    }
}

fn buffer_name(what: ListKind) -> &'static str {
    match what {
        ListKind::Branches => "*vc-branches*",
        ListKind::Tags => "*vc-tags*",
        ListKind::Stashes => "*vc-stashes*",
    }
}

fn render(app: &mut App, id: BufferId, view: ListView) {
    let (title, items) = read(&view.fileset, view.what);
    let mut text = format!("{title}\n\n");
    let mut decos = vec![DecoSpec::new(
        0,
        title.chars().count(),
        DecoFace::Vc(VcFace::VcDirHeader),
    )];
    let mut offset = text.chars().count();
    let w = items
        .iter()
        .map(|(n, _)| n.chars().count())
        .max()
        .unwrap_or(0);
    for (n, d) in &items {
        let current = d == "current";
        let line = format!(
            "{} {n:<w$}  {}",
            if current { '*' } else { ' ' },
            if current { "" } else { d.as_str() }
        );
        let line = line.trim_end().to_string();
        let face = if current {
            VcFace::VcDirMark
        } else if view.what == ListKind::Stashes {
            VcFace::LogViewRevision
        } else {
            VcFace::LogViewRef
        };
        decos.push(DecoSpec::new(
            offset + 2,
            offset + 2 + n.chars().count(),
            DecoFace::Vc(face),
        ));
        text.push_str(&line);
        text.push('\n');
        offset += line.chars().count() + 1;
    }
    if items.is_empty() {
        text.push_str("  (none)\n");
    }
    let view = ListView { items, ..view };
    if let Some(b) = app.editor.state.get_buffer_mut(id) {
        let keep = b.point().line.max(FIRST);
        b.set_locked(false);
        b.set_contents(&text);
        b.set_major_mode(MODE);
        b.set_decorations(DECO, decos);
        b.set_locked(true);
        b.set_point(Pos::new(keep.min(b.line_count().saturating_sub(1)), 2));
    }
    app.vc.lists.insert(id, view);
}

pub fn open(app: &mut App, what: ListKind) {
    let fs = match super::deduce_fileset(app, true) {
        Ok(f) => f,
        Err(e) => return app.error(e),
    };
    let caps = fs.backend().caps();
    let ok = match what {
        ListKind::Branches => caps.branches,
        ListKind::Tags => caps.tags,
        ListKind::Stashes => caps.stash,
    };
    if !ok {
        return app.error(format!("{} doesn't have those", fs.kind.name()));
    }
    let name = buffer_name(what);
    let id = match app.editor.find_buffer(name) {
        Some(id) => id,
        None => app.editor.add_buffer(Buffer::new(name)),
    };
    if let Some(b) = app.editor.state.get_buffer_mut(id) {
        b.set_point(Pos::new(FIRST, 0));
    }
    render(
        app,
        id,
        ListView {
            fileset: fs,
            what,
            items: Vec::new(),
        },
    );
    if app.active_id() != id {
        crate::windows::pop_to_other_window(app);
        app.switch_to(id);
    }
}

/// Re-read every list of `root` (after a branch or stash changed).
pub fn refresh_lists(app: &mut App, root: &Path) {
    let ids: Vec<BufferId> = app
        .vc
        .lists
        .iter()
        .filter(|(_, v)| v.fileset.root == root)
        .map(|(id, _)| *id)
        .collect();
    for id in ids {
        if let Some(v) = app.vc.lists.get(&id).cloned() {
            render(app, id, v);
        }
    }
}

fn at_point(app: &App) -> Option<(ListView, String)> {
    let v = app.vc.lists.get(&app.active_id())?.clone();
    let line = app.buf_ref().point().line;
    let item = v.items.get(line.checked_sub(FIRST)?)?.0.clone();
    Some((v, item))
}

/// The stash at point, in a stash list.
pub fn stash_at_point(app: &App) -> Option<String> {
    at_point(app)
        .filter(|(v, _)| v.what == ListKind::Stashes)
        .map(|(_, s)| s)
}

/// The list commands. False if `cmd` isn't one.
pub fn execute(app: &mut App, cmd: &str) -> bool {
    match cmd {
        "vc-list-branches" => open(app, ListKind::Branches),
        "vc-list-tags" => open(app, ListKind::Tags),
        "vc-list-stashes" => open(app, ListKind::Stashes),
        "vc-list-refresh" => {
            let id = app.active_id();
            match app.vc.lists.get(&id).cloned() {
                Some(v) => render(app, id, v),
                None => app.error("Not in a list of branches, tags or stashes"),
            }
        }
        c if c.starts_with("vc-list-item-") => {
            let Some((v, item)) = at_point(app) else {
                app.error("Nothing here");
                return true;
            };
            let fs = v.fileset.clone();
            match (c, v.what) {
                ("vc-list-item-action", ListKind::Branches) => {
                    // A remote branch checks out as a local one tracking it.
                    let name = match item.split_once('/') {
                        Some((remote, b))
                            if fs.backend().remotes(&fs.root).iter().any(|r| r == remote) =>
                        {
                            b.to_string()
                        }
                        _ => item,
                    };
                    super::answered(app, Ask::SwitchBranch(fs), name);
                }
                ("vc-list-item-action", ListKind::Tags) => {
                    super::answered(app, Ask::RetrieveTag(fs), item)
                }
                ("vc-list-item-action", ListKind::Stashes) => {
                    super::stash_op(app, &fs, &item, StashOp::Show)
                }
                ("vc-list-item-log", ListKind::Branches | ListKind::Tags) => {
                    super::answered(app, Ask::BranchLog(fs), item)
                }
                ("vc-list-item-merge", ListKind::Branches) => {
                    super::answered(app, Ask::Merge(fs), item)
                }
                ("vc-list-item-rename", ListKind::Branches) => {
                    super::answered(app, Ask::RenameBranch(fs), item)
                }
                ("vc-list-item-delete", ListKind::Branches) => {
                    if v.items.iter().any(|(n, d)| n == &item && d == "current") {
                        app.error("That's the current branch: switch to another first");
                    } else {
                        let p = format!("Delete branch {item}? (y or n) ");
                        app.prompt(
                            p,
                            crate::minibuffer::Purpose::VcAsk {
                                what: Ask::DeleteBranch(fs),
                                candidates: Vec::new(),
                            },
                        );
                        // y/n here is just RET with the name typed.
                        if let Some(mb) = app.minibuffer.as_mut() {
                            mb.prompt = "Delete branch: ".into();
                            mb.input = item;
                        }
                    }
                }
                ("vc-list-item-delete", ListKind::Stashes) => {
                    app.prompt(
                        format!("Drop {item}? (y or n) "),
                        crate::minibuffer::Purpose::Confirm(crate::minibuffer::ConfirmAction::Vc(
                            Confirm::DropStash(fs, item),
                        )),
                    );
                }
                ("vc-list-item-apply", ListKind::Stashes) => {
                    super::stash_op(app, &fs, &item, StashOp::Apply)
                }
                ("vc-list-item-pop", ListKind::Stashes) => {
                    super::stash_op(app, &fs, &item, StashOp::Pop)
                }
                _ => app.error("That doesn't apply here"),
            }
        }
        _ => return false,
    }
    true
}
