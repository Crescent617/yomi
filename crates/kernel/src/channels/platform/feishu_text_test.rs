use super::*;

/// 契约：信封时间戳按 daemon 本地时区渲染（UTC 直出的回归会让
/// +0800 用户看到慢 8 小时的头，且与兜底分支时区不一致）。
#[test]
fn envelope_timestamp_renders_in_local_timezone() {
    let ts_millis = 1_700_000_000_000_i64;
    let expect = chrono::DateTime::from_timestamp_millis(ts_millis)
        .unwrap()
        .with_timezone(&chrono::Local)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let parse = FeishuAdapter::parse_feishu_timestamp;
    assert_eq!(parse(&serde_json::json!(ts_millis.to_string())), expect);
    // 字符串与数字两种形态、秒/毫秒/微秒三种单位。
    assert_eq!(parse(&serde_json::json!(ts_millis)), expect);
    assert_eq!(parse(&serde_json::json!("1700000000")), expect);
    assert_eq!(parse(&serde_json::json!("1700000000000000")), expect);
}

/// `@_user_N` 占位符按 mentions 数组落成中性契约：有名字带名字，
/// 无名字裸 `<@open_id>`；数组外的字面量不动（手打的无条目）。
#[test]
fn rewrite_user_mention_keys_resolves_id_and_name() {
    let mentions = vec![
        serde_json::json!({ "key": "@_user_1", "id": { "open_id": "ou_bot" }, "name": "小嘟" }),
        serde_json::json!({ "key": "@_user_2", "id": { "open_id": "ou_alice" }, "name": "爱丽丝" }),
        serde_json::json!({ "key": "@_user_3", "id": { "open_id": "ou_noname" } }),
    ];
    assert_eq!(
        super::rewrite_user_mention_keys(
            "@_user_1 叫 @_user_2 和 @_user_3 看，@_user_9 是手打的",
            Some(&mentions)
        ),
        "<@ou_bot>小嘟 叫 <@ou_alice>爱丽丝 和 <@ou_noname> 看，@_user_9 是手打的"
    );
    // 无 mentions / 空数组：原文不动。
    assert_eq!(
        super::rewrite_user_mention_keys("@_user_1 hi", None),
        "@_user_1 hi"
    );
    assert_eq!(
        super::rewrite_user_mention_keys("@_user_1 hi", Some(&vec![])),
        "@_user_1 hi"
    );
}

/// at 标签改写同时吃 `id=` 与 `user_id=`（post `content_v2` 原文形态）。
#[test]
fn rewrite_at_tag_segment_accepts_user_id_form() {
    let mut out = String::new();
    super::rewrite_at_tag_segment(
        r#"<at id=ou_a>甲</at> <at user_id="ou_b">乙</at> <at user_id=ou_c></at>"#,
        &mut out,
    );
    assert_eq!(out, "<@ou_a>甲 <@ou_b>乙 <@ou_c>");
}

/// key 前缀腐蚀回归：`@_user_1` 是 `@_user_10` 的前缀——超过 9 个
/// 提及时朴素替换会把 `@_user_10` 啃成渣（边界负向断言根治）；
/// `strip_bot_mention` 同型。
#[test]
fn rewrite_user_mention_keys_prefix_collision_safe() {
    let mentions = vec![
        serde_json::json!({ "key": "@_user_1", "id": { "open_id": "ou_one" }, "name": "一" }),
        serde_json::json!({ "key": "@_user_10", "id": { "open_id": "ou_ten" }, "name": "十" }),
    ];
    assert_eq!(
        super::rewrite_user_mention_keys("@_user_10 和 @_user_1", Some(&mentions)),
        "<@ou_ten>十 和 <@ou_one>一"
    );
}

#[test]
fn strip_bot_mention_prefix_collision_safe() {
    let mentions = vec![
        serde_json::json!({ "key": "@_user_1", "id": { "open_id": "bot-id" } }),
        serde_json::json!({ "key": "@_user_10", "id": { "open_id": "alice-id" } }),
    ];
    assert_eq!(
        super::strip_bot_mention("@_user_1 叫 @_user_10 看", Some(&mentions), Some("bot-id")),
        "叫 @_user_10 看"
    );
}

