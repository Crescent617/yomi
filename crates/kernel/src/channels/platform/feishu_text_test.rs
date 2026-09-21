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
