use anyhow::Result;
use clap::Parser;
use code_common::CliConfigOverrides;
use code_core::config::{Config, ConfigOverrides};
use code_core::global_usage_tracker::{
    scan_global_usage,
    GlobalUsageScanOptions,
    GlobalUsageSnapshot,
    ModelBucket,
    UsageBucket,
    UsageTotals,
};
use code_protocol::num_format::format_with_separators;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Parser)]
pub struct UsageCommand {
    #[clap(flatten)]
    pub config_overrides: CliConfigOverrides,

    /// Override the session logs directory (defaults to ~/.code/sessions plus slot mirrors)
    #[clap(long = "sessions-dir", value_name = "DIR")]
    pub sessions_dir: Option<PathBuf>,

    /// Maximum worker threads to use while parsing logs (default: CPU count)
    #[clap(long = "workers", value_name = "N")]
    pub workers: Option<usize>,

    /// Print per-session totals after the aggregate summary
    #[clap(long)]
    pub verbose: bool,
}

impl UsageCommand {
    pub fn run(mut self) -> Result<()> {
        let config = load_config_or_exit(self.config_overrides.take());
        let mut options = GlobalUsageScanOptions::new(config.code_home);
        if let Some(dir) = self.sessions_dir.take() {
            options = options.with_sessions_override(dir);
        }
        if let Some(workers) = self.workers.take() {
            options = options.with_max_workers(workers);
        }
        options = options.with_record_sessions(self.verbose);

        let snapshot = scan_global_usage(options)?;
        print_text_summary(&snapshot, self.verbose);
        Ok(())
    }
}

fn load_config_or_exit(overrides: CliConfigOverrides) -> Config {
    let cli_overrides = match overrides.parse_overrides() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error parsing -c overrides: {e}");
            std::process::exit(1);
        }
    };
    let config_overrides = ConfigOverrides::default();
    match Config::load_with_cli_overrides(cli_overrides, config_overrides) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("Error loading configuration: {e}");
            std::process::exit(1);
        }
    }
}

fn print_text_summary(snapshot: &GlobalUsageSnapshot, verbose: bool) {
    let generated_at = snapshot.generated_at.format("%Y-%m-%d %H:%M:%S UTC");
    println!("Global token usage as of {generated_at}");
    println!(
        "Sessions processed: {}  ·  missing totals: {}",
        snapshot.sessions_processed, snapshot.sessions_missing_totals
    );

    println!("\nTotals:");
    println!(
        "  Non-cached input : {} tokens",
        fmt_tokens(snapshot.totals.non_cached_input_tokens)
    );
    println!(
        "  Cached input     : {} tokens",
        fmt_tokens(snapshot.totals.cached_input_tokens)
    );
    println!(
        "  Output           : {} tokens",
        fmt_tokens(snapshot.totals.output_tokens)
    );
    println!(
        "  Reasoning output : {} tokens",
        fmt_tokens(snapshot.totals.reasoning_output_tokens)
    );
    println!(
        "  Total            : {} tokens",
        fmt_tokens(snapshot.totals.total_tokens)
    );
    println!(
        "  Estimated cost   : ${:.4}",
        snapshot.totals.cost_usd
    );

    println!("\nRecent usage windows:");
    print_trailing_line("Last 1 hour", &snapshot.trailing.last_hour);
    print_trailing_line("Last 12 hours", &snapshot.trailing.last_twelve_hours);
    print_trailing_line("Last day", &snapshot.trailing.last_day);
    print_trailing_line("Last 7 days", &snapshot.trailing.last_seven_days);
    print_trailing_line("Last 30 days", &snapshot.trailing.last_thirty_days);
    print_trailing_line("Last year", &snapshot.trailing.last_year);

    print_model_groups(snapshot);
    print_source_cards(snapshot);
    print_bucket_section("Hourly usage (last 12 hours)", &snapshot.hourly_buckets);
    print_bucket_section("12-hour usage (last 7 days)", &snapshot.twelve_hour_buckets);
    print_bucket_section("Daily usage (last 7 days)", &snapshot.daily_buckets);
    print_bucket_section("Weekly usage (last 8 weeks)", &snapshot.weekly_buckets);
    print_bucket_section("Monthly usage (last 6 months)", &snapshot.monthly_buckets);

    if let Some(session) = &snapshot.largest_session {
        println!(
            "\nLargest session: {} · {} tokens ({})",
            session.session_id,
            fmt_tokens(session.totals.total_tokens),
            session.model_bucket.as_str()
        );
    }

    if verbose && !snapshot.per_session.is_empty() {
        println!("\nPer-session totals:");
        for session in &snapshot.per_session {
            println!(
                "- {} [{}]: non-cached={} cached={} output={} total={} cost=${:.4}",
                session.session_id,
                session.model_bucket.as_str(),
                fmt_tokens(session.totals.non_cached_input_tokens),
                fmt_tokens(session.totals.cached_input_tokens),
                fmt_tokens(
                    session.totals.output_tokens + session.totals.reasoning_output_tokens
                ),
                fmt_tokens(session.totals.total_tokens),
                session.totals.cost_usd
            );
        }
    }
}

