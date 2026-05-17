//! Initialization methods for Model

use anyhow::Result;
use std::time::Duration;
use tuirealm::{
    application::Application,
    listener::EventListenerCfg,
    props::{AttrValue, Attribute},
    subscription::{EventClause, Sub, SubClause},
};

use crate::{
    attr,
    components::{
        tips::get_random_tip, ChatViewComponent, FuzzyPickerComponent, HelpDialog,
        InfoBarComponent, InputComponent, PickerConfig, SelectDialogComponent, StatusBarComponent,
        TodoListComponent,
    },
    id::Id,
    msg::{Msg, UserEvent},
};

use super::types::Model;

impl Model {
    /// Initialize input history in the `InputBox` component
    pub fn init_input_history(&mut self) -> Result<()> {
        // Serialize history to JSON string
        let history_json = serde_json::to_string(&self.input_history)?;
        self.app.attr(
            &Id::InputBox,
            Attribute::Custom(attr::HISTORY),
            AttrValue::String(history_json),
        )?;
        // Set working directory for file completion
        let working_dir_str = self.working_dir.to_string_lossy().to_string();
        let _ = self.app.attr(
            &Id::InputBox,
            Attribute::Custom(attr::WORKING_DIR),
            AttrValue::String(working_dir_str),
        );
        Ok(())
    }

