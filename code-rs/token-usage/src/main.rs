use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use code_core::config::find_code_home;
use code_core::global_usage_tracker::{
    scan_global_usage,
    GlobalUsageScanOptions,
    GlobalUsageSnapshot,
    ModelBucket,
    SourceUsage,
    UsageBucket,
    UsageTotals,
};
use crossterm::event::{self, Event, KeyCode, KeyEvent};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use crossterm::{execute, terminal};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

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

#[derive(Parser, Debug, Clone)]
#[command(author, version, about = "Rust global token usage viewer", long_about = None)]
struct Args {
    /// Override the session logs directory (default scans ~/.code + ~/.codex + slots)
    #[arg(long = "sessions-dir", value_name = "DIR")]
    sessions_dir: Option<PathBuf>,

    /// Number of worker threads to use while parsing session logs
    #[arg(
        long = "workers",
        value_name = "N",
        value_parser = clap::value_parser!(usize),
        help = "Number of parsing threads (default: min(32, CPU count))"
    )]
    workers: Option<usize>,

    /// Display per-session totals in the detailed panel
    #[arg(long = "verbose")]
    verbose: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppStatus {
    Idle,
    Scanning,
    Ready,
    Error,
}

#[derive(Debug, Clone)]
struct ScanConfig {
    code_home: PathBuf,
    sessions_dir: Option<PathBuf>,
    workers: Option<usize>,
    verbose_sessions: bool,
}

#[derive(Debug)]
enum ScanResult {
    Snapshot(GlobalUsageSnapshot, DateTime<Utc>),
    Error(String),
}

#[derive(Debug)]
enum AppCommand {
    Refresh,
    ToggleVerbose,
    Quit,
}

struct App {
    status: AppStatus,
    last_snapshot: Option<GlobalUsageSnapshot>,
    last_updated: Option<DateTime<Utc>>,
    last_error: Option<String>,
    verbose_sessions: bool,
    request_in_flight: bool,
}

impl App {
    fn new(verbose: bool) -> Self {
        Self {
            status: AppStatus::Idle,
            last_snapshot: None,
            last_updated: None,
            last_error: None,
            verbose_sessions: verbose,
            request_in_flight: false,
        }
    }

    fn apply_result(&mut self, result: ScanResult) {
        self.request_in_flight = false;
        match result {
            ScanResult::Snapshot(snapshot, ts) => {
                self.last_snapshot = Some(snapshot);
                self.last_updated = Some(ts);
                self.last_error = None;
                self.status = AppStatus::Ready;
            }
            ScanResult::Error(err) => {
                self.last_error = Some(err);
                self.status = AppStatus::Error;
            }
        }
    }

    fn mark_scanning(&mut self) {
        self.status = AppStatus::Scanning;
        self.request_in_flight = true;
    }

    fn toggle_verbose(&mut self) {
        self.verbose_sessions = !self.verbose_sessions;
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let code_home = find_code_home().context("failed to locate CODE_HOME")?;
    let scan_cfg = ScanConfig {
        code_home,
        sessions_dir: args.sessions_dir,
        workers: args.workers.filter(|w| *w > 0),
        verbose_sessions: args.verbose,
    };

    let (scan_tx, scan_rx) = mpsc::channel::<AppCommand>();
    let (result_tx, result_rx) = mpsc::channel::<ScanResult>();
    start_scan_worker(scan_cfg.clone(), scan_rx, result_tx)?;

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, terminal::EnterAlternateScreen, event::EnableMouseCapture)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(scan_cfg.verbose_sessions);
    request_refresh(&scan_tx, &mut app)?;

    let res = run_app(&mut terminal, &mut app, &scan_tx, &result_rx);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        terminal::LeaveAlternateScreen,
        event::DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    res
}

fn start_scan_worker(
    cfg: ScanConfig,
    rx: Receiver<AppCommand>,
    tx: Sender<ScanResult>,
) -> Result<()> {
    thread::spawn(move || {
        let mut verbose = cfg.verbose_sessions;
        for cmd in rx {
            match cmd {
                AppCommand::Refresh => {
                    let request = build_scan_options(&cfg, verbose);
                    let result = scan_once(request);
                    let _ = tx.send(result);
                }
                AppCommand::ToggleVerbose => {
                    verbose = !verbose;
                }
                AppCommand::Quit => break,
            }
        }
    });
    Ok(())
}

