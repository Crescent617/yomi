use super::*;
use crate::event::StopReason;
use crate::types::{Message, SessionId};

fn sid() -> SessionId {
    SessionId::from("sess_test")
}

fn agent_event() -> Event {
    Event::Agent(AgentEvent::Lifecycle {
        state: AgentStatus::Running,
    })
}

fn internal_message_replaced() -> Event {
    Event::Internal(InternalEvent::MessageReplaced {
        messages: vec![std::sync::Arc::new(Message::default())],
    })
}

fn internal_message_added(role: Role) -> Event {
    Event::Internal(InternalEvent::MessageAdded {
        message: std::sync::Arc::new(Message {
            role,
            ..Default::default()
        }),
    })
}

#[test]
fn internal_events_are_never_buffered_or_published() {
    let buffer = EventBuffer::new(100);
    let subscribers = SessionSubscribers::new(16);
    let all_tx = broadcast::channel(16).0;
    let session = sid();
    let mut rx = subscribers.subscribe(&session);

    forward_envelope(
        &session,
        &Envelope::new(session.clone(), internal_message_replaced()),
        &buffer,
        &subscribers,
        &all_tx,
    );
    forward_envelope(
        &session,
        &Envelope::new(session.clone(), internal_message_added(Role::Tool)),
        &buffer,
        &subscribers,
        &all_tx,
    );

    assert!(buffer.get_after(&session, None).is_empty());
    assert!(rx.try_recv().is_err());
}

#[test]
fn message_added_clears_replay_buffer() {
    let buffer = EventBuffer::new(100);
    let subscribers = SessionSubscribers::new(16);
    let all_tx = broadcast::channel(16).0;
    let session = sid();
    let mut rx = subscribers.subscribe(&session);

    forward_envelope(
        &session,
        &Envelope::new(session.clone(), agent_event()),
        &buffer,
        &subscribers,
        &all_tx,
    );
    assert_eq!(buffer.get_after(&session, None).len(), 1);
    assert!(rx.try_recv().is_ok());

    for role in [Role::System, Role::User, Role::Assistant] {
        forward_envelope(
            &session,
            &Envelope::new(session.clone(), agent_event()),
            &buffer,
            &subscribers,
            &all_tx,
        );
        forward_envelope(
            &session,
            &Envelope::new(session.clone(), internal_message_added(role)),
            &buffer,
            &subscribers,
            &all_tx,
        );
        assert!(
            buffer.get_after(&session, None).is_empty(),
            "MessageAdded({role:?}) must clear the replay buffer"
        );
        // Drain the published agent event for the next round.
        while rx.try_recv().is_ok() {}
    }
}

#[test]
fn wire_events_are_buffered_and_published() {
    let buffer = EventBuffer::new(100);
    let subscribers = SessionSubscribers::new(16);
    let all_tx = broadcast::channel(16).0;
    let session = sid();
    let mut rx = subscribers.subscribe(&session);

    let envelope = Envelope::new(session.clone(), agent_event());
    let event_id = envelope.event_id.clone();
    forward_envelope(&session, &envelope, &buffer, &subscribers, &all_tx);

    let replayed = buffer.get_after(&session, None);
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].event_id, event_id);

    let received = rx.try_recv().expect("event must be published");
    assert_eq!(received.event_id, event_id);
}

#[test]
fn stopped_lifecycle_removes_buffer_entry() {
    let buffer = EventBuffer::new(100);
    let subscribers = SessionSubscribers::new(16);
    let all_tx = broadcast::channel(16).0;
    let session = sid();
    let mut rx = subscribers.subscribe(&session);

    forward_envelope(
        &session,
        &Envelope::new(session.clone(), agent_event()),
        &buffer,
        &subscribers,
        &all_tx,
    );
    forward_envelope(
        &session,
        &Envelope::new(
            session.clone(),
            Event::Agent(AgentEvent::Lifecycle {
                state: AgentStatus::Stopped {
                    reason: StopReason::Completed {
                        finish_reason: None,
                    },
                },
            }),
        ),
        &buffer,
        &subscribers,
        &all_tx,
    );

    assert!(buffer.get_after(&session, None).is_empty());
    // Both events are still delivered to real-time subscribers.
    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_ok());
}

#[test]
fn all_subscribers_receive_wire_events_only() {
    let buffer = EventBuffer::new(100);
    let subscribers = SessionSubscribers::new(16);
    let all_tx = broadcast::channel(16).0;
    let mut all_rx = all_tx.subscribe();
    let session = sid();

    // Internal events never reach the cross-session feed.
    forward_envelope(
        &session,
        &Envelope::new(session.clone(), internal_message_replaced()),
        &buffer,
        &subscribers,
        &all_tx,
    );
    assert!(
        all_rx.try_recv().is_err(),
        "internal events stay off the global feed"
    );

    // Regular wire events are fanned out to every subscriber.
    let envelope = Envelope::new(session.clone(), agent_event());
    let event_id = envelope.event_id.clone();
    forward_envelope(&session, &envelope, &buffer, &subscribers, &all_tx);
    let received = all_rx.try_recv().expect("wire event joins the global feed");
    assert_eq!(received.event_id, event_id);
    assert_eq!(received.session_id, session);
}
