use super::ChatWidget;
use crate::colors;
use crate::util::buffer::fill_rect;
use chrono::{DateTime, Utc};
use code_core::protocol::{Op, RunningTaskInfo, RunningTaskKind, RunningTasksSnapshotEvent};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use std::cell::{Cell, RefCell};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap, Widget};
use unicode_width::UnicodeWidthChar;

/// State backing the task-manager overlay.
#[derive(Default)]
pub(super) struct TaskManagerState {
    overlay: RefCell<Option<TaskManagerOverlay>>,
    body_rows: Cell<u16>,
}

impl TaskManagerState {
    pub fn is_visible(&self) -> bool {
        self.overlay.borrow().is_some()
    }

    pub fn begin_refresh(&self) {
        let mut overlay = self.overlay.borrow_mut();
        if let Some(overlay) = overlay.as_mut() {
            overlay.loading = true;
            overlay.status_message = Some("Refreshing running tasks…".to_string());
        } else {
            *overlay = Some(TaskManagerOverlay::loading());
        }
    }

    pub fn close(&self) {
        self.overlay.borrow_mut().take();
    }

    fn with_overlay_mut<R>(&self, f: impl FnOnce(&mut TaskManagerOverlay) -> R) -> Option<R> {
        let mut borrow = self.overlay.borrow_mut();
        borrow.as_mut().map(f)
    }

    fn with_overlay_mut_or_create<R>(&self, f: impl FnOnce(&mut TaskManagerOverlay) -> R) -> R {
        let mut borrow = self.overlay.borrow_mut();
        if borrow.is_none() {
            *borrow = Some(TaskManagerOverlay::loading());
        }
        f(borrow.as_mut().expect("overlay present"))
    }

    pub fn set_body_rows(&self, rows: u16) {
        self.body_rows.set(rows.max(1));
    }

    pub fn body_rows(&self) -> u16 {
        self.body_rows.get().max(1)
    }
}

#[derive(Clone)]
struct TaskManagerOverlay {
    tasks: Vec<RunningTaskInfo>,
    selected: usize,
    scroll: u16,
    loading: bool,
    last_updated: Option<DateTime<Utc>>,
    status_message: Option<String>,
}

enum TaskKeyAction {
    None,
    Redraw,
    Close,
    Refresh,
    Cancel { id: String, sub_id: Option<String>, kind: RunningTaskKind },
}

impl TaskManagerOverlay {
    fn loading() -> Self {
        Self {
            tasks: Vec::new(),
            selected: 0,
            scroll: 0,
            loading: true,
            last_updated: None,
            status_message: Some("Fetching running tasks…".to_string()),
        }
    }

    fn set_tasks(&mut self, mut tasks: Vec<RunningTaskInfo>) {
        tasks.sort_by_key(|task| task.started_at_ms);
        self.tasks = tasks;
        if self.tasks.is_empty() {
            self.selected = 0;
            self.scroll = 0;
        } else if self.selected >= self.tasks.len() {
            self.selected = self.tasks.len().saturating_sub(1);
        }
    }

    fn ensure_selection_visible(&mut self, rows: usize) {
        if self.tasks.is_empty() {
            self.scroll = 0;
            self.selected = 0;
            return;
        }
        let max_scroll = self.tasks.len().saturating_sub(rows);
        if self.scroll as usize > max_scroll {
            self.scroll = max_scroll as u16;
        }
        if self.selected >= self.tasks.len() {
            self.selected = self.tasks.len().saturating_sub(1);
        }
        if self.selected < self.scroll as usize {
            self.scroll = self.selected as u16;
        } else if self.selected >= self.scroll as usize + rows {
            let new_scroll = self.selected + 1 - rows;
            self.scroll = new_scroll as u16;
        }
    }

    fn current_task(&self) -> Option<&RunningTaskInfo> {
        self.tasks.get(self.selected)
    }

    fn move_selection_up(&mut self, rows: usize) {
        if self.selected > 0 {
            self.selected -= 1;
            self.ensure_selection_visible(rows);
        }
    }

    fn move_selection_down(&mut self, rows: usize) {
        if self.selected + 1 < self.tasks.len() {
            self.selected += 1;
            self.ensure_selection_visible(rows);
        }
    }

    fn page_up(&mut self, rows: usize) {
        self.selected = self.selected.saturating_sub(rows.saturating_sub(1).max(1));
        self.scroll = self.scroll.saturating_sub(rows as u16);
        self.ensure_selection_visible(rows);
    }

    fn page_down(&mut self, rows: usize) {
        if self.tasks.is_empty() {
            return;
        }
        let max_index = self.tasks.len().saturating_sub(1);
        let delta = rows.saturating_sub(1).max(1);
        self.selected = (self.selected + delta).min(max_index);
        let max_scroll = self.tasks.len().saturating_sub(rows);
        let desired_scroll = self.selected.saturating_sub(rows.saturating_sub(1));
        self.scroll = desired_scroll.min(max_scroll) as u16;
        self.ensure_selection_visible(rows);
    }
}

