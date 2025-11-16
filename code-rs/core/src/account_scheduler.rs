use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use code_app_server_protocol::AuthMode;
use tracing::warn;

use crate::account_usage::{self, StoredRateLimitSnapshot};
use crate::auth_accounts::{self, StoredAccount};

const DEFAULT_PRIORITY_SCORE: f64 = 10_000.0;
const MIN_TIME_FRACTION: f64 = 0.01;
const DEFAULT_COOLDOWN_SECS: i64 = 15;

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
    last_selected_order: HashMap<String, u64>,
    next_order: u64,
}

impl AccountScheduler {
    pub fn new(code_home: PathBuf) -> Self {
        Self {
            code_home,
            cooldowns: HashMap::new(),
            last_selected_order: HashMap::new(),
            next_order: 1,
        }
    }

    /// Pick the next account ordered by priority score and round‑robin fairness.
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

        let mut best: Option<Candidate> = None;
        for account in accounts.iter() {
            if !has_credentials(account) {
                continue;
            }

            if self.is_blocked(&account.id, now) {
                continue;
            }

            let snapshot = snapshots.get(&account.id).cloned();
            let score = snapshot
                .as_ref()
                .and_then(|entry| compute_priority(entry, now))
                .unwrap_or(DEFAULT_PRIORITY_SCORE);
            let last_used = self.last_selected_order.get(&account.id).copied();
            let candidate = Candidate {
                selection: AccountSelection {
                    account_id: account.id.clone(),
                    label: account.label.clone(),
                    plan: plan_for_account(account),
                    snapshot,
                },
                score,
                last_used,
            };

            best = match best {
                None => Some(candidate),
                Some(current_best) => Some(pick_preferred(current_best, candidate)),
            };
        }

        let chosen = best?;
        self.last_selected_order
            .insert(chosen.selection.account_id.clone(), self.next_order);
        self.next_order = self.next_order.saturating_add(1);

        Some(chosen.selection)
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

#[derive(Clone)]
struct Candidate {
    selection: AccountSelection,
    score: f64,
    last_used: Option<u64>,
}

fn pick_preferred(a: Candidate, b: Candidate) -> Candidate {
    use std::cmp::Ordering;

    match a.score.partial_cmp(&b.score).unwrap_or(Ordering::Equal) {
        Ordering::Greater => a,
        Ordering::Less => b,
        Ordering::Equal => match compare_last_used(a.last_used, b.last_used) {
            Ordering::Less => a,
            Ordering::Greater => b,
            Ordering::Equal => {
                if a.selection.account_id <= b.selection.account_id {
                    a
                } else {
                    b
                }
            }
        },
    }
}

fn compare_last_used(a: Option<u64>, b: Option<u64>) -> std::cmp::Ordering {
    match (a, b) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (Some(a), Some(b)) => a.cmp(&b),
    }
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
