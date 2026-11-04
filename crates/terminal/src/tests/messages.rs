// SPDX-License-Identifier: MIT OR Apache-2.0
//! *Messages*: the echo-area log.

use super::*;

#[test]
fn echo_messages_are_recorded_in_messages() {
    let mut app = app_with("one\n");
    app.msg("Hello");
    assert_eq!(app.message.as_deref(), Some("Hello"));
    let m = messages_buffer(&app);
    assert!(m.contains("[echo] Hello"), "{m:?}");
    app.msg("Bye");
    assert!(messages_buffer(&app).contains("[echo] Bye"));
}

#[test]
fn errors_are_recorded_and_ring_the_bell() {
    let mut app = app_with("one\n");
    app.error("End of buffer");
    assert!(app.bell);
    let m = messages_buffer(&app);
    assert!(m.contains("[forge] error End of buffer"), "{m:?}");
}

#[test]
fn message_log_max_trims_the_oldest_lines() {
    let mut app = app_with("one\n");
    app.message_log_max = forge_core::MessageLogMax::Lines(3);
    for i in 0..5 {
        app.msg(format!("m{i}"));
    }
    let m = messages_buffer(&app);
    assert!(!m.contains("m0"), "{m:?}");
    assert!(!m.contains("m1"), "{m:?}");
    assert!(
        m.contains("m2") && m.contains("m3") && m.contains("m4"),
        "{m:?}"
    );
}

#[test]
fn message_log_max_can_be_unlimited_or_off() {
    let mut app = app_with("one\n");
    app.message_log_max = forge_core::MessageLogMax::Unlimited;
    for i in 0..200 {
        app.msg(format!("u{i}"));
    }
    assert!(messages_buffer(&app).contains("u199"));

    let mut app = app_with("one\n");
    app.message_log_max = forge_core::MessageLogMax::Off;
    app.msg("not recorded");
    assert!(
        app.editor.find_buffer("*Messages*").is_none(),
        "nothing was logged"
    );
}

#[test]
fn repeated_and_progress_messages_combine_into_one_line() {
    let mut app = app_with("one\n");
    app.msg("Working...");
    app.msg("Working...");
    let m = messages_buffer(&app);
    assert_eq!(m.matches("[echo]").count(), 1, "the duplicate is collapsed");

    app.msg("Working... done");
    let m = messages_buffer(&app);
    assert_eq!(
        m.matches("[echo]").count(),
        1,
        "the longer message replaced it"
    );
    assert!(m.contains("Working... done"), "{m:?}");
}

#[test]
fn inhibit_message_hides_the_echo_but_still_records() {
    let mut app = app_with("one\n");
    app.inhibit_message = true;
    app.msg("Quiet");
    assert!(app.message.is_none(), "not shown in the echo area");
    assert!(messages_buffer(&app).contains("Quiet"));
}

#[test]
fn view_echo_area_messages_opens_the_messages_buffer() {
    let mut app = app_with("one\n");
    app.msg("A note");
    app.run_command("view-echo-area-messages");
    assert_eq!(app.buf_ref().name(), "*Messages*");
}

#[test]
fn with_temp_message_restores_the_previous_message() {
    let mut app = app_with("one\n");
    app.msg("before");
    app.with_temp_message("busy...", |a| {
        assert_eq!(a.message.as_deref(), Some("busy..."));
    });
    assert_eq!(app.message.as_deref(), Some("before"));
}