fn build_scan_options(cfg: &ScanConfig, verbose: bool) -> GlobalUsageScanOptions {
    let mut options = GlobalUsageScanOptions::new(cfg.code_home.clone());
    if let Some(dir) = &cfg.sessions_dir {
        options = options.with_sessions_override(dir.clone());
    }
    if let Some(workers) = cfg.workers {
        options = options.with_max_workers(workers);
    }
    options.with_record_sessions(verbose)
}

fn scan_once(options: GlobalUsageScanOptions) -> ScanResult {
    match scan_global_usage(options) {
        Ok(snapshot) => {
            let generated = snapshot.generated_at;
            ScanResult::Snapshot(snapshot, generated)
        }
        Err(err) => ScanResult::Error(err.to_string()),
    }
}

fn run_app(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
    scan_tx: &Sender<AppCommand>,
    result_rx: &Receiver<ScanResult>,
) -> Result<()> {
    let mut last_draw = Instant::now();
    loop {
        while let Ok(result) = result_rx.try_recv() {
            app.apply_result(result);
        }

        if last_draw.elapsed() >= Duration::from_millis(16) {
            terminal.draw(|frame| draw_ui(frame, app))?;
            last_draw = Instant::now();
        }

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if handle_key_event(key, app, scan_tx)? {
                    break;
                }
            }
        }
    }
    Ok(())
}

fn handle_key_event(key: KeyEvent, app: &mut App, scan_tx: &Sender<AppCommand>) -> Result<bool> {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => {
            let _ = scan_tx.send(AppCommand::Quit);
            return Ok(true);
        }
        KeyCode::Char('r') => {
            request_refresh(scan_tx, app)?;
        }
        KeyCode::Char('v') => {
            app.toggle_verbose();
            let _ = scan_tx.send(AppCommand::ToggleVerbose);
            request_refresh(scan_tx, app)?;
        }
        _ => {}
    }
    Ok(false)
}

fn request_refresh(scan_tx: &Sender<AppCommand>, app: &mut App) -> Result<()> {
    app.mark_scanning();
    scan_tx
        .send(AppCommand::Refresh)
        .context("failed to send refresh request")
}

fn draw_ui(frame: &mut Frame<'_>, app: &App) {
    let size = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(3),
                Constraint::Length(8),
                Constraint::Min(10),
            ]
            .as_ref(),
        )
        .split(size);

    draw_header(frame, chunks[0], app);
    draw_totals(frame, chunks[1], app);
    draw_detail(frame, chunks[2], app);
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let status = match app.status {
        AppStatus::Idle => "Idle",
        AppStatus::Scanning => "Scanning",
        AppStatus::Ready => "Ready",
        AppStatus::Error => "Error",
    };
    let timestamp = app
        .last_updated
        .map(|ts| ts.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "—".to_string());
    let help = "q:quit  r:refresh  v:toggle sessions";
    let text = format!(
        "Status: {status}    Last updated: {timestamp}    {help}"
    );
    let mut lines = vec![Line::from(text)];
    if let Some(snapshot) = &app.last_snapshot {
        lines.push(Line::from(format!(
            "Sessions processed: {}  missing totals: {}",
            snapshot.sessions_processed, snapshot.sessions_missing_totals
        )));
    }
    if let Some(err) = app.last_error.as_ref() {
        lines.push(
            Line::from(err.clone()).style(Style::default().fg(Color::Red)),
        );
    }
    let block = Block::default().borders(Borders::ALL).title("Global Usage");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }),
        inner,
    );
}

