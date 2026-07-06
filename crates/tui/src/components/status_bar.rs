//! Status bar component for TUI
//!
//! Shows current mode at the bottom (vim-style) with three sections:
//! [LEFT: mode] [CENTER: tips] [RIGHT: context usage / scroll progress]

use tuirealm::{
    command::{Cmd, CmdResult},
    component::{AppComponent, Component},
    event::Event,
    props::{AttrValue, Attribute, Props, QueryResult},
    ratatui::{
        layout::{Alignment, Constraint, Layout, Rect},
        style::{Modifier, Style},
        text::{Line, Span},
        widgets::Paragraph,
        Frame,
    },
    state::State,
};

use crate::utils::text::truncate_by_width;

use crate::{attr, msg::Msg, theme::colors};
use kernel::permission::Level;
use std::ops::{Deref, DerefMut};

/// Tip message for status bar (center section)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tip {
    pub content: String,
    /// Duration in milliseconds, 0 = no timeout
    pub duration_ms: u64,
}

impl Tip {
    pub fn new(content: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            content: content.into(),
            duration_ms,
        }
    }

    /// Convert to `AttrValue` using `PropPayload::Any` for downcast
    pub fn to_attr_value(&self) -> tuirealm::props::AttrValue {
        use tuirealm::props::{AttrValue, PropPayload};
        AttrValue::Payload(PropPayload::Any(Box::new(self.clone())))
    }
}

/// Application mode for status bar display
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppMode {
    #[default]
    Normal,
    Browse,
}

/// Status bar showing current mode (vim-style at bottom)
/// Layout: [mode] [center] [scroll progress (optional)] [model] [ctx usage]
#[derive(Debug, Default)]
pub struct StatusBar {
    props: Props,
    mode: AppMode,
    /// Current token usage and context window size (tokens, `context_window`)
    ctx_usage: Option<(u32, u32)>,
    /// Permission level for displaying YOLO mode
    permission_level: Option<Level>,
    /// Scroll progress (`current_line`, `total_lines`)
    /// Displayed in browse mode always, or in normal mode when user scrolled up
    scroll_progress: Option<(usize, usize)>,
    /// Model name (shown at right, next to context usage)
    model_name: Option<String>,
}

impl StatusBar {
    pub fn new() -> Self {
        Self::default()
    }

    pub const fn set_mode(&mut self, mode: AppMode) {
        self.mode = mode;
    }

    /// Update context window usage (current tokens, max tokens)
    pub const fn set_ctx_usage(&mut self, tokens: u32, context_window: u32) {
        self.ctx_usage = Some((tokens, context_window));
    }

    /// Set permission level for YOLO mode display
    pub fn set_permission_level(&mut self, level: Level) {
        self.permission_level = Some(level);
    }

    /// Set scroll progress for browse mode (`current_line`, `total_lines`)
    pub const fn set_scroll_progress(&mut self, current: usize, total: usize) {
        self.scroll_progress = Some((current, total));
    }

    /// Clear scroll progress (when exiting browse mode)
    pub const fn clear_scroll_progress(&mut self) {
        self.scroll_progress = None;
    }

    /// Set model name for display in the right section
    pub fn set_model_name(&mut self, name: impl Into<String>) {
        self.model_name = Some(name.into());
    }

    fn render_mode_section(&self) -> Span<'static> {
        let (bg, text) = match self.mode {
            AppMode::Normal => {
                // Use warning color for YOLO mode
                if self.permission_level == Some(Level::Dangerous) {
                    (colors::accent_warning(), " YOLO ".to_string())
                } else {
                    (colors::accent_success(), String::new())
                }
            }
            AppMode::Browse => (colors::accent_system(), " BROWSE ".to_string()),
        };
        let fg = colors::selected_bg();

        Span::styled(
            text,
            Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
        )
    }

    fn render_center_section() -> Paragraph<'static> {
        Paragraph::new("")
    }

    fn render_scroll_progress_section(&self) -> Span<'static> {
        // Show scroll progress when available (browse mode always, normal mode when scrolled)
        if let Some((current, total)) = self.scroll_progress {
            let text = format!("[{current}/{total}]");
            Span::styled(
                text,
                Style::default()
                    .fg(colors::text_secondary())
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled("", Style::default())
        }
    }

    fn render_model_name_section(&self) -> Span<'static> {
        if let Some(ref name) = self.model_name {
            let text = if name.len() > 20 {
                truncate_by_width(name, 20, "...")
            } else {
                name.clone()
            };
            Span::styled(
                text,
                Style::default()
                    .fg(colors::text_secondary())
                    .add_modifier(Modifier::ITALIC),
            )
        } else {
            Span::styled("", Style::default())
        }
    }

    fn render_context_usage_section(&self) -> Span<'static> {
        // Display context window usage: "0.5% (128K)"
        #[allow(clippy::cast_precision_loss)]
        if let Some((tokens, context_window)) = self.ctx_usage {
            let percentage = tokens as f32 / context_window as f32;
            let cw_k = context_window / 1000;
            let text = format!("{:>4.1}% ({}K)", percentage * 100.0, cw_k);

            // Color based on usage level
            let fg = if percentage >= 0.9 {
                colors::accent_error() // Red for high usage
            } else if percentage >= 0.7 {
                colors::accent_warning() // Yellow for medium-high usage
            } else {
                colors::text_secondary() // Default for normal usage
            };

            Span::styled(text, Style::default().fg(fg))
        } else {
            Span::styled("", Style::default())
        }
    }
}

