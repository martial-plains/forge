// SPDX-License-Identifier: MIT OR Apache-2.0
//! The interactive tutorial (C-h t, `help-with-tutorial`).
//!
//! The buffer is `*Tutorial*` in [`MODE`], built from `docs/tutorial.org`
//! through the same `Block`/`Span` pipeline as the manual, so the keys and
//! command names in it are real links. The document is a list of exercises,
//! each a heading tagged `:exercise:` with a `:COMMAND:` property naming the
//! command that completes it. Whenever that command runs while the tutorial
//! is open, the exercise is checked off, and the reader can walk the
//! exercises with n/p and leave with q. Where the reader stopped is kept in
//! [`TutorialState`], so C-h t resumes.
//!
//! The tutorial's own source is parsed once; a test walks every key name the
//! exercises mention through the keymaps, so the text cannot drift from the
//! editor.

use std::sync::OnceLock;

use forge_core::{LinkSpan, LinkTarget};
use forge_docs::Manual;

use crate::app::App;

/// The tutorial's source, bundled like the manual.
pub const TUTORIAL_SRC: &str = include_str!("../../../docs/tutorial.org");

/// The tutorial buffer's major mode.
pub const MODE: &str = "tutorial-mode";

/// The tutorial buffer's name.
pub const BUFFER: &str = "*Tutorial*";

/// The tag that marks an exercise heading.
const EXERCISE_TAG: &str = "exercise";

/// Where the reader is and how far they have got.
#[derive(Debug, Default, Clone)]
pub struct TutorialState {
    /// The exercise being read (an index into the exercise order).
    pub current: usize,
    /// One flag per exercise, in order: has it been done?
    pub done: Vec<bool>,
}

/// The parsed tutorial, once.
fn doc() -> &'static Manual {
    static DOC: OnceLock<Manual> = OnceLock::new();
    DOC.get_or_init(|| Manual::parse(TUTORIAL_SRC))
}

/// The nodes that are exercises, in document order.
fn exercises() -> Vec<usize> {
    let manual = doc();
    manual
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.tags.iter().any(|t| t == EXERCISE_TAG))
        .map(|(i, _)| i)
        .collect()
}

/// The number of exercises.
pub fn exercise_count() -> usize {
    exercises().len()
}

/// The command that completes exercise `n`, if it declares one.
fn exercise_command(n: usize) -> Option<String> {
    let manual = doc();
    let node = exercises().get(n).copied()?;
    manual.nodes[node]
        .property_list("COMMAND")
        .first()
        .map(|s| s.to_string())
}

/// Fill in `done` for the current exercises if the state is fresh (after a
/// reload, or when the source grew). Completed flags are kept.
fn sync(app: &mut App) {
    let n = exercise_count();
    if app.tutorial.done.len() != n {
        app.tutorial.done.resize(n, false);
        if app.tutorial.current >= n {
            app.tutorial.current = n.saturating_sub(1);
        }
    }
}

/// C-h t / `help-with-tutorial`: open the tutorial where the reader stopped.
pub fn open(app: &mut App) {
    sync(app);
    show(app);
}

/// n / `tutorial-next-exercise`: the next exercise.
pub fn next(app: &mut App) {
    let n = exercise_count();
    if n == 0 {
        return;
    }
    if app.tutorial.current + 1 < n {
        app.tutorial.current += 1;
        show(app);
    } else {
        app.msg("That is the last exercise");
    }
}

/// p / `tutorial-prev`: the previous exercise.
pub fn prev(app: &mut App) {
    if app.tutorial.current > 0 {
        app.tutorial.current -= 1;
        show(app);
    } else {
        app.msg("That is the first exercise");
    }
}

/// q / `tutorial-exit`: leave the tutorial.
pub fn exit(app: &mut App) {
    match app.other_buffer() {
        Some(id) => app.switch_to(id),
        None => app.msg("No other buffer"),
    }
}

/// After every command: if the tutorial is open, check off the exercise
/// whose `:COMMAND:` just ran (the first one still undone).
pub fn after_command(app: &mut App) {
    if app.editor.find_buffer(BUFFER).is_none() {
        return;
    }
    let cmd = app.last_command.clone();
    if cmd.is_empty() {
        return;
    }
    sync(app);
    let hit = (0..exercise_count())
        .find(|&i| !app.tutorial.done[i] && exercise_command(i).as_deref() == Some(cmd.as_str()));
    if let Some(i) = hit {
        app.tutorial.done[i] = true;
        let title = exercise_title(i);
        let done = app.tutorial.done.iter().filter(|d| **d).count();
        app.msg(format!(
            "Exercise done: {title} ({done} of {})",
            exercise_count()
        ));
        show(app);
    }
}

/// The title of exercise `n`, without its tag.
fn exercise_title(n: usize) -> String {
    doc().nodes[exercises()[n]].title.clone()
}

