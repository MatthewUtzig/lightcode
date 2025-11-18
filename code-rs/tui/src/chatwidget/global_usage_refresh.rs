use std::path::PathBuf;

use chrono::Local;
use code_core::account_usage::{collect_global_usage_summary, GlobalUsageSummary};
use code_protocol::num_format::format_with_separators;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::thread_spawner;

const TOKENS_PER_MILLION: f64 = 1_000_000.0;
const TOKENS_PER_THOUSAND: f64 = 1_000.0;

pub(super) fn start_global_usage_refresh(
    app_event_tx: AppEventSender,
    code_home: PathBuf,
) {
    let fallback_tx = app_event_tx.clone();
    if thread_spawner::spawn_lightweight("global-usage", move || {
        match collect_global_usage_summary(&code_home) {
            Ok(summary) => {
                let formatted = format_summary(&summary);
                app_event_tx.send(AppEvent::GlobalUsageSummaryReady { summary: formatted });
            }
            Err(err) => {
                let message = format!("Failed to compute global usage: {}", err);
                app_event_tx.send(AppEvent::GlobalUsageSummaryFailed { message });
            }
        }
    })
    .is_none()
    {
        fallback_tx.send(AppEvent::GlobalUsageSummaryFailed {
            message: "Failed to start global usage task: worker limit reached".to_string(),
        });
    }
}

fn format_summary(summary: &GlobalUsageSummary) -> String {
    let account_count = summary.accounts.len();
    if account_count == 0 {
        return "No usage recorded yet".to_string();
    }

    let total_tokens = summary.totals.total_tokens;
    let total_display = format_tokens(total_tokens);
    let last_hour: u64 = summary
        .accounts
        .iter()
        .map(|acct| acct.tokens_last_hour.total_tokens)
        .sum();
    let last_hour_display = format_tokens(last_hour);
    let updated = summary
        .last_updated
        .map(|ts| ts.with_timezone(&Local).format("%b %-d, %Y %-I:%M %p %Z").to_string())
        .unwrap_or_else(|| "never".to_string());

    let account_label = if account_count == 1 { "account" } else { "accounts" };
    format!(
        "{} {} · {} tokens total · {} last hour · updated {}",
        account_count, account_label, total_display, last_hour_display, updated
    )
}

fn format_tokens(count: u64) -> String {
    if count >= 50_000_000 {
        format!("{:.1}M", count as f64 / TOKENS_PER_MILLION)
    } else if count >= 1_000_000 {
        format!("{:.2}M", count as f64 / TOKENS_PER_MILLION)
    } else if count >= 20_000 {
        format!("{:.0}k", count as f64 / TOKENS_PER_THOUSAND)
    } else if count >= 1_000 {
        format!("{:.1}k", count as f64 / TOKENS_PER_THOUSAND)
    } else {
        format_with_separators(count)
    }
}