pub(super) fn handle_running_tasks_snapshot(
    widget: &mut ChatWidget,
    event: RunningTasksSnapshotEvent,
) {
    widget
        .task_manager
        .with_overlay_mut_or_create(|overlay| {
            overlay.loading = false;
            overlay.status_message = None;
            overlay.last_updated = Some(Utc::now());
            overlay.set_tasks(event.tasks);
        });
    widget.request_redraw();
}

pub(super) fn handle_key(widget: &mut ChatWidget, key: KeyEvent) -> bool {
    if !widget.task_manager.is_visible() {
        return false;
    }
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return true;
    }
    let visible_rows = widget.task_manager.body_rows() as usize;
    let action = widget.task_manager.with_overlay_mut(|overlay| match key.code {
        KeyCode::Esc | KeyCode::Char('q') => TaskKeyAction::Close,
        KeyCode::Char('r') | KeyCode::Char('R') => TaskKeyAction::Refresh,
        KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => {
            overlay.move_selection_up(visible_rows);
            TaskKeyAction::Redraw
        }
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => {
            overlay.move_selection_down(visible_rows);
            TaskKeyAction::Redraw
        }
        KeyCode::PageUp => {
            overlay.page_up(visible_rows);
            TaskKeyAction::Redraw
        }
        KeyCode::PageDown => {
            overlay.page_down(visible_rows);
            TaskKeyAction::Redraw
        }
        KeyCode::Char('c') | KeyCode::Char('C') => {
            if let Some(task) = overlay.current_task().cloned() {
                if task.can_cancel {
                    overlay.status_message =
                        Some(format!("Cancel requested for {}", task.label));
                    TaskKeyAction::Cancel {
                        id: task.id,
                        sub_id: task.sub_id,
                        kind: task.kind,
                    }
                } else {
                    overlay.status_message = Some("Task cannot be cancelled".to_string());
                    TaskKeyAction::Redraw
                }
            } else {
                TaskKeyAction::None
            }
        }
        KeyCode::Enter => TaskKeyAction::Close,
        _ => TaskKeyAction::None,
    });

    let Some(action) = action else {
        return false;
    };

    match action {
        TaskKeyAction::None => {}
        TaskKeyAction::Redraw => widget.request_redraw(),
        TaskKeyAction::Close => {
            widget.task_manager.close();
            widget.request_redraw();
        }
        TaskKeyAction::Refresh => {
            widget.task_manager.begin_refresh();
            widget.request_running_tasks_snapshot();
        }
        TaskKeyAction::Cancel { id, sub_id, kind } => {
            widget.submit_op(Op::TerminateTask { id, sub_id, kind });
            widget.request_redraw();
        }
    }
    true
}

pub(super) fn render_task_manager_overlay(
    widget: &ChatWidget,
    area: Rect,
    history_area: Rect,
    buf: &mut Buffer,
) {
    let mut overlay_borrow = widget.task_manager.overlay.borrow_mut();
    let Some(overlay) = overlay_borrow.as_mut() else {
        return;
    };
    let scrim_style = Style::default()
        .bg(colors::overlay_scrim())
        .fg(colors::text_dim());
    fill_rect(buf, area, None, scrim_style);

    let padding = 1u16;
    let width = history_area.width.saturating_sub(padding * 2).max(40);
    let height = history_area.height;
    let x = history_area.x + (history_area.width.saturating_sub(width)) / 2;
    let window = Rect {
        x,
        y: history_area.y,
        width,
        height,
    };

    Clear.render(window, buf);
    let title = build_title_spans(overlay);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(title))
        .style(Style::default().bg(colors::background()))
        .border_style(
            Style::default()
                .fg(colors::border())
                .bg(colors::background()),
        );
    let inner = block.inner(window);
    block.render(window, buf);

    let layout = Layout::vertical([
        Constraint::Length(3),
        Constraint::Fill(1),
        Constraint::Length(2),
    ])
    .split(inner);

    render_summary(overlay, layout[0], buf);
    widget.task_manager.set_body_rows(layout[1].height);
    let visible_rows = widget.task_manager.body_rows() as usize;
    overlay.ensure_selection_visible(visible_rows);
    render_task_list(overlay, layout[1], visible_rows, buf);
    render_footer(overlay, layout[2], buf);
}

fn render_summary(overlay: &TaskManagerOverlay, area: Rect, buf: &mut Buffer) {
    if area.height == 0 {
        return;
    }
    let mut lines: Vec<Line> = Vec::new();
    let mut status = format!("{} running task{}",
        overlay.tasks.len(),
        if overlay.tasks.len() == 1 { "" } else { "s" }
    );
    if overlay.loading {
        status.push_str(" · refreshing…");
    } else if let Some(updated) = overlay.last_updated {
        status.push_str(&format!(
            " · updated {}",
            updated.with_timezone(&chrono::Local).format("%H:%M:%S")
        ));
    }
    lines.push(Line::from(vec![Span::styled(
        status,
        Style::default().fg(colors::text()),
    )]));

    if let Some(message) = overlay.status_message.as_ref() {
        lines.push(Line::from(vec![Span::styled(
            message.clone(),
            Style::default().fg(colors::info()),
        )]));
    }

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: true })
        .style(Style::default().bg(colors::background()).fg(colors::text()));
    Widget::render(paragraph, area, buf);
}

