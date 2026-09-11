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
