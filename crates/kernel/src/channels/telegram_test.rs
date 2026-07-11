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
