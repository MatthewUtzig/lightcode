use super::BottomPane;
use super::bottom_pane_view::BottomPaneView;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use code_common::model_presets::ModelPreset;
use code_core::config_types::ReasoningEffort;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::prelude::Widget;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::cmp::Ordering;
use std::collections::HashMap;

use super::settings_panel::{render_panel, PanelFrameStyle};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ModelSelectionTarget {
    Session,
    Auto,
    Review,
}

#[derive(Clone, Debug)]
pub(crate) struct ModelSelectionEntry {
    pub target: ModelSelectionTarget,
    pub model: String,
    pub effort: ReasoningEffort,
    pub inherits_from_session: bool,
}

impl ModelSelectionEntry {
    pub fn new(
        target: ModelSelectionTarget,
        model: String,
        effort: ReasoningEffort,
        inherits_from_session: bool,
    ) -> Self {
        Self {
            target,
            model,
            effort,
            inherits_from_session,
        }
    }
}

#[derive(Clone, Debug)]
struct TargetContext {
    model: String,
    effort: ReasoningEffort,
    inherits_from_session: bool,
}

impl ModelSelectionTarget {
    fn panel_title(self) -> &'static str {
        match self {
            ModelSelectionTarget::Session => "Select Model & Reasoning",
            ModelSelectionTarget::Auto => "Select Auto Drive Model",
            ModelSelectionTarget::Review => "Select Review Model & Reasoning",
        }
    }

    fn current_label(self) -> &'static str {
        match self {
            ModelSelectionTarget::Session => "Current model",
            ModelSelectionTarget::Auto => "Auto Drive model",
            ModelSelectionTarget::Review => "Review model",
        }
    }

    fn reasoning_label(self) -> &'static str {
        match self {
            ModelSelectionTarget::Session => "Reasoning effort",
            ModelSelectionTarget::Auto => "Auto Drive reasoning",
            ModelSelectionTarget::Review => "Review reasoning",
        }
    }

    fn short_label(self) -> &'static str {
        match self {
            ModelSelectionTarget::Session => "Session",
            ModelSelectionTarget::Auto => "Auto Drive",
            ModelSelectionTarget::Review => "Review",
        }
    }
}

pub(crate) struct ModelSelectionView {
    presets: Vec<ModelPreset>,
    selected_index: usize,
    current_model: String,
    current_effort: ReasoningEffort,
    app_event_tx: AppEventSender,
    is_complete: bool,
    target: ModelSelectionTarget,
    available_targets: Vec<ModelSelectionTarget>,
    target_state: HashMap<ModelSelectionTarget, TargetContext>,
    auto_inherit_selected: bool,
}

impl ModelSelectionView {
    pub fn new(
        presets: Vec<ModelPreset>,
        entries: Vec<ModelSelectionEntry>,
        app_event_tx: AppEventSender,
    ) -> Self {
        assert!(!entries.is_empty(), "model selection requires at least one target");

        let mut target_state: HashMap<ModelSelectionTarget, TargetContext> = HashMap::new();
        let mut available_targets = Vec::with_capacity(entries.len());
        for entry in entries {
            available_targets.push(entry.target);
            target_state.insert(
                entry.target,
                TargetContext {
                    model: entry.model,
                    effort: entry.effort,
                    inherits_from_session: entry.inherits_from_session,
                },
            );
        }

        let initial_target = available_targets[0];
        let initial_context = target_state
            .get(&initial_target)
            .expect("model selection target context");
        let inherits_flag = initial_context.inherits_from_session;
        let initial_model = initial_context.model.clone();
        let initial_effort = initial_context.effort;
        let initial_index = Self::initial_selection(&presets, &initial_model, initial_effort);
        Self {
            presets,
            selected_index: initial_index,
            current_model: initial_model,
            current_effort: initial_effort,
            app_event_tx,
            is_complete: false,
            target: initial_target,
            available_targets,
            target_state,
            auto_inherit_selected: matches!(initial_target, ModelSelectionTarget::Auto)
                && inherits_flag,
        }
    }

