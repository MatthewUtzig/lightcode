use super::*;
use crate::glitch_animation::{self, IntroColorMode, SPARKSI_LIME_GREEN, SPARKSI_LIGHT_BLUE};
use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

const INTRO_ANIMATION_DURATION: Duration = Duration::from_secs(2);
const INTRO_FADE_DURATION: Duration = Duration::from_millis(800);
const INTRO_PUSH_DURATION: Duration = Duration::from_millis(900);
const AVENUE_WORD: &str = "AVENUE";
const SPARKSI_WORD: &str = "SPARKSI";

pub(crate) struct AnimatedWelcomeCell {
    start_time: Instant,
    completed: Cell<bool>,
    fade_start: RefCell<Option<Instant>>,
    faded_out: Cell<bool>,
    locked_height: Cell<Option<u16>>,
    hidden: Cell<bool>,
    push_start: RefCell<Option<Instant>>,
    push_completed: Cell<bool>,
}

impl AnimatedWelcomeCell {
    pub(crate) fn new() -> Self {
        Self {
            start_time: Instant::now(),
            completed: Cell::new(false),
            fade_start: RefCell::new(None),
            faded_out: Cell::new(false),
            locked_height: Cell::new(None),
            hidden: Cell::new(false),
            push_start: RefCell::new(None),
            push_completed: Cell::new(false),
        }
    }

    fn fade_start(&self) -> Option<Instant> {
        *self.fade_start.borrow()
    }

    fn set_fade_start(&self) {
        let mut slot = self.fade_start.borrow_mut();
        if slot.is_none() {
            *slot = Some(Instant::now());
        }
    }

    pub(crate) fn begin_fade(&self) {
        self.set_fade_start();
    }

    pub(crate) fn should_remove(&self) -> bool {
        self.faded_out.get()
    }
}

impl HistoryCell for AnimatedWelcomeCell {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn kind(&self) -> HistoryCellType {
        HistoryCellType::AnimatedWelcome
    }

    fn display_lines(&self) -> Vec<Line<'static>> {
        vec![
            Line::from(""),
            Line::from("Welcome to Code"),
            Line::from(crate::greeting::greeting_placeholder()),
            Line::from(""),
        ]
    }

    fn desired_height(&self, width: u16) -> u16 {
        if let Some(h) = self.locked_height.get() {
            return h.saturating_add(3);
        }

        let cols: u16 = 23;
        let base_rows: u16 = 7;
        let max_scale: u16 = 3;
        let scale = if width >= cols {
            (width / cols).min(max_scale).max(1)
        } else {
            1
        };
        let h = base_rows.saturating_mul(scale);
        self.locked_height.set(Some(h));
        h.saturating_add(3)
    }

    fn has_custom_render(&self) -> bool {
        true
    }

    fn custom_render(&self, area: Rect, buf: &mut Buffer) {
        if self.hidden.get() {
            return;
        }

        let locked_h = self.locked_height.get().unwrap_or(21);
        let height = locked_h.min(area.height);
        let positioned_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height,
        };

        // Phase 1: AVENUE animation / fade
        if let Some(fade_time) = self.fade_start() {
            let fade_elapsed = fade_time.elapsed();
            let fade_progress = fade_elapsed.as_secs_f32() / INTRO_FADE_DURATION.as_secs_f32();
            let alpha = (1.0 - fade_progress).clamp(0.0, 1.0);
            if fade_elapsed < INTRO_FADE_DURATION && !self.push_completed.get() {
                glitch_animation::render_intro_word_with_options(
                    positioned_area,
                    buf,
                    1.0,
                    Some(alpha),
                    AVENUE_WORD,
                    IntroColorMode::Rainbow,
                    0,
                    true,
                );
            }
            // Kick off push phase near the end of fade if not already started
            if fade_elapsed >= INTRO_FADE_DURATION.saturating_sub(Duration::from_millis(400))
                && self.push_start.borrow().is_none()
            {
                *self.push_start.borrow_mut() = Some(Instant::now());
            }
        } else {
            let elapsed = self.start_time.elapsed();
            if elapsed < INTRO_ANIMATION_DURATION && !self.completed.get() {
                let progress = elapsed.as_secs_f32() / INTRO_ANIMATION_DURATION.as_secs_f32();
                glitch_animation::render_intro_word_with_options(
                    positioned_area,
                    buf,
                    progress,
                    None,
                    AVENUE_WORD,
                    IntroColorMode::Rainbow,
                    0,
                    true,
                );
            } else {
                self.completed.set(true);
                self.set_fade_start();
                glitch_animation::render_intro_word_with_options(
                    positioned_area,
                    buf,
                    1.0,
                    None,
                    AVENUE_WORD,
                    IntroColorMode::Rainbow,
                    0,
                    true,
                );
            }
        }

        // Phase 2: SPARKSI pushes AVENUE off-screen once fade is underway/completed
        if let Some(push_time) = self.push_start.borrow().as_ref() {
            let push_elapsed = push_time.elapsed();
            let push_t = (push_elapsed.as_secs_f32()
                / INTRO_PUSH_DURATION.as_secs_f32())
                .clamp(0.0, 1.0);

            // Compute horizontal offsets: SPARKSI moves from left offscreen to center; AVENUE moves right offscreen
            let width = positioned_area.width as i32;
            let avenue_offset = (push_t * width as f32) as i32; // shove right
            let sparksi_offset = -width + (push_t * width as f32) as i32; // enter from left

            glitch_animation::render_intro_word_with_options(
                positioned_area,
                buf,
                1.0,
                Some(0.0),
                AVENUE_WORD,
                IntroColorMode::Rainbow,
                avenue_offset,
                false,
            );

            glitch_animation::render_intro_word_with_options(
                positioned_area,
                buf,
                1.0,
                Some(1.0),
                SPARKSI_WORD,
                IntroColorMode::Gradient {
                    start: SPARKSI_LIGHT_BLUE,
                    end: SPARKSI_LIME_GREEN,
                },
                sparksi_offset,
                false,
            );

            if push_elapsed >= INTRO_PUSH_DURATION {
                self.push_completed.set(true);
                self.faded_out.set(true);
            }
        }
    }

    fn is_animating(&self) -> bool {
        if !self.completed.get() {
            if self.start_time.elapsed() < INTRO_ANIMATION_DURATION {
                return true;
            }
            self.completed.set(true);
        }

        if let Some(fade_time) = self.fade_start() {
            if !self.faded_out.get() || !self.push_completed.get() {
                if fade_time.elapsed() < INTRO_FADE_DURATION {
                    return true;
                }
            }
        }

        if let Some(push_time) = self.push_start.borrow().as_ref() {
            if push_time.elapsed() < INTRO_PUSH_DURATION {
                return true;
            }
        }

        false
    }

    fn trigger_fade(&self) {
        AnimatedWelcomeCell::begin_fade(self);
    }

    fn should_remove(&self) -> bool {
        AnimatedWelcomeCell::should_remove(self)
    }
}