/// name 是用户可控输入：角括号改写为全角，伪造中性标签注入出站的
/// 路被堵死（`<@…>` 会被 `rewrite_mentions` 当成真提及）。
#[test]
fn rewrite_user_mention_keys_sanitizes_name_angle_brackets() {
    let mentions = vec![serde_json::json!({
        "key": "@_user_1",
        "id": { "open_id": "ou_a" },
        "name": "x<@ou_evil>y"
    })];
    assert_eq!(
        super::rewrite_user_mention_keys("@_user_1", Some(&mentions)),
        "<@ou_a>x〈@ou_evil〉y"
    );
}

/// 单遍替换根治 fold 复扫注入：显示名若藏着其他 key 形态，也不会
/// 连锁替换出原文没有的提及（2026-09-21 评审 Minor）。
#[test]
fn rewrite_user_mention_keys_single_pass_no_chain_replacement() {
    let mentions = vec![
        serde_json::json!({ "key": "@_user_1", "id": { "open_id": "ou_evil" }, "name": "@_user_2" }),
        serde_json::json!({ "key": "@_user_2", "id": { "open_id": "ou_victim" }, "name": "受害者" }),
    ];
    // 若逐 key fold 复扫：第一遍 <@ou_evil>@_user_2 里的 @_user_2 会
    // 被第二遍替换成 <@ou_victim>受害者——原文没有的提及被制造出来。
    assert_eq!(
        super::rewrite_user_mention_keys("@_user_1 说话", Some(&mentions)),
        "<@ou_evil>@_user_2 说话"
    );
}

/// 历史命令保持原始形态交给下游过滤器：`@bot /clear` 改写前判定
/// （2026-09-21 评审 Major——先改写的话 `is_command_text` 失效，
/// 命令行漏进 `recent_chat_history`，`/bind` 还会带出 session id）。
#[test]
fn extract_history_content_keeps_command_in_raw_form() {
    let cmd = json!({
        "msg_type": "text",
        "body": { "content": json!({ "text": "@_user_1 /clear" }).to_string() },
        "mentions": [ { "key": "@_user_1", "id": { "open_id": "ou_bot" }, "name": "小嘟" } ]
    });
    let (text, _) = super::FeishuAdapter::extract_history_content(&cmd);
    assert_eq!(text, "@_user_1 /clear", "command stays in raw form");
    assert!(
        crate::channels::hub::command::is_command_text(&text),
        "and the hub filter still recognizes it"
    );

    let bind = json!({
        "msg_type": "text",
        "body": { "content": json!({ "text": "@_user_1 /bind sess_secret" }).to_string() },
        "mentions": [ { "key": "@_user_1", "id": { "open_id": "ou_bot" } } ]
    });
    let (text, _) = super::FeishuAdapter::extract_history_content(&bind);
    assert!(crate::channels::hub::command::is_command_text(&text));

    // 非命令照常改写。
    let normal = json!({
        "msg_type": "text",
        "body": { "content": json!({ "text": "@_user_1 今天天气" }).to_string() },
        "mentions": [ { "key": "@_user_1", "id": { "open_id": "ou_alice" }, "name": "爱丽丝" } ]
    });
    let (text, _) = super::FeishuAdapter::extract_history_content(&normal);
    assert_eq!(text, "<@ou_alice>爱丽丝 今天天气");
}

/// 多字节起首 key 不切半个字符（评审 Minor：pos+1 的 ASCII 假设）。
#[test]
fn replace_key_bounded_multibyte_first_char_no_panic() {
    // 假命中（更长 key 的后缀是 ASCII 字母）：保留首字符按 UTF-8 步进。
    let out = super::replace_key_bounded("看@用户1x 与 @用户1 好", "@用户1", "<@ou_a>");
    assert_eq!(out, "看@用户1x 与 <@ou_a> 好");
}