    fn initial_selection(
        presets: &[ModelPreset],
        current_model: &str,
        current_effort: ReasoningEffort,
    ) -> usize {
        // Prefer an exact match on model + effort, fall back to first model match, then first entry.
        if let Some((idx, _)) = presets.iter().enumerate().find(|(_, preset)| {
            preset.model.eq_ignore_ascii_case(current_model)
                && Self::preset_effort(preset) == current_effort
        }) {
            return idx;
        }

        if let Some((idx, _)) = presets
            .iter()
            .enumerate()
            .find(|(_, preset)| preset.model.eq_ignore_ascii_case(current_model))
        {
            return idx;
        }

        0
    }

    fn apply_target(&mut self, target: ModelSelectionTarget) {
        if let Some(ctx) = self.target_state.get(&target) {
            self.target = target;
            self.current_model = ctx.model.clone();
            self.current_effort = ctx.effort;
            self.selected_index =
                Self::initial_selection(&self.presets, &self.current_model, self.current_effort);
            self.auto_inherit_selected = matches!(target, ModelSelectionTarget::Auto)
                && ctx.inherits_from_session;
        }
    }

    fn cycle_target(&mut self, forward: bool) {
        if self.available_targets.len() <= 1 {
            return;
        }
        let mut idx = self
            .available_targets
            .iter()
            .position(|candidate| candidate == &self.target)
            .unwrap_or(0);
        if forward {
            idx = (idx + 1) % self.available_targets.len();
        } else if idx == 0 {
            idx = self.available_targets.len() - 1;
        } else {
            idx -= 1;
        }
        let next_target = self.available_targets[idx];
        if next_target != self.target {
            self.apply_target(next_target);
        }
    }

    fn preset_effort(preset: &ModelPreset) -> ReasoningEffort {
        preset
            .effort
            .map(ReasoningEffort::from)
            .unwrap_or(ReasoningEffort::Medium)
    }

    fn format_model_header(model: &str) -> String {
        let mut parts = Vec::new();
        for (idx, part) in model.split('-').enumerate() {
            if idx == 0 {
                parts.push(part.to_ascii_uppercase());
                continue;
            }

            let mut chars = part.chars();
            let formatted = match chars.next() {
                Some(first) if first.is_ascii_alphabetic() => {
                    let mut s = String::new();
                    s.push(first.to_ascii_uppercase());
                    s.push_str(chars.as_str());
                    s
                }
                Some(first) => {
                    let mut s = String::new();
                    s.push(first);
                    s.push_str(chars.as_str());
                    s
                }
                None => String::new(),
            };
            parts.push(formatted);
        }

        parts.join("-")
    }

    fn move_selection_up(&mut self) {
        if self.presets.is_empty() {
            return;
        }
        let sorted = self.sorted_indices();
        if sorted.is_empty() {
            return;
        }

        if matches!(self.target, ModelSelectionTarget::Auto) {
            if self.auto_inherit_selected {
                self.auto_inherit_selected = false;
                self.selected_index = *sorted.last().unwrap_or(&0);
                return;
            }
            let current_pos = sorted
                .iter()
                .position(|&idx| idx == self.selected_index)
                .unwrap_or(0);
            if current_pos == 0 {
                self.auto_inherit_selected = true;
                return;
            }
            self.selected_index = sorted[current_pos - 1];
            return;
        }

        let current_pos = sorted
            .iter()
            .position(|&idx| idx == self.selected_index)
            .unwrap_or(0);
        let new_pos = if current_pos == 0 {
            sorted.len() - 1
        } else {
            current_pos - 1
        };
        self.selected_index = sorted[new_pos];
    }

