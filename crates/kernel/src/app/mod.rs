//! Application layer - session and coordinator management

pub mod coordinator;
pub mod session;

pub use coordinator::Coordinator;
pub use session::{Session, SessionConfig};
