use std::sync::Arc;
use tokio::time::{sleep, timeout, Duration};

use crate::comms::PubSub;

#[tokio::test]
async fn test_filter_excludes_events() {
    let bus: Arc<PubSub<i32, &'static str>> = PubSub::new();
    let mut sub = bus.subscribe_filtered("alpha", |&n| n % 2 == 0);

    // Even values pass the filter
    bus.publish("alpha", 2).unwrap();
    bus.publish("alpha", 4).unwrap();

    // Odd values are excluded
    bus.publish("alpha", 1).unwrap();
    bus.publish("alpha", 3).unwrap();

    assert_eq!(sub.recv().await, Some(("alpha", 2)));
    assert_eq!(sub.recv().await, Some(("alpha", 4)));

    // Give the forwarder a moment to finish
    sleep(Duration::from_millis(50)).await;
    // No more events should be queued
    assert!(timeout(Duration::from_millis(10), sub.recv())
        .await
        .is_err());
}

#[tokio::test]
async fn test_filter_does_not_affect_other_listeners() {
    let bus: Arc<PubSub<i32, &'static str>> = PubSub::new();

    let mut even_sub = bus.subscribe_filtered("alpha", |&n| n % 2 == 0);
    let mut all_sub = bus.subscribe("alpha");

    bus.publish("alpha", 1).unwrap();

    // even_sub should NOT receive this (1 is odd)
    // all_sub SHOULD receive it
    assert_eq!(all_sub.recv().await, Some(("alpha", 1)));

    // even_sub should have nothing
    sleep(Duration::from_millis(50)).await;
    assert!(timeout(Duration::from_millis(10), even_sub.recv())
        .await
        .is_err());
}

#[tokio::test]
async fn test_subscriber_filter_isolation() {
    let bus: Arc<PubSub<i32, &'static str>> = PubSub::new();

    let mut sub_a = bus.subscribe_filtered("alpha", |&n| n > 10);
    let mut sub_b = bus.subscribe_filtered("alpha", |&n| n < 5);

    bus.publish("alpha", 3).unwrap();
    bus.publish("alpha", 20).unwrap();
    bus.publish("alpha", 7).unwrap();

    assert_eq!(sub_a.recv().await, Some(("alpha", 20)));
    assert_eq!(sub_b.recv().await, Some(("alpha", 3)));

    sleep(Duration::from_millis(50)).await;
    assert!(timeout(Duration::from_millis(10), sub_a.recv())
        .await
        .is_err());
    assert!(timeout(Duration::from_millis(10), sub_b.recv())
        .await
        .is_err());
}

#[tokio::test]
async fn test_registration_is_synchronous() {
    let bus: Arc<PubSub<i32, &'static str>> = PubSub::new();
    let mut sub = bus.subscribe("alpha");
    // No yield between subscribe and publish: the registration must
    // already be visible to the forwarder.
    bus.publish("alpha", 42).unwrap();
    assert_eq!(
        timeout(Duration::from_secs(1), sub.recv()).await.unwrap(),
        Some(("alpha", 42))
    );
}

#[tokio::test]
async fn test_subscriber_recv_ends_on_shutdown() {
    let bus: Arc<PubSub<i32, &'static str>> = PubSub::new();
    let mut sub = bus.subscribe("alpha");
    bus.shutdown();
    // Listener senders are dropped at shutdown: recv() returns None
    // instead of pending forever.
    assert_eq!(
        timeout(Duration::from_secs(1), sub.recv()).await.unwrap(),
        None
    );

    // Subscribing after shutdown yields an immediately-closed receiver.
    let mut late = bus.subscribe("alpha");
    assert_eq!(
        timeout(Duration::from_secs(1), late.recv()).await.unwrap(),
        None
    );
}
