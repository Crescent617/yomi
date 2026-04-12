//! Banner component for chat header
//!
//! Shows mascot and system info with blinking animation.

use tuirealm::{
    command::{Cmd, CmdResult},
    props::{AttrValue, Attribute, Props},
    ratatui::{
        layout::{Constraint, Direction, Layout, Rect},
        text::{Line, Span},
    },
    Component, Frame, MockComponent, State,
};

use crate::{msg::Msg, theme::colors};

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
#[derive(Debug, Clone, Default)]
pub struct BannerData {
    pub working_dir: String,
    pub skills: Vec<String>,
}

impl BannerData {
    pub const fn new(working_dir: String, skills: Vec<String>) -> Self {
        Self {
            working_dir,
            skills,
        }
    }

    /// Get info lines for right panel
    pub fn info_lines(&self) -> Vec<String> {
        let working_dir = if self.working_dir.is_empty() {
            "~".to_string()
        } else {
            self.working_dir.clone()
        };
        let skills_str = if self.skills.is_empty() {
            "None".to_string()
        } else {
            // Limit to max 20 skills
            const MAX_SKILLS: usize = 20;
            let display_count = self.skills.len().min(MAX_SKILLS);
            let result = self.skills[..display_count].join(", ");
            if self.skills.len() > MAX_SKILLS {
                format!("{result}, +{} more", self.skills.len() - MAX_SKILLS)
            } else {
                result
            }
        };

        vec![
            "Hello!".to_string(),
            format!("CWD: {working_dir}"),
            format!("Skills: {skills_str}"),
        ]
    }
}

/// Banner component showing mascot and system info
#[derive(Debug, Default)]
pub struct BannerComponent {
    props: Props,
    working_dir: String,
    skills: Vec<String>,
    mascot_animator: MascotAnimator,
}

impl BannerComponent {
    pub fn new() -> Self {
        Self {
            props: Props::default(),
            working_dir: String::new(),
            skills: Vec::new(),
            mascot_animator: MascotAnimator::default(),
        }
    }

    /// Process tick for animation, returns true if redraw needed
    pub fn tick(&mut self) -> bool {
        self.mascot_animator.tick()
    }
}

impl MockComponent for BannerComponent {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let banner_data = BannerData {
            working_dir: self.working_dir.clone(),
            skills: self.skills.clone(),
        };

        // Split into two columns: mascot (left) and info (right)
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(8), Constraint::Min(20)])
            .split(area);

        let mascot_area = columns[0];
        let info_area = columns[1];

        // Render mascot (left column, centered vertically)
        let mascot_lines: Vec<Line> = self
            .mascot_animator
            .current_lines()
            .into_iter()
            .map(|line| Line::from(Span::styled(line.to_string(), colors::accent_system())))
            .collect();

        let mascot_paragraph = tuirealm::ratatui::widgets::Paragraph::new(mascot_lines)
            .alignment(tuirealm::ratatui::layout::Alignment::Center);
        frame.render_widget(mascot_paragraph, mascot_area);

        // Render info (right column)
        let info_lines: Vec<Line> = banner_data
            .info_lines()
            .into_iter()
            .map(|text| Line::from(Span::styled(text, colors::text_secondary())))
            .collect();

        let info_paragraph = tuirealm::ratatui::widgets::Paragraph::new(info_lines)
            .alignment(tuirealm::ratatui::layout::Alignment::Left);
        frame.render_widget(info_paragraph, info_area);
    }

    fn query(&self, attr: Attribute) -> Option<AttrValue> {
        self.props.get(attr)
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        match attr {
            Attribute::Custom("working_dir") => {
                if let AttrValue::String(dir) = value {
                    self.working_dir = dir;
                }
            }
            Attribute::Custom("skills") => {
                if let AttrValue::String(skills) = value {
                    self.skills = skills.split(',').map(|s| s.trim().to_string()).collect();
                }
            }
            _ => {
                self.props.set(attr, value);
            }
        }
    }

    fn state(&self) -> State {
        State::None
    }

    fn perform(&mut self, _cmd: Cmd) -> CmdResult {
        CmdResult::None
    }
}

impl Component<Msg, crate::msg::UserEvent> for BannerComponent {
    fn on(&mut self, ev: tuirealm::Event<crate::msg::UserEvent>) -> Option<Msg> {
        // Handle tick events for blinking animation
        if ev == tuirealm::Event::Tick && self.tick() {
            // Animation state changed, trigger redraw
            return Some(Msg::Redraw);
        }
        None
    }
}
