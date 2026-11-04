// SPDX-License-Identifier: MIT OR Apache-2.0
//! completion tests.

use super::*;

#[test]
fn vertical_minibuffer_completion() {
    use forge_core::completion::Style;
    let mut app = app_with("");
    app.editor.completion.vertical = Some((5, "test".into()));
    app.editor.completion.styles = Some((vec![Style::Orderless], "test".into()));
    press(&mut app, "M-x");
    answer_typed(&mut app, "buf kill");
    let (rows, count) = crate::minibuffer::vertical_view(&mut app).unwrap();
    assert_eq!(rows[0].0, "kill-buffer");
    assert!(rows[0].2, "the first is selected");
    assert!(count.starts_with("1/"), "{count}");
    // Moving and picking.
    app.minibuffer.as_mut().unwrap().input.clear();
    answer_typed(&mut app, "goto-l");
    press(&mut app, "RET");
    assert_eq!(
        app.minibuffer.as_ref().map(|m| m.prompt.as_str()),
        Some("Goto line: ")
    );
    press(&mut app, "C-g");

    press(&mut app, "C-x b");
    let (rows, _) = crate::minibuffer::vertical_view(&mut app).unwrap();
    assert!(!rows.is_empty());
    press(&mut app, "C-g");

    // The popup at point, from buffer words.
    app.editor.completion.at_point = Some((
        forge_core::completion::AtPoint {
            popup: true,
            ..Default::default()
        },
        "t".into(),
    ));
    let mut app2 = app_with("alpha alphabet alpine\n");
    app2.editor.completion = app.editor.completion.clone();
    press(&mut app2, "M->");
    press(&mut app2, "alp");
    press(&mut app2, "C-M-i");
    let popup = crate::capf::popup(&app2).unwrap();
    let names: Vec<&str> = popup.rows.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(names, ["alpine", "alphabet", "alpha"]);
    press(&mut app2, "h");
    let names: Vec<String> = crate::capf::popup(&app2)
        .unwrap()
        .rows
        .into_iter()
        .map(|r| r.text)
        .collect();
    assert_eq!(names, ["alphabet", "alpha"], "filtered as you type");
    press(&mut app2, "C-n RET");
    assert_eq!(text(&app2), "alpha alphabet alpine\nalpha");
    assert!(crate::capf::popup(&app2).is_none());
    // Redisplay puts the popup under the word.
    press(&mut app2, "RET alp C-M-i");
    let mut term = Terminal::new(TestBackend::new(40, 10)).unwrap();
    term.draw(|f| {
        ui::render(f, &mut app2);
    })
    .unwrap();
    let screen = format!("{:?}", term.backend().buffer());
    assert!(screen.contains(" alpine"), "{screen}");
    press(&mut app2, "C-g");
    assert!(crate::capf::popup(&app2).is_none());

    // Without the popup: the common part, and the list in the echo area.
    let mut app3 = app_with("alpha alphabet\n");
    press(&mut app3, "M-> al C-M-i");
    assert_eq!(text(&app3), "alpha alphabet\nalpha");
    assert_eq!(app3.message.as_deref(), Some("{alphabet | alpha}"));
}

#[test]
fn m_tab_expands_to_the_common_prefix() {
    let mut app = app_with("formally format formatter\n");
    app.editor.completion.at_point = Some((
        forge_core::completion::AtPoint {
            popup: true,
            ..Default::default()
        },
        "t".into(),
    ));
    // What the corfu plugin sets: shortest first, then alphabetical.
    app.editor.completion.sorting = Some((vec![forge_core::completion::Sort::Length], "t".into()));
    press(&mut app, "M->");
    press(&mut app, "fo");
    press(&mut app, "C-M-i");
    let names: Vec<String> = crate::capf::popup(&app)
        .unwrap()
        .rows
        .into_iter()
        .map(|r| r.text)
        .collect();
    // Length first, then alphabetical: 6, 6 and 9.
    assert_eq!(names, ["format", "formally", "formatter"], "length order");

    // M-TAB: "fo" -> "forma", the longest prefix they all share.
    press(&mut app, "M-TAB");
    assert_eq!(text(&app), "formally format formatter\nforma");
    assert!(crate::capf::popup(&app).is_some(), "the popup stays open");
    let names: Vec<String> = crate::capf::popup(&app)
        .unwrap()
        .rows
        .into_iter()
        .map(|r| r.text)
        .collect();
    // Every match shares the new prefix, so all of them still match: an
    // expansion can't change the candidate set, only what point is on.
    assert_eq!(names, ["format", "formally", "formatter"]);
    // Nothing more to expand, so the key falls through to the ordinary
    // M-TAB binding (completion-at-point).
    press(&mut app, "M-TAB");
    assert_eq!(text(&app), "formally format formatter\nforma");
    press(&mut app, "C-g");
}

