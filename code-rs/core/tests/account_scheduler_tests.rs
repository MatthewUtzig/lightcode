use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use code_core::account_scheduler::{compute_weight, slot_identity as scheduler_slot_identity, AccountScheduler, SchedulerOutcome};
use code_core::account_usage::{self, record_rate_limit_snapshot};
use code_core::auth_accounts::{self, upsert_api_key_account, upsert_chatgpt_account, StoredAccount};
use code_core::protocol::RateLimitSnapshotEvent;
use code_core::token_data::{parse_id_token, TokenData};
use std::collections::HashMap;
use tempfile::tempdir;

struct CodeHomeGuard {
    saved: Vec<(&'static str, Option<String>)>,
}

impl CodeHomeGuard {
    fn new(path: &std::path::Path) -> Self {
        let keys = ["CODE_HOME", "CODEX_HOME", "HOME"];
        let mut saved = Vec::new();
        for key in keys { saved.push((key, std::env::var(key).ok())); }
        unsafe {
            std::env::set_var("CODE_HOME", path);
            std::env::set_var("HOME", path);
            std::env::remove_var("CODEX_HOME");
        }
        Self { saved }
    }
}

impl Drop for CodeHomeGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..) {
            unsafe {
                if let Some(val) = value {
                    std::env::set_var(key, val);
                } else {
                    std::env::remove_var(key);
                }
            }
        }
    }
}

fn snapshot_with_usage(used_percent: f64, window_minutes: u64) -> RateLimitSnapshotEvent {
    RateLimitSnapshotEvent {
        primary_used_percent: 0.0,
        secondary_used_percent: used_percent,
        primary_to_secondary_ratio_percent: 100.0,
        primary_window_minutes: window_minutes,
        secondary_window_minutes: window_minutes,
        primary_reset_after_seconds: None,
        secondary_reset_after_seconds: Some(window_minutes * 60),
        account_id: None,
    }
}

fn record_snapshot(home: &std::path::Path, account_id: &str, used_percent: f64) {
    let snap = snapshot_with_usage(used_percent, 60);
    record_rate_limit_snapshot(home, account_id, None, &snap, Utc::now()).unwrap();
}

fn record_snapshot_with_reset(
    home: &std::path::Path,
    account_id: &str,
    used_percent: f64,
    reset_secs: Option<u64>,
) {
    let mut snap = snapshot_with_usage(used_percent, 60);
    snap.secondary_reset_after_seconds = reset_secs;
    record_rate_limit_snapshot(home, account_id, None, &snap, Utc::now()).unwrap();
}

fn make_chatgpt_tokens(account_id: &str) -> TokenData {
    let jwt = fake_jwt(account_id);
    TokenData {
        id_token: parse_id_token(&jwt).expect("id token"),
        access_token: "access".into(),
        refresh_token: "refresh".into(),
        account_id: Some(account_id.to_string()),
    }
}

fn slot_identity(account: &StoredAccount) -> String {
    scheduler_slot_identity(account)
}

fn collect_identity_weights(
    code_home: &std::path::Path,
    now: DateTime<Utc>,
) -> HashMap<String, f64> {
    let snapshots = account_usage::list_rate_limit_snapshots(code_home).expect("snapshots");
    let accounts = auth_accounts::list_accounts(code_home).expect("accounts");
    let snapshot_map: HashMap<_, _> = snapshots
        .into_iter()
        .map(|record| (record.account_id.clone(), record))
        .collect();

    let mut weights = HashMap::new();
    for account in accounts {
        let Some(snapshot) = snapshot_map.get(&account.id) else { continue; };
        let Some(weight) = snapshot.snapshot.as_ref().map(|_| compute_weight(snapshot, now)) else {
            continue;
        };
        if weight <= 0.0 {
            continue;
        }
        let identity = slot_identity(&account);
        *weights.entry(identity).or_insert(0.0) += weight;
    }
    weights
}

