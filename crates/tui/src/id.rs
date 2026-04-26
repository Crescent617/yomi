//! Component IDs for tuirealm

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Id {
    ChatView,
    InfoBar, // Token/stream info above input
    InputBox,
    StatusBar,     // Mode indicator at bottom (vim-style)
    Mascot,        // Cat mascot
    Banner,        // Banner with mascot and system info (empty state)
    Dialog,        // Select dialog for permission confirmation
    HistoryPicker, // Fuzzy finder for input history (C-r)
}