fn draw_totals(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let snapshot = match &app.last_snapshot {
        Some(s) => s,
        None => {
            render_placeholder(frame, area, "Totals");
            return;
        }
    };
    let layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let totals_lines = vec![
        format_total_line("Non-cached", snapshot.totals.non_cached_input_tokens),
        format_total_line("Cached", snapshot.totals.cached_input_tokens),
        format_total_line(
            "Output",
            snapshot.totals.output_tokens + snapshot.totals.reasoning_output_tokens,
        ),
        format_total_line("Total", snapshot.totals.total_tokens),
        format!("Cost: ${:.2}", snapshot.totals.cost_usd),
    ];
    let totals_para = Paragraph::new(join_lines(&totals_lines)).wrap(Wrap { trim: false });
    frame.render_widget(
        totals_para.block(Block::default().borders(Borders::ALL).title("Totals")),
        layout[0],
    );

    let trailing_lines = vec![
        format_window_line("Last hour", &snapshot.trailing.last_hour),
        format_window_line("Last 12h", &snapshot.trailing.last_twelve_hours),
        format_window_line("Last day", &snapshot.trailing.last_day),
        format_window_line("Last 7d", &snapshot.trailing.last_seven_days),
        format_window_line("Last 30d", &snapshot.trailing.last_thirty_days),
        format_window_line("Last year", &snapshot.trailing.last_year),
    ];
    let trailing_para = Paragraph::new(join_lines(&trailing_lines)).wrap(Wrap { trim: true });
    frame.render_widget(
        trailing_para
            .block(Block::default().borders(Borders::ALL).title("Recent windows")),
        layout[1],
    );
}

fn draw_detail(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let snapshot = match &app.last_snapshot {
        Some(s) => s,
        None => {
            render_placeholder(frame, area, "Details");
            return;
        }
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(11), Constraint::Length(7), Constraint::Min(12)])
        .split(area);

    draw_model_groups(frame, rows[0], snapshot);
    draw_source_panel(frame, rows[1], &snapshot.source_usage);
    draw_bucket_panel(frame, rows[2], snapshot, app.verbose_sessions);
}

fn draw_bucket_panel(
    frame: &mut Frame<'_>,
    area: Rect,
    snapshot: &GlobalUsageSnapshot,
    show_sessions: bool,
) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Min(6),
        ])
        .split(area);

    let top_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[0]);
    render_bucket_section(
        frame,
        top_cols[0],
        "Hourly (last 12)",
        &snapshot.hourly_buckets,
        12,
    );
    render_bucket_section(
        frame,
        top_cols[1],
        "12-hour (last 14)",
        &snapshot.twelve_hour_buckets,
        14,
    );

    let mid_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[1]);
    render_bucket_section(
        frame,
        mid_cols[0],
        "Daily (last 7)",
        &snapshot.daily_buckets,
        7,
    );
    render_bucket_section(
        frame,
        mid_cols[1],
        "Weekly (last 8)",
        &snapshot.weekly_buckets,
        8,
    );

    let bottom_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[2]);
    render_bucket_section(
        frame,
        bottom_cols[0],
        "Monthly (last 6)",
        &snapshot.monthly_buckets,
        6,
    );

    let session_lines = session_summary_lines(snapshot, show_sessions);
    frame.render_widget(
        Paragraph::new(join_lines(&session_lines))
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL).title("Sessions")),
        bottom_cols[1],
    );
}

fn bucket_lines(_title: &str, buckets: &[UsageBucket], limit: usize) -> Vec<String> {
    let mut lines = Vec::new();
    if buckets.is_empty() {
        lines.push("  (no data)".to_string());
        return lines;
    }
    for bucket in buckets.iter().take(limit) {
        let label = format!(
            "{}-{}",
            bucket.start.format("%m-%d %H:%M"),
            bucket.end.format("%H:%M")
        );
        lines.push(format!(
            "  {}  {}  ${:.2}",
            label,
            format_token_number(bucket.totals.total_tokens),
            bucket.totals.cost_usd
        ));
    }
    lines
}

fn session_summary_lines(snapshot: &GlobalUsageSnapshot, verbose: bool) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!(
        "Processed: {} (missing {})",
        snapshot.sessions_processed, snapshot.sessions_missing_totals
    ));
    if let Some(sess) = &snapshot.largest_session {
        lines.push(format!(
            "Largest: {} [{}] {}",
            sess.session_id,
            sess.model_bucket.as_str(),
            format_token_number(sess.totals.total_tokens)
        ));
    }
    if verbose {
        if snapshot.per_session.is_empty() {
            lines.push("No per-session data".to_string());
        } else {
            lines.push("Recent sessions:".to_string());
            for sess in snapshot.per_session.iter().take(8) {
                lines.push(format!(
                    "- {} [{}] {}",
                    sess.session_id,
                    sess.model_bucket.as_str(),
                    format_token_number(sess.totals.total_tokens)
                ));
            }
        }
    } else {
        lines.push("(Press v to show per-session totals)".to_string());
    }
    lines
}

