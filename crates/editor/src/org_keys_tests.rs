// SPDX-License-Identifier: MIT OR Apache-2.0
//! Org behaviour checked through real key presses on an Org buffer.
#![cfg(test)]
use crate::app::App;
use forge_core::{Buffer, Editor, Pos};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn org(text: &str, line: usize, col: usize) -> App {
    let mut app = App::new(Editor::default());
    let id = app.editor.add_buffer(Buffer::new("t.org"));
    {
        let b = app.editor.state.get_buffer_mut(id).unwrap();
        b.set_contents(text);
        b.set_major_mode("org-mode");
        b.set_point(Pos { line, col });
    }
    app.editor.show_buffer(id);
    app
}
fn key(app: &mut App, spec: &str) {
    for part in spec.split(' ') {
        let mut m = KeyModifiers::NONE;
        let mut rest = part;
        loop {
            if let Some(r) = rest.strip_prefix("C-") {
                m |= KeyModifiers::CONTROL;
                rest = r;
            } else if let Some(r) = rest.strip_prefix("M-") {
                m |= KeyModifiers::ALT;
                rest = r;
            } else if let Some(r) = rest.strip_prefix("S-") {
                m |= KeyModifiers::SHIFT;
                rest = r;
            } else {
                break;
            }
        }
        let code = match rest {
            "TAB" => {
                if m.contains(KeyModifiers::SHIFT) {
                    KeyCode::BackTab
                } else {
                    KeyCode::Tab
                }
            }
            "SPC" => KeyCode::Char(' '),
            "RET" => KeyCode::Enter,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            s => KeyCode::Char(s.chars().next().unwrap()),
        };
        app.handle_key(KeyEvent::new(code, m));
    }
}
fn text(app: &App) -> String {
    app.editor.state.active_buffer().unwrap().get_contents()
}
#[allow(dead_code)]
fn show(label: &str, app: &App) {
    let b = app.editor.state.active_buffer().unwrap();
    println!(
        "--- {label} (point {:?})\n{}",
        b.cursor_pos(),
        b.get_contents()
    );
}

fn typed(app: &mut App, s: &str) {
    for c in s.chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
}

fn prompt(app: &App) -> Option<String> {
    app.minibuffer.as_ref().map(|m| m.prompt.clone())
}

fn agenda_app(text: &str) -> App {
    let mut app = App::new(Editor::default());
    let mut b = Buffer::new("tasks.org");
    b.set_contents(text);
    b.set_major_mode("org-mode");
    let id = app.editor.add_buffer(b);
    app.editor.show_buffer(id);
    app.editor.org_files_loaded = true;
    app.editor.org_files.clear();
    app
}

fn agenda_view(app: &App) -> String {
    let id = app.editor.find_buffer("*Org Agenda*").expect("agenda");
    app.editor.state.get_buffer(id).unwrap().get_contents()
}

#[test]
fn table_cells_with_links_align_by_what_shows() {
    let mut a = org("| a | [[http://x.y][link]] |\n| cccc | dddddd |\n", 0, 2);
    key(&mut a, "C-c C-c");
    assert_eq!(
        text(&a),
        "| a    | [[http://x.y][link]]   |\n| cccc | dddddd |\n"
    );
}

#[test]
fn table_keys_follow_org() {
    let t = "| a | b |\n| 1 | 2 |\n| 3 | 4 |\n";
    let mut a = org(t, 1, 2);
    key(&mut a, "C-c SPC");
    assert_eq!(text(&a), "| a | b |\n|   | 2 |\n| 3 | 4 |\n", "blank field");
    let mut a = org(t, 1, 2);
    key(&mut a, "RET");
    assert_eq!(text(&a), t, "RET only moves down a row");
    assert_eq!(a.editor.state.active_buffer().unwrap().cursor_pos().0, 2);
    let mut a = org(t, 1, 2);
    key(&mut a, "M-S-up");
    assert_eq!(text(&a), "| a | b |\n| 3 | 4 |\n", "delete row");
    let mut a = org(t, 1, 2);
    key(&mut a, "M-S-down");
    assert_eq!(text(&a), "| a | b |\n|   |   |\n| 1 | 2 |\n| 3 | 4 |\n");
    let mut a = org(t, 1, 2);
    key(&mut a, "C-c =");
    assert_eq!(prompt(&a).as_deref(), Some("Field formula: "));
    let mut a = org("| b | 2 |\n| a | 1 |\n", 0, 2);
    key(&mut a, "C-c ^");
    assert!(prompt(&a).is_some(), "C-c ^ in a table asks how to sort");
}