    fn move_selection_down(&mut self) {
        if self.presets.is_empty() {
            return;
        }
        let sorted = self.sorted_indices();
        if sorted.is_empty() {
            return;
        }

        if matches!(self.target, ModelSelectionTarget::Auto) {
            if self.auto_inherit_selected {
                self.auto_inherit_selected = false;
                self.selected_index = sorted[0];
                return;
            }
            let current_pos = sorted
                .iter()
                .position(|&idx| idx == self.selected_index)
                .unwrap_or(0);
            if current_pos + 1 >= sorted.len() {
                self.auto_inherit_selected = true;
                return;
            }
            self.selected_index = sorted[current_pos + 1];
            return;
        }

        let current_pos = sorted
            .iter()
            .position(|&idx| idx == self.selected_index)
            .unwrap_or(0);
        let new_pos = (current_pos + 1) % sorted.len();
        self.selected_index = sorted[new_pos];
    }

    fn confirm_selection(&mut self) {
        if matches!(self.target, ModelSelectionTarget::Auto) && self.auto_inherit_selected {
            if let Some(session_ctx) = self.target_state.get(&ModelSelectionTarget::Session) {
                let _ = self.app_event_tx.send(AppEvent::UpdateAutoModelSelection {
                    model: session_ctx.model.clone(),
                });
            }
            self.is_complete = true;
            return;
        }
        if let Some(preset) = self.presets.get(self.selected_index) {
            let effort = Self::preset_effort(preset);
            match self.target {
                ModelSelectionTarget::Session => {
                    let _ = self.app_event_tx.send(AppEvent::UpdateModelSelection {
                        model: preset.model.to_string(),
                        effort: Some(effort),
                    });
                }
                ModelSelectionTarget::Auto => {
                    let _ = self
                        .app_event_tx
                        .send(AppEvent::UpdateAutoModelSelection { model: preset.model.to_string() });
                }
                ModelSelectionTarget::Review => {
                    let _ = self.app_event_tx.send(AppEvent::UpdateReviewModelSelection {
                        model: preset.model.to_string(),
                        effort,
                    });
                }
            }
        }
        self.is_complete = true;
    }

    fn content_line_count(&self) -> u16 {
        // Current model + reasoning effort + optional target/note rows.
        let mut lines: u16 = 2;
        if self.available_targets.len() > 1 {
            lines = lines.saturating_add(1);
        }
        if matches!(self.target, ModelSelectionTarget::Auto) {
            lines = lines.saturating_add(1);
        }
        if self.auto_override_differs() {
            lines = lines.saturating_add(1);
        }
        // Spacer before preset list.
        lines = lines.saturating_add(1);

        if matches!(self.target, ModelSelectionTarget::Auto) {
            lines = lines.saturating_add(1);
        }

        let mut previous_model: Option<&str> = None;
        for idx in self.sorted_indices() {
            let preset = &self.presets[idx];
            let is_new_model = previous_model
                .map(|prev| !prev.eq_ignore_ascii_case(&preset.model))
                .unwrap_or(true);

            if is_new_model {
                if previous_model.is_some() {
                    // Spacer between model groups.
                    lines = lines.saturating_add(1);
                }
                // Header when entering a new model group.
                lines = lines.saturating_add(1);
                if Self::model_description(preset.model).is_some() {
                    lines = lines.saturating_add(1);
                }
                previous_model = Some(preset.model);
            }

            // The preset entry row.
            lines = lines.saturating_add(1);
        }

        // Spacer before footer plus footer hint row.
        lines.saturating_add(2)
    }

    fn sorted_indices(&self) -> Vec<usize> {
        let mut indices: Vec<usize> = (0..self.presets.len()).collect();
        indices.sort_by(|&a, &b| Self::compare_presets(&self.presets[a], &self.presets[b]));
        indices
    }

