// SPDX-License-Identifier: MIT OR Apache-2.0
use forge_core::Editor;

#[test]
fn editor_starts_with_active_buffer() {
    let mut ed = Editor::new();
    ed.create_buffer("*scratch*");
    let buf = ed.state.active_buffer_mut().expect("active buffer");
    buf.insert_char('F');
    assert_eq!(buf.get_contents(), "F");
    assert_eq!(buf.name(), "*scratch*");
}