fn reference_weighted_order(weights: &[(String, f64)], iterations: usize) -> Vec<String> {
    #[derive(Clone, Copy)]
    struct State {
        weight: f64,
        current: f64,
    }

    let total_weight: f64 = weights.iter().map(|(_, w)| *w).sum();
    let mut states: HashMap<&str, State> = weights
        .iter()
        .map(|(name, weight)| (name.as_str(), State { weight: *weight, current: 0.0 }))
        .collect();
    let mut order = Vec::new();

    for _ in 0..iterations {
        let mut best_name: Option<&str> = None;
        let mut best_value = f64::MIN;
        for (name, state) in states.iter_mut() {
            state.current += state.weight;
            if state.current > best_value {
                best_value = state.current;
                best_name = Some(name);
            }
        }
        let winner = best_name.expect("at least one identity");
        if let Some(state) = states.get_mut(winner) {
            state.current -= total_weight;
        }
        order.push(winner.to_string());
    }

    order
}

fn fake_jwt(account_id: &str) -> String {
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
        "email": format!("{account_id}@example.com"),
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

#[test]
fn smooth_weighted_round_robin_balances_equal_weights() {
    let home = tempdir().unwrap();
    let _guard = CodeHomeGuard::new(home.path());
    let acc_a = upsert_api_key_account(home.path(), "sk-a".into(), None, false).unwrap();
    let acc_b = upsert_api_key_account(home.path(), "sk-b".into(), None, false).unwrap();

    record_snapshot(home.path(), &acc_a.id, 50.0);
    record_snapshot(home.path(), &acc_b.id, 50.0);

    let mut scheduler = AccountScheduler::new(home.path().to_path_buf());
    let now = Utc::now();

    let mut counts: HashMap<String, usize> = HashMap::new();
    for _ in 0..20 {
        let pick = scheduler.next_account(now).unwrap().account_id;
        if pick == acc_a.id || pick == acc_b.id {
            *counts.entry(pick).or_insert(0) += 1;
        }
    }

    let a_count = *counts.get(&acc_a.id).unwrap_or(&0);
    let b_count = *counts.get(&acc_b.id).unwrap_or(&0);
    assert!(a_count > 0 && b_count > 0, "scheduler should select both accounts");
    assert!((a_count as isize - b_count as isize).abs() <= 1);
}

#[test]
fn smooth_weighted_round_robin_respects_weight_ratios() {
    let home = tempdir().unwrap();
    let _guard = CodeHomeGuard::new(home.path());
    let heavy = upsert_api_key_account(home.path(), "sk-heavy".into(), None, false).unwrap();
    let light = upsert_api_key_account(home.path(), "sk-light".into(), None, false).unwrap();

    // Lower used percent → higher remaining → higher weight.
    record_snapshot(home.path(), &heavy.id, 10.0); // high weight
    record_snapshot(home.path(), &light.id, 50.0); // lower weight

    let mut scheduler = AccountScheduler::new(home.path().to_path_buf());
    let now = Utc::now();

    let mut heavy_count = 0;
    let mut light_count = 0;
    for _ in 0..40 {
        let id = scheduler.next_account(now).unwrap().account_id;
        if id == heavy.id {
            heavy_count += 1;
        } else if id == light.id {
            light_count += 1;
        }
    }

    assert!(heavy_count > light_count, "heavier account should be chosen more often");
}

#[test]
fn scheduler_skips_account_during_cooldown() {
    let home = tempdir().unwrap();
    let _guard = CodeHomeGuard::new(home.path());
    let acc_a = upsert_api_key_account(home.path(), "sk-a".into(), None, false).unwrap();
    let acc_b = upsert_api_key_account(home.path(), "sk-b".into(), None, false).unwrap();

    record_snapshot(home.path(), &acc_a.id, 50.0);
    record_snapshot(home.path(), &acc_b.id, 50.0);

    let mut scheduler = AccountScheduler::new(home.path().to_path_buf());
    let now = Utc::now();

    let first = scheduler.next_account(now).unwrap();
    scheduler.record_outcome(
        &first.account_id,
        SchedulerOutcome::RateLimited {
            resume_at: Some(now + Duration::seconds(60)),
        },
    );

    let second = scheduler.next_account(now).unwrap();
    assert_ne!(first.account_id, second.account_id);
}

#[test]
fn cooldown_expires_and_account_returns() {
    let home = tempdir().unwrap();
    let _guard = CodeHomeGuard::new(home.path());
    let acc_a = upsert_api_key_account(home.path(), "sk-a".into(), None, false).unwrap();

    record_snapshot(home.path(), &acc_a.id, 50.0);

    let mut scheduler = AccountScheduler::new(home.path().to_path_buf());
    let now = Utc::now();

    let first = scheduler.next_account(now).unwrap();
    scheduler.record_outcome(
        &first.account_id,
        SchedulerOutcome::RateLimited {
            resume_at: Some(now + Duration::seconds(10)),
        },
    );

    // Still blocked before resume time.
    assert!(scheduler.next_account(now + Duration::seconds(5)).is_none());

    // Available after cooldown passes.
    let after = scheduler.next_account(now + Duration::seconds(15)).unwrap();
    assert_eq!(after.account_id, first.account_id);
}

#[test]
fn scheduler_handles_duplicate_slots_and_cooldowns() {
    let home = tempdir().unwrap();
    let _guard = CodeHomeGuard::new(home.path());
    let now = Utc::now();

    let heavy = upsert_api_key_account(home.path(), "sk-heavy".into(), Some("primary".into()), false).unwrap();
    record_snapshot(home.path(), &heavy.id, 10.0);

    let dup_tokens = make_chatgpt_tokens("dup-account");
    let dup_slot_a = upsert_chatgpt_account(
        home.path(),
        dup_tokens.clone(),
        now,
        Some("dup-a".into()),
        false,
    )
    .unwrap();
    record_snapshot(home.path(), &dup_slot_a.id, 40.0);

    let dup_slot_b = upsert_chatgpt_account(
        home.path(),
        dup_tokens,
        now,
        Some("dup-b".into()),
        false,
    )
    .unwrap();
    record_snapshot(home.path(), &dup_slot_b.id, 70.0);

    let exhausted = upsert_api_key_account(
        home.path(),
        "sk-exhausted".into(),
        Some("exhausted".into()),
        false,
    )
    .unwrap();
    record_snapshot_with_reset(home.path(), &exhausted.id, 100.0, Some(1800));

    let accounts = auth_accounts::list_accounts(home.path()).expect("accounts");
    let identity_map: HashMap<_, _> = accounts
        .iter()
        .map(|acc| (acc.id.clone(), slot_identity(acc)))
        .collect();

    let identity_weights = collect_identity_weights(home.path(), now);
    let expected_order = reference_weighted_order(
        &identity_weights.iter().map(|(k, v)| (k.clone(), *v)).collect::<Vec<_>>(),
        12,
    );

    let mut scheduler = AccountScheduler::new(home.path().to_path_buf());
    let mut actual_order = Vec::new();
    for _ in 0..12 {
        let selection = scheduler.next_account(now).unwrap();
        let identity = identity_map.get(&selection.account_id).unwrap().clone();
        actual_order.push(identity);
    }

    assert_eq!(actual_order, expected_order);

    let exhausted_identity = identity_map.get(&exhausted.id).unwrap();
    assert!(!actual_order.iter().any(|id| id == exhausted_identity));

    // Cool down the heavy identity and ensure it is skipped until resume.
    let heavy_identity = identity_map.get(&heavy.id).unwrap().clone();
    scheduler.record_outcome(
        &heavy.id,
        SchedulerOutcome::RateLimited {
            resume_at: Some(now + Duration::seconds(30)),
        },
    );

    for _ in 0..5 {
        let identity = identity_map
            .get(&scheduler.next_account(now).unwrap().account_id)
            .unwrap();
        assert_ne!(identity, &heavy_identity, "cooled identity should be skipped");
    }

    let resumed_identity = identity_map
        .get(&scheduler.next_account(now + Duration::seconds(31)).unwrap().account_id)
        .unwrap()
        .clone();
    assert_eq!(resumed_identity, heavy_identity);
}
