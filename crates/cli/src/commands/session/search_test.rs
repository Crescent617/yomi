use super::*;

#[test]
fn contains_ci_ascii_case_variants() {
    assert!(contains_ci("Hello WORLD", "world"));
    assert!(contains_ci("Hello WORLD", "hello"));
    assert!(!contains_ci("Hello WORLD", "mars"));
    assert!(contains_ci("anything", ""));
    assert!(!contains_ci("ab", "abc"));
}

#[test]
fn contains_ci_non_ascii() {
    // ASCII needle 命中多字节 haystack（不切片，无 panic）
    assert!(contains_ci("中文 Context 混合", "context"));
    // 非 ASCII needle 直接包含
    assert!(contains_ci("中文 Context 混合", "中文"));
    // 非 ASCII needle 带 ASCII 字母的混合
    assert!(contains_ci("使用 Goal模式 工作", "goal模式"));
    assert!(!contains_ci("完全无关", "中文"));
}

#[test]
fn count_ci_non_overlapping() {
    assert_eq!(count_ci("ababab", "ab"), 3);
    assert_eq!(count_ci("AAAA", "aa"), 2);
    assert_eq!(count_ci("aaa", "aa"), 1); // 非重叠
    assert_eq!(count_ci("nothing here", "xyz"), 0);
    assert_eq!(count_ci("空 needle 不应入此分支", ""), 0);
    // 多字节 haystack 计数不 panic
    assert_eq!(count_ci("目标 Goal 与 goal 并存", "goal"), 2);
}

#[test]
fn snippet_windows_and_ellipsis() {
    let text = "abcdefghijklmnopqrstuvwxyz";
    // 命中点不在首尾部时两端都加省略号
    let s = snippet(text, "mn", 3).unwrap();
    assert_eq!(s, "…jklmnopq…");
    let long = "a".repeat(200);
    let s = snippet(&format!("{long} needle {long}"), "needle", 5).unwrap();
    assert!(s.starts_with('…') && s.ends_with('…'));
    assert!(s.contains("needle"));
    // 未命中返回 None
    assert!(snippet(text, "zzz", 3).is_none());
}

#[test]
fn snippet_unicode_expansion_no_panic() {
    // 'İ' (U+0130) to_lowercase 展开为 2 字符，approx 可能越过 end
    let text = format!("{} needle", "İ".repeat(80));
    let s = snippet(&text, "needle", 60).unwrap();
    assert!(s.contains("needle"));
}

#[test]
fn harvest_collects_tool_arguments_and_skips_heavy() {
    let line = serde_json::json!({
        "role": "assistant",
        "content": [{"type": "text", "text": "正文"}],
        "tool_calls": [{
            "id": "c1", "name": "shell",
            "arguments": {"command": "make build", "workdir": "/tmp/x"}
        }]
    });
    let mut out = Vec::new();
    harvest_text(&line, false, &mut out);
    let (_, text) = out.pop().unwrap();
    assert!(
        text.contains("make build"),
        "arguments 内容应可检索: {text}"
    );
    assert!(text.contains("正文"));

    // thinking 默认排除、verbose 纳入；image_url/base64 恒排除
    let line = serde_json::json!({
        "role": "assistant",
        "content": [
            {"type": "thinking", "thinking": "秘密思考", "signature": "sig"},
            {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}},
        ]
    });
    let mut out = Vec::new();
    harvest_text(&line, false, &mut out);
    let all = out.pop().map(|(_, t)| t).unwrap_or_default();
    // type 鉴别词（"thinking"/"image_url"）会被采入属预期噪声；
    // 关键是 thinking 内容与 base64 默认都不采。
    assert!(!all.contains("秘密思考"), "{all}");
    assert!(!all.contains("AAAA"), "{all}");
    let mut out = Vec::new();
    harvest_text(&line, true, &mut out);
    let all = out.pop().map(|(_, t)| t).unwrap_or_default();
    assert!(all.contains("秘密思考"));
    assert!(!all.contains("AAAA"));
}
