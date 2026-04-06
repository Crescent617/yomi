//! Theme and styling configuration for the TUI
//! All colors are configurable at runtime through semantic color names

use ratatui::style::{Color, Modifier, Style};
use std::sync::RwLock;

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
            text_primary: Color::Rgb(245, 245, 250),
            text_secondary: Color::Rgb(160, 160, 175),
            text_muted: Color::Rgb(100, 100, 115),

            // Accent colors - Purple theme
            accent_user: Color::Rgb(196, 150, 255),  // Purple
            accent_system: Color::Rgb(100, 200, 255), // Blue
            accent_success: Color::Rgb(100, 220, 140), // Green
            accent_warning: Color::Rgb(255, 200, 100), // Yellow
            accent_error: Color::Rgb(255, 100, 100),   // Red

            // Code colors
            code_bg: Color::Rgb(35, 35, 45),
            code_fg: Color::Rgb(140, 220, 240),
            code_border: Color::Rgb(50, 50, 65),

            // UI elements
            border: Color::Rgb(45, 45, 58),
            border_active: Color::Rgb(80, 80, 100),
            divider: Color::Rgb(40, 40, 52),
        }
    }
}

impl ThemeConfig {
    /// Create a light theme
    pub const fn light() -> Self {
        Self {
            background: Color::Rgb(250, 250, 252),
            surface: Color::Rgb(255, 255, 255),
            surface_hover: Color::Rgb(240, 240, 245),

            text_primary: Color::Rgb(30, 30, 35),
            text_secondary: Color::Rgb(100, 100, 115),
            text_muted: Color::Rgb(150, 150, 165),

            accent_user: Color::Rgb(120, 80, 200),    // Deep purple
            accent_system: Color::Rgb(50, 120, 200),  // Blue
            accent_success: Color::Rgb(40, 150, 90),  // Green
            accent_warning: Color::Rgb(200, 150, 50), // Orange
            accent_error: Color::Rgb(200, 60, 60),    // Red

            code_bg: Color::Rgb(245, 245, 248),
            code_fg: Color::Rgb(60, 120, 140),
            code_border: Color::Rgb(220, 220, 228),

            border: Color::Rgb(220, 220, 230),
            border_active: Color::Rgb(180, 180, 200),
            divider: Color::Rgb(230, 230, 238),
        }
    }

    /// Create a high-contrast theme
    pub const fn high_contrast() -> Self {
        Self {
            background: Color::Black,
            surface: Color::Rgb(20, 20, 20),
            surface_hover: Color::Rgb(40, 40, 40),

            text_primary: Color::White,
            text_secondary: Color::Rgb(200, 200, 200),
            text_muted: Color::Rgb(150, 150, 150),

            accent_user: Color::Cyan,
            accent_system: Color::Yellow,
            accent_success: Color::Green,
            accent_warning: Color::Rgb(255, 165, 0),
            accent_error: Color::Red,

            code_bg: Color::Rgb(10, 10, 10),
            code_fg: Color::Green,
            code_border: Color::Rgb(80, 80, 80),

            border: Color::Rgb(100, 100, 100),
            border_active: Color::White,
            divider: Color::Rgb(80, 80, 80),
        }
    }

    /// Create a warm theme (amber/orange accents)
    pub const fn warm() -> Self {
        Self {
            background: Color::Rgb(25, 20, 18),
            surface: Color::Rgb(35, 28, 25),
            surface_hover: Color::Rgb(45, 38, 35),

            text_primary: Color::Rgb(255, 250, 245),
            text_secondary: Color::Rgb(200, 190, 180),
            text_muted: Color::Rgb(140, 130, 120),

            accent_user: Color::Rgb(255, 170, 100),   // Amber
            accent_system: Color::Rgb(100, 200, 220), // Cyan
            accent_success: Color::Rgb(140, 220, 120), // Green
            accent_warning: Color::Rgb(255, 200, 80),  // Yellow
            accent_error: Color::Rgb(255, 100, 100),   // Red

            code_bg: Color::Rgb(30, 25, 22),
            code_fg: Color::Rgb(220, 180, 140),
            code_border: Color::Rgb(60, 50, 45),

            border: Color::Rgb(60, 50, 45),
            border_active: Color::Rgb(100, 90, 80),
            divider: Color::Rgb(50, 42, 38),
        }
    }

