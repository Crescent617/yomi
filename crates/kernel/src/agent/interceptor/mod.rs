use crate::types::{ContentBlock, Message};
use async_trait::async_trait;
use std::sync::Arc;

mod todo;

pub use todo::TodoReminderInterceptor;

/// Context passed to interceptors, giving them access to conversation history
/// so they can make context-aware decisions.
pub struct InterceptCtx<'a> {
    pub session_id: &'a str,
    pub history: &'a [Arc<Message>],
}

/// Trait for intercepting and modifying user messages before they are processed by the agent.
///
/// This provides an extension point for injecting system reminders, context augmentation,
/// or other message transformations in a pluggable way.
#[async_trait]
pub trait UserMsgInterceptor: Send + Sync {
    /// Intercept and possibly modify user message content.
    ///
    /// `ctx` provides access to the session id and full message history.
    async fn intercept(&self, content: &mut Vec<ContentBlock>, ctx: &InterceptCtx<'_>);
}

/// A composite interceptor that runs multiple interceptors in sequence.
pub struct Interceptors {
    interceptors: Vec<Arc<dyn UserMsgInterceptor>>,
}

impl Interceptors {
    pub fn new(interceptors: Vec<Arc<dyn UserMsgInterceptor>>) -> Self {
        Self { interceptors }
    }

    pub fn empty() -> Self {
        Self {
            interceptors: Vec::new(),
        }
    }
}

#[async_trait]
impl UserMsgInterceptor for Interceptors {
    async fn intercept(&self, content: &mut Vec<ContentBlock>, ctx: &InterceptCtx<'_>) {
        for interceptor in &self.interceptors {
            interceptor.intercept(content, ctx).await;
        }
    }
}
