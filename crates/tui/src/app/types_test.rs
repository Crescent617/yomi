use super::{format_short_id, model_picker_items};
use kernel::kernel::ModelInfo;

fn model(name: &str, model_id: &str, provider: &str, ctx: u32) -> ModelInfo {
    ModelInfo {
        name: name.to_string(),
        model_id: model_id.to_string(),
        provider: provider.to_string(),
        context_window: ctx,
    }
}

#[test]
fn test_model_picker_items_marks_and_sorts_current_first() {
    let models = vec![
        model("a-model", "model-a", "openai", 128_000),
        model("b-model", "model-b", "acme", 256_000),
        model("z-model", "model-z", "anthropic", 200_000),
    ];

    let items = model_picker_items(&models, "b-model");
    assert_eq!(items.len(), 3);

    // Current model first, marked with ●
    assert_eq!(items[0].id, "b-model");
    assert!(items[0].label.starts_with("● "));

    // Others keep original order, unmarked
    assert_eq!(items[1].id, "a-model");
    assert!(items[1].label.starts_with("  "));
    assert_eq!(items[2].id, "z-model");
}

#[test]
fn test_model_picker_items_meta_format() {
    let models = vec![model("b-model", "model-b", "acme", 256_000)];
    let items = model_picker_items(&models, "b-model");
    assert_eq!(items[0].meta.as_deref(), Some("acme · model-b · 256k ctx"));
}

#[test]
fn test_model_picker_items_unknown_current() {
    // Current key not in the list (e.g. stale db value): nothing marked
    let models = vec![model("a", "ma", "p", 1000), model("b", "mb", "p", 2000)];
    let items = model_picker_items(&models, "gone");
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|i| !i.label.starts_with('●')));
    assert_eq!(items[0].id, "a");
}

#[test]
fn test_model_picker_items_empty() {
    assert!(model_picker_items(&[], "any").is_empty());
}

#[test]
fn test_format_short_id() {
    assert_eq!(format_short_id("short"), "short");
    assert_eq!(format_short_id("123456789012"), "123456789012");
    assert_eq!(format_short_id("1234567890123456"), "123456...3456");
}

// ── merged_pending ────────────────────────────────────────────────────

use super::merged_pending;
use kernel::comms::{MailboxItem, MailboxItemKind, MailboxSnapshot};
use kernel::types::MailboxItemId;

fn item(kind: MailboxItemKind, preview: &str) -> MailboxItem {
    MailboxItem {
        id: MailboxItemId::new(),
        kind,
        preview: preview.to_string(),
        text: Some(preview.to_string()),
        blocks_len: 1,
        enqueued_at: chrono::Utc::now(),
    }
}

#[test]
fn merged_pending_is_steer_first_then_fifo_queue() {
    let snapshot = MailboxSnapshot {
        steer: vec![item(MailboxItemKind::Steer, "s1")],
        queue: vec![
            item(MailboxItemKind::Queue, "q1"),
            item(MailboxItemKind::Queue, "q2"),
        ],
    };
    let merged = merged_pending(&snapshot);
    let previews: Vec<&str> = merged.iter().map(|i| i.preview.as_str()).collect();
    assert_eq!(previews, ["s1", "q1", "q2"]);
}
