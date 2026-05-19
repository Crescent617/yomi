//! Banner component for empty chat state
//!
//! Shows mascot and system info centered in the terminal.
//! The mascot blinks by default, but animation can be disabled via the
//! `{ENV_PREFIX}DISABLE_ANIMATION` environment variable.

use kernel::ENV_PREFIX;
use std::ops::{Deref, DerefMut};
use tuirealm::{
    command::{Cmd, CmdResult},
    component::{AppComponent, Component},
    event::Event,
    props::{AttrValue, Attribute, Props, QueryResult},
    ratatui::{
        layout::{Alignment, Rect},
        style::{Modifier, Style},
        text::{Line, Span},
        widgets::Paragraph,
        Frame,
    },
    state::State,
};

use crate::{attr, msg::Msg, theme::colors, utils::text::truncate_by_width};

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

/// Returns a random frame index using the system's RNG.
fn random_frame_index() -> usize {
    rand::random::<u32>() as usize % MASCOT_FRAMES.len()
}

/// Check if animation is disabled via `{ENV_PREFIX}DISABLE_ANIMATION` env var.
fn is_animation_disabled() -> bool {
    std::env::var(format!("{ENV_PREFIX}DISABLE_ANIMATION")).is_ok()
}

/// Mascot animator with optional blinking.
///
/// By default, the mascot cycles through frames (blink animation).
/// When `{ENV_PREFIX}DISABLE_ANIMATION` is set, a random frame is chosen on startup
/// and held statically.
#[derive(Debug, Clone)]
pub struct MascotAnimator {
    frame_index: usize,
    ticks_remaining: u8,
    animation_disabled: bool,
}

impl Default for MascotAnimator {
    fn default() -> Self {
        if is_animation_disabled() {
            Self {
                frame_index: random_frame_index(),
                ticks_remaining: 0,
                animation_disabled: true,
            }
        } else {
            Self {
                frame_index: 0,
                ticks_remaining: MASCOT_FRAMES[0].1,
                animation_disabled: false,
            }
        }
    }
}

impl MascotAnimator {
    /// Called on each tick (10Hz), returns true if frame changed.
    pub fn tick(&mut self) -> bool {
        if self.animation_disabled {
            return false;
        }

        if self.ticks_remaining > 0 {
            self.ticks_remaining -= 1;
            false
        } else {
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
    pub tip: String,
}

impl BannerData {
    pub fn new(working_dir: String) -> Self {
        Self {
            working_dir,
            tip: String::new(),
        }
    }

    /// Returns styled lines: title, model/permissions, cwd, tip
    pub fn info_lines(&self) -> Vec<Line<'_>> {
        let config = crate::config();

        let working_dir = if self.working_dir.is_empty() {
            "~"
        } else {
            &self.working_dir
        };

        // Truncate model name if too long
        let model_name = &config.agent.model.model_id;
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

        let mut lines = vec![
            title_line,
            Line::from(Span::styled(
                format!("{model_str} · auto-approve {auto_approve}"),
                colors::text_secondary(),
            )),
            Line::from(Span::styled(
                format!(" {working_dir}"),
                colors::text_secondary(),
            )),
        ];

        // Tip at the bottom
        if !self.tip.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                self.tip.clone(),
                Style::default()
                    .fg(colors::text_secondary())
                    .add_modifier(Modifier::ITALIC),
            )));
        }

        lines
    }
}

/// Banner component: renders mascot + system info centered in the area.
/// Shown when `ChatView` has no messages.
#[derive(Debug, Default)]
pub struct Banner {
    props: Props,
    data: BannerData,
    mascot: MascotAnimator,
}

impl Banner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set working directory for display.
    pub fn set_working_dir(&mut self, dir: impl Into<String>) {
        self.data.working_dir = dir.into();
    }

    /// Set tip text for display.
    pub fn set_tip(&mut self, tip: impl Into<String>) {
        self.data.tip = tip.into();
    }
}

impl Component for Banner {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let mascot_lines = self.mascot.current_lines();
        let info_lines = self.data.info_lines();

        let total_lines = mascot_lines.len() + 1 + info_lines.len();
        let start_y = area
            .y
            .saturating_add((area.height.saturating_sub(total_lines as u16)) / 2);

        let mut y = start_y;

        // Render mascot (centered, using terminal system/default color)
        for line in mascot_lines {
            if y >= area.y + area.height {
                break;
            }
            let para = Paragraph::new(line)
                .alignment(Alignment::Center)
                .style(colors::accent_system());
            frame.render_widget(
                para,
                Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: 1,
                },
            );
            y += 1;
        }

        // Spacer
        y += 1;

        // Render info lines (centered)
        for line in info_lines {
            if y >= area.y + area.height {
                break;
            }
            let para = Paragraph::new(line).alignment(Alignment::Center);
            frame.render_widget(
                para,
                Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: 1,
                },
            );
            y += 1;
        }
    }

    fn query(&self, attr: Attribute) -> Option<QueryResult<'_>> {
        self.props
            .get(attr)
            .map(|v| QueryResult::Borrowed(v.into()))
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        match attr {
            Attribute::Custom(attr::WORKING_DIR) => {
                if let AttrValue::String(dir) = value {
                    self.set_working_dir(dir);
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

/// Component wrapper for `Banner`.
pub struct BannerComponent {
    component: Banner,
}

impl Default for BannerComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl BannerComponent {
    pub fn new() -> Self {
        Self {
            component: Banner::new(),
        }
    }
}

impl Deref for BannerComponent {
    type Target = Banner;

    fn deref(&self) -> &Self::Target {
        &self.component
    }
}

impl DerefMut for BannerComponent {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.component
    }
}

impl Component for BannerComponent {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        self.component.view(frame, area);
    }

    fn query(&self, attr: Attribute) -> Option<QueryResult<'_>> {
        self.component.query(attr)
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        self.component.attr(attr, value);
    }

    fn state(&self) -> State {
        self.component.state()
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        self.component.perform(cmd)
    }
}

impl AppComponent<Msg, crate::msg::UserEvent> for BannerComponent {
    fn on(&mut self, ev: &Event<crate::msg::UserEvent>) -> Option<Msg> {
        match *ev {
            Event::Tick => {
                if self.component.mascot.tick() {
                    Some(Msg::Redraw)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}
