use super::TelegramAdapter;

#[test]
fn command_messages_are_isolated_from_regular_batches() {
    let batches = TelegramAdapter::split_command_batches(
        vec!["before", "/stop@yomi_bot", "after", "more"],
        |text| text.starts_with('/'),
        |_| 1,
    );

    assert_eq!(
        batches,
        vec![
            vec!["before"],
            vec!["/stop@yomi_bot"],
            vec!["after", "more"],
        ]
    );
}

#[test]
fn regular_messages_stay_in_one_batch() {
    let batches = TelegramAdapter::split_command_batches(vec!["one", "two"], |_| false, |_| 1);

    assert_eq!(batches, vec![vec!["one", "two"]]);
}

#[test]
fn batches_split_at_sender_change() {
    // Access control and reactions target the merged message's sender, so
    // a batch must never mix senders.
    let batches = TelegramAdapter::split_command_batches(
        vec![("a", 1u64), ("b", 1), ("c", 2), ("d", 1)],
        |_| false,
        |&(_, sender)| sender,
    );

    assert_eq!(
        batches,
        vec![vec![("a", 1), ("b", 1)], vec![("c", 2)], vec![("d", 1)]]
    );
}

#[test]
fn command_isolation_resets_sender_grouping() {
    // Same sender around a command must not merge across it.
    let batches = TelegramAdapter::split_command_batches(
        vec![("a", 1u64), ("/stop", 1), ("b", 1)],
        |&(text, _)| text.starts_with('/'),
        |&(_, sender)| sender,
    );

    assert_eq!(
        batches,
        vec![vec![("a", 1)], vec![("/stop", 1)], vec![("b", 1)]]
    );
}

#[test]
fn cap_message_length_keeps_short_text() {
    assert_eq!(super::cap_message_length("hello"), "hello");
}

#[test]
fn cap_message_length_truncates_utf16_safely() {
    // 汉 costs 1 UTF-16 unit per char.
    let long = "汉".repeat(5000);
    let capped = super::cap_message_length(&long);
    assert!(capped.ends_with("...(内容已截断)"));
    assert!(capped.encode_utf16().count() <= super::MAX_MESSAGE_UTF16_UNITS);

    // Non-BMP chars cost 2 UTF-16 units each: a char-count cap would let
    // 3000 emoji (6000 units) through and fail the whole send.
    let emoji = "🎉".repeat(3000);
    let capped = super::cap_message_length(&emoji);
    assert!(capped.ends_with("...(内容已截断)"));
    assert!(capped.encode_utf16().count() <= super::MAX_MESSAGE_UTF16_UNITS);
}
