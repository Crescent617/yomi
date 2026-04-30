//! Banner component for chat header
//!
//! Shows mascot and system info with blinking animation.

use tuirealm::ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::{theme::colors, utils::text::truncate_by_width};

/// Yomi version constant
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Mascot ASCII art frames
const MASCOT_FRAMES: &[(&str, u8)] = &[
    // (file content, duration in ticks at 10Hz = 100ms per tick)
    (include_str!("assets/mascot_normal.txt"), 30), // 3s normal
    (include_str!("assets/mascot_eye_closed.txt"), 2), // 200ms blink
    (include_str!("assets/mascot_eye_opened.txt"), 30), // 3s opened
    (include_str!("assets/mascot_eye_closed.txt"), 2), // 200ms blink
];

/// Simple mascot animator - cycles through frames with different durations
#[derive(Debug, Clone)]
pub struct MascotAnimator {
    frame_index: usize,
    ticks_remaining: u8,
}

impl Default for MascotAnimator {
    fn default() -> Self {
        Self {
            frame_index: 0,
            ticks_remaining: MASCOT_FRAMES[0].1,
        }
    }
}

impl MascotAnimator {
    /// Called on each tick (10Hz), returns true if frame changed
    pub fn tick(&mut self) -> bool {
        if self.ticks_remaining > 0 {
            self.ticks_remaining -= 1;
            false
        } else {
            // Move to next frame
            self.frame_index = (self.frame_index + 1) % MASCOT_FRAMES.len();
            self.ticks_remaining = MASCOT_FRAMES[self.frame_index].1;
            true
        }
    }

    /// Get current mascot ASCII art lines as a Vec for indexed access
    pub fn current_lines(&self) -> Vec<&str> {
        MASCOT_FRAMES[self.frame_index].0.lines().collect()
    }
}

/// Banner data for rendering (used by `ChatView`)
/// Holds `working_dir`, other info comes from global config
#[derive(Debug, Clone, Default)]
pub struct BannerData {
    pub working_dir: String,
}

impl BannerData {
    pub fn new(working_dir: String) -> Self {
        Self { working_dir }
    }

    /// Returns styled lines: title, model/permissions, cwd, skills
    pub fn info_lines(&self) -> Vec<Line<'_>> {
        let config = crate::config();

        let working_dir = if self.working_dir.is_empty() {
            "~"
        } else {
            &self.working_dir
        };

        // Truncate model name if too long
        let model_name = &config.model.model_id;
        let model_str = if model_name.len() > 40 {
            truncate_by_width(model_name, 40, "...")
        } else if model_name.is_empty() {
            "-".to_string()
        } else {
            model_name.clone()
        };

        let auto_approve = config.auto_approve.to_string();

        // Title line: Yomi (primary, bold) + version (secondary, non-bold)
        let title_line = Line::from(vec![
            Span::styled(
                "Yomi ",
                Style::default()
                    .fg(colors::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("v{VERSION}"),
                Style::default().fg(colors::text_secondary()),
            ),
        ]);

        // Info lines (secondary color)
        vec![
            title_line,
            Line::from(Span::styled(
                format!("{model_str} · auto-approve {auto_approve}"),
                colors::text_secondary(),
            )),
            Line::from(Span::styled(
                format!(" {working_dir}"),
                colors::text_secondary(),
            )),
        ]
    }
}
