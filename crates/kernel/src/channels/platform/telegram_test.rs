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

#[test]
fn strip_bot_mention_removes_the_handle() {
    use super::strip_bot_mention as strip;
    assert_eq!(strip("@yomi_bot /help", "yomi_bot"), "/help");
    // Telegram usernames are case-insensitive; the handle may arrive in
    // any case.
    assert_eq!(strip("@Yomi_Bot /help", "yomi_bot"), "/help");
    assert_eq!(strip("hey @yomi_bot look", "yomi_bot"), "hey  look");
    // A longer handle sharing the prefix is someone/something else.
    assert_eq!(strip("@yomi_bot2 hi", "yomi_bot"), "@yomi_bot2 hi");
    // Unknown bot identity: leave the text alone.
    assert_eq!(strip("@yomi_bot /help", ""), "@yomi_bot /help");
    // CJK around the handle must not trip byte slicing.
    assert_eq!(strip("你好 @yomi_bot 在吗", "yomi_bot"), "你好  在吗");
    assert_eq!(strip("@yomi_bot", "yomi_bot"), "");
}

#[test]
fn directed_at_bot_recognizes_native_command_form() {
    use super::directed_at_bot as directed;
    assert!(directed("/help@yomi_bot", "yomi_bot"));
    assert!(directed("/watch@Yomi_Bot off", "yomi_bot"));
    // Mid-text directed commands still address the bot (same stance as a
    // mid-text @mention).
    assert!(directed("看这个 /help@yomi_bot", "yomi_bot"));
    // Directed at ANOTHER bot, bare commands, and prose do not count.
    assert!(!directed("/help@other_bot", "yomi_bot"));
    assert!(!directed("/help", "yomi_bot"));
    assert!(!directed("@yomi_bot /help", "yomi_bot"));
    assert!(!directed("/help@yomi_bot", ""));
}

#[test]
fn telegram_mention_renders_numeric_id_as_link() {
    assert_eq!(
        super::telegram_mention("123456"),
        "[123456](tg://user?id=123456)"
    );
}

#[test]
fn telegram_mention_leaves_foreign_id_as_is() {
    // feishu open_id 之类非数字 id 不重写（MarkdownV2 特殊字符会搞挂整条发送）
    assert_eq!(super::telegram_mention("ou_abc123"), "<@ou_abc123>");
}

#[test]
fn tg_display_name_prefers_full_name_then_username() {
    use teloxide_core::types::User;
    let from = |v: serde_json::Value| serde_json::from_value::<User>(v).unwrap();

    let named = from(serde_json::json!({
        "id": 42, "is_bot": false, "first_name": "华儒", "last_name": "李",
        "username": "crescent"
    }));
    assert_eq!(
        super::tg_display_name(Some(&named)).as_deref(),
        Some("华儒 李")
    );

    let first_only = from(serde_json::json!({
        "id": 42, "is_bot": false, "first_name": "华儒"
    }));
    assert_eq!(
        super::tg_display_name(Some(&first_only)).as_deref(),
        Some("华儒")
    );

    // TG requires first_name but it can be empty → username fallback.
    let username_only = from(serde_json::json!({
        "id": 42, "is_bot": false, "first_name": "", "username": "crescent"
    }));
    assert_eq!(
        super::tg_display_name(Some(&username_only)).as_deref(),
        Some("crescent")
    );

    assert_eq!(super::tg_display_name(None), None);
}