impl Component for StatusBar {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let has_scroll = self.scroll_progress.is_some();

        // Layout: [mode] [center] [right content]
        // Right content contains scroll (optional) + context
        let constraints = vec![
            Constraint::Min(0),  // Mode (auto width)
            Constraint::Fill(1), // Center (empty)
            Constraint::Min(0),  // Right side: scroll? + context
        ];

        let chunks = Layout::horizontal(constraints).split(area);

        // Mode (left)
        frame.render_widget(
            Paragraph::new(Line::from(vec![self.render_mode_section()])),
            chunks[0],
        );

        // Center section: empty
        frame.render_widget(Self::render_center_section(), chunks[1]);

        // Right side content: scroll (optional) + model + context (right-aligned)
        let mut right_spans = Vec::new();
        if has_scroll {
            right_spans.push(self.render_scroll_progress_section());
            right_spans.push(Span::raw(" "));
        }
        if self.model_name.is_some() {
            right_spans.push(self.render_model_name_section());
            right_spans.push(Span::raw(" · "));
        }
        right_spans.push(self.render_context_usage_section());
        frame.render_widget(
            Paragraph::new(Line::from(right_spans)).alignment(Alignment::Right),
            chunks[2],
        );
    }

    fn query(&self, attr: Attribute) -> Option<QueryResult<'_>> {
        self.props
            .get(attr)
            .map(|v| QueryResult::Borrowed(v.into()))
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        match attr {
            Attribute::Custom(attr::SET_MODE) => {
                if let AttrValue::Number(mode_val) = value {
                    self.mode = match mode_val {
                        1 => AppMode::Browse,
                        _ => AppMode::Normal,
                    };
                }
            }
            Attribute::Custom(attr::SET_CTX_USAGE) => {
                // Parse "tokens\x00context_window" format
                if let AttrValue::String(value_str) = value {
                    let parts: Vec<&str> = value_str.split('\x00').collect();
                    if parts.len() == 2 {
                        if let (Ok(tokens), Ok(context_window)) =
                            (parts[0].parse::<u32>(), parts[1].parse::<u32>())
                        {
                            self.set_ctx_usage(tokens, context_window);
                        }
                    }
                }
            }
            Attribute::Custom(attr::SET_PERMISSION_LEVEL) => {
                // Parse permission level: 0 = Safe, 1 = Caution, 2 = Dangerous
                if let AttrValue::Number(level_val) = value {
                    self.permission_level = match level_val {
                        0 => Some(Level::Safe),
                        1 => Some(Level::Caution),
                        2 => Some(Level::Dangerous),
                        _ => None,
                    };
                }
            }
            Attribute::Custom(attr::SET_SCROLL_PROGRESS) => {
                // Parse "current\x00total" format
                if let AttrValue::String(value_str) = value {
                    let parts: Vec<&str> = value_str.split('\x00').collect();
                    if parts.len() == 2 {
                        if let (Ok(current), Ok(total)) =
                            (parts[0].parse::<usize>(), parts[1].parse::<usize>())
                        {
                            self.set_scroll_progress(current, total);
                        }
                    }
                }
            }
            Attribute::Custom(attr::CLEAR_SCROLL_PROGRESS) => {
                self.clear_scroll_progress();
            }
            Attribute::Custom(attr::SET_MODEL_NAME) => {
                if let AttrValue::String(name) = value {
                    self.set_model_name(name);
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

/// Component wrapper for `StatusBar`
pub struct StatusBarComponent {
    component: StatusBar,
}

impl Default for StatusBarComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl StatusBarComponent {
    pub fn new() -> Self {
        Self {
            component: StatusBar::new(),
        }
    }
}

impl Deref for StatusBarComponent {
    type Target = StatusBar;

    fn deref(&self) -> &Self::Target {
        &self.component
    }
}

impl DerefMut for StatusBarComponent {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.component
    }
}

impl Component for StatusBarComponent {
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

impl AppComponent<Msg, crate::msg::UserEvent> for StatusBarComponent {
    fn on(&mut self, _ev: &Event<crate::msg::UserEvent>) -> Option<Msg> {
        None
    }
}