    /// Create a forest/green theme
    pub const fn forest() -> Self {
        Self {
            background: Color::Rgb(15, 25, 20),
            surface: Color::Rgb(22, 35, 28),
            surface_hover: Color::Rgb(32, 48, 38),

            text_primary: Color::Rgb(245, 255, 250),
            text_secondary: Color::Rgb(180, 200, 185),
            text_muted: Color::Rgb(120, 140, 125),

            accent_user: Color::Rgb(140, 220, 140),   // Green
            accent_system: Color::Rgb(140, 200, 255), // Blue
            accent_success: Color::Rgb(120, 255, 160), // Bright green
            accent_warning: Color::Rgb(255, 220, 100), // Yellow
            accent_error: Color::Rgb(255, 120, 120),   // Red

            code_bg: Color::Rgb(20, 30, 25),
            code_fg: Color::Rgb(160, 230, 170),
            code_border: Color::Rgb(40, 60, 50),

            border: Color::Rgb(40, 60, 50),
            border_active: Color::Rgb(80, 120, 90),
            divider: Color::Rgb(35, 52, 42),
        }
    }
}

// Global theme configuration - thread-safe
static THEME_CONFIG: RwLock<ThemeConfig> = RwLock::new(ThemeConfig {
    background: Color::Rgb(18, 18, 23),
    surface: Color::Rgb(28, 28, 36),
    surface_hover: Color::Rgb(38, 38, 48),
    text_primary: Color::Rgb(245, 245, 250),
    text_secondary: Color::Rgb(160, 160, 175),
    text_muted: Color::Rgb(100, 100, 115),
    accent_user: Color::Rgb(196, 150, 255),
    accent_system: Color::Rgb(100, 200, 255),
    accent_success: Color::Rgb(100, 220, 140),
    accent_warning: Color::Rgb(255, 200, 100),
    accent_error: Color::Rgb(255, 100, 100),
    code_bg: Color::Rgb(35, 35, 45),
    code_fg: Color::Rgb(140, 220, 240),
    code_border: Color::Rgb(50, 50, 65),
    border: Color::Rgb(45, 45, 58),
    border_active: Color::Rgb(80, 80, 100),
    divider: Color::Rgb(40, 40, 52),
});

/// Get the current theme configuration
pub fn current_theme() -> ThemeConfig {
    THEME_CONFIG.read().map(|t| *t).unwrap_or_default()
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

    pub const fn light() -> ThemeConfig {
        ThemeConfig::light()
    }

    pub const fn high_contrast() -> ThemeConfig {
        ThemeConfig::high_contrast()
    }

    pub const fn warm() -> ThemeConfig {
        ThemeConfig::warm()
    }

    pub const fn forest() -> ThemeConfig {
        ThemeConfig::forest()
    }
}

/// Color accessors - use these to get current theme colors
pub mod colors {
    use super::current_theme;
    use ratatui::style::Color;

    pub fn background() -> Color { current_theme().background }
    pub fn surface() -> Color { current_theme().surface }
    pub fn surface_hover() -> Color { current_theme().surface_hover }

    pub fn text_primary() -> Color { current_theme().text_primary }
    pub fn text_secondary() -> Color { current_theme().text_secondary }
    pub fn text_muted() -> Color { current_theme().text_muted }

    pub fn accent_user() -> Color { current_theme().accent_user }
    pub fn accent_system() -> Color { current_theme().accent_system }
    pub fn accent_success() -> Color { current_theme().accent_success }
    pub fn accent_warning() -> Color { current_theme().accent_warning }
    pub fn accent_error() -> Color { current_theme().accent_error }

    pub fn code_bg() -> Color { current_theme().code_bg }
    pub fn code_fg() -> Color { current_theme().code_fg }
    pub fn code_border() -> Color { current_theme().code_border }

    pub fn border() -> Color { current_theme().border }
    pub fn border_active() -> Color { current_theme().border_active }
    pub fn divider() -> Color { current_theme().divider }
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
        Style::default().fg(colors::text_primary())
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
        Style::default().fg(colors::text_muted())
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
        Style::default()
            .fg(colors::accent_success())
    }

    /// Warning style
    pub fn warning() -> Style {
        Style::default()
            .fg(colors::accent_warning())
    }
}

/// Block characters for drawing UI elements
pub mod chars {
    // Vertical borders for message blocks
    pub const USER_BAR: &str = "│";
    pub const USER_CORNER_TOP: &str = "╭";
    pub const USER_CORNER_BOTTOM: &str = "╰";

    // Section indicators
    pub const FOLD_COLLAPSED: &str = "▶";
    pub const FOLD_EXPANDED: &str = "▼";
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
    fn test_theme_presets() {
        let _default = ThemeConfig::default();
        let _light = ThemeConfig::light();
        let _high_contrast = ThemeConfig::high_contrast();
        let _warm = ThemeConfig::warm();
        let _forest = ThemeConfig::forest();
    }

    #[test]
    fn test_theme_switching() {
        set_theme(ThemeConfig::light());
        assert_eq!(colors::background(), ThemeConfig::light().background);

        set_theme(ThemeConfig::default());
        assert_eq!(colors::background(), ThemeConfig::default().background);
    }

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
