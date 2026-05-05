//! Common UI operations

use tuirealm::props::{AttrValue, Attribute};
use tuirealm::terminal::TerminalAdapter;

use crate::app::state::StreamingStatus;
use crate::app::streaming::StreamingOps;
use crate::app::types::Model;
use crate::attr;
use crate::components::info_bar::Notification;
use crate::id::Id;

/// Common UI operations trait
pub trait UiOps {
    fn show_notification(&mut self, notification: &Notification);
    fn show_error_message(&mut self, message: impl Into<String>);
    fn handle_streaming_error(
        &mut self,
        status: StreamingStatus,
        error_message: impl Into<String>,
    );
    fn scroll_chat_to_bottom(&mut self);
    fn update_scroll_progress(&mut self);
    fn suspend_process(&mut self);
}

impl UiOps for Model {
    fn show_notification(&mut self, notification: &Notification) {
        let _ = self.app.attr(
            &Id::InfoBar,
            Attribute::Custom(attr::SHOW_NOTIFICATION),
            notification.to_attr_value(),
        );
    }

    fn show_error_message(&mut self, message: impl Into<String>) {
        let msg = message.into();
        let _ = self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::ADD_ERROR_MESSAGE),
            AttrValue::String(msg),
        );
    }

    fn handle_streaming_error(
        &mut self,
        status: StreamingStatus,
        error_message: impl Into<String>,
    ) {
        self.stop_streaming(status);
        self.show_error_message(error_message);
        self.state.should_redraw = true;
    }

    fn scroll_chat_to_bottom(&mut self) {
        let _ = self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::SCROLL_TO_BOTTOM),
            AttrValue::Flag(true),
        );
    }

    fn update_scroll_progress(&mut self) {
        if let Ok(Some(query_result)) = self
            .app
            .query(&Id::ChatView, Attribute::Custom(attr::SCROLL_PROGRESS))
        {
            if let AttrValue::String(progress_str) = query_result.into_attr() {
                let parts: Vec<&str> = progress_str.split('\x00').collect();
                if parts.len() == 3 {
                    let is_scrolled = parts[2] == "1";
                    if is_scrolled {
                        let scroll_data = format!("{}\x00{}", parts[0], parts[1]);
                        let _ = self.app.attr(
                            &Id::StatusBar,
                            Attribute::Custom(attr::SET_SCROLL_PROGRESS),
                            AttrValue::String(scroll_data),
                        );
                    } else {
                        let _ = self.app.attr(
                            &Id::StatusBar,
                            Attribute::Custom(attr::CLEAR_SCROLL_PROGRESS),
                            AttrValue::Flag(true),
                        );
                    }
                }
            }
        }
    }

    #[cfg(unix)]
    fn suspend_process(&mut self) {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::getpid;
        use std::io::Write;

        // Restore terminal state before suspending
        let _ = self.terminal.leave_alternate_screen();
        let _ = self.terminal.disable_raw_mode();
        let _ = self.terminal.disable_mouse_capture();
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::DisableBracketedPaste,
            crossterm::event::PopKeyboardEnhancementFlags
        );

        let _ = crossterm::execute!(std::io::stdout(), crossterm::cursor::Show);
        let _ = std::io::stdout().flush();

        let pid = getpid();
        if let Err(e) = kill(pid, Signal::SIGSTOP) {
            tracing::error!("Failed to send SIGSTOP: {}", e);
        }

        // Re-initialize terminal after resume
        std::thread::sleep(std::time::Duration::from_millis(50));

        let _ = self.terminal.enable_raw_mode();
        let _ = self.terminal.enter_alternate_screen();
        let _ = self.terminal.enable_mouse_capture();
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
            crossterm::cursor::Hide,
            crossterm::event::EnableBracketedPaste,
            crossterm::event::PushKeyboardEnhancementFlags(
                crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            )
        );

        // Force a full terminal refresh
        self.refresh_mode_toggle();
    }

    #[cfg(not(unix))]
    fn suspend_process(&mut self) {
        tracing::warn!("Suspend not supported on this platform");
    }
}

/// Internal helper trait for suspend/resume
trait SuspendHelper {
    fn refresh_mode_toggle(&mut self);
}

impl SuspendHelper for Model {
    fn refresh_mode_toggle(&mut self) {
        use crate::app::state::AppMode;
        use crate::attr;
        use tuirealm::props::{AttrValue, Attribute};

        // Get current mode value from status bar (approximation)
        // Toggle twice to force refresh
        let alt_mode = AppMode::Browse;

        let _ = self.app.attr(
            &Id::StatusBar,
            Attribute::Custom(attr::SET_MODE),
            AttrValue::Number(alt_mode as isize),
        );
        let _ = self.app.attr(
            &Id::InputBox,
            Attribute::Custom(attr::MODE),
            AttrValue::Number(alt_mode as isize),
        );

        self.state.should_redraw = true;
        self.view();

        let _ = self.app.attr(
            &Id::StatusBar,
            Attribute::Custom(attr::SET_MODE),
            AttrValue::Number(AppMode::Normal as isize),
        );
        let _ = self.app.attr(
            &Id::InputBox,
            Attribute::Custom(attr::MODE),
            AttrValue::Number(AppMode::Normal as isize),
        );

        self.state.should_redraw = true;
        self.view();
    }
}

// Need to import view for SuspendHelper
use crate::app::render::Render;
