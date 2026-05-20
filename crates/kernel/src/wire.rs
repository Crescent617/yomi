use crate::event::{ControlCommand, Event};
use crate::permissions::Level;
use crate::types::ContentBlock;
use serde::{Deserialize, Serialize};

/// All operations a client can request from the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestMethod {
    CreateSession {
        project_path: String,
        auto_approve_level: Level,
    },
    RestoreSession {
        session_id: String,
        project_path: String,
        auto_approve_level: Level,
    },
    ForkSession {
        parent_id: String,
        project_path: String,
        auto_approve_level: Level,
    },
    SendMessage {
        session_id: String,
        blocks: Vec<ContentBlock>,
    },
    Command {
        session_id: String,
        cmd: ControlCommand,
    },
    Subscribe {
        session_id: String,
    },
    Unsubscribe {
        session_id: String,
    },
    ListSessions,
    GetSessionMessages {
        session_id: String,
    },
    ListSessionsFiltered(crate::storage::session::ListArgs),
    GetCheckpoints {
        session_id: String,
    },
    GetTodos {
        session_id: String,
    },
    ShutdownSession {
        session_id: String,
    },
    DeleteSession {
        session_id: String,
    },
}

/// Response body — tagged union, no serde magic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ResponseBody {
    Ok { result: serde_json::Value },
    Err { error: RpcError },
}

/// Wire-level message envelope for IPC between kernel daemon and clients.
///
/// Uses JSON over length-prefixed frames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "ty", rename_all = "snake_case")]
pub enum WireMsg {
    /// Client → Server: request with id.
    Request { id: u64, method: RequestMethod },

    /// Server → Client: response to a request.
    Response { id: u64, body: ResponseBody },

    /// Server → Client: event push from kernel.
    Event { session_id: String, event: Event },

    /// Heartbeat ping.
    Ping,

    /// Heartbeat pong.
    Pong,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: String,
    pub message: String,
}

/// Next request ID generator (thread-safe).
#[derive(Debug)]
pub struct RequestIdGenerator {
    next: std::sync::atomic::AtomicU64,
}

impl RequestIdGenerator {
    pub fn new() -> Self {
        Self {
            next: std::sync::atomic::AtomicU64::new(1),
        }
    }

    pub fn next(&self) -> u64 {
        self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }
}

impl Default for RequestIdGenerator {
    fn default() -> Self {
        Self::new()
    }
}