fn print_trailing_line(label: &str, totals: &UsageTotals) {
    if totals.total_tokens == 0 {
        println!("  {label:<14} : —");
        return;
    }
    println!(
        "  {label:<14} : {} tokens (input {} · cached {} · output {})",
        fmt_tokens(totals.total_tokens),
        fmt_tokens(totals.non_cached_input_tokens),
        fmt_tokens(totals.cached_input_tokens),
        fmt_tokens(totals.output_tokens + totals.reasoning_output_tokens)
    );
}

fn print_model_groups(snapshot: &GlobalUsageSnapshot) {
    println!("\nPer-model totals and cost estimates:");
    if snapshot.model_usage.is_empty() {
        println!("  (no sessions)");
        return;
    }

    let mut map = BTreeMap::new();
    for entry in &snapshot.model_usage {
        map.insert(entry.bucket, entry.totals.clone());
    }

    for (group, buckets) in MODEL_DISPLAY_GROUPS.iter() {
        let mut group_totals = UsageTotals::default();
        for bucket in *buckets {
            if let Some(value) = map.get(bucket) {
                accumulate_usage_totals(&mut group_totals, value);
            }
        }
        if group_totals.total_tokens == 0 {
            continue;
        }
        println!("- {group}:");
        println!(
            "    tokens={} · cost=${:.4}",
            fmt_tokens(group_totals.total_tokens),
            group_totals.cost_usd
        );
        for bucket in *buckets {
            if let Some(value) = map.get(bucket) {
                println!(
                    "      {:<18} tokens={} cost=${:.4}",
                    bucket.as_str(),
                    fmt_tokens(value.total_tokens),
                    value.cost_usd
                );
            }
        }
    }
}

fn print_source_cards(snapshot: &GlobalUsageSnapshot) {
    println!("\nTop sources:");
    if snapshot.source_usage.is_empty() {
        println!("  (no sessions)");
        return;
    }
    for entry in &snapshot.source_usage {
        println!(
            "  {:<24} {:>12} tokens   ${:.4}",
            entry.label,
            fmt_tokens(entry.totals.total_tokens),
            entry.totals.cost_usd
        );
    }
}

fn accumulate_usage_totals(dst: &mut UsageTotals, src: &UsageTotals) {
    dst.non_cached_input_tokens = dst
        .non_cached_input_tokens
        .saturating_add(src.non_cached_input_tokens);
    dst.cached_input_tokens = dst
        .cached_input_tokens
        .saturating_add(src.cached_input_tokens);
    dst.output_tokens = dst.output_tokens.saturating_add(src.output_tokens);
    dst.reasoning_output_tokens = dst
        .reasoning_output_tokens
        .saturating_add(src.reasoning_output_tokens);
    dst.total_tokens = dst.total_tokens.saturating_add(src.total_tokens);
    dst.cost_usd += src.cost_usd;
}

fn print_bucket_section(label: &str, buckets: &[UsageBucket]) {
    if buckets.is_empty() {
        return;
    }
    println!("\n{label}:");
    for bucket in buckets {
        let window = format!(
            "{}-{}",
            bucket.start.format("%m-%d %H:%M"),
            bucket.end.format("%H:%M")
        );
        println!(
            "  {}  {} tokens (cost ${:.4})",
            window,
            fmt_tokens(bucket.totals.total_tokens),
            bucket.totals.cost_usd
        );
    }
}

fn fmt_tokens(value: u64) -> String {
    const SCALES: &[(u64, &str)] = &[(1_000_000_000_000, "T"), (1_000_000_000, "B"), (1_000_000, "M"), (1_000, "K")];
    for (scale, suffix) in SCALES {
        if value >= *scale {
            return format!("{:.2}{}", value as f64 / *scale as f64, suffix);
        }
    }
    format_with_separators(value)
}

const MODEL_DISPLAY_GROUPS: &[(&str, &[ModelBucket])] = &[
    (
        "gpt-5-codex",
        &[
            ModelBucket::Gpt5Codex,
            ModelBucket::Gpt51Codex,
            ModelBucket::CodeGpt5Codex,
            ModelBucket::ChatGpt51Codex,
        ],
    ),
    ("gpt-5", &[ModelBucket::Gpt5, ModelBucket::Gpt51]),
    (
        "gpt-5-codex-mini",
        &[
            ModelBucket::Gpt5Mini,
            ModelBucket::Gpt51CodexMini,
            ModelBucket::CodeGpt5CodexMini,
            ModelBucket::CodeGpt5Mini,
            ModelBucket::ChatGpt51CodexMini,
        ],
    ),
    ("other", &[ModelBucket::Other]),
];

trait TakeOverrides {
    fn take(&mut self) -> CliConfigOverrides;
}

impl TakeOverrides for CliConfigOverrides {
    fn take(&mut self) -> CliConfigOverrides {
        std::mem::take(self)
    }
}
