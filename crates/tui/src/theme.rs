//! Theme and styling configuration for the TUI
//! All colors are configurable at runtime through semantic color names

use std::sync::{LazyLock, RwLock};
use tuirealm::ratatui::style::{Color, Modifier, Style};

/// Semantic color configuration - modify these to customize the theme
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemeConfig {
    // Core background colors
    /// Main background color
    pub background: Color,
    /// Input area / elevated surface background
    pub surface: Color,
    /// Hover state / secondary surface
    pub surface_hover: Color,

    // Text colors
    /// Primary text color (main content)
    pub text_primary: Color,
    /// Secondary text color (descriptions, metadata)
    pub text_secondary: Color,
    /// Muted text color (placeholders, disabled)
    pub text_muted: Color,

    // Accent colors (can be customized to any theme)
    /// User message accent (prompts, user indicators)
    pub accent_user: Color,
    /// User message background color
    pub user_msg_bg: Color,
    /// System/tool accent (tool calls, system messages)
    pub accent_system: Color,
    /// Success states
    pub accent_success: Color,
    /// Warning states
    pub accent_warning: Color,
    /// Error states
    pub accent_error: Color,

    // Code block colors
    /// Code block background
    pub code_bg: Color,
    /// Code text color
    pub code_fg: Color,
    /// Code block border
    pub code_border: Color,

    // UI elements
    /// Border color
    pub border: Color,
    /// Active/focused border
    pub border_active: Color,
    /// Divider lines
    pub divider: Color,
}

impl Default for ThemeConfig {
    /// Default theme with transparent backgrounds
    fn default() -> Self {
        Self {
            // Core backgrounds - default to transparent
            background: Color::Reset,
            surface: Color::Reset,
            surface_hover: Color::Reset,

            // Text colors
            text_primary: hex("#F5F5FA"),
            text_secondary: hex("#90909F"),
            text_muted: hex("#808090"),

            // Accent colors - Purple theme
            accent_user: hex("#C4C6CF"),
            user_msg_bg: hex("#2A2A35"),
            accent_system: hex("#64C8FF"),
            accent_success: hex("#64DC8C"),
            accent_warning: hex("#FFC864"),
            accent_error: hex("#FF6464"),

            // Code colors
            code_bg: hex("#23232D"),
            code_fg: hex("#8CDCF0"),
            code_border: hex("#707080"),

            // UI elements
            border: hex("#707080"),
            border_active: hex("#A0A0AF"),
            divider: hex("#707080"),
        }
    }
}

// Global theme configuration - thread-safe
static THEME_CONFIG: LazyLock<RwLock<ThemeConfig>> =
    LazyLock::new(|| RwLock::new(ThemeConfig::default()));

/// Get the current theme configuration
pub fn current_theme() -> ThemeConfig {
    THEME_CONFIG.read().map(|t| *t).unwrap()
}

/// Set the global theme configuration
pub fn set_theme(config: ThemeConfig) {
    if let Ok(mut theme) = THEME_CONFIG.write() {
        *theme = config;
    }
}

/// Reset to default theme
pub fn reset_theme() {
    set_theme(ThemeConfig::default());
}

/// Theme presets
pub mod presets {
    use super::ThemeConfig;

    pub fn default() -> ThemeConfig {
        ThemeConfig::default()
    }
}

/// Color accessors - use these to get current theme colors
pub mod colors {
    use super::current_theme;
    use tuirealm::ratatui::style::Color;

    pub fn background() -> Color {
        current_theme().background
    }
    pub fn surface() -> Color {
        current_theme().surface
    }
    pub fn surface_hover() -> Color {
        current_theme().surface_hover
    }

    pub fn text_primary() -> Color {
        current_theme().text_primary
    }
    pub fn text_secondary() -> Color {
        current_theme().text_secondary
    }
    pub fn text_muted() -> Color {
        current_theme().text_muted
    }

    pub fn accent_user() -> Color {
        current_theme().accent_user
    }
    pub fn user_msg_bg() -> Color {
        current_theme().user_msg_bg
    }
    pub fn accent_system() -> Color {
        current_theme().accent_system
    }
    pub fn accent_success() -> Color {
        current_theme().accent_success
    }
    pub fn accent_warning() -> Color {
        current_theme().accent_warning
    }
    pub fn accent_error() -> Color {
        current_theme().accent_error
    }

    pub fn code_bg() -> Color {
        current_theme().code_bg
    }
    pub fn code_fg() -> Color {
        current_theme().code_fg
    }
    pub fn code_border() -> Color {
        current_theme().code_border
    }

    pub fn border() -> Color {
        current_theme().border
    }
    pub fn border_active() -> Color {
        current_theme().border_active
    }
    pub fn divider() -> Color {
        current_theme().divider
    }
}

/// Style presets - dynamically use current theme
pub struct Styles;

impl Styles {
    /// User message header style
    pub fn user_header() -> Style {
        Style::default()
            .fg(colors::accent_user())
            .add_modifier(Modifier::BOLD)
    }