/// Show the tutorial in `*Tutorial*`, with links, scrolled to the current
/// exercise.
pub fn show(app: &mut App) {
    sync(app);
    let (text, links, offsets) = render(app);
    app.show_special_buffer(BUFFER, MODE, &text);
    app.buf().set_links(links);
    if let Some(off) = offsets.get(app.tutorial.current) {
        app.buf().set_offset(*off);
    }
}

/// Render the tutorial: the whole document, with each exercise's title
/// prefixed by a checkbox, and a link layer over keys and command names.
/// Also returns the char offset of each exercise heading.
fn render(app: &App) -> (String, Vec<LinkSpan>, Vec<usize>) {
    let manual = doc();
    let ex = exercises();
    let mut out = String::new();
    let mut links = Vec::new();
    let mut offsets = Vec::new();

    let done = app.tutorial.done.iter().filter(|d| **d).count();
    if !ex.is_empty() {
        out.push_str(&format!(
            "Exercise {} of {} ({} done) — n for the next, p for the previous, q to leave\n\n",
            (app.tutorial.current + 1).min(ex.len()),
            ex.len(),
            done
        ));
    }

    for (i, node) in manual.nodes.iter().enumerate() {
        if !out.is_empty() {
            out.push('\n');
        }
        // Exercise headings carry a checkbox; everyone gets an underline.
        if let Some(n) = ex.iter().position(|&e| e == i) {
            offsets.push(out.chars().count());
            let mark = if app.tutorial.done.get(n).copied().unwrap_or(false) {
                "[x] "
            } else {
                "[ ] "
            };
            let start = out.chars().count();
            out.push_str(mark);
            let _ = start;
            out.push_str(&node.title);
            let title_start = out.chars().count() - node.title.chars().count();
            links.push(LinkSpan {
                start: title_start,
                end: out.chars().count(),
                target: LinkTarget::Node(node.title.clone()),
                face: None,
            });
        } else {
            out.push_str(&node.title);
        }
        out.push('\n');
        out.push_str(&"=".repeat(node.title.chars().count() + 4));
        out.push_str("\n\n");
        crate::docview::blocks(&node.body, app, &mut out, &mut links);
    }
    (out, links, offsets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::{Buffer, Editor, Lookup};

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

    #[test]
    fn there_are_exercises_each_with_a_real_command() {
        assert!(exercise_count() >= 10, "the tutorial has exercises");
        for i in 0..exercise_count() {
            let cmd =
                exercise_command(i).unwrap_or_else(|| panic!("exercise {i} has no :COMMAND:"));
            assert!(
                cmd_exists(&cmd),
                "exercise {i} names {cmd}, which is not a command"
            );
        }
    }

    fn cmd_exists(cmd: &str) -> bool {
        let app = app();
        app.editor.commands.contains(cmd)
    }

    #[test]
    fn open_shows_the_tutorial_with_a_checkbox_per_exercise() {
        let mut a = app();
        open(&mut a);
        assert!(a.editor.find_buffer(BUFFER).is_some());
        assert_eq!(a.buf_ref().major_mode(), MODE);
        let t = text(&a);
        assert!(t.contains("Forge tutorial"));
        assert_eq!(
            t.matches("[ ] ").count(),
            exercise_count(),
            "one unchecked box per exercise"
        );
        assert!(!a.buf_ref().links().is_empty());
    }

    #[test]
    fn every_key_the_tutorial_mentions_is_bound() {
        let a = app();
        let (text, links, _) = render(&a);
        let keys = links
            .iter()
            .filter_map(|l| match &l.target {
                LinkTarget::Key(k) => Some(k.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(!keys.is_empty(), "the tutorial mentions some keys");
        let maps = a.editor.active_keymaps_now();
        for key in keys {
            let seq: Vec<String> = key.split_whitespace().map(str::to_string).collect();
            assert!(
                !matches!(maps.lookup(&seq), Lookup::Undefined),
                "{key} (mentioned in the tutorial) is not bound: {text:?}"
            );
        }
    }

    #[test]
    fn running_an_exercise_command_checks_it_off() {
        let mut a = app();
        open(&mut a);
        assert!(!a.tutorial.done[0]);
        a.run_command("next-line");
        assert!(a.tutorial.done[0], "exercise 1 is next-line");
        // The buffer now shows the box checked.
        assert!(text(&a).contains("[x] Exercise 1"));
    }

    #[test]
    fn next_and_prev_walk_the_exercises() {
        let mut a = app();
        open(&mut a);
        assert_eq!(a.tutorial.current, 0);
        next(&mut a);
        assert_eq!(a.tutorial.current, 1);
        prev(&mut a);
        assert_eq!(a.tutorial.current, 0);
        prev(&mut a);
        assert_eq!(a.tutorial.current, 0, "stops at the first");
    }

    #[test]
    fn exit_returns_to_the_buffer_before() {
        let mut a = app();
        open(&mut a);
        exit(&mut a);
        assert_ne!(a.buf_ref().name(), BUFFER);
    }
}