#[test]
fn list_items_move_cycle_and_insert_like_org() {
    let l = "- one\n- two\n  - nested\n- three\n";
    let mut a = org(l, 1, 3);
    key(&mut a, "M-RET");
    assert_eq!(
        text(&a),
        "- one\n- two\n  - nested\n- \n- three\n",
        "after the whole item"
    );
    let mut a = org(l, 1, 3);
    key(&mut a, "M-S-RET");
    assert_eq!(
        text(&a),
        "- one\n- two\n  - nested\n- [ ] \n- three\n",
        "a checkbox item"
    );
    let mut a = org(l, 1, 3);
    key(&mut a, "M-up");
    assert_eq!(text(&a), "- two\n  - nested\n- one\n- three\n");
    assert_eq!(a.editor.state.active_buffer().unwrap().cursor_pos(), (0, 3));
    let mut a = org(l, 1, 3);
    key(&mut a, "M-down");
    assert_eq!(text(&a), "- one\n- three\n- two\n  - nested\n");
    let mut a = org(l, 1, 3);
    key(&mut a, "S-right");
    assert_eq!(
        text(&a),
        "+ one\n+ two\n  - nested\n+ three\n",
        "the whole level"
    );
    assert_eq!(a.editor.state.active_buffer().unwrap().cursor_pos(), (1, 3));
    let mut a = org(l, 1, 3);
    key(&mut a, "C-c -");
    assert_eq!(text(&a), "+ one\n+ two\n  - nested\n+ three\n");
    let mut a = org(l, 1, 3);
    key(&mut a, "S-down");
    assert_eq!(
        a.editor.state.active_buffer().unwrap().cursor_pos().0,
        3,
        "next item"
    );
    let n = "1. a\n2. b\n3. c\n";
    let mut a = org(n, 0, 4);
    key(&mut a, "M-RET");
    assert_eq!(text(&a), "1. a\n2. \n3. b\n4. c\n", "renumbered");
    let mut a = org(n, 1, 3);
    key(&mut a, "M-up");
    assert_eq!(text(&a), "1. b\n2. a\n3. c\n");
}

#[test]
fn return_in_an_item_lines_up_under_its_text() {
    let mut a = org("- item\n", 0, 6);
    key(&mut a, "RET");
    assert_eq!(text(&a), "- item\n  \n");
    let mut a = org("1. item\n", 0, 7);
    key(&mut a, "RET");
    assert_eq!(text(&a), "1. item\n   \n");
}

#[test]
fn priority_and_notes_have_their_keys() {
    let mut a = org("* TODO One\n", 0, 4);
    key(&mut a, "C-c ,");
    assert!(prompt(&a).unwrap().starts_with("Priority"));
    typed(&mut a, "A");
    assert_eq!(text(&a), "* TODO [#A] One\n");
    assert_eq!(
        a.editor.state.active_buffer().unwrap().cursor_pos(),
        (0, 4),
        "point stays"
    );
    let mut a = org("* TODO One\n", 0, 4);
    key(&mut a, "C-c C-z");
    assert_eq!(prompt(&a).as_deref(), Some("Note: "));
    typed(&mut a, "hello");
    let t = text(&a);
    assert!(
        t.contains(":LOGBOOK:") && t.contains("- Note taken on [") && t.contains("hello"),
        "{t}"
    );
}