#[test]
fn quickselect_picks_a_candidate_by_its_key() {
    let mut app = app_with("alpha alphabet alpine\n");
    app.editor.completion.at_point = Some((
        forge_core::completion::AtPoint {
            popup: true,
            ..Default::default()
        },
        "t".into(),
    ));
    set_policy(&mut app, |p| p.quick1 = "as".into());
    app.editor.completion.sorting = Some((vec![forge_core::completion::Sort::Length], "t".into()));
    press(&mut app, "M->");
    press(&mut app, "alp");
    press(&mut app, "C-M-i");
    let names: Vec<String> = crate::capf::popup(&app)
        .unwrap()
        .rows
        .into_iter()
        .map(|r| r.text)
        .collect();
    assert_eq!(names, ["alpha", "alpine", "alphabet"]);
    // Letters are just letters until quick select is asked for.
    crate::capf::cmd_quick(&mut app, forge_core::completion::QuickAction::Insert);
    // "a" is the first row's key.
    press(&mut app, "a");
    assert_eq!(text(&app), "alpha alphabet alpine\nalpha");
    assert!(crate::capf::popup(&app).is_none(), "it inserts and quits");

    // "s" names the second of the two matches (alpha, alphabet).
    let mut app2 = app_with("alpha alphabet\n");
    app2.editor.completion.at_point = app.editor.completion.at_point.clone();
    set_policy(&mut app2, |p| p.quick1 = "as".into());
    app2.editor.completion.sorting = Some((vec![forge_core::completion::Sort::Length], "t".into()));
    press(&mut app2, "M->");
    press(&mut app2, "alp");
    press(&mut app2, "C-M-i");
    crate::capf::cmd_quick(&mut app2, forge_core::completion::QuickAction::Insert);
    press(&mut app2, "s");
    assert_eq!(text(&app2), "alpha alphabet\nalphabet");
}

#[test]
fn typing_a_quick_key_letter_types_it() {
    let mut app = popup_app("alpha alphabet alpine\n");
    press(&mut app, "a");
    assert_eq!(
        text(&app),
        "alpha alphabet alpine\nalpa",
        "no quick select unless asked"
    );
}

#[test]
fn corfu_moves_first_last_pages_and_cycles() {
    let mut app = popup_app("alpha alphabet alpine alpaca alpacas\n");
    assert_eq!(popup_selected(&app).as_deref(), Some("alpha"));
    press(&mut app, "C-p");
    assert_eq!(
        popup_selected(&app).as_deref(),
        Some("alphabet"),
        "C-p on the first wraps to the last"
    );
    crate::capf::cmd_first(&mut app);
    assert_eq!(popup_selected(&app).as_deref(), Some("alpha"));
    crate::capf::cmd_last(&mut app);
    assert_eq!(popup_selected(&app).as_deref(), Some("alphabet"));
    set_policy(&mut app, |p| p.cycle = false);
    press(&mut app, "C-n");
    assert_eq!(
        popup_selected(&app).as_deref(),
        Some("alphabet"),
        "without cycling it stops"
    );
    app.editor.completion.at_point = app.editor.completion.at_point.take().map(|(mut a, p)| {
        a.lines = 3;
        (a, p)
    });
    crate::capf::cmd_first(&mut app);
    crate::capf::cmd_scroll(&mut app, true);
    assert_eq!(
        popup_selected(&app).as_deref(),
        Some("alpine"),
        "a page down is lines-1 = 2 rows"
    );
    crate::capf::cmd_scroll(&mut app, false);
    assert_eq!(popup_selected(&app).as_deref(), Some("alpha"));
}

