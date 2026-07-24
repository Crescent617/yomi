use super::TelegramAdapter;

#[test]
fn command_messages_are_isolated_from_regular_batches() {
    let batches = TelegramAdapter::split_command_batches(
        vec!["before", "/stop@yomi_bot", "after", "more"],
        |text| text.starts_with('/'),
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
    let batches = TelegramAdapter::split_command_batches(vec!["one", "two"], |_| false);

    assert_eq!(batches, vec![vec!["one", "two"]]);
}

#[test]
fn cap_message_length_keeps_short_text() {
    assert_eq!(super::cap_message_length("hello"), "hello");
}

#[test]
fn cap_message_length_truncates_char_safely() {
    let long = "汉".repeat(5000);
    let capped = super::cap_message_length(&long);
    assert!(capped.ends_with("...(内容已截断)"));
    let body = capped.trim_end_matches("...(内容已截断)").trim_end();
    // truncate_chars keeps `max` chars and appends the ellipsis on top.
    assert_eq!(body.chars().count(), super::MAX_MESSAGE_CHARS + 1);
    assert!(body.ends_with('…'));
}
