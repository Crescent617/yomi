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

#[tokio::test]
async fn test_emit_on_drop_sends_event_on_scope_exit() {
    let bus: Arc<PubSub<i32, &'static str>> = PubSub::new();
    let handle = bus.handle("alpha");
    let mut sub = bus.subscribe("alpha");

    {
        let _guard = handle.emit_on_drop(4);
    }

    assert_eq!(sub.recv().await, Some(("alpha", 4)));
}

#[tokio::test]
async fn test_emit_on_drop_survives_task_abort() {
    let bus: Arc<PubSub<i32, &'static str>> = PubSub::new();
    let handle = bus.handle("alpha");
    let mut sub = bus.subscribe("alpha");

    let task = tokio::spawn(async move {
        let _guard = handle.emit_on_drop(4);
        std::future::pending::<()>().await;
    });
    tokio::task::yield_now().await;
    task.abort();

    assert_eq!(
        timeout(Duration::from_secs(5), sub.recv())
            .await
            .ok()
            .flatten(),
        Some(("alpha", 4))
    );
}

#[tokio::test]
async fn test_drop_counter_and_custom_capacity() {
    // capacity=1、无人消费：第 1 条进队，其余被丢并累计进 dropped
    // （2026-08-21 事故的回归测试：丢件必须可数、可查）。
    let bus: Arc<PubSub<i32, &'static str>> = PubSub::new();
    let mut sub = bus.subscribe_all_filtered_with_capacity(1, |_| true);
    let id = sub.id();

    for i in 0..4 {
        bus.publish("k", i).unwrap();
    }
    // forwarder 是异步派发，给它跑完的时间（与既有测试同一量级）。
    sleep(Duration::from_millis(100)).await;

    let dropped = bus.listener_dropped(id).expect("listener registered");
    assert_eq!(dropped, 3, "3 of 4 events must be dropped, got {dropped}");

    // 队列里那一条仍然完好可读。
    assert_eq!(sub.recv().await, Some(("k", 0)));
    // 未知 id 返回 None。
    assert_eq!(bus.listener_dropped(u64::MAX - 1), None);
}