fn render_bucket_section(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    buckets: &[UsageBucket],
    limit: usize,
) {
    let lines = bucket_lines(title, buckets, limit);
    frame.render_widget(
        Paragraph::new(join_lines(&lines))
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
}

fn draw_model_groups(frame: &mut Frame<'_>, area: Rect, snapshot: &GlobalUsageSnapshot) {
    let mut usage_by_bucket: BTreeMap<ModelBucket, UsageTotals> = BTreeMap::new();
    for entry in &snapshot.model_usage {
        usage_by_bucket.insert(entry.bucket, entry.totals.clone());
    }

    let mut lines = Vec::new();
    for (group_label, members) in MODEL_DISPLAY_GROUPS {
        let mut group_total = UsageTotals::default();
        let mut member_lines = Vec::new();
        for bucket in *members {
            if let Some(value) = usage_by_bucket.get(bucket) {
                accumulate_totals(&mut group_total, value);
                member_lines.push(format!(
                    "    {:<18} tokens={} cost=${:.2}",
                    bucket.as_str(),
                    format_token_number(value.total_tokens),
                    value.cost_usd
                ));
            }
        }
        if group_total.total_tokens == 0 && member_lines.is_empty() {
            continue;
        }
        lines.push(format!(
            "{:<16} tokens={} cost=${:.2}",
            group_label,
            format_token_number(group_total.total_tokens),
            group_total.cost_usd
        ));
        lines.extend(member_lines);
    }
    if lines.is_empty() {
        lines.push("(no model usage)".to_string());
    }
    frame.render_widget(
        Paragraph::new(join_lines(&lines))
            .wrap(Wrap { trim: true })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Model groups"),
            ),
        area,
    );
}

fn draw_source_panel(frame: &mut Frame<'_>, area: Rect, sources: &[SourceUsage]) {
    let mut lines = Vec::new();
    for entry in sources.iter().take(8) {
        lines.push(format!(
            "{:24} tokens={} cost=${:.2}",
            entry.label,
            format_token_number(entry.totals.total_tokens),
            entry.totals.cost_usd
        ));
    }
    if lines.is_empty() {
        lines.push("(no sources)".to_string());
    }
    frame.render_widget(
        Paragraph::new(join_lines(&lines))
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL).title("Top sources")),
        area,
    );
}

fn join_lines(lines: &[String]) -> Text<'_> {
    lines
        .iter()
        .map(|line| Line::from(line.clone()))
        .collect::<Vec<_>>()
        .into()
}

fn render_placeholder(frame: &mut Frame<'_>, area: Rect, title: &str) {
    let block = Block::default().borders(Borders::ALL).title(title);
    frame.render_widget(Paragraph::new("(no data)").block(block), area);
}

fn accumulate_totals(target: &mut UsageTotals, value: &UsageTotals) {
    target.non_cached_input_tokens = target
        .non_cached_input_tokens
        .saturating_add(value.non_cached_input_tokens);
    target.cached_input_tokens = target
        .cached_input_tokens
        .saturating_add(value.cached_input_tokens);
    target.output_tokens = target.output_tokens.saturating_add(value.output_tokens);
    target.reasoning_output_tokens = target
        .reasoning_output_tokens
        .saturating_add(value.reasoning_output_tokens);
    target.total_tokens = target.total_tokens.saturating_add(value.total_tokens);
    target.cost_usd += value.cost_usd;
}

fn format_total_line(label: &str, value: u64) -> String {
    format!("{label:<12} {}", format_token_number(value))
}

fn format_window_line(label: &str, totals: &UsageTotals) -> String {
    if totals.total_tokens == 0 {
        return format!("{label:<10} —");
    }
    let non_cached = format_token_number(totals.non_cached_input_tokens);
    let cached = format_token_number(totals.cached_input_tokens);
    let output = format_token_number(totals.output_tokens + totals.reasoning_output_tokens);
    format!(
        "{label:<10} nc={} cached={} out={} cost=${:.2}",
        non_cached, cached, output, totals.cost_usd
    )
}

fn format_token_number(value: u64) -> String {
    const SCALES: &[(u64, &str)] = &[
        (1_000_000_000_000, "T"),
        (1_000_000_000, "B"),
        (1_000_000, "M"),
        (1_000, "K"),
    ];
    for (scale, suffix) in SCALES {
        if value >= *scale {
            let scaled = value as f64 / *scale as f64;
            return format!("{scaled:.2}{suffix}");
        }
    }
    format!("{value}")
}
