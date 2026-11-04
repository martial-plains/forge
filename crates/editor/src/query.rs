// SPDX-License-Identifier: MIT OR Apache-2.0
//! M-% query-replace and C-M-% query-replace-regexp: step through the
//! matches, asking about each one (the matching is done by
//! `forge_core::replace`).

use crate::app::App;
use forge_core::Pos;
use forge_core::replace::{ReplaceOptions, Replacer};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const HELP: &str = "y/SPC replace, n/DEL skip, ! replace all, . replace and stop, \
                    , replace and stay, q/RET stop, C-g quit";

pub struct QueryReplace {
    replacer: Replacer,
    from: String,
    to: String,
    /// Just did `,`: the replaced text is shown; the next y/n moves on.
    stayed: bool,
}

/// What M-% and friends were asked to do, carried between the prompts.
#[derive(Debug, Clone)]
pub struct ReplaceRequest {
    pub regexp: bool,
    pub delimited: bool,
    /// Ask about each match (M-%) or replace them all (replace-string).
    pub query: bool,
    /// Char offsets of the active region when the command started.
    pub region: Option<(usize, usize)>,
}

impl ReplaceRequest {
    pub fn name(&self) -> &'static str {
        match (self.query, self.regexp) {
            (true, false) => "Query replace",
            (true, true) => "Query replace regexp",
            (false, false) => "Replace string",
            (false, true) => "Replace regexp",
        }
    }
}

/// Begin replacing (after both prompts).
pub fn start(app: &mut App, from: String, to: String, req: &ReplaceRequest) {
    let origin = app.buf_ref().point();
    let start = req
        .region
        .map_or_else(|| app.buf_ref().point_offset(), |(a, _)| a);
    let opts = ReplaceOptions {
        regexp: req.regexp,
        delimited: req.delimited,
        start,
        end: req.region.map(|(_, b)| b),
    };
    let mut replacer = match Replacer::new(app.buf_ref(), &from, &to, &opts) {
        Ok(r) => r,
        Err(e) => return app.error(e),
    };
    app.last_replace = Some((from.clone(), to.clone()));
    app.buf().push_mark(origin, false);
    if !req.query {
        replacer.replace_all(app.buf());
        let n = replacer.replaced;
        return app.msg(format!(
            "Replaced {n} occurrence{}",
            if n == 1 { "" } else { "s" }
        ));
    }
    app.query = Some(QueryReplace {
        replacer,
        from,
        to,
        stayed: false,
    });
    next(app);
}

/// Find and show the next match, or finish.
fn next(app: &mut App) {
    let Some(mut q) = app.query.take() else {
        return;
    };
    q.stayed = false;
    match q.replacer.find_next(app.buf_ref()) {
        Some((a, b)) => {
            let (pa, pb) = (
                app.buf_ref().offset_to_pos(a),
                app.buf_ref().offset_to_pos(b),
            );
            show(app, &q, pa, pb);
            app.query = Some(q);
        }
        None => finish(app, q, None),
    }
}

fn show(app: &mut App, q: &QueryReplace, a: Pos, b: Pos) {
    app.buf().set_point(b);
    app.highlight = Some((a, b));
    app.msg(format!(
        "Query replacing {} with {}: (? for help)",
        q.from, q.to
    ));
}

fn finish(app: &mut App, q: QueryReplace, note: Option<&str>) {
    app.highlight = None;
    let n = q.replacer.replaced;
    let s = if n == 1 { "" } else { "s" };
    match note {
        Some(note) => app.msg(format!("{note}; replaced {n} occurrence{s}")),
        None => app.msg(format!("Replaced {n} occurrence{s}")),
    }
    app.last_command = "query-replace".into();
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    let Some(mut q) = app.query.take() else {
        return;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let plain = !ctrl && !alt;
    match key.code {
        KeyCode::Char('y') | KeyCode::Char(' ') if plain => {
            if !q.stayed {
                replace(app, &mut q);
            }
            app.query = Some(q);
            next(app);
        }
        KeyCode::Char('n') if plain => {
            q.replacer.skip();
            app.query = Some(q);
            next(app);
        }
        KeyCode::Backspace => {
            q.replacer.skip();
            app.query = Some(q);
            next(app);
        }
        KeyCode::Char('!') if plain => {
            if !q.stayed {
                replace(app, &mut q);
            }
            q.replacer.replace_all(app.buf());
            finish(app, q, None);
        }
        KeyCode::Char('.') if plain => {
            if !q.stayed {
                replace(app, &mut q);
            }
            finish(app, q, None);
        }
        KeyCode::Char(',') if plain => {
            if !q.stayed {
                if let Some((a, b)) = replace(app, &mut q) {
                    let (pa, pb) = (
                        app.buf_ref().offset_to_pos(a),
                        app.buf_ref().offset_to_pos(b),
                    );
                    app.buf().set_point(pb);
                    app.highlight = Some((pa, pb));
                }
                q.stayed = true;
            }
            app.msg(format!(
                "Query replacing {} with {}: (y or n to go on)",
                q.from, q.to
            ));
            app.query = Some(q);
        }
        KeyCode::Char('?') if plain => {
            app.msg(HELP);
            app.query = Some(q);
        }
        KeyCode::Char('q') if plain => finish(app, q, None),
        KeyCode::Enter => finish(app, q, None),
        KeyCode::Char('g') if ctrl => finish(app, q, Some("Quit")),
        _ => {
            // Anything else stops, then does its normal job.
            finish(app, q, None);
            app.dispatch_key(key);
        }
    }
}

fn replace(app: &mut App, q: &mut QueryReplace) -> Option<(usize, usize)> {
    let r = q.replacer.replace_current(app.buf());
    if let Some((_, end)) = r {
        app.buf().set_offset(end);
    }
    r
}
