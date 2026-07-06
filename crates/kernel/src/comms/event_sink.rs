use crate::comms::EventBusHandle;
use crate::event::Envelope;

/// 统一事件发送接口。主 agent 使用 `EventBusHandle`，
/// 测试可以使用 `NoOpEventSink` 或自定义收集器。
pub trait EventSink: Send + Sync {
    fn emit(&self, envelope: Envelope);
}

impl EventSink for EventBusHandle {
    fn emit(&self, envelope: Envelope) {
        self.try_send(envelope).ok();
    }
}

/// 空实现，用于不需要事件发送的场景。
pub struct NoOpEventSink;

impl EventSink for NoOpEventSink {
    fn emit(&self, _: Envelope) {}
}