#[test]
fn corfu_reset_deselects_then_quits_and_prompt_preselect_waits() {
    let mut app = popup_app("alpha alphabet alpine\n");
    crate::capf::cmd_reset(&mut app);
    assert!(
        crate::capf::popup(&app).is_some() && popup_selected(&app).is_none(),
        "nothing selected, popup open"
    );
    press(&mut app, "RET");
    assert!(
        crate::capf::popup(&app).is_none(),
        "RET with nothing selected only closes"
    );
    assert_eq!(text(&app), "alpha alphabet alpine\nalp");
    // Preselect 'prompt: nothing selected until a move.
    let mut app = app_with("alpha alphabet alpine\n");
    set_policy(&mut app, |p| p.preselect_prompt = true);
    app.editor.completion.at_point = Some((
        forge_core::completion::AtPoint {
            popup: true,
            ..Default::default()
        },
        "t".into(),
    ));
    press(&mut app, "M->");
    press(&mut app, "alp");
    press(&mut app, "C-M-i");
    assert!(popup_selected(&app).is_none());
    press(&mut app, "C-n");
    assert!(popup_selected(&app).is_some());
    press(&mut app, "TAB");
    assert!(
        text(&app).ends_with("\nalpha")
            || text(&app).ends_with("\nalpine")
            || text(&app).ends_with("\nalphabet")
    );
}

#[test]
fn corfu_boundary_quit_policies() {
    use forge_core::completion::Quit;
    // The default, 'separator, keeps the popup when the separator is what
    // point crossed.
    let mut app = popup_app("alpha alphabet\n");
    press(&mut app, "SPC");
    assert!(
        app.completion.session.is_some(),
        "'separator keeps the popup across a separator"
    );

    // 'always quits as soon as point leaves the word.
    let mut app = popup_app("alpha alphabet\n");
    set_policy(&mut app, |p| p.quit_at_boundary = Quit::Always);
    press(&mut app, "SPC");
    assert!(
        app.completion.session.is_none(),
        "'always quits at the boundary"
    );

    // A continue-command keeps it even when the boundary rule says quit.
    let mut app = popup_app("alpha alphabet\n");
    set_policy(&mut app, |p| {
        p.quit_at_boundary = Quit::Always;
        p.continue_commands.push("self-insert-command".into());
    });
    press(&mut app, "SPC");
    assert!(
        app.completion.session.is_some(),
        "a continue-command keeps the popup"
    );
}

#[test]
fn corfu_no_match_quit_policies() {
    use forge_core::completion::Quit;
    // 'never keeps the (empty) session alive.
    let mut app = popup_app("alpha alphabet\n");
    set_policy(&mut app, |p| p.quit_no_match = Quit::Never);
    press(&mut app, "zzz");
    assert!(
        app.completion.session.is_some(),
        "'never keeps an empty popup"
    );

    // 'always quits once nothing matches.
    let mut app = popup_app("alpha alphabet\n");
    set_policy(&mut app, |p| p.quit_no_match = Quit::Always);
    press(&mut app, "zzz");
    assert!(
        app.completion.session.is_none(),
        "'always quits when nothing matches"
    );
}

#[test]
fn corfu_insert_separator_keeps_the_component_before_it() {
    let mut app = popup_app("alpha alphabet\n");
    crate::capf::cmd_separator(&mut app);
    assert_eq!(text(&app), "alpha alphabet\nalp ");
    assert!(
        app.completion.session.is_some(),
        "the session survives the separator"
    );
}

#[test]
fn corfu_insert_separator_lets_the_next_component_be_typed() {
    use forge_core::completion::{Quit, Style};
    let mut app = popup_app("alpha alphabet\n");
    app.editor.completion.styles = Some((vec![Style::Orderless], "t".into()));
    // A source that returns two-component candidates.
    crate::capf::delivered(
        &mut app,
        0,
        forge_core::Pos::new(1, 0),
        vec![
            forge_core::completion::Candidate::new("alpha beta", None),
            forge_core::completion::Candidate::new("gamma delta", None),
        ],
    );
    set_policy(&mut app, |p| p.quit_no_match = Quit::Never);
    crate::capf::cmd_separator(&mut app);
    press(&mut app, "b");
    assert!(
        app.completion.session.is_some(),
        "typing the next component keeps the popup open"
    );
    assert_eq!(
        popup_names(&app),
        ["alpha beta"],
        "the input spans both components"
    );
}

#[test]
fn corfu_sort_override_keeps_the_source_order() {
    let source = vec![
        forge_core::completion::Candidate::new("alphabet", None),
        forge_core::completion::Candidate::new("alpha", None),
    ];
    // Sorted by length: shortest first.
    let mut app = popup_app("alpha alphabet\n");
    app.editor.completion.sorting = Some((vec![forge_core::completion::Sort::Length], "t".into()));
    crate::capf::delivered(&mut app, 0, forge_core::Pos::new(1, 0), source.clone());
    assert_eq!(popup_names(&app), ["alpha", "alphabet"]);

    // corfu-sort-override-function: an input matching a pattern keeps the
    // source order.
    let mut app = popup_app("alpha alphabet\n");
    app.editor.completion.sorting = Some((vec![forge_core::completion::Sort::Length], "t".into()));
    app.editor.completion.sort_override = Some((vec!["^alp".into()], "t".into()));
    crate::capf::delivered(&mut app, 0, forge_core::Pos::new(1, 0), source);
    assert_eq!(popup_names(&app), ["alphabet", "alpha"]);
}