    fn compare_presets(a: &ModelPreset, b: &ModelPreset) -> Ordering {
        let model_rank = Self::model_rank(a.model).cmp(&Self::model_rank(b.model));
        if model_rank != Ordering::Equal {
            return model_rank;
        }

        let model_name_rank = a
            .model
            .to_ascii_lowercase()
            .cmp(&b.model.to_ascii_lowercase());
        if model_name_rank != Ordering::Equal {
            return model_name_rank;
        }

        let effort_rank = Self::effort_rank(Self::preset_effort(a))
            .cmp(&Self::effort_rank(Self::preset_effort(b)));
        if effort_rank != Ordering::Equal {
            return effort_rank;
        }

        a.label.cmp(b.label)
    }

    fn model_rank(model: &str) -> u8 {
        if model.eq_ignore_ascii_case("gpt-5.1-codex") {
            0
        } else if model.eq_ignore_ascii_case("gpt-5.1-codex-mini") {
            1
        } else if model.eq_ignore_ascii_case("gpt-5.1") {
            2
        } else {
            3
        }
    }

    fn model_description(model: &str) -> Option<&'static str> {
        if model.eq_ignore_ascii_case("gpt-5.1-codex") {
            Some("Optimized for coding.")
        } else if model.eq_ignore_ascii_case("gpt-5.1-codex-mini") {
            Some("Optimized for coding. Cheaper, faster, but less capable.")
        } else if model.eq_ignore_ascii_case("gpt-5.1") {
            Some("Broad world knowledge with strong general reasoning.")
        } else {
            None
        }
    }

    fn effort_rank(effort: ReasoningEffort) -> u8 {
        match effort {
            ReasoningEffort::High => 0,
            ReasoningEffort::Medium => 1,
            ReasoningEffort::Low => 2,
            ReasoningEffort::Minimal => 3,
            ReasoningEffort::None => 4,
        }
    }

    fn effort_label(effort: ReasoningEffort) -> &'static str {
        match effort {
            ReasoningEffort::High => "High",
            ReasoningEffort::Medium => "Medium",
            ReasoningEffort::Low => "Low",
            ReasoningEffort::Minimal => "Minimal",
            ReasoningEffort::None => "None",
        }
    }

    fn effort_description(effort: ReasoningEffort) -> &'static str {
        match effort {
            ReasoningEffort::Minimal => {
                "Minimal reasoning. When speed is more important than accuracy. (fastest)"
            }
            ReasoningEffort::Low => "Basic reasoning. Works quickly in simple code bases. (fast)",
            ReasoningEffort::Medium => "Balanced reasoning. Ideal for most tasks. (default)",
            ReasoningEffort::High => {
                "Deep reasoning. Useful when solving difficult problems. (slower)"
            }
            ReasoningEffort::None => "Reasoning disabled",
        }
    }
}

