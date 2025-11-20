use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::thread;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use rayon::prelude::*;
use rayon::ThreadPoolBuilder;
use serde_json::Value;
use tracing::warn;
use walkdir::WalkDir;

use crate::config::legacy_code_home_dir_for_read;

const SESSIONS_SUBDIR: &str = "sessions";
const SLOT_DIR_NAME: &str = "slot";

const TOKEN_FIELDS: [&str; 5] = [
    "input_tokens",
    "cached_input_tokens",
    "output_tokens",
    "reasoning_output_tokens",
    "total_tokens",
];

#[derive(Debug, Clone, Default)]
pub struct UsageTotals {
    pub non_cached_input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
    pub cost_usd: f64,
}

impl UsageTotals {
    fn add(&mut self, other: &UsageTotals) {
        self.non_cached_input_tokens = self
            .non_cached_input_tokens
            .saturating_add(other.non_cached_input_tokens);
        self.cached_input_tokens = self
            .cached_input_tokens
            .saturating_add(other.cached_input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.reasoning_output_tokens = self
            .reasoning_output_tokens
            .saturating_add(other.reasoning_output_tokens);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
        self.cost_usd += other.cost_usd;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ModelBucket {
    Gpt5,
    Gpt5Codex,
    Gpt5Mini,
    Gpt51,
    Gpt51Codex,
    Gpt51CodexMini,
    CodeGpt5Codex,
    CodeGpt5CodexMini,
    CodeGpt5Mini,
    ChatGpt51Codex,
    ChatGpt51CodexMini,
    Other,
}

impl ModelBucket {
    pub fn as_str(&self) -> &'static str {
        match self {
            ModelBucket::Gpt5 => "gpt-5",
            ModelBucket::Gpt5Codex => "gpt-5-codex",
            ModelBucket::Gpt5Mini => "gpt-5-mini",
            ModelBucket::Gpt51 => "gpt-5.1",
            ModelBucket::Gpt51Codex => "gpt-5.1-codex",
            ModelBucket::Gpt51CodexMini => "gpt-5.1-codex-mini",
            ModelBucket::CodeGpt5Codex => "code-gpt-5-codex",
            ModelBucket::CodeGpt5CodexMini => "code-gpt-5-codex-mini",
            ModelBucket::CodeGpt5Mini => "code-gpt-5-mini",
            ModelBucket::ChatGpt51Codex => "chatgpt-5.1-codex",
            ModelBucket::ChatGpt51CodexMini => "chatgpt-5.1-codex-mini",
            ModelBucket::Other => "other",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ModelUsage {
    pub bucket: ModelBucket,
    pub totals: UsageTotals,
}

#[derive(Debug, Clone)]
pub struct SourceUsage {
    pub label: String,
    pub totals: UsageTotals,
}

#[derive(Debug, Clone)]
pub struct UsageBucket {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub totals: UsageTotals,
}

#[derive(Debug, Clone, Default)]
pub struct TrailingUsageTotals {
    pub last_hour: UsageTotals,
    pub last_twelve_hours: UsageTotals,
    pub last_day: UsageTotals,
    pub last_seven_days: UsageTotals,
    pub last_thirty_days: UsageTotals,
    pub last_year: UsageTotals,
}

#[derive(Debug, Clone)]
pub struct SessionUsage {
    pub session_id: String,
    pub model_bucket: ModelBucket,
    pub totals: UsageTotals,
}

#[derive(Debug, Clone, Default)]
pub struct GlobalUsageSnapshot {
    pub generated_at: DateTime<Utc>,
    pub sessions_processed: usize,
    pub sessions_missing_totals: usize,
    pub totals: UsageTotals,
    pub model_usage: Vec<ModelUsage>,
    pub source_usage: Vec<SourceUsage>,
    pub trailing: TrailingUsageTotals,
    pub hourly_buckets: Vec<UsageBucket>,
    pub twelve_hour_buckets: Vec<UsageBucket>,
    pub daily_buckets: Vec<UsageBucket>,
    pub weekly_buckets: Vec<UsageBucket>,
    pub monthly_buckets: Vec<UsageBucket>,
    pub largest_session: Option<SessionUsage>,
    pub per_session: Vec<SessionUsage>,
}

#[derive(Debug, Clone)]
pub struct GlobalUsageScanOptions {
    pub code_home: PathBuf,
    pub sessions_dir_override: Option<PathBuf>,
    pub legacy_code_home: Option<PathBuf>,
    pub max_workers: Option<usize>,
    pub record_sessions: bool,
}

impl GlobalUsageScanOptions {
    pub fn new(code_home: PathBuf) -> Self {
        Self {
            code_home,
            sessions_dir_override: None,
            legacy_code_home: legacy_code_home_dir_for_read(),
            max_workers: None,
            record_sessions: false,
        }
    }

    pub fn with_sessions_override(mut self, dir: PathBuf) -> Self {
        self.sessions_dir_override = Some(dir);
        self
    }

    pub fn with_max_workers(mut self, workers: usize) -> Self {
        if workers > 0 {
            self.max_workers = Some(workers);
        }
        self
    }

    pub fn with_record_sessions(mut self, record: bool) -> Self {
        self.record_sessions = record;
        self
    }

    fn effective_worker_count(&self) -> usize {
        if let Some(explicit) = self.max_workers {
            return explicit.max(1);
        }
        let fallback = thread::available_parallelism()
            .ok()
            .map(|n| n.get())
            .unwrap_or(4);
        fallback.min(32).max(1)
    }
}

pub fn scan_global_usage(options: GlobalUsageScanOptions) -> Result<GlobalUsageSnapshot> {
    scan_global_usage_at(options, Utc::now())
}

pub fn scan_global_usage_at(
    options: GlobalUsageScanOptions,
    now: DateTime<Utc>,
) -> Result<GlobalUsageSnapshot> {
    let worker_count = options.effective_worker_count();
    let mut parser = SessionAggregator::new(now, options.record_sessions);
    parser.scan(&options, worker_count)?;
    Ok(parser.finish())
}

struct SessionAggregator {
    now: DateTime<Utc>,
    record_sessions: bool,
    totals: UsageTotals,
    model_totals: BTreeMap<ModelBucket, UsageTotals>,
    source_totals: BTreeMap<String, UsageTotals>,
    timeline_events: Vec<UsageEvent>,
    sessions_processed: usize,
    sessions_missing_totals: usize,
    largest_session: Option<SessionUsage>,
    per_session: Vec<SessionUsage>,
}

impl SessionAggregator {
    fn new(now: DateTime<Utc>, record_sessions: bool) -> Self {
        Self {
            now,
            record_sessions,
            totals: UsageTotals::default(),
            model_totals: BTreeMap::new(),
            source_totals: BTreeMap::new(),
            timeline_events: Vec::new(),
            sessions_processed: 0,
            sessions_missing_totals: 0,
            largest_session: None,
            per_session: Vec::new(),
        }
    }

    fn scan(&mut self, options: &GlobalUsageScanOptions, workers: usize) -> Result<()> {
        let sources = collect_session_sources(options);
        let mut tasks: Vec<(PathBuf, String)> = Vec::new();
        for source in sources {
            if !source.directory.exists() {
                continue;
            }
            for entry in WalkDir::new(&source.directory)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if entry.file_type().is_file()
                    && entry.path().extension().and_then(OsStr::to_str) == Some("jsonl")
                {
                    tasks.push((entry.into_path(), source.label.clone()));
                }
            }
        }

        tasks.sort_by(|a, b| a.0.cmp(&b.0));

        let results = parse_session_logs(tasks, workers);

        for (path, label, result) in results {
            match result {
                Ok(result) => {
                    if let Some(final_totals) = result.final_totals.clone() {
                        self.sessions_processed += 1;
                        self.consume_session(&label, result.bucket, final_totals.clone());
                        if self.record_sessions {
                            self.per_session.push(SessionUsage {
                                session_id: result.session_id.clone(),
                                model_bucket: result.bucket,
                                totals: final_totals.clone(),
                            });
                        }
                        match &self.largest_session {
                            Some(current) if final_totals.total_tokens <= current.totals.total_tokens => {}
                            _ => {
                                self.largest_session = Some(SessionUsage {
                                    session_id: result.session_id.clone(),
                                    model_bucket: result.bucket,
                                    totals: final_totals,
                                });
                            }
                        }
                    } else {
                        self.sessions_missing_totals += 1;
                    }
                    self.timeline_events.extend(result.events);
                }
                Err(err) => {
                    warn!(?path, "failed to parse session log: {err}");
                }
            }
        }

        Ok(())
    }

    fn consume_session(&mut self, label: &str, bucket: ModelBucket, totals: UsageTotals) {
        self.totals.add(&totals);
        self.model_totals
            .entry(bucket)
            .or_insert_with(UsageTotals::default)
            .add(&totals);
        self.source_totals
            .entry(label.to_string())
            .or_insert_with(UsageTotals::default)
            .add(&totals);
    }

    fn finish(self) -> GlobalUsageSnapshot {
        let mut model_usage: Vec<ModelUsage> = self
            .model_totals
            .into_iter()
            .map(|(bucket, totals)| ModelUsage { bucket, totals })
            .collect();
        model_usage.sort_by(|a, b| {
            b.totals
                .total_tokens
                .cmp(&a.totals.total_tokens)
                .then_with(|| a.bucket.as_str().cmp(b.bucket.as_str()))
        });

        let mut source_usage: Vec<SourceUsage> = self
            .source_totals
            .into_iter()
            .map(|(label, totals)| SourceUsage { label, totals })
            .collect();
        source_usage.sort_by(|a, b| {
            b.totals
                .total_tokens
                .cmp(&a.totals.total_tokens)
                .then_with(|| a.label.cmp(&b.label))
        });

        let hourly_buckets = compute_time_buckets(
            &self.timeline_events,
            12,
            Duration::hours(1),
            self.now,
        );
        let twelve_hour_buckets = compute_time_buckets(
            &self.timeline_events,
            14,
            Duration::hours(12),
            self.now,
        );
        let daily_buckets = compute_time_buckets(
            &self.timeline_events,
            7,
            Duration::days(1),
            self.now,
        );
        let weekly_buckets = compute_time_buckets(
            &self.timeline_events,
            8,
            Duration::days(7),
            self.now,
        );
        let monthly_buckets = compute_time_buckets(
            &self.timeline_events,
            6,
            Duration::days(30),
            self.now,
        );

        let trailing = TrailingUsageTotals {
            last_hour: compute_rolling_usage(&self.timeline_events, Duration::hours(1), self.now),
            last_twelve_hours: compute_rolling_usage(
                &self.timeline_events,
                Duration::hours(12),
                self.now,
            ),
            last_day: compute_rolling_usage(&self.timeline_events, Duration::days(1), self.now),
            last_seven_days: compute_rolling_usage(&self.timeline_events, Duration::days(7), self.now),
            last_thirty_days: compute_rolling_usage(
                &self.timeline_events,
                Duration::days(30),
                self.now,
            ),
            last_year: compute_rolling_usage(&self.timeline_events, Duration::days(365), self.now),
        };

        GlobalUsageSnapshot {
            generated_at: self.now,
            sessions_processed: self.sessions_processed,
            sessions_missing_totals: self.sessions_missing_totals,
            totals: self.totals,
            model_usage,
            source_usage,
            trailing,
            hourly_buckets,
            twelve_hour_buckets,
            daily_buckets,
            weekly_buckets,
            monthly_buckets,
            largest_session: self.largest_session,
            per_session: self.per_session,
        }
    }
}

fn parse_session_logs(
    tasks: Vec<(PathBuf, String)>,
    workers: usize,
) -> Vec<(PathBuf, String, Result<SessionParseResult>)> {
    if workers <= 1 {
        return tasks
            .into_iter()
            .map(|(path, label)| {
                let result = parse_session_log(&path, &label);
                (path, label, result)
            })
            .collect();
    }

    let job = || {
        tasks
            .into_par_iter()
            .map(|(path, label)| {
                let result = parse_session_log(&path, &label);
                (path, label, result)
            })
            .collect()
    };

    match ThreadPoolBuilder::new().num_threads(workers).build() {
        Ok(pool) => pool.install(job),
        Err(_) => job(),
    }
}

struct SessionSource {
    label: String,
    directory: PathBuf,
}

fn collect_session_sources(options: &GlobalUsageScanOptions) -> Vec<SessionSource> {
    if let Some(custom) = &options.sessions_dir_override {
        return vec![SessionSource {
            label: custom.display().to_string(),
            directory: custom.clone(),
        }];
    }

    let mut sources = Vec::new();
    let code_sessions = options.code_home.join(SESSIONS_SUBDIR);
    sources.extend(expand_with_slots(".code", &code_sessions));

    if let Some(legacy) = &options.legacy_code_home {
        let codex_sessions = legacy.join(SESSIONS_SUBDIR);
        sources.extend(expand_with_slots(".codex", &codex_sessions));
    }

    sources
}

fn expand_with_slots(label: &str, base_dir: &Path) -> Vec<SessionSource> {
    let mut sources = Vec::new();
    sources.push(SessionSource {
        label: label.to_string(),
        directory: base_dir.to_path_buf(),
    });

    if let Some(parent) = base_dir.parent() {
        let slot_root = parent.join(SLOT_DIR_NAME);
        if let Ok(entries) = std::fs::read_dir(&slot_root) {
            let mut slot_dirs: Vec<PathBuf> = entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .collect();
            slot_dirs.sort();
            for slot_dir in slot_dirs {
                let sessions = slot_dir.join(SESSIONS_SUBDIR);
                if sessions.exists() {
                    let slot_name = slot_dir
                        .file_name()
                        .and_then(OsStr::to_str)
                        .unwrap_or("slot");
                    sources.push(SessionSource {
                        label: format!("{label}/slot/{slot_name}"),
                        directory: sessions,
                    });
                }
            }
        }
    }

    sources
}

#[derive(Debug, Clone)]
struct UsageEvent {
    timestamp: DateTime<Utc>,
    deltas: UsageTotals,
}

struct SessionParseResult {
    session_id: String,
    bucket: ModelBucket,
    final_totals: Option<UsageTotals>,
    events: Vec<UsageEvent>,
}

fn parse_session_log(path: &Path, source_label: &str) -> Result<SessionParseResult> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut buffer = String::new();

    let mut session_id = path
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_string();
    let mut current_model = load_snapshot_model(path);
    if current_model.is_none() && source_label.starts_with(".code") {
        current_model = Some("gpt-5".to_string());
    }

    let mut totals_map: HashMap<&'static str, u64> = TOKEN_FIELDS.iter().map(|&f| (f, 0)).collect();
    let mut events = Vec::new();
    let mut session_totals = UsageTotals::default();

    while reader.read_line(&mut buffer)? != 0 {
        let line = buffer.trim();
        if line.is_empty() {
            buffer.clear();
            continue;
        }

        let entry: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(err) => {
                warn!(?path, "invalid json entry: {err}");
                buffer.clear();
                continue;
            }
        };

        match entry.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                if let Some(id) = entry
                    .get("payload")
                    .and_then(|p| p.get("id"))
                    .and_then(Value::as_str)
                {
                    session_id = id.to_string();
                }
                if let Some(model) = entry
                    .get("payload")
                    .and_then(|p| p.get("model"))
                    .and_then(Value::as_str)
                {
                    current_model = Some(model.to_string());
                }
            }
            Some("turn_context") => {
                if let Some(model) = entry
                    .get("payload")
                    .and_then(|p| p.get("model"))
                    .and_then(Value::as_str)
                {
                    current_model = Some(model.to_string());
                }
            }
            Some("event_msg") | Some("event") => {
                if let Some(payload) = extract_event_payload(&entry) {
                    match payload.kind {
                        "token_count" => {
                            if let Some(delta) = process_token_count(
                                payload.info,
                                entry.get("timestamp").and_then(Value::as_str),
                                current_model.as_deref().unwrap_or("gpt-5"),
                                &mut totals_map,
                                &mut events,
                            ) {
                                session_totals.add(&delta);
                            }
                        }
                        "turn_context" => {
                            if let Some(model) = payload
                                .payload
                                .and_then(|p| p.get("model"))
                                .and_then(Value::as_str)
                            {
                                current_model = Some(model.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }

        buffer.clear();
    }

    let bucket = current_model
        .as_deref()
        .map(ModelBucket::from_model_name)
        .unwrap_or(ModelBucket::Gpt5);

    let final_totals = if session_totals.total_tokens > 0 {
        Some(session_totals)
    } else {
        None
    };

    Ok(SessionParseResult {
        session_id,
        bucket,
        final_totals,
        events,
    })
}

struct EventPayload<'a> {
    kind: &'a str,
    info: Option<&'a Value>,
    payload: Option<&'a Value>,
}

fn extract_event_payload<'a>(entry: &'a Value) -> Option<EventPayload<'a>> {
    if entry.get("type").and_then(Value::as_str) == Some("event") {
        let payload = entry.get("payload")?.get("msg")?;
        Some(EventPayload {
            kind: payload.get("type").and_then(Value::as_str).unwrap_or(""),
            info: payload.get("info"),
            payload: Some(payload),
        })
    } else {
        let payload = entry.get("payload")?;
        Some(EventPayload {
            kind: payload.get("type").and_then(Value::as_str).unwrap_or(""),
            info: payload.get("info"),
            payload: Some(payload),
        })
    }
}

fn process_token_count(
    info: Option<&Value>,
    timestamp: Option<&str>,
    model_name: &str,
    totals_map: &mut HashMap<&'static str, u64>,
    events: &mut Vec<UsageEvent>,
) -> Option<UsageTotals> {
    let usage = info?.get("total_token_usage")?;

    let mut deltas = UsageTotals::default();
    let mut delta_input = 0u64;
    let mut delta_cached = 0u64;

    for field in TOKEN_FIELDS {
        if let Some(value) = usage.get(field).and_then(Value::as_u64) {
            let prev = totals_map.get_mut(field).unwrap();
            let delta = value.saturating_sub(*prev);
            *prev = value;
            match field {
                "input_tokens" => delta_input = delta,
                "cached_input_tokens" => {
                    delta_cached = delta;
                    deltas.cached_input_tokens = delta;
                }
                "output_tokens" => deltas.output_tokens = delta,
                "reasoning_output_tokens" => deltas.reasoning_output_tokens = delta,
                "total_tokens" => deltas.total_tokens = delta,
                _ => {}
            }
        }
    }

    deltas.non_cached_input_tokens = delta_input.saturating_sub(delta_cached);

    let bucket = ModelBucket::from_model_name(model_name);
    let billable_output = deltas.output_tokens + deltas.reasoning_output_tokens;
    deltas.cost_usd = estimate_cost(bucket, deltas.non_cached_input_tokens, deltas.cached_input_tokens, billable_output);

    if let Some(ts) = timestamp.and_then(parse_timestamp) {
        events.push(UsageEvent {
            timestamp: ts,
            deltas: deltas.clone(),
        });
    }

    Some(deltas)
}

fn parse_timestamp(raw: &str) -> Option<DateTime<Utc>> {
    let normalized = if let Some(stripped) = raw.strip_suffix('Z') {
        format!("{}+00:00", stripped)
    } else {
        raw.to_string()
    };
    DateTime::parse_from_rfc3339(&normalized)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
}

fn compute_time_buckets(
    events: &[UsageEvent],
    bucket_count: usize,
    bucket_size: Duration,
    now: DateTime<Utc>,
) -> Vec<UsageBucket> {
    if bucket_count == 0 {
        return Vec::new();
    }

    let end = now;
    let start = end - bucket_size * (bucket_count as i32);
    let mut buckets = Vec::with_capacity(bucket_count);
    for idx in 0..bucket_count {
        let bucket_start = start + bucket_size * (idx as i32);
        let bucket_end = bucket_start + bucket_size;
        buckets.push(UsageBucket {
            start: bucket_start,
            end: bucket_end,
            totals: UsageTotals::default(),
        });
    }

    for event in events {
        if event.timestamp < start || event.timestamp >= end {
            continue;
        }
        let offset = event.timestamp - start;
        let idx = (offset.num_seconds() / bucket_size.num_seconds()).clamp(0, bucket_count as i64 - 1);
        if let Some(bucket) = buckets.get_mut(idx as usize) {
            bucket.totals.add(&event.deltas);
        }
    }

    buckets
}

fn compute_rolling_usage(
    events: &[UsageEvent],
    duration: Duration,
    now: DateTime<Utc>,
) -> UsageTotals {
    let window_start = now - duration;
    let mut totals = UsageTotals::default();
    for event in events {
        if event.timestamp >= window_start && event.timestamp <= now {
            totals.add(&event.deltas);
        }
    }
    totals
}

impl ModelBucket {
    pub fn from_model_name(model: &str) -> Self {
        let normalized = model.to_lowercase();
        if normalized.contains("gpt-5.1-codex-mini") || normalized.contains("gpt51codexmini") {
            ModelBucket::Gpt51CodexMini
        } else if normalized.contains("gpt-5.1-codex") || normalized.contains("gpt51codex") {
            ModelBucket::Gpt51Codex
        } else if normalized.contains("chatgpt-5.1-codex-mini") || normalized.contains("chatgpt51mini") {
            ModelBucket::ChatGpt51CodexMini
        } else if normalized.contains("chatgpt-5.1-codex") || normalized.contains("chatgpt51") {
            ModelBucket::ChatGpt51Codex
        } else if normalized.contains("gpt-5.1") || normalized.contains("gpt51") {
            ModelBucket::Gpt51
        } else if normalized.contains("code-gpt-5-codex-mini") {
            ModelBucket::CodeGpt5CodexMini
        } else if normalized.contains("code-gpt-5-codex") {
            ModelBucket::CodeGpt5Codex
        } else if normalized.contains("code-gpt-5-mini") {
            ModelBucket::CodeGpt5Mini
        } else if normalized.contains("gpt-5-codex") {
            ModelBucket::Gpt5Codex
        } else if normalized.contains("gpt-5-mini") {
            ModelBucket::Gpt5Mini
        } else if normalized.contains("gpt-5") {
            ModelBucket::Gpt5
        } else {
            ModelBucket::Other
        }
    }
}

fn load_snapshot_model(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_string_lossy();
    let snapshot_path = path.with_file_name(format!("{stem}.snapshot.json"));
    let file = File::open(snapshot_path).ok()?;
    let json: Value = serde_json::from_reader(BufReader::new(file)).ok()?;
    let records = json.get("records")?.as_array()?;
    for record in records {
        if let Some(plain) = record.get("PlainMessage") {
            if let Some(lines) = plain.get("lines").and_then(Value::as_array) {
                for line in lines {
                    if let Some(spans) = line.get("spans").and_then(Value::as_array) {
                        for span in spans {
                            if let Some(text) = span.get("text").and_then(Value::as_str) {
                                let trimmed = text.trim();
                                if trimmed.to_lowercase().starts_with("model:") {
                                    return trimmed.splitn(2, ':').nth(1).map(|s| s.trim().to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

fn estimate_cost(
    bucket: ModelBucket,
    non_cached: u64,
    cached: u64,
    output: u64,
) -> f64 {
    let (non_cached_rate, cached_rate, output_rate) = match bucket {
        ModelBucket::Gpt5
        | ModelBucket::Gpt5Codex
        | ModelBucket::Gpt51
        | ModelBucket::Gpt51Codex
        | ModelBucket::CodeGpt5Codex
        | ModelBucket::ChatGpt51Codex => (1.25, 0.125, 10.0),
        ModelBucket::Gpt5Mini
        | ModelBucket::Gpt51CodexMini
        | ModelBucket::CodeGpt5CodexMini
        | ModelBucket::CodeGpt5Mini
        | ModelBucket::ChatGpt51CodexMini => (0.25, 0.025, 2.0),
        ModelBucket::Other => (1.25, 0.125, 10.0),
    };

    tokens_to_cost(non_cached, non_cached_rate)
        + tokens_to_cost(cached, cached_rate)
        + tokens_to_cost(output, output_rate)
}

fn tokens_to_cost(tokens: u64, rate: f64) -> f64 {
    if tokens == 0 {
        0.0
    } else {
        (tokens as f64 / 1_000_000.0) * rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    use chrono::{TimeZone};
    use serde_json::json;

    fn write_session(dir: &Path, name: &str, lines: &[Value]) {
        let path = dir.join(format!("{name}.jsonl"));
        let body = lines
            .iter()
            .map(|line| serde_json::to_string(line).expect("serialize"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(path, body).expect("write session");
    }

    fn session_meta(id: &str, model: &str) -> Value {
        json!({"type":"session_meta","payload":{"id":id,"model":model}})
    }

    fn token_event(
        timestamp: &str,
        input: u64,
        cached: u64,
        output: u64,
        reasoning: u64,
        total: u64,
    ) -> Value {
        json!({
            "type":"event_msg",
            "timestamp": timestamp,
            "payload":{
                "type":"token_count",
                "info":{
                    "total_token_usage":{
                        "input_tokens":input,
                        "cached_input_tokens":cached,
                        "output_tokens":output,
                        "reasoning_output_tokens":reasoning,
                        "total_tokens":total
                    }
                }
            }
        })
    }

    #[test]
    fn aggregates_simple_session() {
        let temp = TempDir::new().expect("tempdir");
        let code_home = temp.path().join(".code");
        let sessions = code_home.join(SESSIONS_SUBDIR);
        fs::create_dir_all(&sessions).expect("session dir");

        let log_path = sessions.join("sess-1.jsonl");
        fs::write(
            &log_path,
            r#"{"type":"session_meta","payload":{"id":"sess-1","model":"gpt-5.1-codex"}}
{"type":"event_msg","timestamp":"2025-11-19T00:00:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":5,"reasoning_output_tokens":1,"total_tokens":16}}}}
{"type":"event_msg","timestamp":"2025-11-19T00:10:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":30,"cached_input_tokens":6,"output_tokens":25,"reasoning_output_tokens":4,"total_tokens":65}}}}
"#,
        )
        .expect("write log");

        let options = GlobalUsageScanOptions::new(code_home)
            .with_sessions_override(sessions.clone());
        let snapshot = scan_global_usage(options).expect("scan");
        assert_eq!(snapshot.sessions_processed, 1);
        assert_eq!(snapshot.sessions_missing_totals, 0);
        assert_eq!(snapshot.totals.non_cached_input_tokens, 24); // (10-2)+(20-4)
        assert_eq!(snapshot.totals.output_tokens, 25);
        assert_eq!(snapshot.totals.reasoning_output_tokens, 4);
        assert_eq!(snapshot.model_usage.len(), 1);
        assert_eq!(snapshot.source_usage.len(), 1);
    }

    #[test]
    fn monotonic_deltas_never_double_count() {
        let temp = TempDir::new().expect("tempdir");
        let code_home = temp.path().join(".code");
        let sessions = code_home.join(SESSIONS_SUBDIR);
        fs::create_dir_all(&sessions).expect("session dir");

        write_session(
            &sessions,
            "sess-rolling",
            &[
                session_meta("sess-rolling", "gpt-5.1-codex"),
                token_event("2025-11-19T00:00:00Z", 100, 30, 50, 10, 190),
                token_event("2025-11-19T00:05:00Z", 110, 35, 60, 15, 230),
                token_event("2025-11-19T00:10:00Z", 105, 40, 100, 25, 270),
            ],
        );

        let options = GlobalUsageScanOptions::new(code_home)
            .with_sessions_override(sessions.clone());
        let snapshot = scan_global_usage(options).expect("scan");

        assert_eq!(snapshot.sessions_processed, 1);
        assert_eq!(snapshot.totals.non_cached_input_tokens, 75);
        assert_eq!(snapshot.totals.cached_input_tokens, 40);
        assert_eq!(snapshot.totals.output_tokens, 100);
        assert_eq!(snapshot.totals.reasoning_output_tokens, 25);
        assert_eq!(snapshot.totals.total_tokens, 270);
    }

    #[test]
    fn model_buckets_and_costs_match_tables() {
        let temp = TempDir::new().expect("tempdir");
        let code_home = temp.path().join(".code");
        let sessions = code_home.join(SESSIONS_SUBDIR);
        fs::create_dir_all(&sessions).expect("session dir");

        write_session(
            &sessions,
            "sess-premium",
            &[
                session_meta("sess-premium", "gpt-5.1-codex"),
                token_event(
                    "2025-11-19T01:00:00Z",
                    1_000_000,
                    200_000,
                    500_000,
                    0,
                    1_700_000,
                ),
            ],
        );

        write_session(
            &sessions,
            "sess-mini",
            &[
                session_meta("sess-mini", "code-gpt-5-codex-mini"),
                token_event(
                    "2025-11-19T02:00:00Z",
                    400_000,
                    100_000,
                    150_000,
                    0,
                    650_000,
                ),
            ],
        );

        let options = GlobalUsageScanOptions::new(code_home)
            .with_sessions_override(sessions.clone());
        let snapshot = scan_global_usage(options).expect("scan");

        assert_eq!(snapshot.sessions_processed, 2);
        assert_eq!(snapshot.model_usage.len(), 2);

        let total_cost = snapshot.totals.cost_usd;
        let expected_cost = 6.4025; // derived from the MODEL_COSTS table
        assert!((total_cost - expected_cost).abs() < 1e-6);

        let premium = snapshot
            .model_usage
            .iter()
            .find(|entry| matches!(entry.bucket, ModelBucket::Gpt51Codex))
            .expect("premium bucket");
        assert_eq!(premium.totals.total_tokens, 1_700_000);

        let mini = snapshot
            .model_usage
            .iter()
            .find(|entry| matches!(entry.bucket, ModelBucket::CodeGpt5CodexMini))
            .expect("mini bucket");
        assert_eq!(mini.totals.total_tokens, 650_000);
    }

    #[test]
    fn time_buckets_and_trailing_windows_match_python_ranges() {
        let temp = TempDir::new().expect("tempdir");
        let code_home = temp.path().join(".code");
        let sessions = code_home.join(SESSIONS_SUBDIR);
        fs::create_dir_all(&sessions).expect("session dir");

        write_session(
            &sessions,
            "sess-timeline",
            &[
                session_meta("sess-timeline", "gpt-5"),
                token_event("2025-01-01T10:15:00Z", 10, 0, 0, 0, 10),
                token_event("2025-01-01T11:30:00Z", 20, 0, 0, 0, 20),
            ],
        );

        let now = Utc
            .with_ymd_and_hms(2025, 1, 1, 12, 0, 0)
            .single()
            .expect("valid timestamp");
        let options = GlobalUsageScanOptions::new(code_home)
            .with_sessions_override(sessions.clone());
        let snapshot = scan_global_usage_at(options, now).expect("scan");

        assert_eq!(snapshot.trailing.last_hour.total_tokens, 10);
        assert_eq!(snapshot.trailing.last_twelve_hours.total_tokens, 20);
        assert_eq!(snapshot.trailing.last_day.total_tokens, 20);

        assert_eq!(snapshot.hourly_buckets.len(), 12);
        let last_bucket = snapshot.hourly_buckets.last().expect("bucket");
        assert_eq!(last_bucket.totals.total_tokens, 10);
    }
}