#[test]
fn changing_state_or_priority_keeps_point_on_the_heading() {
    let t = "* TODO [#B] Task\nSCHEDULED: <2026-11-02 Mon +1w>\n";
    let mut a = org(t, 0, 4);
    key(&mut a, "S-up");
    assert_eq!(a.editor.state.active_buffer().unwrap().cursor_pos(), (0, 4));
    let mut a = org(t, 0, 4);
    key(&mut a, "C-c C-t");
    key(&mut a, "C-c C-t");
    assert_eq!(
        a.editor.state.active_buffer().unwrap().cursor_pos().0,
        0,
        "a repeating task"
    );
}

#[test]
fn time_ranges_evaluate() {
    let mut a = org("<2026-11-02 Mon 09:00>--<2026-11-02 Mon 11:30>\n", 0, 3);
    key(&mut a, "C-c C-y");
    assert_eq!(
        text(&a),
        "<2026-11-02 Mon 09:00>--<2026-11-02 Mon 11:30> => 2:30\n"
    );
    let mut a = org("[2026-11-02 Mon 09:00]--[2026-11-03 Tue 11:30]\n", 0, 3);
    key(&mut a, "C-c C-y");
    assert!(text(&a).ends_with(" => 1d 2:30\n"), "{}", text(&a));
}

#[test]
fn the_agenda_is_read_only_and_formatted() {
    // Dates relative to today, since the agenda is laid out from the real date.
    let (t, _) = crate::org_cmds::now();
    let d = |n: i64| {
        let x = t.add_days(n);
        format!("{:04}-{:02}-{:02} {}", x.y, x.m, x.d, x.day_name())
    };
    let text = format!("#+TODO: TODO NEXT | DONE\n* TODO [#A] Write report :work:\nDEADLINE: <{}>\n* NEXT Call bob :phone:\nSCHEDULED: <{}>\n* TODO Repeater\nSCHEDULED: <{} 09:00 +1d>\n", d(2), d(0), d(0));
    let mut a = agenda_app(&text);
    crate::commands::execute(&mut a, "org-agenda-list");
    let v = agenda_view(&a);
    assert!(v.contains("9:00...... Scheduled:  TODO Repeater"), "{v}");
    assert!(v.contains("In   2 d.:  TODO [#A] Write report"), "{v}");
    assert!(!v.contains("(today)"), "{v}");
    key(&mut a, "i");
    assert_eq!(agenda_view(&a), v, "typing does not edit the agenda");
}

#[test]
fn the_todo_list_sorts_by_state_then_priority_and_shows_no_dates() {
    let text = "#+TODO: TODO NEXT | DONE\n* TODO [#A] Write report :work:\nDEADLINE: <2026-11-04 Wed>\n* NEXT Call bob\nSCHEDULED: <2026-11-02 Mon>\n* TODO Plain\n";
    let mut a = agenda_app(text);
    crate::commands::execute(&mut a, "org-todo-list");
    let v = agenda_view(&a);
    let order: Vec<&str> = v.lines().skip(1).collect();
    assert!(order[0].contains("NEXT Call bob"), "{v}");
    assert!(order[1].contains("[#A] Write report"), "{v}");
    assert!(order[2].contains("Plain"), "{v}");
    assert!(!v.contains("2026-11"), "{v}");
}

#[test]
fn agenda_keys_shift_dates_and_reach_the_usual_commands() {
    let text = "* TODO Write report\nDEADLINE: <2026-11-04 Wed>\n";
    let mut a = agenda_app(text);
    crate::commands::execute(&mut a, "org-agenda-list");
    key(&mut a, "n");
    key(&mut a, "S-right");
    let id = a.editor.find_buffer("tasks.org").unwrap();
    assert!(
        a.editor
            .state
            .get_buffer(id)
            .unwrap()
            .get_contents()
            .contains("<2026-11-05 Thu>")
    );
    key(&mut a, "n");
    key(&mut a, "C-c ,");
    assert!(
        prompt(&a).unwrap().starts_with("Priority"),
        "{:?}",
        prompt(&a)
    );
}

