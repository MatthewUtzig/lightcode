use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use code_app_server_protocol::AuthMode;
use tracing::warn;

use crate::account_usage::{self, StoredRateLimitSnapshot};
use crate::auth_accounts::{self, StoredAccount};

const DEFAULT_PRIORITY_SCORE: f64 = 10_000.0;
const MIN_TIME_FRACTION: f64 = 0.01;
const DEFAULT_COOLDOWN_SECS: i64 = 15;
const MIN_EFFECTIVE_WEIGHT: f64 = 0.001;
const R_CRITICAL: f64 = 0.25;
const R_LOW: f64 = 1.0;
const R_SURPLUS: f64 = 1.5;
const R_CAP: f64 = 4.0;
const U_MIN: f64 = 0.1;
const U_BASE: f64 = 1.0;
const U_MAX: f64 = 2.0;

#[derive(Debug, Clone)]
pub struct AccountSelection {
    pub account_id: String,
    pub label: Option<String>,
    pub plan: Option<String>,
    pub snapshot: Option<StoredRateLimitSnapshot>,
}

#[derive(Debug, Clone, Copy)]
pub enum SchedulerOutcome {
    Success,
    RateLimited { resume_at: Option<DateTime<Utc>> },
}

/// Picks the next account to use for a model request based on remaining quota,
/// reset timers, and recent cooldown events.
pub struct AccountScheduler {
    code_home: PathBuf,
    cooldowns: HashMap<String, DateTime<Utc>>,
    weights: HashMap<String, WeightedState>,
}

impl AccountScheduler {
    pub fn new(code_home: PathBuf) -> Self {
        Self {
            code_home,
            cooldowns: HashMap::new(),
            weights: HashMap::new(),
        }
    }

    /// Pick the next account using smooth weighted round‑robin.
    pub fn next_account(&mut self, now: DateTime<Utc>) -> Option<AccountSelection> {
        self.prune_expired_cooldowns(now);

        let snapshots = match account_usage::list_rate_limit_snapshots(&self.code_home) {
            Ok(entries) => entries
                .into_iter()
                .map(|entry| (entry.account_id.clone(), entry))
                .collect::<HashMap<_, _>>(),
            Err(err) => {
                warn!("failed to read rate-limit snapshots: {err:#}");
                HashMap::new()
            }
        };

        let accounts = match auth_accounts::list_accounts(&self.code_home) {
            Ok(accounts) => accounts,
            Err(err) => {
                warn!("failed to list accounts: {err:#}");
                return None;
            }
        };

        let mut totals_by_identity: HashMap<String, f64> = HashMap::new();
        let mut slots: Vec<SlotCandidate> = Vec::new();

        for account in accounts.iter() {
            if !has_credentials(account) || self.is_blocked(&account.id, now) {
                continue;
            }

            let snapshot = snapshots.get(&account.id).cloned();
            let weight = snapshot
                .as_ref()
                .map(|entry| compute_weight(entry, now))
                .unwrap_or(DEFAULT_PRIORITY_SCORE)
                .max(MIN_EFFECTIVE_WEIGHT);

            let identity = slot_identity(account);
            *totals_by_identity.entry(identity.clone()).or_insert(0.0) += weight;

            slots.push(SlotCandidate {
                selection: AccountSelection {
                    account_id: account.id.clone(),
                    label: account.label.clone(),
                    plan: plan_for_account(account),
                    snapshot,
                },
                weight,
                identity,
            });
        }

        // Drop weights for identities that disappeared.
        if !self.weights.is_empty() {
            let valid_ids: HashSet<_> = totals_by_identity.keys().cloned().collect();
            self.weights.retain(|id, _| valid_ids.contains(id));
        }

        let total_weight: f64 = totals_by_identity.values().sum();

        if total_weight <= 0.0 {
            return None;
        }

        let mut best_identity: Option<String> = None;
        let mut best_current = f64::MIN;

        for (identity, weight_sum) in totals_by_identity.iter() {
            let state = self
                .weights
                .entry(identity.clone())
                .or_insert_with(|| WeightedState {
                    weight: *weight_sum,
                    current: 0.0,
                });
            state.weight = *weight_sum;
            state.current += state.weight;
            if state.current > best_current {
                best_current = state.current;
                best_identity = Some(identity.clone());
            }
        }

        let best_identity = best_identity?;
        if let Some(state) = self.weights.get_mut(&best_identity) {
            state.current -= total_weight;
        }

        // Choose a concrete slot for the winning identity. Prefer the heaviest slot, falling back
        // to lexicographic order for determinism.
        let selection = slots
            .into_iter()
            .filter(|slot| slot.identity == best_identity)
            .max_by(|a, b| {
                a.weight
                    .partial_cmp(&b.weight)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.selection.account_id.cmp(&b.selection.account_id))
            })
            .map(|slot| slot.selection)
            .expect("selected identity must have at least one slot");