#[test]
fn corfu_preselect_valid_waits_on_a_nonfirst_exact_match() {
    use forge_core::completion::Preselect;
    let mut app = popup_app("alpha alphabet\n");
    app.editor.completion.preselect = Some((Preselect::Valid, "t".into()));
    // "alp" is not itself a candidate, so the first is selected.
    assert_eq!(popup_selected(&app).as_deref(), Some("alpha"));
    // Typing to an exact candidate that is not the first selects nothing
    // (corfu-preselect 'valid's default).
    press(&mut app, "ha");
    assert!(crate::capf::popup(&app).is_some());
    assert!(popup_selected(&app).is_none(), "valid preselect waits");
    press(&mut app, "C-n");
    assert!(popup_selected(&app).is_some());
}

#[test]
fn corfu_on_exact_match_inserts_quits_or_shows_a_lone_candidate() {
    use forge_core::completion::{Candidate, ExactMatch};
    let one = vec![Candidate::new("alpha", None)];
    // insert: the lone candidate is auto-inserted once it is the only
    // completion left.
    let mut app = popup_app("alpha\n");
    app.editor.completion.on_exact_match = Some((ExactMatch::Insert, "t".into()));
    crate::capf::delivered(&mut app, 0, forge_core::Pos::new(1, 0), one.clone());
    assert!(crate::capf::popup(&app).is_none());
    assert!(text(&app).ends_with("\nalpha"));

    // quit: the lone candidate closes the popup instead.
    let mut app = popup_app("alpha\n");
    app.editor.completion.on_exact_match = Some((ExactMatch::Quit, "t".into()));
    crate::capf::delivered(&mut app, 0, forge_core::Pos::new(1, 0), one.clone());
    assert!(crate::capf::popup(&app).is_none());

    // show: the popup stays up with the lone candidate.
    let mut app = popup_app("alpha\n");
    app.editor.completion.on_exact_match = Some((ExactMatch::Show, "t".into()));
    crate::capf::delivered(&mut app, 0, forge_core::Pos::new(1, 0), one);
    assert_eq!(popup_names(&app), ["alpha"]);
}

#[test]
fn corfu_preview_current_commits_the_selection_on_input() {
    use forge_core::completion::PreviewCurrent;
    // Off, the default: typing only refilters; nothing is committed.
    let mut app = popup_app("alpha alphabet\n");
    press(&mut app, "h");
    assert!(crate::capf::popup(&app).is_some());
    assert_eq!(text(&app), "alpha alphabet\nalph");

    // insert: the selected candidate is committed before the typed input.
    let mut app = popup_app("alpha alphabet\n");
    app.editor.completion.preview_current = Some((PreviewCurrent::Insert, "t".into()));
    assert_eq!(popup_selected(&app).as_deref(), Some("alpha"));
    press(&mut app, "h");
    assert!(
        crate::capf::popup(&app).is_none(),
        "committing closes the popup"
    );
    assert_eq!(text(&app), "alpha alphabet\nalphah");

    // A popup command is a continue command: it moves the selection, so a
    // following key commits that candidate instead of the first one.
    let mut app = popup_app("alpha alphabet\n");
    app.editor.completion.preview_current = Some((PreviewCurrent::Insert, "t".into()));
    press(&mut app, "C-n");
    assert!(crate::capf::popup(&app).is_some(), "C-n only moves");
    assert_eq!(popup_selected(&app).as_deref(), Some("alphabet"));
    press(&mut app, "e");
    assert_eq!(text(&app), "alpha alphabet\nalphabete");
}

