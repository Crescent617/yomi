//! View/rendering methods

use tuirealm::{
    ratatui::layout::{Constraint, Direction, Layout},
    state::{State, StateValue},
    terminal::TerminalAdapter,
};

use crate::id::Id;

use super::types::{AppMode, Model};

/// Overlay components that render on top of the main UI.
/// These components manage their own visibility internally.
///
/// When adding a new overlay component:
/// 1. Add its Id to this list
/// 2. Ensure it's mounted in `init_app()` in `init.rs`
pub(crate) const OVERLAY_COMPONENTS: &[Id] = &[
    Id::Dialog,
    Id::HistoryPicker,
    Id::SessionPicker,
    Id::CheckpointPicker,
    Id::HelpDialog,
    Id::TodoList,
];

impl Model {
    /// Main view method - renders all components
    pub fn view(&mut self) {
        // Update scroll progress on each redraw (throttled by frame rate)
        // Shows progress when scrolled up, clears when at bottom
        self.update_scroll_progress();

        // Pre-fetch content to calculate height without borrowing self in closure
        let input_content =
            if let Ok(State::Single(StateValue::String(content))) = self.app.state(&Id::InputBox) {
                content
            } else {
                String::new()
            };

        let _ = self.terminal.draw(|f| {
            // Calculate input height inside draw closure to access terminal area
            let input_height =
                Self::calculate_input_height_for_content(&input_content, f.area().width);

            if self.mode == AppMode::Browse {
                // Browse mode: full screen chat view with status bar
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints(
                        [
                            Constraint::Min(3),    // Main content area (includes banner)
                            Constraint::Length(1), // Status bar
                        ]
                        .as_ref(),
                    )
                    .split(f.area());

                self.app.view(&Id::ChatView, f, chunks[0]);
                // Status bar shows current mode (vim-style)
                self.app.view(&Id::StatusBar, f, chunks[1]);
            } else {
                // Normal mode: show all components
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints(
                        [
                            Constraint::Min(3),               // Main content area (chat with banner)
                            Constraint::Length(1),            // Info bar (tokens/streaming)
                            Constraint::Length(input_height), // Input area
                            Constraint::Length(1),            // Status bar
                        ]
                        .as_ref(),
                    )
                    .split(f.area());

                // ChatView includes banner at top (scrolls with content)
                self.app.view(&Id::ChatView, f, chunks[0]);
                // Info bar shows streaming progress
                self.app.view(&Id::InfoBar, f, chunks[1]);
                // InputBox renders last and sets cursor position
                self.app.view(&Id::InputBox, f, chunks[2]);
                // Status bar shows current mode (vim-style)
                self.app.view(&Id::StatusBar, f, chunks[3]);
            }

            // Render overlay components on top (they manage their own visibility)
            for id in OVERLAY_COMPONENTS {
                self.app.view(id, f, f.area());
            }
        });
    }
}