    /// User message content style
    pub fn user_content() -> Style {
        Style::default()
            .fg(colors::text_primary())
            .bg(colors::user_msg_bg())
    }

    /// Assistant message content style
    pub fn assistant_content() -> Style {
        Style::default().fg(colors::text_primary())
    }

    /// System message style
    pub fn system() -> Style {
        Style::default()
            .fg(colors::text_secondary())
            .add_modifier(Modifier::ITALIC)
    }

    /// Input prompt style
    pub fn input_prompt() -> Style {
        Style::default()
            .fg(colors::accent_user())
            .add_modifier(Modifier::BOLD)
    }

    /// Input text style
    pub fn input_text() -> Style {
        Style::default().fg(colors::text_primary())
    }

    /// Placeholder style
    pub fn placeholder() -> Style {
        Style::default().fg(colors::text_muted())
    }

    /// Code block style
    pub fn code_block() -> Style {
        Style::default().fg(colors::code_fg())
    }

    /// Code language tag style
    pub fn code_lang() -> Style {
        Style::default()
            .fg(colors::text_secondary())
            .add_modifier(Modifier::BOLD)
    }

    /// Inline code style
    pub fn inline_code() -> Style {
        Style::default()
            .fg(colors::code_fg())
            .add_modifier(Modifier::BOLD)
    }

    /// Thinking section header
    pub fn thinking_header() -> Style {
        Style::default()
            .fg(colors::text_secondary())
            .add_modifier(Modifier::ITALIC)
    }

    /// Thinking content
    pub fn thinking_content() -> Style {
        Style::default()
            .fg(colors::text_secondary())
            .add_modifier(Modifier::ITALIC)
    }

    /// Tool header
    pub fn tool_header() -> Style {
        Style::default()
            .fg(colors::accent_system())
            .add_modifier(Modifier::BOLD)
    }

    /// Tool content
    pub fn tool_content() -> Style {
        Style::default().fg(colors::text_secondary())
    }

    /// Streaming cursor
    pub fn cursor() -> Style {
        Style::default().fg(colors::accent_user())
    }

    /// Spinner style
    pub fn spinner() -> Style {
        Style::default()
            .fg(colors::accent_user())
            .add_modifier(Modifier::BOLD)
    }

    /// Error style
    pub fn error() -> Style {
        Style::default()
            .fg(colors::accent_error())
            .add_modifier(Modifier::BOLD)
    }

    /// Success style
    pub fn success() -> Style {
        Style::default().fg(colors::accent_success())
    }

    /// Warning style
    pub fn warning() -> Style {
        Style::default().fg(colors::accent_warning())
    }
}

/// Block characters for drawing UI elements
pub mod chars {
    // Vertical borders for message blocks
    pub const USER_BAR: &str = "│";
    pub const USER_CORNER_TOP: &str = "╭";
    pub const USER_CORNER_BOTTOM: &str = "╰";

    // Section indicators
    pub const FOLD_COLLAPSED: &str = ""; // 先不用
    pub const FOLD_EXPANDED: &str = ""; // 先不用
    pub const BULLET: &str = "•";

    // Input
    pub const INPUT_PROMPT: &str = "❯";
    pub const INPUT_PROMPT_MULTI: &str = "│";

    // Code block
    pub const CODE_TOP_LEFT: &str = "╭";
    pub const CODE_TOP_RIGHT: &str = "╮";
    pub const CODE_BOTTOM_LEFT: &str = "╰";
    pub const CODE_BOTTOM_RIGHT: &str = "╯";
    pub const CODE_HORIZONTAL: &str = "─";
    pub const CODE_VERTICAL: &str = "│";

    // Spinner frames
    pub const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
}

/// Get spinner character for frame index
pub fn spinner_char(frame: usize) -> &'static str {
    chars::SPINNER[frame % chars::SPINNER.len()]
}

/// Helper to create a custom color from RGB values
pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

/// Helper to create a custom color from hex string (e.g., "#FF5733")
pub fn hex(color_hex: &str) -> Color {
    let hex = color_hex.trim_start_matches('#');
    if hex.len() == 6 {
        if let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&hex[0..2], 16),
            u8::from_str_radix(&hex[2..4], 16),
            u8::from_str_radix(&hex[4..6], 16),
        ) {
            return Color::Rgb(r, g, b);
        }
    }
    Color::White // fallback
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_color() {
        assert_eq!(hex("#FF5733"), Color::Rgb(255, 87, 51));
        assert_eq!(hex("#000000"), Color::Rgb(0, 0, 0));
        assert_eq!(hex("#FFFFFF"), Color::Rgb(255, 255, 255));
    }

    #[test]
    fn test_styles_use_current_theme() {
        set_theme(ThemeConfig::default());
        let style = Styles::user_header();
        assert_eq!(style.fg, Some(colors::accent_user()));
    }
}