fn render_task_list(overlay: &TaskManagerOverlay, area: Rect, visible_rows: usize, buf: &mut Buffer) {
    if area.height == 0 {
        return;
    }
    let lines = build_task_lines(overlay, area.width as usize);
    let visible_rows = visible_rows.max(1);
    let max_offset = lines.len().saturating_sub(visible_rows);
    let skip = (overlay.scroll as usize).min(max_offset);
    let end = (skip + visible_rows).min(lines.len());
    let slice = if skip < lines.len() { &lines[skip..end] } else { &[] };
    let paragraph = Paragraph::new(slice.to_vec())
        .wrap(Wrap { trim: false })
        .style(Style::default().bg(colors::background()));
    Widget::render(paragraph, area, buf);
}

fn render_footer(_overlay: &TaskManagerOverlay, area: Rect, buf: &mut Buffer) {
    if area.height == 0 {
        return;
    }
    let instructions = "↑/↓ select  ·  PageUp/PageDown scroll  ·  c cancel task  ·  r refresh  ·  Esc close";
    let line = Line::from(vec![Span::styled(
        truncate_to_width(instructions, area.width as usize),
        Style::default()
            .fg(colors::text_dim())
            .add_modifier(Modifier::ITALIC),
    )]);
    let paragraph = Paragraph::new(vec![line]).wrap(Wrap { trim: true });
    Widget::render(paragraph, area, buf);
}

fn build_title_spans(overlay: &TaskManagerOverlay) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    spans.push(Span::styled(
        " Task manager ",
        Style::default()
            .fg(colors::text())
            .add_modifier(Modifier::BOLD),
    ));
    if overlay.loading {
        spans.push(Span::styled(
            " refreshing… ",
            Style::default().fg(colors::info()),
        ));
    }
    spans.push(Span::styled(
        " — Esc close · r refresh · c cancel",
        Style::default().fg(colors::text_dim()),
    ));
    spans
}

fn build_task_lines(overlay: &TaskManagerOverlay, width: usize) -> Vec<Line<'static>> {
    if overlay.tasks.is_empty() {
        let msg = if overlay.loading {
            "Waiting for running tasks…"
        } else {
            "No running tasks"
        };
        return vec![Line::from(vec![Span::styled(
            msg,
            Style::default().fg(colors::text_dim()),
        )])];
    }

    let kind_col = 12usize;
    let duration_col = 9usize;
    let gap = 3usize;
    let desc_width = width
        .saturating_sub(kind_col + duration_col + gap)
        .max(8);

    overlay
        .tasks
        .iter()
        .enumerate()
        .map(|(idx, task)| {
            let mut spans = Vec::new();
            let kind = truncate_to_width(format_kind(task.kind), kind_col);
            let duration = truncate_to_width(&format_elapsed(task.started_at_ms), duration_col);
            let desc = truncate_to_width(&summarize_command(task), desc_width);

            spans.push(Span::styled(
                format!("{:kind_col$}", kind, kind_col = kind_col),
                Style::default().fg(colors::text_dim()),
            ));
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                format!("{:>duration_col$}", duration, duration_col = duration_col),
                Style::default().fg(colors::text_dim()),
            ));
            spans.push(Span::raw("   "));
            let desc_style = if task.can_cancel {
                Style::default().fg(colors::text())
            } else {
                Style::default().fg(colors::text_dim())
            };
            spans.push(Span::styled(desc, desc_style));

            if idx == overlay.selected {
                for span in spans.iter_mut() {
                    span.style = span
                        .style
                        .bg(colors::selection())
                        .fg(colors::background());
                }
            }

            Line::from(spans)
        })
        .collect()
}

fn summarize_command(info: &RunningTaskInfo) -> String {
    if !info.command_line.is_empty() {
        info.command_line.join(" ")
    } else {
        info.label.clone()
    }
}

fn format_kind(kind: RunningTaskKind) -> &'static str {
    match kind {
        RunningTaskKind::ForegroundExec => "Exec",
        RunningTaskKind::BackgroundExec => "Background",
        RunningTaskKind::Agent => "Agent",
    }
}

fn format_elapsed(start_ms: u64) -> String {
    let now_ms = Utc::now().timestamp_millis();
    let start_ms = start_ms as i64;
    let elapsed_ms = now_ms.saturating_sub(start_ms).max(0) as u64;
    let secs = elapsed_ms / 1000;
    if secs >= 3600 {
        format!("{:02}h{:02}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{:02}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{:02}s", secs)
    }
}

fn truncate_to_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let mut width = 0usize;
    let mut out = String::new();
    for ch in text.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if width + ch_width > max_width {
            out.push('…');
            return out;
        }
        out.push(ch);
        width += ch_width;
    }
    out
}

impl ChatWidget<'_> {
    pub(crate) fn request_running_tasks_snapshot(&self) {
        self.submit_op(Op::ListRunningTasks);
    }
}