#[test]
fn corfu_auto_mode_honors_trigger_characters_and_commands() {
    use forge_core::completion::AtPoint;
    let at = |app: &mut App| {
        app.editor.completion.at_point = Some((
            AtPoint {
                popup: true,
                auto: true,
                delay_ms: 0,
                min_prefix: 1,
                lines: 10,
            },
            "t".into(),
        ));
    };
    // Only the listed trigger character arms auto mode.
    let mut app = app_with("alpha alphabet\n");
    at(&mut app);
    app.editor.completion.auto_trigger = Some((vec!['l'], "t".into()));
    press(&mut app, "M->");
    press(&mut app, "a");
    crate::capf::on_idle(&mut app);
    assert!(crate::capf::popup(&app).is_none(), "'a' is not a trigger");
    press(&mut app, "l");
    crate::capf::on_idle(&mut app);
    assert!(crate::capf::popup(&app).is_some(), "'l' triggers auto mode");

    // A command list that doesn't include self-insert never arms.
    let mut app = app_with("alpha alphabet\n");
    at(&mut app);
    app.editor.completion.auto_commands = Some((vec!["corfu-*".into()], "t".into()));
    press(&mut app, "M->");
    press(&mut app, "al");
    crate::capf::on_idle(&mut app);
    assert!(
        crate::capf::popup(&app).is_none(),
        "self-insert isn't an auto command here"
    );
}

#[test]
fn corfu_indexed_mode_numbers_rows_and_a_prefix_takes_one() {
    let mut app = popup_app("alpha alphabet alpine\n");
    set_policy(&mut app, |p| p.indexed = true);
    assert_eq!(popup_names(&app), ["0 alpha", "1 alpine", "2 alphabet"]);
    press(&mut app, "C-u");
    press(&mut app, "2");
    press(&mut app, "TAB");
    assert_eq!(text(&app), "alpha alphabet alpine\nalphabet");
}

#[test]
fn corfu_quick_jump_selects_and_labels_rows() {
    let mut app = popup_app("alpha alphabet alpine\n");
    crate::capf::cmd_quick(&mut app, forge_core::completion::QuickAction::Jump);
    assert_eq!(popup_names(&app), ["a alpha", "s alpine", "d alphabet"]);
    press(&mut app, "d");
    assert_eq!(
        popup_selected(&app).as_deref(),
        Some("alphabet"),
        "labels go away once a key is used"
    );
    assert!(crate::capf::popup(&app).is_some(), "jump only selects");
}

