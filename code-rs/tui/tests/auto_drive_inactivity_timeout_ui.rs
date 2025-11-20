use code_tui::test_helpers::{render_chat_widget_to_vt100, ChatWidgetHarness};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use strip_ansi_escapes::strip;

#[test]
fn misc_settings_lists_timeout_options() {
    let mut harness = ChatWidgetHarness::new();
    harness.open_misc_settings_overlay();

    let frame = normalize_output(render_chat_widget_to_vt100(&mut harness, 100, 30));
    assert!(frame.contains("Misc Settings"), "frame=\n{}", frame);
    assert!(frame.contains("Auto Drive inactivity timeout"), "frame=\n{}", frame);

    for option in ["Off", "15 minutes", "30 minutes", "60 minutes", "120 minutes"] {
        assert!(frame.contains(option), "missing option {option} in frame=\n{frame}");
    }
}

#[test]
fn misc_settings_apply_updates_active_option() {
    let mut harness = ChatWidgetHarness::new();
    harness.open_misc_settings_overlay();

    // Move to the "30 minutes" option (default selection is 60 minutes) and apply it.
    harness.send_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    harness.send_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let frame = normalize_output(render_chat_widget_to_vt100(&mut harness, 100, 30));
    assert!(
        frame.contains("● 30 minutes"),
        "expected 30 minute option to be active after selection. frame=\n{}",
        frame
    );
}

fn normalize_output(text: String) -> String {
    let stripped = strip(text.as_bytes()).expect("strip ANSI");
    String::from_utf8(stripped).expect("utf8")
}
