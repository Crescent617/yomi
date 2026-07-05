use crate::comms::EventBusHandle;
use crate::event::Event;

/// 统一事件发送接口。主 agent 使用 `EventBusHandle`，子 agent 使用 `mpsc::Sender<Event>`，
/// 测试可以使用 `NoOpEventSink` 或自定义收集器。
pub trait EventSink: Send + Sync {
    fn emit(&self, event: Event);
}

impl EventSink for EventBusHandle {
    fn emit(&self, event: Event) {
        self.try_send(event).ok();
    }
}

impl EventSink for tokio::sync::mpsc::Sender<Event> {
    fn emit(&self, event: Event) {
        self.try_send(event).ok();
    }
}

/// 空实现，用于不需要事件发送的场景。
pub struct NoOpEventSink;

impl EventSink for NoOpEventSink {
    fn emit(&self, _: Event) {}
}
