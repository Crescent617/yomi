//! Application initialization

use std::time::Duration;

use anyhow::Result;
use tuirealm::application::Application;
use tuirealm::listener::EventListenerCfg;
use tuirealm::props::{AttrValue, Attribute};
use tuirealm::subscription::{EventClause, Sub, SubClause};

use kernel::permissions::Level;
use kernel::storage::{JsonTodoStore, TodoStore};
use tuirealm::terminal::CrosstermTerminalAdapter;

use crate::app::types::{Model, OnInputHook};
use crate::app::state::AppState;
use crate::attr;
use crate::components::{
    ChatViewComponent, FuzzyPickerComponent, HelpDialog, InfoBarComponent, InputComponent,
    PickerConfig, SelectDialogComponent, StatusBarComponent, TodoListComponent,
};
use crate::components::tips::get_random_tip;
use crate::components::status_bar::Tip;
use crate::id::Id;
use crate::msg::{Msg, UserEvent};

/// Model construction trait
pub trait Init: Sized {
    #[allow(clippy::too_many_arguments)]
    fn new(
        event_rx: tokio::sync::broadcast::Receiver<kernel::event::Event>,
        input_tx: tokio::sync::mpsc::Sender<Vec<kernel::types::ContentBlock>>,
        ctrl_tx: tokio::sync::mpsc::Sender<kernel::event::ControlCommand>,
        session_store: std::sync::Arc<dyn kernel::storage::SessionStore>,
        input_history: Vec<String>,
        working_dir: std::path::PathBuf,
        session_messages: Vec<kernel::types::Message>,
        initial_message: Option<String>,
        session_id: String,
        on_input_hook: Option<OnInputHook>,
    ) -> Result<Self>;
}

/// Component initialization trait
#[allow(async_fn_in_trait)]
pub trait ComponentInit {
    fn init_app() -> Result<Application<Id, Msg, UserEvent>>;
    fn init_input_history(&mut self) -> Result<()>;
    fn init_session_messages(&mut self) -> Result<()>;
    fn init_banner(&mut self) -> Result<()>;
    fn init_status_bar(&mut self) -> Result<()>;
    fn init_ctx_usage(&mut self, tokens: u32, context_window: u32) -> Result<()>;
    async fn init_todo_list(&mut self) -> Result<()>;
    fn update_banner(&mut self) -> Result<()>;
}

impl Init for Model {
    #[allow(clippy::too_many_arguments)]
    fn new(
        event_rx: tokio::sync::broadcast::Receiver<kernel::event::Event>,
        input_tx: tokio::sync::mpsc::Sender<Vec<kernel::types::ContentBlock>>,
        ctrl_tx: tokio::sync::mpsc::Sender<kernel::event::ControlCommand>,
        session_store: std::sync::Arc<dyn kernel::storage::SessionStore>,
        input_history: Vec<String>,
        working_dir: std::path::PathBuf,
        session_messages: Vec<kernel::types::Message>,
        initial_message: Option<String>,
        session_id: String,
        on_input_hook: Option<OnInputHook>,
    ) -> Result<Self> {
        let terminal = CrosstermTerminalAdapter::new()?;
        let app = Self::init_app()?;

        Ok(Self {
            app,
            state: AppState {
                quit: false,
                should_redraw: true,
                is_streaming: false,
                should_create_new_session: false,
                initial_message,
                switch_to_session: None,
            },
            terminal,
            event_rx,
            input_tx,
            ctrl_tx,
            session_store,
            current_content: String::new(),
            current_thinking: String::new(),
            thinking_start_time: None,
            pending_permission: None,
            initial_history_len: input_history.len(),
            input_history,
            working_dir,
            session_messages,
            session_id,
            permission_level: crate::config().auto_approve,
            queued_message: None,
            on_input_hook,
        })
    }
}

impl ComponentInit for Model {
    fn init_app() -> Result<Application<Id, Msg, UserEvent>> {
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
        .with_callbacks(Msg::HistorySelected, || Msg::CloseHistoryPicker);
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
        .with_callbacks(Msg::SessionSelected, || Msg::CloseSessionPicker);
        app.mount(
            Id::SessionPicker,
            Box::new(session_picker),
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

        Ok(app)
    }

    fn init_input_history(&mut self) -> Result<()> {
        let history_json = serde_json::to_string(&self.input_history)?;
        self.app.attr(
            &Id::InputBox,
            Attribute::Custom(attr::HISTORY),
            AttrValue::String(history_json),
        )?;
        let working_dir_str = self.working_dir.to_string_lossy().to_string();
        let _ = self.app.attr(
            &Id::InputBox,
            Attribute::Custom(attr::WORKING_DIR),
            AttrValue::String(working_dir_str),
        );
        Ok(())
    }

    fn init_session_messages(&mut self) -> Result<()> {
        let context_window = crate::config().agent.compactor.context_window;

        if self.session_messages.is_empty() {
            self.init_ctx_usage(0, context_window)?;
            return Ok(());
        }

        let initial_tokens: u32 = self
            .session_messages
            .iter()
            .filter_map(|m| m.token_usage.map(|u| u.total_tokens))
            .next_back()
            .unwrap_or_else(|| {
                use kernel::utils::tokens;
                self.session_messages
                    .iter()
                    .map(|m| tokens::estimate_tokens(&m.text_content()))
                    .sum::<usize>() as u32
            });

        self.init_ctx_usage(initial_tokens, context_window)?;

        let messages: Vec<kernel::types::Message> = std::mem::take(&mut self.session_messages);
        self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::INIT_HISTORY),
            AttrValue::Payload(tuirealm::props::PropPayload::Any(Box::new(messages))),
        )?;
        Ok(())
    }

    fn init_banner(&mut self) -> Result<()> {
        self.update_banner()
    }

    fn init_status_bar(&mut self) -> Result<()> {
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

        let tip = get_random_tip();
        self.app.attr(
            &Id::StatusBar,
            Attribute::Custom(attr::SHOW_TIP),
            Tip::new(format!("💡 {tip}"), 10000).to_attr_value(),
        )?;

        Ok(())
    }

    fn init_ctx_usage(&mut self, tokens: u32, context_window: u32) -> Result<()> {
        let usage_str = format!("{tokens}\x00{context_window}");
        self.app.attr(
            &Id::StatusBar,
            Attribute::Custom(attr::SET_CTX_USAGE),
            AttrValue::String(usage_str),
        )?;
        Ok(())
    }

    async fn init_todo_list(&mut self) -> Result<()> {
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

    fn update_banner(&mut self) -> Result<()> {
        let working_dir = self.working_dir.to_string_lossy().to_string();
        self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::SET_BANNER),
            AttrValue::String(working_dir),
        )?;
        Ok(())
    }
}