    /// Display session messages in `ChatView` and calculate initial token usage for `StatusBar`
    pub fn init_session_messages(&mut self) -> Result<()> {
        let context_window = crate::config().agent.compactor.context_window;

        if self.session_messages.is_empty() {
            // Still initialize StatusBar with 0 tokens
            self.init_ctx_usage(0, context_window)?;
            return Ok(());
        }

        // Calculate initial token usage from messages
        let initial_tokens: u32 = self
            .session_messages
            .iter()
            .filter_map(|m| m.token_usage.map(|u| u.total_tokens))
            .next_back()
            .unwrap_or_else(|| {
                // Estimate tokens from all messages if no usage data
                use kernel::utils::tokens;
                self.session_messages
                    .iter()
                    .map(|m| tokens::estimate_tokens(&m.text_content()))
                    .sum::<usize>() as u32
            });

        // Initialize StatusBar with calculated tokens
        self.init_ctx_usage(initial_tokens, context_window)?;

        // Pass messages via Payload to avoid serialization
        let messages: Vec<kernel::types::Message> = std::mem::take(&mut self.session_messages);
        self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::INIT_HISTORY),
            AttrValue::Payload(tuirealm::props::PropPayload::Any(Box::new(messages))),
        )?;
        Ok(())
    }

    /// Initialize banner (data comes from global config and `working_dir`)
    pub fn init_banner(&mut self) -> Result<()> {
        self.update_banner()
    }

    /// Initialize status bar with permission level for YOLO mode display
    pub fn init_status_bar(&mut self) -> Result<()> {
        use kernel::permissions::Level;

        let level_val = match self.permission_level {
            Level::Safe => 0,
            Level::Caution => 1,
            Level::Dangerous => 2,
        };
        self.app.attr(
            &Id::StatusBar,
            Attribute::Custom(attr::SET_PERMISSION_LEVEL),
            AttrValue::Number(level_val),
        )?;

        // Inject a random tip on startup
        let tip = get_random_tip();
        self.app.attr(
            &Id::StatusBar,
            Attribute::Custom(attr::SHOW_TIP),
            crate::components::status_bar::Tip::new(format!("💡 {tip}"), 10000).to_attr_value(),
        )?;

        Ok(())
    }

    /// Initialize context window display in status bar
    pub fn init_ctx_usage(&mut self, tokens: u32, context_window: u32) -> Result<()> {
        let usage_str = format!("{tokens}\x00{context_window}");
        self.app.attr(
            &Id::StatusBar,
            Attribute::Custom(attr::SET_CTX_USAGE),
            AttrValue::String(usage_str),
        )?;
        Ok(())
    }

    /// Initialize todo list from file storage
    pub async fn init_todo_list(&mut self) -> Result<()> {
        use kernel::storage::{JsonTodoStore, TodoStore};
        let todo_storage = JsonTodoStore::new(&crate::config().data_dir);
        if let Some(todo_json) = todo_storage.load(&self.session_id).await? {
            self.app.attr(
                &Id::TodoList,
                Attribute::Custom(attr::SET_TODOS),
                AttrValue::String(todo_json),
            )?;
        }
        Ok(())
    }

    /// Update banner in `ChatView` (data comes from global config and `working_dir`)
    pub fn update_banner(&mut self) -> Result<()> {
        let working_dir = self.working_dir.to_string_lossy().to_string();
        self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::SET_BANNER),
            AttrValue::String(working_dir),
        )?;
        Ok(())
    }

    /// Initialize the tuirealm Application with all components
    pub(crate) fn init_app() -> Result<Application<Id, Msg, UserEvent>> {
        let mut app = Application::init(
            EventListenerCfg::default()
                .crossterm_input_listener(Duration::from_millis(10), 10)
                .tick_interval(Duration::from_millis(100)),
        );

        // Mount unified chat view component (includes scrollable banner)
        app.mount(
            Id::ChatView,
            Box::new(ChatViewComponent::new()),
            vec![
                Sub::new(EventClause::Tick, SubClause::Always),
                Sub::new(EventClause::Any, SubClause::Always),
            ],
        )?;

        // Mount info bar component (token/streaming status)
        app.mount(
            Id::InfoBar,
            Box::new(InfoBarComponent::new()),
            vec![
                Sub::new(EventClause::Tick, SubClause::Always),
                Sub::new(EventClause::Any, SubClause::Always),
            ],
        )?;

        // Mount input component
        app.mount(Id::InputBox, Box::new(InputComponent::new()), vec![])?;

        // Mount status bar component (vim-style mode indicator at bottom)
        app.mount(
            Id::StatusBar,
            Box::new(StatusBarComponent::new()),
            vec![
                Sub::new(EventClause::Tick, SubClause::Always),
                Sub::new(EventClause::Any, SubClause::Always),
            ],
        )?;

        // Mount select dialog component (hidden by default, for permission confirmation)
        app.mount(
            Id::Dialog,
            Box::new(SelectDialogComponent::new("Dialog")),
            vec![Sub::new(EventClause::Any, SubClause::Always)],
        )?;

        // Mount history picker component (hidden by default, for C-r history search)
        let history_picker = FuzzyPickerComponent::new(
            PickerConfig::new("History").with_placeholder("Search history..."),
        )
        .with_callbacks(crate::msg::Msg::HistorySelected, || {
            crate::msg::Msg::CloseHistoryPicker
        });
        app.mount(
            Id::HistoryPicker,
            Box::new(history_picker),
            vec![Sub::new(EventClause::Any, SubClause::Always)],
        )?;

        // Mount session picker component (hidden by default, for /sessions command)
        let session_picker = FuzzyPickerComponent::new(
            PickerConfig::new("Switch Session")
                .with_placeholder("Search sessions...")
                .with_max_height(12),
        )
        .with_callbacks(crate::msg::Msg::SessionSelected, || {
            crate::msg::Msg::CloseSessionPicker
        });
        app.mount(
            Id::SessionPicker,
            Box::new(session_picker),
            vec![Sub::new(EventClause::Any, SubClause::Always)],
        )?;

        // Mount checkpoint picker component (hidden by default, for /rewind command)
        let checkpoint_picker = FuzzyPickerComponent::new(
            PickerConfig::new("Rewind to Checkpoint")
                .with_placeholder(
                    "Search checkpoints... (Enter=Both, C-c=Conversation only, C-f=Files only)",
                )
                .with_max_height(15),
        )
        .with_callbacks(
            |id| crate::msg::Msg::CheckpointSelected(id, crate::msg::RewindTarget::Both),
            || crate::msg::Msg::CloseCheckpointPicker,
        );
        app.mount(
            Id::CheckpointPicker,
            Box::new(checkpoint_picker),
            vec![Sub::new(EventClause::Any, SubClause::Always)],
        )?;

        // Mount help dialog component (hidden by default)
        app.mount(
            Id::HelpDialog,
            Box::new(HelpDialog::new("Keyboard Shortcuts")),
            vec![Sub::new(EventClause::Any, SubClause::Always)],
        )?;

        // Mount todo list component (floating panel)
        app.mount(
            Id::TodoList,
            Box::new(TodoListComponent::new()),
            vec![Sub::new(EventClause::Tick, SubClause::Always)],
        )?;

        // Set focus to input box
        app.active(&Id::InputBox)?;

        // Debug-only: verify all overlay components are properly mounted
        // This catches the common mistake of adding to OVERLAY_COMPONENTS but forgetting to mount
        #[cfg(debug_assertions)]
        Self::verify_overlays_mounted(&app);

        Ok(app)
    }

    /// Verify that all overlay components declared in `OVERLAY_COMPONENTS` are mounted.
    /// Panics in debug mode if there's a mismatch, helping catch setup errors early.
    #[cfg(debug_assertions)]
    fn verify_overlays_mounted(app: &Application<Id, Msg, UserEvent>) {
        use crate::app::view::OVERLAY_COMPONENTS;

        for id in OVERLAY_COMPONENTS {
            // Query any attribute to verify component exists
            assert!(
                app.query(id, Attribute::Focus).is_ok(),
                "Overlay component {id:?} is in OVERLAY_COMPONENTS but not mounted! \
                 Did you forget to call app.mount() in init_app()?",
            );
        }
    }
}
