use super::*;

#[test]
fn sanitize_header_name_strips_forgery_chars() {
    use sanitize_header_name as s;
    assert_eq!(s("华儒"), Some("华儒".to_string()));
    assert_eq!(s("a]b\nc"), Some("a b c".to_string()));
    assert_eq!(s("[chat_id: x]"), Some("chat_id: x".to_string()));
    assert_eq!(s("]]\n["), None, "剥光回退裸 id");
    assert_eq!(s("  a  b  "), Some("a b".to_string()));
}

#[test]
fn metadata_header_sender_variants_and_optional_segments() {
    // 命名发送者 + 全部可选段。
    assert_eq!(
        metadata_header(
            "ts",
            HeaderSender::User {
                name: Some("李华儒"),
                id: "ou_1"
            },
            "oc_1",
            "om_1",
            Some("omt_1"),
            Some("om_r"),
            "feishu",
        ),
        "[ts][from: 李华儒 (ou_1)][chat_id: oc_1][msg_id: om_1][thread: omt_1][root: om_r][platform: feishu]"
    );
    // 名字剥光/缺失回退裸 id。
    assert_eq!(
        metadata_header(
            "ts",
            HeaderSender::User {
                name: Some("]]\n["),
                id: "ou_1"
            },
            "oc_1",
            "om_1",
            None,
            None,
            "telegram",
        ),
        "[ts][from_user_id: ou_1][chat_id: oc_1][msg_id: om_1][platform: telegram]"
    );
    // 伪造字符的名字被 sanitize 后仍走 [from: …]。
    assert_eq!(
        metadata_header(
            "ts",
            HeaderSender::User {
                name: Some("恶 意 [chat_id: x] 名"),
                id: "ou_evil"
            },
            "oc_1",
            "om_1",
            None,
            None,
            "feishu",
        ),
        "[ts][from: 恶 意 chat_id: x 名 (ou_evil)][chat_id: oc_1][msg_id: om_1][platform: feishu]"
    );
    // 固定标签（CLI 合成触发）。
    assert_eq!(
        metadata_header(
            "ts",
            HeaderSender::Label("yomi-cli"),
            "oc_1",
            "om_1",
            None,
            None,
            "feishu",
        ),
        "[ts][from: yomi-cli][chat_id: oc_1][msg_id: om_1][platform: feishu]"
    );
}
