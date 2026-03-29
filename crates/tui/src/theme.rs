use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone)]
pub struct Theme {
    pub background: Color,
    pub foreground: Color,
    pub accent: Color,
    pub user_color: Color,
    pub assistant_color: Color,
    pub thinking_color: Color,
    pub system_color: Color,
    pub error_color: Color,
    pub warning_color: Color,
    pub success_color: Color,
    pub border_color: Color,
    pub selection: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            background: Color::Black,
            foreground: Color::Rgb(200, 200, 200),
            accent: Color::Rgb(100, 150, 255),
            user_color: Color::Rgb(100, 200, 100),
            assistant_color: Color::Rgb(150, 150, 255),
            thinking_color: Color::Rgb(120, 120, 120),
            system_color: Color::Rgb(150, 150, 150),
            error_color: Color::Rgb(255, 100, 100),
            warning_color: Color::Rgb(255, 200, 100),
            success_color: Color::Rgb(100, 255, 100),
            border_color: Color::Rgb(60, 60, 60),
            selection: Color::Rgb(40, 40, 80),
        }
    }

    pub fn base(&self) -> Style {
        Style::default().fg(self.foreground).bg(self.background)
    }

    pub fn user(&self) -> Style {
        Style::default().fg(self.user_color)
    }

    pub fn assistant(&self) -> Style {
        Style::default().fg(self.assistant_color)
    }

    pub fn thinking(&self) -> Style {
        Style::default()
            .fg(self.thinking_color)
            .add_modifier(Modifier::ITALIC | Modifier::DIM)
    }

    pub fn system(&self) -> Style {
        Style::default().fg(self.system_color)
    }

    pub fn error(&self) -> Style {
        Style::default().fg(self.error_color).add_modifier(Modifier::BOLD)
    }

    pub fn warning(&self) -> Style {
        Style::default().fg(self.warning_color)
    }

    pub fn success(&self) -> Style {
        Style::default().fg(self.success_color)
    }

    pub fn border(&self) -> Style {
        Style::default().fg(self.border_color)
    }

    pub fn accent(&self) -> Style {
        Style::default().fg(self.accent).add_modifier(Modifier::BOLD)
    }
}