#[test]
fn dates_can_be_typed_as_month_names() {
    use forge_core::org_time::{Date, parse_date_input};
    let today = Date::new(2026, 11, 2);
    let iso = |s: &str| parse_date_input(s, today, today).map(|(d, _)| d.iso());
    assert_eq!(iso("nov 20").as_deref(), Some("2026-11-20"));
    assert_eq!(iso("20 nov 2027").as_deref(), Some("2027-11-20"));
    assert_eq!(iso("2026-11-05 Thu").as_deref(), Some("2026-11-05"));
    assert_eq!(iso("++1w").as_deref(), Some("2026-11-09"));
    assert_eq!(
        parse_date_input("dec 5 14:30", today, today).map(|(d, t)| (d.iso(), t)),
        Some(("2026-12-05".to_string(), Some((14, 30))))
    );
}

#[test]
fn deleting_a_table_row_leaves_the_text_after_the_table_alone() {
    let t = "| a | b |\n| 1 | 2 |\n| 3 | 4 |\nafter the table\n";
    let mut a = org(t, 1, 2);
    key(&mut a, "M-S-up");
    assert_eq!(text(&a), "| a | b |\n| 3 | 4 |\nafter the table\n");
    // The last row too.
    let mut a = org(t, 2, 2);
    key(&mut a, "M-S-up");
    assert_eq!(text(&a), "| a | b |\n| 1 | 2 |\nafter the table\n");
}

#[test]
fn fast_todo_selection_and_closing_notes() {
    let t = "#+TODO: TODO(t) NEXT(n) | DONE(d) CANCELLED(c)\n* TODO Task\n";
    let mut a = org(t, 1, 3);
    key(&mut a, "C-c C-t");
    assert_eq!(prompt(&a).as_deref(), Some("Select todo state: "));
    typed(&mut a, "n");
    assert_eq!(
        text(&a),
        "#+TODO: TODO(t) NEXT(n) | DONE(d) CANCELLED(c)\n* NEXT Task\n"
    );
    assert_eq!(a.editor.state.active_buffer().unwrap().cursor_pos(), (1, 3));
    // Without keys it just cycles.
    let mut a = org("* TODO Task\n", 0, 3);
    key(&mut a, "C-c C-t");
    assert_eq!(text(&a), "* DONE Task\n");
    // lognotedone asks for a note and files it under the heading.
    let mut a = org("#+STARTUP: lognotedone\n* TODO Task\n", 1, 3);
    key(&mut a, "C-c C-t");
    assert!(
        prompt(&a).unwrap().starts_with("Closing note"),
        "{:?}",
        prompt(&a)
    );
    typed(&mut a, "shipped");
    let t = text(&a);
    assert!(
        t.contains("CLOSED: [") && t.contains("- CLOSING NOTE [") && t.contains("shipped"),
        "{t}"
    );
}

#[test]
fn the_week_view_has_a_time_grid_a_now_line_and_flushed_tags() {
    let today = crate::org_cmds::now().0.iso();
    let text = format!(
        "* TODO Standup :work:\nSCHEDULED: <{today} 09:00>\n* TODO Untimed\nSCHEDULED: <{today}>\n"
    );
    let mut a = agenda_app(&text);
    crate::commands::execute(&mut a, "org-agenda-list");
    let v = agenda_view(&a);
    let lines: Vec<&str> = v.lines().collect();
    let at = |needle: &str| lines.iter().position(|l| l.contains(needle)).expect(needle);
    // Grid rows sit around the 9:00 entry, in time order; untimed last.
    assert!(
        lines
            .iter()
            .any(|l| l.contains(" 8:00...... ----------------")),
        "{v}"
    );
    assert!(
        at(" 8:00......") < at("9:00...... Scheduled:")
            && at("9:00...... Scheduled:") < at("10:00......"),
        "{v}"
    );
    assert!(at("20:00......") < at("Untimed"), "{v}");
    assert!(lines.iter().any(|l| l.contains("now - - - -")), "{v}");
    // Tags end at the right edge (80 columns when the window is unknown).
    let standup = lines[at("Standup")];
    assert!(standup.ends_with(":work:"), "{standup}");
    assert_eq!(forge_core::width::line_width(standup), 79, "{standup}");
}