        Some(selection)
    }

    pub fn record_outcome(&mut self, account_id: &str, outcome: SchedulerOutcome) {
        match outcome {
            SchedulerOutcome::Success => {
                self.cooldowns.remove(account_id);
            }
            SchedulerOutcome::RateLimited { resume_at } => {
                let resume = resume_at.unwrap_or_else(|| {
                    Utc::now() + Duration::seconds(DEFAULT_COOLDOWN_SECS)
                });
                self.cooldowns.insert(account_id.to_string(), resume);
            }
        }
    }

    fn prune_expired_cooldowns(&mut self, now: DateTime<Utc>) {
        self.cooldowns.retain(|_, until| *until > now);
    }

    fn is_blocked(&self, account_id: &str, now: DateTime<Utc>) -> bool {
        self.cooldowns
            .get(account_id)
            .map_or(false, |until| *until > now)
    }
}

#[derive(Debug, Clone)]
struct WeightedState {
    weight: f64,
    current: f64,
}

#[derive(Debug, Clone)]
struct SlotCandidate {
    selection: AccountSelection,
    weight: f64,
    identity: String,
}

fn has_credentials(account: &StoredAccount) -> bool {
    match account.mode {
        AuthMode::ApiKey => account.openai_api_key.is_some(),
        AuthMode::ChatGPT => account.tokens.is_some(),
    }
}

fn plan_for_account(account: &StoredAccount) -> Option<String> {
    account
        .tokens
        .as_ref()
        .and_then(|t| t.id_token.get_chatgpt_plan_type())
}

fn compute_priority(snapshot: &StoredRateLimitSnapshot, now: DateTime<Utc>) -> Option<f64> {
    let event = snapshot.snapshot.as_ref()?;

    let total_minutes = event.secondary_window_minutes.max(1) as f64;
    let total_seconds = total_minutes * 60.0;
    let remaining_pct = (100.0 - event.secondary_used_percent).clamp(0.0, 100.0);

    let seconds_remaining = snapshot
        .secondary_next_reset_at
        .map(|dt| (dt - now).num_seconds().max(0) as f64)
        .unwrap_or(total_seconds);

    let time_fraction = (seconds_remaining / total_seconds).clamp(MIN_TIME_FRACTION, 1.0);
    Some(remaining_pct / time_fraction)
}

pub fn compute_weight(snapshot: &StoredRateLimitSnapshot, now: DateTime<Utc>) -> f64 {
    // Remaining fraction of the secondary window (treat as weekly window surrogate).
    let ratio = compute_priority(snapshot, now).unwrap_or(DEFAULT_PRIORITY_SCORE) / 100.0;
    let urgency = urgency_multiplier(ratio);
    let health = health_multiplier(snapshot);
    ratio.max(MIN_EFFECTIVE_WEIGHT) * urgency * health
}

fn urgency_multiplier(ratio: f64) -> f64 {
    if ratio <= R_CRITICAL {
        return U_MIN;
    }
    if ratio >= R_CAP {
        return U_MAX;
    }

    if ratio < R_LOW {
        // Interpolate between U_MIN and U_BASE
        let t = (ratio - R_CRITICAL) / (R_LOW - R_CRITICAL);
        return U_MIN + t * (U_BASE - U_MIN);
    }

    if ratio < R_SURPLUS {
        return U_BASE;
    }

    // Between SURPLUS and CAP: interpolate up to U_MAX
    let t = (ratio - R_SURPLUS) / (R_CAP - R_SURPLUS);
    U_BASE + t * (U_MAX - U_BASE)
}

fn health_multiplier(_snapshot: &StoredRateLimitSnapshot) -> f64 {
    // Health data not yet persisted; assume healthy.
    1.0
}

pub fn slot_identity(account: &StoredAccount) -> String {
    if !account.id.starts_with("slot-") {
        return account.id.clone();
    }

    account
        .tokens
        .as_ref()
        .and_then(|t| t.account_id.clone())
        .unwrap_or_else(|| account.id.clone())
}