#[test]
fn corfu_history_mode_remembers_and_ranks_across_sessions() {
    let dir = std::env::temp_dir().join(format!("forge-corfu-hist-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let file = dir.join("hist");
    let mut app = popup_app("alpha alphabet alpine\n");
    app.completion.history_file = Some(file.clone());
    set_policy(&mut app, |p| p.history = true);
    press(&mut app, "C-n");
    press(&mut app, "C-n");
    press(&mut app, "TAB");
    assert!(text(&app).ends_with("\nalphabet"));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "alphabet\n");
    // A fresh editor, same file: alphabet comes first now (history order).
    let mut app = app_with("alpha alphabet alpine\n");
    app.completion.history_file = Some(file.clone());
    app.editor.completion.at_point = Some((
        forge_core::completion::AtPoint {
            popup: true,
            ..Default::default()
        },
        "t".into(),
    ));
    app.editor.completion.sorting = Some((vec![forge_core::completion::Sort::History], "t".into()));
    set_policy(&mut app, |p| p.history = true);
    press(&mut app, "M->");
    press(&mut app, "alp");
    press(&mut app, "C-M-i");
    assert_eq!(popup_names(&app)[0], "alphabet");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn corfu_echo_mode_shows_the_note_after_the_delay() {
    let mut app = popup_app("alpha alphabet alpine\n");
    crate::capf::delivered(
        &mut app,
        0,
        forge_core::Pos::new(1, 0),
        vec![
            forge_core::completion::Candidate::new("alpha", Some("a note".into())),
            forge_core::completion::Candidate::new("alpine", None),
        ],
    );
    set_policy(&mut app, |p| {
        p.echo = true;
        p.echo_delay_ms = 0;
    });
    app.message = None;
    crate::capf::on_idle(&mut app);
    assert!(
        app.message.is_none(),
        "the first look only starts the clock"
    );
    crate::capf::on_idle(&mut app);
    assert_eq!(app.message.as_deref(), Some("alpha: a note"));
    assert!(crate::capf::echo_wait(&app).is_none(), "shown once");
}

#[test]
fn a_candidate_can_carry_a_margin_and_be_deprecated() {
    let mut app = popup_app("alpha alpine\n");
    crate::capf::delivered(
        &mut app,
        0,
        forge_core::Pos::new(1, 0),
        vec![
            forge_core::completion::Candidate {
                text: "alpha".into(),
                annotation: Some("a note".into()),
                margin: Some("f".into()),
                deprecated: false,
                documentation: None,
                location: None,
            },
            forge_core::completion::Candidate {
                text: "alpine".into(),
                annotation: None,
                margin: Some("v".into()),
                deprecated: true,
                documentation: None,
                location: None,
            },
        ],
    );
    let p = crate::capf::popup(&app).unwrap();
    assert_eq!(p.input, "alp", "the popup knows the typed text");
    assert_eq!(p.rows[0].margin, "f");
    assert!(!p.rows[0].deprecated);
    assert_eq!(p.rows[1].margin, "v");
    assert!(p.rows[1].deprecated);
    assert_eq!(p.rows[1].annotation, "");
}

#[test]
fn a_selection_carries_documentation_and_a_location() {
    let mut app = popup_app("alpha alpine\n");
    crate::capf::delivered(
        &mut app,
        0,
        forge_core::Pos::new(1, 0),
        vec![
            forge_core::completion::Candidate {
                text: "alpha".into(),
                annotation: None,
                margin: None,
                deprecated: false,
                documentation: Some("The first letter.".into()),
                location: Some(forge_core::completion::Location {
                    path: Some("/src/foo.rs".into()),
                    line: 3,
                    column: 2,
                }),
            },
            forge_core::completion::Candidate {
                text: "alpine".into(),
                annotation: None,
                margin: None,
                deprecated: false,
                documentation: Some("A range.".into()),
                location: None,
            },
        ],
    );
    let sel = crate::capf::selected(&app).expect("a match is selected");
    let (doc, loc) = if sel.text == "alpha" {
        (
            "The first letter.",
            Some(forge_core::completion::Location {
                path: Some("/src/foo.rs".into()),
                line: 3,
                column: 2,
            }),
        )
    } else {
        ("A range.", None)
    };
    assert_eq!(sel.documentation.as_deref(), Some(doc));
    assert_eq!(sel.location, loc);
}

#[test]
fn the_auxiliary_popup_draws_beside_the_popup() {
    // corfu-popupinfo: a plugin asks for documentation, and the selected
    // candidate's text shows in a second popup next to the first.
    let mut app = popup_app("alpha alpine\n");
    crate::capf::delivered(
        &mut app,
        0,
        forge_core::Pos::new(1, 0),
        vec![forge_core::completion::Candidate {
            text: "alpha".into(),
            annotation: None,
            margin: None,
            deprecated: false,
            documentation: Some("The first letter of the alphabet.".into()),
            location: None,
        }],
    );
    app.editor.completion.info = Some((
        forge_core::completion::Info {
            show_documentation: true,
            show_location: false,
            delay_ms: 0,
            max_width: 40,
            max_height: 10,
        },
        "test".into(),
    ));
    let spec = crate::capf::popup_info(&app).expect("the auxiliary popup shows");
    assert!(
        spec.lines.iter().any(|l| l.contains("first letter")),
        "{:?}",
        spec.lines
    );
    let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
    term.draw(|f| {
        ui::render(f, &mut app);
    })
    .unwrap();
    let screen = format!("{:?}", term.backend().buffer());
    assert!(screen.contains("first letter"), "{screen}");
}

#[test]
fn completing_read_multiple_completes_and_validates_each_field() {
    // A Rhai command reads several values, the way completing-read-multiple
    // does: fields split on commas (or tabs), each one completed on its own.
    let (mut app, dir) = app_with_init(
        r#"
        forge.command.create("pick-words", || {
          forge.ui.select(["alpha", "alpine", "alphabet"],
            #{ prompt: "Words: ", multiple: true },
            |items| {
              if items != () {
                let text = "";
                for item in items {
                  if text != "" { text += "|"; }
                  text += item;
                }
                forge.buffer.current().insert(text);
              }
            });
        });
        "#,
    );

    press(&mut app, "M-x pick-words RET");
    assert!(
        app.minibuffer.is_some(),
        "the command opened its multiple prompt"
    );

    // The first field completes to what all three share.
    answer_typed(&mut app, "al");
    press(&mut app, "TAB");
    assert_eq!(app.minibuffer.as_ref().unwrap().input, "alp");

    // Typing a separator keeps what came before and completes the next
    // field on its own.
    answer_typed(&mut app, "ha,");
    press(&mut app, "TAB");
    assert_eq!(app.minibuffer.as_ref().unwrap().input, "alpha,alp");

    answer_typed(&mut app, "ine");
    press(&mut app, "RET");
    assert!(app.minibuffer.is_none(), "a valid list exits");
    assert_eq!(text(&app), "alpha|alpine");

    // A field that isn't a candidate keeps the prompt open.
    press(&mut app, "M-x pick-words RET");
    answer_typed(&mut app, "alpha,zzz");
    press(&mut app, "RET");
    let note = app
        .minibuffer
        .as_ref()
        .and_then(|m| m.note.clone())
        .unwrap_or_default();
    assert!(note.contains("No match"), "{note}");
    press(&mut app, "C-g");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_conditional_layer_takes_its_key_only_while_it_holds() {
    let mut app = app_with("one two\nthree\n");
    // A layer with no condition is never up, so binding into it by mistake
    // can't take the key away.
    app.editor
        .keymaps
        .bind(
            &forge_core::Scope::Layer("region".into()),
            "C-f",
            "kill-line",
            forge_core::keymap::Origin::User,
        )
        .unwrap();
    press(&mut app, "C-f");
    assert_eq!(
        app.last_command, "forward-char",
        "with no condition, the global binding answers"
    );
    // With one, it does.
    app.editor
        .keymaps
        .add_layer("region", forge_core::keymap::Condition::RegionActive);
    press(&mut app, "C-f");
    assert_eq!(app.last_command, "forward-char", "and no region active yet");
    press(&mut app, "C-SPC");
    assert!(app.buf_ref().mark_active(), "a region is active");
    press(&mut app, "C-f");
    assert_eq!(
        app.last_command, "kill-line",
        "so the layer answers, and being above the modes it wins"
    );
}

#[test]
fn a_popup_key_can_be_rebound_without_taking_the_rest() {
    let mut app = app_with("alpha alphabet\n");
    app.editor.completion.at_point = Some((
        forge_core::completion::AtPoint {
            popup: true,
            ..Default::default()
        },
        "t".into(),
    ));
    app.editor.completion.sorting = Some((vec![forge_core::completion::Sort::Length], "t".into()));
    // A plugin's own key in the popup's map, the way corfu binds M-g there.
    app.editor
        .keymaps
        .bind(
            &forge_core::Scope::Prompt(crate::capf::PROMPT.into()),
            "M-1",
            "completion-popup-next",
            forge_core::keymap::Origin::Package,
        )
        .unwrap();
    press(&mut app, "M->");
    press(&mut app, "alp");
    press(&mut app, "C-M-i");
    assert!(crate::capf::popup(&app).is_some(), "the popup is open");
    press(&mut app, "M-1");
    assert!(
        crate::capf::popup(&app).is_some(),
        "the rebound key ran in the popup and it stayed open"
    );
    // A key the popup doesn't bind falls through to the editor, so it types
    // and the popup filters out of existence instead of eating it.
    press(&mut app, "z");
    assert!(
        crate::capf::popup(&app).is_none(),
        "nothing matches, so the popup is gone"
    );
    assert_eq!(
        text(&app),
        "alpha alphabet\nalpz",
        "and the key typed as usual"
    );
}

#[test]
fn the_plugin_can_ask_about_the_candidate_under_point() {
    let mut app = app_with("alpha alphabet\n");
    app.editor.completion.at_point = Some((
        forge_core::completion::AtPoint {
            popup: true,
            ..Default::default()
        },
        "t".into(),
    ));
    press(&mut app, "M->");
    press(&mut app, "C-n");
    press(&mut app, "alp");
    press(&mut app, "C-M-i");
    let sel = crate::minibuffer::selected(&app).or_else(|| crate::capf::selected(&app));
    let sel = sel.expect("a list is open");
    assert!(
        ["alpha", "alphabet"].contains(&sel.text.as_str()),
        "{}",
        sel.text
    );
    assert_eq!(sel.index, 1);
    assert!(sel.total >= 2);
    press(&mut app, "C-n");
    let moved = crate::capf::selected(&app).expect("still open");
    assert_eq!(moved.index, 2);
    press(&mut app, "C-g");
    assert!(crate::capf::selected(&app).is_none());
}

#[test]
fn keys_run_as_if_typed_and_esc_can_be_bound() {
    let mut app = app_with("abc");
    app.execute_keys("M-> x y C-a C-k");
    assert_eq!(text(&app), "");
    assert_eq!(app.editor.kill_ring.current(), Some("abcxy"));
    // With ESC bound on its own (a modal plugin's insert state), it's a
    // key, not Meta.
    app.editor
        .modes
        .define_minor("t-insert-mode", Some("t"))
        .unwrap();
    app.buf().enable_minor_mode("t-insert-mode".to_string());
    app.editor
        .keymaps
        .bind(
            &forge_core::Scope::Mode("t-insert-mode".into()),
            "ESC",
            "end-of-buffer",
            forge_core::Origin::Package,
        )
        .unwrap();
    app.buf().insert_str("one two");
    press(&mut app, "C-a ESC");
    assert_eq!(point(&app), Pos::new(0, 7));
    // Elsewhere ESC is still Meta.
    app.buf().disable_minor_mode("t-insert-mode");
    press(&mut app, "C-a ESC f");
    assert_eq!(point(&app), Pos::new(0, 3));
    // The cursor shape option reaches the frontend.
    app.buf().set_option("cursor", "bar").unwrap();
    let mut term = Terminal::new(TestBackend::new(30, 5)).unwrap();
    term.draw(|f| {
        ui::render(f, &mut app);
    })
    .unwrap();
    assert_eq!(app.cursor_shape, forge_core::decoration::CursorShape::Bar);
}

#[test]
fn moving_a_completion_to_the_minibuffer_picks_and_inserts() {
    let mut app = popup_app("alpha alphabet alpine\n");
    assert!(crate::capf::popup(&app).is_some(), "open before the move");
    // M-m: corfu-move-to-minibuffer.
    press(&mut app, "M-m");
    assert!(crate::capf::popup(&app).is_none(), "the popup gives way");
    let mb = app.minibuffer.as_ref().expect("a prompt is up");
    assert_eq!(mb.input, "alp", "the typed text carries into the prompt");
    // The list is up with no plugin listening, filtered by the input.
    let (rows, count) = crate::minibuffer::vertical_view(&mut app).unwrap();
    let names: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(names, ["alpha", "alpine", "alphabet"]);
    assert!(count.starts_with("1/3"), "{count}");
    // Pick the last (C-n twice) and RET: it replaces the word where it was.
    press(&mut app, "C-n C-n RET");
    assert_eq!(text(&app), "alpha alphabet alpine\nalphabet");
    assert!(app.minibuffer.is_none());

    // C-g gives up and leaves the buffer as it was.
    let mut app2 = popup_app("alpha alphabet alpine\n");
    press(&mut app2, "M-m");
    press(&mut app2, "C-g");
    assert_eq!(text(&app2), "alpha alphabet alpine\nalp");
    assert!(app2.minibuffer.is_none());
}

#[test]
fn the_list_first_and_last_keys_jump_the_highlight() {
    let mut app = popup_app("alpha alphabet alpine\n");
    press(&mut app, "M-m");
    // M-< and M-> are bound by the corfu plugin; these are the host
    // commands they run.
    crate::minibuffer::cmd_list_last(&mut app);
    assert_eq!(
        crate::minibuffer::selected(&app).map(|s| s.text),
        Some("alphabet".to_string())
    );
    crate::minibuffer::cmd_list_first(&mut app);
    assert_eq!(
        crate::minibuffer::selected(&app).map(|s| s.text),
        Some("alpha".to_string())
    );
}

#[test]
fn the_completion_controls_drive_the_minibuffer_list() {
    // A completion plugin like vertico moves the prompt's list through the
    // same controls corfu uses for its popup (completion.first/.last/...):
    // with no popup open, they land on the minibuffer's list.
    let mut app = popup_app("alpha alphabet alpine\n");
    press(&mut app, "M-m");
    for (control, want) in [
        (forge_core::completion::Control::Last, "alphabet"),
        (forge_core::completion::Control::First, "alpha"),
        (forge_core::completion::Control::Next(1), "alpine"),
        (forge_core::completion::Control::Previous(1), "alpha"),
    ] {
        app.editor.completion_control.push(control);
        crate::capf::pump(&mut app);
        assert_eq!(
            crate::minibuffer::selected(&app).map(|s| s.text),
            Some(want.to_string())
        );
    }
}

#[test]
fn the_list_wraps_at_the_ends_when_vertico_cycle_is_on() {
    let mut app = popup_app("alpha alphabet alpine\n");
    press(&mut app, "M-m");
    // vertico-cycle off (the default): C-n at the last stays there.
    crate::minibuffer::cmd_list_last(&mut app);
    crate::minibuffer::cmd_list_next(&mut app);
    assert_eq!(
        crate::minibuffer::selected(&app).map(|s| s.text),
        Some("alphabet".to_string())
    );
    // On: C-n wraps to the first, and C-p from the first to the last.
    app.editor.completion.list_cycle = Some((true, "t".to_string()));
    crate::minibuffer::cmd_list_next(&mut app);
    assert_eq!(
        crate::minibuffer::selected(&app).map(|s| s.text),
        Some("alpha".to_string())
    );
    crate::minibuffer::cmd_list_prev(&mut app);
    assert_eq!(
        crate::minibuffer::selected(&app).map(|s| s.text),
        Some("alphabet".to_string())
    );
}
