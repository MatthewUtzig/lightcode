use std::path::Path;
use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use code_core::auth::{write_auth_json, AuthDotJson};
use code_core::account_usage::record_rate_limit_snapshot;
use code_core::auth_accounts::{upsert_api_key_account, upsert_chatgpt_account};
use code_core::protocol::RateLimitSnapshotEvent;
use code_core::token_data::{parse_id_token, TokenData};
use code_tui::test_helpers::{render_chat_widget_to_vt100, ChatWidgetHarness};
use strip_ansi_escapes::strip;
use base64::Engine;

#[test]
fn limits_overlay_displays_selection_chance_and_duplicate_notice() {
    let mut harness = ChatWidgetHarness::new();
    seed_limits_fixtures(harness.code_home());
    let _snaps = code_core::account_usage::list_rate_limit_snapshots(harness.code_home()).unwrap();
    let _accounts = code_core::auth_accounts::list_accounts(harness.code_home()).unwrap();
    harness.suppress_rate_limit_refresh();
    harness.show_limits_settings_ui();

    let frame = normalize_output(render_chat_widget_to_vt100(&mut harness, 100, 30));
    let frame_lower = frame.to_ascii_lowercase();
    assert!(frame_lower.contains("selection chance"), "frame=\n{}", frame);
    assert!(frame.contains("Dup Slot A"));
    assert!(frame.contains("Dup Slot B"));
    assert!(frame.contains("Duplicate slot configuration detected"));
    let out_of_tokens_matches = [
        "Out of hourly tokens",
        "Out of weekly tokens",
        "Hourly limit exhausted",
        "Weekly limit exhausted",
    ]
    .iter()
    .any(|needle| frame.contains(needle));
    assert!(
        out_of_tokens_matches,
        "frame did not include scoped out-of-tokens line, frame=\n{}",
        frame
    );
}

#[test]
fn slot_default_tab_uses_auth_email_and_single_selection_summary() {
    let legacy_home = tempfile::tempdir().expect("legacy home");
    let code_home = legacy_home.path().join(".codex");
    std::fs::create_dir_all(&code_home).expect(".codex");

    seed_root_auth(&code_home, "zig@avenue.co");
    record_snapshot(
        "slot-default",
        10.0,
        Some(3600),
        &code_home,
        Utc::now(),
    );

    let other_tokens = chatgpt_tokens("acct-other", "other@example.com");
    let other = upsert_chatgpt_account(
        &code_home,
        other_tokens,
        Utc::now(),
        Some("Other Slot".into()),
        false,
    )
    .expect("other account");
    record_snapshot(&other.id, 40.0, Some(5400), &code_home, Utc::now());

    let mut harness = ChatWidgetHarness::new_with_home(code_home.clone());
    harness.suppress_rate_limit_refresh();
    harness.show_limits_settings_ui();
    let mut frame = normalize_output(render_chat_widget_to_vt100(&mut harness, 100, 30));
    let mut attempts = 0;
    while !frame.contains("zig@avenue.co") && attempts < 8 {
        harness.send_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        frame = normalize_output(render_chat_widget_to_vt100(&mut harness, 100, 30));
        attempts += 1;
    }
    assert!(
        frame.contains("zig@avenue.co"),
        "limits tab should reference zig@avenue.co, frame=\n{}",
        frame
    );
    let selection_lines = frame.matches("Selection chance:").count();
    assert_eq!(
        selection_lines, 1,
        "expected exactly one account-level selection summary, frame=\n{}",
        frame
    );
}

fn seed_limits_fixtures(code_home: &Path) {
    let now = Utc::now();
    let primary = upsert_api_key_account(
        code_home,
        "sk-primary".into(),
        Some("Primary Slot".into()),
        false,
    )
    .expect("primary");
    record_snapshot(&primary.id, 20.0, Some(3600), code_home, now);

    let dup_tokens = chatgpt_tokens("dup-identity", "dup-a@example.com");
    let dup_a = upsert_chatgpt_account(
        code_home,
        dup_tokens.clone(),
        now,
        Some("Dup Slot A".into()),
        false,
    )
    .expect("dup a");
    record_snapshot(&dup_a.id, 40.0, Some(5400), code_home, now);

    let dup_b = upsert_chatgpt_account(
        code_home,
        chatgpt_tokens("dup-identity", "dup-b@example.com"),
        now,
        Some("Dup Slot B".into()),
        false,
    )
    .expect("dup b");
    record_snapshot(&dup_b.id, 60.0, Some(7200), code_home, now);

    let exhausted = upsert_api_key_account(
        code_home,
        "sk-exhausted".into(),
        Some("Exhausted Slot".into()),
        false,
    )
    .expect("exhausted");
    record_snapshot(&exhausted.id, 100.0, Some(1800), code_home, now);
}

fn chatgpt_tokens(account_id: &str, email: &str) -> TokenData {
    let jwt = fake_jwt(account_id, email);
    TokenData {
        id_token: parse_id_token(&jwt).expect("id token"),
        access_token: "access".into(),
        refresh_token: "refresh".into(),
        account_id: Some(account_id.to_string()),
    }
}

fn record_snapshot(
    account_id: &str,
    used_percent: f64,
    reset_secs: Option<u64>,
    code_home: &Path,
    observed_at: DateTime<Utc>,
) {
    let mut snapshot = RateLimitSnapshotEvent {
        primary_used_percent: 0.0,
        secondary_used_percent: used_percent,
        primary_to_secondary_ratio_percent: 100.0,
        primary_window_minutes: 60,
        secondary_window_minutes: 60,
        primary_reset_after_seconds: None,
        secondary_reset_after_seconds: reset_secs,
        account_id: Some(account_id.to_string()),
    };
    if snapshot.secondary_reset_after_seconds.is_none() {
        snapshot.secondary_reset_after_seconds = Some(3600);
    }
    record_rate_limit_snapshot(code_home, account_id, None, &snapshot, observed_at)
        .expect("record snapshot");
}

fn seed_root_auth(code_home: &Path, email: &str) {
    let tokens = chatgpt_tokens("acct-default", email);
    let auth = AuthDotJson {
        openai_api_key: None,
        tokens: Some(tokens),
        last_refresh: Some(Utc::now()),
    };
    write_auth_json(&code_home.join("auth.json"), &auth).expect("write auth.json");
}

fn normalize_output(text: String) -> String {
    let stripped = strip(text.as_bytes()).expect("strip ANSI");
    String::from_utf8(stripped).expect("utf8")
}

fn fake_jwt(_account_id: &str, email: &str) -> String {
    use serde::Serialize;

    #[derive(Serialize)]
    struct Header {
        alg: &'static str,
        typ: &'static str,
    }

    let header = Header {
        alg: "none",
        typ: "JWT",
    };
    let payload = serde_json::json!({
        "email": email,
        "https://api.openai.com/auth": {
            "chatgpt_plan_type": "pro"
        }
    });

    fn b64(value: &serde_json::Value) -> String {
        let bytes = serde_json::to_vec(value).expect("json bytes");
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    }

    let header_b64 = b64(&serde_json::to_value(header).expect("header"));
    let payload_b64 = b64(&payload);
    let signature_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"sig");
    format!("{header_b64}.{payload_b64}.{signature_b64}")
}
