//! Banner component for chat header
//!
//! Shows mascot and system info with blinking animation.

use tuirealm::{
    command::{Cmd, CmdResult},
    component::{AppComponent, Component},
    event::Event,
    props::{AttrValue, Attribute, Props, QueryResult},
    ratatui::{
        layout::{Constraint, Direction, Layout, Rect},
        text::{Line, Span},
        Frame,
    },
    state::State,
};

use crate::{attr, msg::Msg, theme::colors, utils::text::truncate_by_width};

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

    /// Group skills by prefix (e.g., "superpowers:a", "superpowers:b" -> "superpowers:{a, b}")
    fn group_skills(skills: &[String]) -> Vec<String> {
        const MAX_PER_GROUP: usize = 3;
        use std::collections::HashMap;

        let mut groups: HashMap<String, Vec<String>> = HashMap::new();

        for skill in skills {
            if let Some(colon_pos) = skill.find(':') {
                let prefix = skill[..colon_pos].to_string();
                let suffix = skill[colon_pos + 1..].to_string();
                groups.entry(prefix).or_default().push(suffix);
            } else {
                // No colon, treat as standalone skill
                groups.entry(skill.clone()).or_default();
            }
        }

        let mut result: Vec<String> = Vec::new();

        for (prefix, suffixes) in groups {
            if suffixes.is_empty() {
                result.push(prefix);
            } else if suffixes.len() == 1 {
                result.push(format!("{}:{}", prefix, suffixes[0]));
            } else {
                // Sort suffixes for consistent display
                let mut sorted_suffixes = suffixes;
                sorted_suffixes.sort();

                // Limit to MAX_PER_GROUP
                let total = sorted_suffixes.len();
                let display: Vec<_> = sorted_suffixes.into_iter().take(MAX_PER_GROUP).collect();

                if total > MAX_PER_GROUP {
                    result.push(format!(
                        "{prefix}:{{{}}} (+{})",
                        display.join(", "),
                        total - MAX_PER_GROUP
                    ));
                } else {
                    result.push(format!("{prefix}:{{{}}}", display.join(", ")));
                }
            }
        }

        // Sort for consistent display
        result.sort();
        result
    }

    /// Get info lines for right panel
    pub fn info_lines(&self) -> Vec<String> {
        const MAX_DISPLAY_LEN: usize = 200;

        let working_dir = if self.working_dir.is_empty() {
            "~".to_string()
        } else {
            self.working_dir.clone()
        };

        let skills_str = if self.skills.is_empty() {
            "None".to_string()
        } else {
            // Group skills by prefix
            let grouped = Self::group_skills(&self.skills);

            let mut result = grouped.join(", ");

            if result.len() > MAX_DISPLAY_LEN {
                result = truncate_by_width(&result, MAX_DISPLAY_LEN, "...");
            }

            result
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

impl Component for BannerComponent {
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

    fn query(&self, attr: Attribute) -> Option<QueryResult<'_>> {
        self.props.get(attr).map(|v| v.into())
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        match attr {
            Attribute::Custom(attr::WORKING_DIR) => {
                if let AttrValue::String(dir) = value {
                    self.working_dir = dir;
                }
            }
            Attribute::Custom(attr::SKILLS) => {
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
        CmdResult::NoChange
    }
}

impl AppComponent<Msg, crate::msg::UserEvent> for BannerComponent {
    fn on(&mut self, ev: &Event<crate::msg::UserEvent>) -> Option<Msg> {
        // Handle tick events for blinking animation
        if *ev == Event::Tick && self.tick() {
            // Animation state changed, trigger redraw
            return Some(Msg::Redraw);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_group_skills_empty() {
        let skills: Vec<String> = vec![];
        let grouped = BannerData::group_skills(&skills);
        assert!(grouped.is_empty());
    }

    #[test]
    fn test_group_skills_no_prefix() {
        let skills = vec!["nopua".to_string(), "debug".to_string()];
        let grouped = BannerData::group_skills(&skills);
        assert_eq!(grouped, vec!["debug", "nopua"]);
    }

    #[test]
    fn test_group_skills_single_prefix() {
        let skills = vec!["superpowers:a".to_string(), "superpowers:b".to_string()];
        let grouped = BannerData::group_skills(&skills);
        assert_eq!(grouped, vec!["superpowers:{a, b}"]);
    }

    #[test]
    fn test_group_skills_multiple_prefixes() {
        let skills = vec![
            "superpowers:a".to_string(),
            "superpowers:b".to_string(),
            "caveman:caveman".to_string(),
            "nopua".to_string(),
        ];
        let grouped = BannerData::group_skills(&skills);
        assert_eq!(
            grouped,
            vec!["caveman:caveman", "nopua", "superpowers:{a, b}"]
        );
    }

    #[test]
    fn test_group_skills_single_item_per_prefix() {
        let skills = vec!["superpowers:a".to_string(), "caveman:caveman".to_string()];
        let grouped = BannerData::group_skills(&skills);
        assert_eq!(grouped, vec!["caveman:caveman", "superpowers:a"]);
    }

    #[test]
    fn test_group_skills_sorted() {
        let skills = vec![
            "superpowers:z".to_string(),
            "superpowers:a".to_string(),
            "superpowers:m".to_string(),
        ];
        let grouped = BannerData::group_skills(&skills);
        assert_eq!(grouped, vec!["superpowers:{a, m, z}"]);
    }

    #[test]
    fn test_info_lines_with_grouped_skills() {
        let banner = BannerData::new(
            "/home/user".to_string(),
            vec![
                "superpowers:a".to_string(),
                "superpowers:b".to_string(),
                "caveman:caveman".to_string(),
                "nopua".to_string(),
            ],
        );
        let lines = banner.info_lines();
        assert_eq!(lines[0], "Hello!");
        assert_eq!(lines[1], "CWD: /home/user");
        assert!(lines[2].contains("caveman:caveman"));
        assert!(lines[2].contains("nopua"));
        assert!(lines[2].contains("superpowers:{a, b}"));
    }

    #[test]
    fn test_group_skills_max_3_per_group() {
        let skills = vec![
            "superpowers:a".to_string(),
            "superpowers:b".to_string(),
            "superpowers:c".to_string(),
            "superpowers:d".to_string(),
            "superpowers:e".to_string(),
        ];
        let grouped = BannerData::group_skills(&skills);
        assert_eq!(grouped.len(), 1);
        // Should show first 3 + "(+2)"
        assert!(grouped[0].contains("superpowers:{a, b, c}"));
        assert!(grouped[0].contains("(+2)"));
    }

    #[test]
    fn test_group_skills_exactly_3() {
        let skills = vec![
            "superpowers:a".to_string(),
            "superpowers:b".to_string(),
            "superpowers:c".to_string(),
        ];
        let grouped = BannerData::group_skills(&skills);
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[0], "superpowers:{a, b, c}");
        // Should not have "+"
        assert!(!grouped[0].contains('+'));
    }
}