fn agenda_text(a: &App) -> String {
    let id = a
        .editor
        .find_buffer("*Org Agenda*")
        .expect("an agenda buffer");
    a.editor.state.get_buffer(id).unwrap().get_contents()
}

#[test]
fn the_dispatcher_has_the_rest_of_the_agenda_keys() {
    use crate::org_cmds::Ask;
    let text = "#+TODO: TODO WAIT | DONE\n* TODO Alpha plan\n* NOTE alpha notes\n* TODO Beta\n** WAIT part\n";
    let mut a = agenda_app(text);
    let dispatch = |a: &mut App, k: &str| {
        crate::org_agenda_ui::answer(a, &Ask::AgendaKey, k);
    };
    let answer = |a: &mut App, ask: Ask, s: &str| {
        crate::org_agenda_ui::answer(a, &ask, s);
    };
    // s finds both; S only the TODO entry.
    dispatch(&mut a, "s");
    answer(&mut a, Ask::AgendaSearch, "alpha");
    assert!(agenda_text(&a).contains("Alpha plan") && agenda_text(&a).contains("alpha notes"));
    answer(&mut a, Ask::AgendaSearchTodo, "alpha");
    let t = agenda_text(&a);
    assert!(
        t.contains("Alpha plan") && !t.contains("alpha notes"),
        "{t}"
    );
    // / finds a regexp.
    answer(&mut a, Ask::AgendaOccur, "be.a");
    assert!(agenda_text(&a).contains("Beta"));
    // ! changes what makes a project stuck: with WAIT active, Beta is not stuck.
    crate::org_agenda_ui::answer(&mut a, &Ask::AgendaStuckConfig, "TODO | WAIT");
    dispatch(&mut a, "#");
    assert!(!agenda_text(&a).contains("Beta"), "{}", agenda_text(&a));
    crate::org_agenda_ui::answer(&mut a, &Ask::AgendaStuckConfig, "");
    dispatch(&mut a, "#");
    assert!(agenda_text(&a).contains("Beta"));
    // C defines a custom key.
    crate::org_agenda_ui::answer(&mut a, &Ask::AgendaCustomConfig, "w search alpha");
    dispatch(&mut a, "w");
    assert!(agenda_text(&a).contains("Alpha plan"));
    crate::org_agenda_ui::answer(&mut a, &Ask::AgendaCustomConfig, "w");
    assert!(a.org.custom_agenda.is_empty());
}

#[test]
fn the_dispatcher_is_a_window_that_waits_for_one_key_then_goes() {
    let mut a = agenda_app("* TODO Alpha\n");
    let before = a.editor.windows.count();
    crate::commands::execute(&mut a, "org-agenda");
    assert!(a.one_key.is_some());
    assert_eq!(a.editor.windows.count(), before + 1, "its own window");
    let menu = a.editor.find_buffer("*Agenda Commands*").expect("the menu buffer");
    assert!(
        a.editor.state.get_buffer(menu).unwrap().get_contents().contains("Press key for an agenda command:"),
    );
    // A key that is no command is refused and the menu stays.
    key(&mut a, "z");
    assert!(a.one_key.is_some() && a.editor.find_buffer("*Agenda Commands*").is_some());
    // C-g closes it and leaves nothing behind.
    key(&mut a, "C-g");
    assert!(a.one_key.is_none());
    assert!(a.editor.find_buffer("*Agenda Commands*").is_none());
    assert_eq!(a.editor.windows.count(), before);
    // A real key runs the command after the menu is gone.
    crate::commands::execute(&mut a, "org-agenda");
    key(&mut a, "t");
    assert!(a.one_key.is_none());
    assert!(a.editor.find_buffer("*Agenda Commands*").is_none());
    assert!(agenda_view(&a).contains("Global list of TODO items"), "{}", agenda_view(&a));
}