impl ModelSelectionView {
    pub(crate) fn handle_key_event_direct(&mut self, key_event: KeyEvent) -> bool {
        match key_event {
            KeyEvent {
                code: KeyCode::Up,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.move_selection_up();
                true
            }
            KeyEvent {
                code: KeyCode::Down,
                modifiers: KeyModifiers::NONE,
                ..
            }
            => {
                self.move_selection_down();
                true
            }
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.confirm_selection();
                true
            }
            KeyEvent {
                code: KeyCode::Esc,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.is_complete = true;
                true
            }
            KeyEvent {
                code: KeyCode::Tab,
                modifiers,
                ..
            } => {
                let forward = !modifiers.contains(KeyModifiers::SHIFT);
                self.cycle_target(forward);
                true
            }
            _ => false,
        }
    }

    fn render_panel_body(&self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let mut lines: Vec<Line> = Vec::new();
        if self.available_targets.len() > 1 {
            let mut spans = vec![
                Span::styled(
                    "Target: ",
                    Style::default().fg(crate::colors::text_dim()),
                ),
                Span::styled(
                    self.target.short_label(),
                    Style::default()
                        .fg(crate::colors::primary())
                        .add_modifier(Modifier::BOLD),
                ),
            ];
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                "Tab",
                Style::default().fg(crate::colors::primary()),
            ));
            spans.push(Span::styled(
                " switch target",
                Style::default().fg(crate::colors::text_dim()),
            ));
            lines.push(Line::from(spans));
        }

        lines.push(Line::from(vec![
            Span::styled(
                format!("{}: ", self.target.current_label()),
                Style::default().fg(crate::colors::text_dim()),
            ),
            Span::styled(
                self.current_model.clone(),
                Style::default()
                    .fg(crate::colors::warning())
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        if self.auto_override_differs() {
            if let Some(auto_ctx) = self.target_state.get(&ModelSelectionTarget::Auto) {
                lines.push(Line::from(vec![Span::styled(
                    format!("Auto Drive: {}", auto_ctx.model),
                    Style::default()
                        .fg(crate::colors::text_dim())
                        .add_modifier(Modifier::ITALIC),
                )]));
            }
        }
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}: ", self.target.reasoning_label()),
                Style::default().fg(crate::colors::text_dim()),
            ),
            Span::styled(
                format!("{}", self.current_effort),
                Style::default()
                    .fg(crate::colors::warning())
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        if matches!(self.target, ModelSelectionTarget::Auto) {
            let inherits_session = self
                .target_state
                .get(&self.target)
                .map(|ctx| ctx.inherits_from_session)
                .unwrap_or(false);
            let note = if inherits_session {
                "Auto Drive inherits the session model until you pick an override below."
            } else {
                "Select the session model again to make Auto Drive follow it automatically."
            };
            lines.push(Line::from(vec![Span::styled(
                note,
                Style::default().fg(crate::colors::text_dim()),
            )]));
        }

        lines.push(Line::from(""));

        if matches!(self.target, ModelSelectionTarget::Auto) {
            lines.push(self.render_auto_inherit_row());
        }

        let mut previous_model: Option<&str> = None;
        let sorted_indices = self.sorted_indices();

        for preset_index in sorted_indices {
            let preset = &self.presets[preset_index];
            if previous_model
                .map(|m| !m.eq_ignore_ascii_case(&preset.model))
                .unwrap_or(true)
            {
                if previous_model.is_some() {
                    lines.push(Line::from(""));
                }
                lines.push(Line::from(vec![Span::styled(
                    Self::format_model_header(&preset.model),
                    Style::default()
                        .fg(crate::colors::text_bright())
                        .add_modifier(Modifier::BOLD),
                )]));
                if let Some(desc) = Self::model_description(&preset.model) {
                    lines.push(Line::from(vec![Span::styled(
                        desc,
                        Style::default().fg(crate::colors::text_dim()),
                    )]));
                }
                previous_model = Some(preset.model);
            }

            let is_selected = preset_index == self.selected_index;
            let preset_effort = Self::preset_effort(preset);
            let is_current = preset.model.eq_ignore_ascii_case(&self.current_model)
                && preset_effort == self.current_effort;
            let label = Self::effort_label(preset_effort);
            let mut row_text = label.to_string();
            if is_current {
                row_text.push_str(" (current)");
            }

            let mut indent_style = Style::default();
            if is_selected {
                indent_style = indent_style
                    .bg(crate::colors::selection())
                    .add_modifier(Modifier::BOLD);
            }

            let mut label_style = Style::default().fg(crate::colors::text());
            if is_selected {
                label_style = label_style
                    .bg(crate::colors::selection())
                    .add_modifier(Modifier::BOLD);
            }
            if is_current {
                label_style = label_style.fg(crate::colors::success());
            }

            let mut divider_style = Style::default().fg(crate::colors::text_dim());
            if is_selected {
                divider_style = divider_style
                    .bg(crate::colors::selection())
                    .add_modifier(Modifier::BOLD);
            }

            let mut description_style = Style::default().fg(crate::colors::dim());
            if is_selected {
                description_style = description_style
                    .bg(crate::colors::selection())
                    .add_modifier(Modifier::BOLD);
            }

            let description = Self::effort_description(preset_effort);

            lines.push(Line::from(vec![
                Span::styled("   ", indent_style),
                Span::styled(row_text, label_style),
                Span::styled(" - ", divider_style),
                Span::styled(description, description_style),
            ]));
        }

        lines.push(Line::from(""));
        let mut footer = vec![
            Span::styled("↑↓", Style::default().fg(crate::colors::light_blue())),
            Span::raw(" Navigate  "),
            Span::styled("Enter", Style::default().fg(crate::colors::success())),
            Span::raw(" Select  "),
            Span::styled("Esc", Style::default().fg(crate::colors::error())),
            Span::raw(" Cancel"),
        ];
        if self.available_targets.len() > 1 {
            footer.push(Span::raw("  "));
            footer.push(Span::styled(
                "Tab",
                Style::default().fg(crate::colors::primary()),
            ));
            footer.push(Span::raw(" Switch  "));
            footer.push(Span::styled(
                "Shift+Tab",
                Style::default().fg(crate::colors::primary()),
            ));
            footer.push(Span::raw(" Back"));
        }
        lines.push(Line::from(footer));

        let padded = Rect {
            x: area.x.saturating_add(1),
            y: area.y,
            width: area.width.saturating_sub(1),
            height: area.height,
        };

        Paragraph::new(lines)
            .alignment(Alignment::Left)
            .style(
                Style::default()
                    .bg(crate::colors::background())
                    .fg(crate::colors::text()),
            )
            .render(padded, buf);
    }

    pub(crate) fn render_without_frame(&self, area: Rect, buf: &mut Buffer) {
        self.render_panel_body(area, buf);
    }

}

impl<'a> BottomPaneView<'a> for ModelSelectionView {
    fn handle_key_event(&mut self, _pane: &mut BottomPane<'a>, key_event: KeyEvent) {
        let _ = self.handle_key_event_direct(key_event);
    }

    fn is_complete(&self) -> bool {
        self.is_complete
    }

    fn desired_height(&self, _width: u16) -> u16 {
        // Account for content rows plus bordered block padding.
        let content_lines = self.content_line_count();
        let total = content_lines.saturating_add(2);
        total.max(9)
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let mut title = self.target.panel_title().to_string();
        if self.available_targets.len() > 1 {
            title.push_str(" — ");
            title.push_str(self.target.short_label());
        }
        render_panel(
            area,
            buf,
            &title,
            PanelFrameStyle::bottom_pane(),
            |inner, buf| self.render_panel_body(inner, buf),
        );
    }
}

impl ModelSelectionView {
    fn auto_override_differs(&self) -> bool {
        let auto_ctx = match self.target_state.get(&ModelSelectionTarget::Auto) {
            Some(ctx) => ctx,
            None => return false,
        };
        let session_ctx = match self.target_state.get(&ModelSelectionTarget::Session) {
            Some(ctx) => ctx,
            None => return false,
        };
        !auto_ctx
            .model
            .eq_ignore_ascii_case(&session_ctx.model)
    }

    fn render_auto_inherit_row(&self) -> Line<'static> {
        let mut label_style = Style::default().fg(crate::colors::text());
        let mut description_style = Style::default().fg(crate::colors::dim());
        if self.auto_inherit_selected {
            let highlight = Style::default()
                .bg(crate::colors::selection())
                .add_modifier(Modifier::BOLD);
            label_style = label_style.patch(highlight);
            description_style = description_style.patch(highlight);
        }
        Line::from(vec![
            Span::styled("   ", label_style),
            Span::styled("Inherit session model", label_style),
            Span::styled(" - ", description_style),
            Span::styled(
                "Auto Drive will follow the session model",
                description_style,
            ),
        ])
    }
}
