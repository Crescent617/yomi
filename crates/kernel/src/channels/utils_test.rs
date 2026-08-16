use super::rewrite_mentions;

fn feishu_render(id: &str) -> String {
    format!("<at id={id}></at>")
}

#[test]
fn rewrites_single_mention() {
    let out = rewrite_mentions("cc <@ou_abc123>", &feishu_render);
    assert_eq!(out, "cc <at id=ou_abc123></at>");
}

#[test]
fn rewrites_multiple_mentions() {
    let out = rewrite_mentions("<@ou_a> 和 <@12345> 看一下", &feishu_render);
    assert_eq!(out, "<at id=ou_a></at> 和 <at id=12345></at> 看一下");
}

#[test]
fn skips_fenced_code_blocks() {
    let text = "之前 <@ou_a>\n```\n<@ou_b>\n```\n之后 <@ou_c>";
    let out = rewrite_mentions(text, &feishu_render);
    assert_eq!(
        out,
        "之前 <at id=ou_a></at>\n```\n<@ou_b>\n```\n之后 <at id=ou_c></at>"
    );
}

#[test]
fn skips_indented_fence_marker() {
    let text = "  ```rust\n  let s = \"<@ou_a>\";\n  ```\n<@ou_b>";
    let out = rewrite_mentions(text, &feishu_render);
    assert!(out.contains("let s = \"<@ou_a>\";"));
    assert!(out.ends_with("<at id=ou_b></at>"));
}

#[test]
fn skips_inline_code_spans() {
    let out = rewrite_mentions("语法是 `<@ou_a>`，但 <@ou_b> 是真的", &feishu_render);
    assert_eq!(out, "语法是 `<@ou_a>`，但 <at id=ou_b></at> 是真的");
}

#[test]
fn rejects_overlong_ids() {
    // 65 chars > 64 cap → left as-is (兜底长度限制)
    let long = "a".repeat(65);
    let text = format!("<@{long}>");
    let out = rewrite_mentions(&text, &feishu_render);
    assert_eq!(out, text);

    // exactly 64 still matches
    let max = "a".repeat(64);
    let out = rewrite_mentions(&format!("<@{max}>"), &feishu_render);
    assert_eq!(out, format!("<at id={max}></at>"));
}

#[test]
fn rejects_malformed_mentions() {
    for text in ["<@>", "<@ ou_a>", "<@ou_a", "<@ou_a!>", "< @ou_a>"] {
        assert_eq!(rewrite_mentions(text, &feishu_render), text, "{text}");
    }
}

#[test]
fn empty_and_plain_text_unchanged() {
    assert_eq!(rewrite_mentions("", &feishu_render), "");
    let text = "没有任何提及的回复。";
    assert_eq!(rewrite_mentions(text, &feishu_render), text);
}

#[test]
fn multiline_outside_fences_all_rewritten() {
    let text = "第一行 <@ou_a>\n第二行 <@ou_b>\n";
    let out = rewrite_mentions(text, &feishu_render);
    assert_eq!(out, "第一行 <at id=ou_a></at>\n第二行 <at id=ou_b></at>\n");
}
