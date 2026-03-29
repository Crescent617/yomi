use crate::event::Event;
use anyhow::Result;
use tokio::sync::broadcast;

/// Async event bus with broadcast semantics
#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl EventBus {
    /// Create new event bus with specified capacity
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Publish event to all subscribers (non-blocking)
    pub fn send(&self, event: Event) -> Result<()> {
        match self.tx.send(event) {
            Ok(_) => Ok(()),
            Err(broadcast::error::SendError(_)) => {
                tracing::debug!("Event sent but no active receivers");
                Ok(())
            }
        }
    }

    /// Subscribe to events
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    /// Get number of active subscribers
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::UserEvent;

    #[tokio::test]
    async fn test_broadcast_multiple_subscribers() {
        let bus = EventBus::new(10);

        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        let event = Event::User(UserEvent::Message {
            content: "hello".to_string(),
        });
        bus.send(event.clone()).unwrap();

        let recv1 = rx1.recv().await.unwrap();
        let recv2 = rx2.recv().await.unwrap();

        assert!(matches!(recv1, Event::User(UserEvent::Message { .. })));
        assert!(matches!(recv2, Event::User(UserEvent::Message { .. })));
    }

    #[tokio::test]
    async fn test_subscriber_count() {
        let bus = EventBus::new(10);
        assert_eq!(bus.subscriber_count(), 0);

        let _rx1 = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 1);

        let _rx2 = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 2);

        drop(_rx1);
        tokio::task::yield_now().await;
        assert_eq!(bus.subscriber_count(), 1);
    }
}
