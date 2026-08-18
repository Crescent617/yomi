//! Feishu message/card text extraction and rewriting (pure parsing layer).

use serde_json::json;

/// Legacy-rendered echo of a schema 2.0 card (real content unavailable).
pub(crate) const UPGRADE_CLIENT_NOTICE: &str = "请升级至最新版本客户端，以查看内容";

use super::feishu::FeishuAdapter;

impl FeishuAdapter {
    /// Extract display text and image keys from a history item in one
    /// pass: text messages get their content, posts get concatenated text
    /// runs, everything else becomes a `[msg_type]` placeholder; image
    /// keys come from `image` message bodies and post `img` runs.
    pub(crate) fn extract_history_content(item: &serde_json::Value) -> (String, Vec<String>) {
        let msg_type = item["msg_type"].as_str().unwrap_or("unknown");
        let content: serde_json::Value = item["body"]["content"]
            .as_str()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
        let text = match msg_type {
            "text" => content["text"].as_str().unwrap_or("").to_string(),
            "post" => {
                let text = Self::extract_post_text(&content);
                if text.is_empty() {
                    "[post]".to_string()
                } else {
                    text
                }
            }
            // Bot replies are cards — quoting one must yield its markdown
            // body, not a bare placeholder.
            "interactive" => {
                let text = Self::extract_card_text(&content);
                if text.is_empty() {
                    "[interactive]".to_string()
                } else {
                    text
                }
            }
            other => format!("[{other}]"),
        };
        let image_keys = match msg_type {
            "image" => content["image_key"]
                .as_str()
                .map(|k| vec![k.to_string()])
                .unwrap_or_default(),
            "post" => Self::extract_post_image_keys(&content),
            _ => Vec::new(),
        };
        (text, image_keys)
    }

    /// Locate the post body node: the first known locale with a content
    /// array, else the content itself (bare `{title, content}` form).
    pub(crate) fn post_node(content: &serde_json::Value) -> &serde_json::Value {
        ["zh_cn", "en_us", "ja_jp"]
            .iter()
            .map(|k| &content[*k])
            .find(|n| n["content"].is_array())
            .unwrap_or(content)
    }

    /// Extract readable text from a card (interactive) message body.
    /// Two shapes: the sent card JSON (markdown elements — schema 2.0
    /// `body.elements` or legacy v1 top-level `elements`), and the
    /// get-message API echo (legacy-rendered paragraphs of text runs —
    /// v1 cards keep their real text there). With `card_msg_content_type`
    /// the API echoes the real schema 2.0 body; without it the echo degrades
    /// to the "upgrade client" notice, which must not leak into context.
    /// The header title counts as readable text: for yomi's own status
    /// cards it is the *only* transient signal — the live body rides
    /// inside a collapsible panel (stripped here), so a running card
    /// reads as e.g. "🐾 Typing…" instead of nothing.
    pub(crate) fn extract_card_text(content: &serde_json::Value) -> String {
        let title = content["header"]["title"]["content"]
            .as_str()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or("");
        let with_title = |body: String| {
            if title.is_empty() {
                body
            } else if body.is_empty() {
                title.to_string()
            } else {
                format!("{title}\n{body}")
            }
        };
        let from_markdown = content["body"]["elements"]
            .as_array()
            .or_else(|| content["elements"].as_array())
            .map(|els| {
                els.iter()
                    .filter(|e| e["tag"].as_str() == Some("markdown"))
                    .filter_map(|e| e["content"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        if !from_markdown.is_empty() {
            return with_title(rewrite_card_at_tags(&from_markdown));
        }
        let from_runs = content["elements"]
            .as_array()
            .map(|paras| {
                paras
                    .iter()
                    .map(|para| {
                        para.as_array()
                            .map(|runs| {
                                runs.iter()
                                    .filter_map(|r| r["text"].as_str())
                                    .filter(|t| *t != UPGRADE_CLIENT_NOTICE)
                                    .collect::<String>()
                            })
                            .unwrap_or_default()
                    })
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        with_title(from_runs)
    }

    /// Concatenate a post message's title and paragraph text runs (posts
    /// in other locales degrade to `[post]`).
    pub(crate) fn extract_post_text(content: &serde_json::Value) -> String {
        let node = Self::post_node(content);
        let mut parts = Vec::new();
        if let Some(title) = node["title"]
            .as_str()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            parts.push(title.to_string());
        }
        if let Some(paragraphs) = node["content"].as_array() {
            for para in paragraphs {
                let line: String = para
                    .as_array()
                    .map(|runs| {
                        runs.iter()
                            .filter_map(|r| r["text"].as_str())
                            .collect::<String>()
                    })
                    .unwrap_or_default();
                if !line.is_empty() {
                    parts.push(line);
                }
            }
        }
        parts.join("\n")
    }

    /// Collect the `image_key`s of a post's `img` runs, in paragraph order.
    pub(crate) fn extract_post_image_keys(content: &serde_json::Value) -> Vec<String> {
        Self::post_node(content)["content"]
            .as_array()
            .map(|paras| {
                paras
                    .iter()
                    .flat_map(|p| p.as_array().into_iter().flatten())
                    .filter(|r| r["tag"].as_str() == Some("img"))
                    .filter_map(|r| r["image_key"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Extract display text from a comment reply's content elements:
    /// text runs concatenated, docs links as their URL, @-mentions as
    /// `@bot` (the bot itself) or `@user:{open_id}`.
    pub(crate) fn extract_reply_text(
        elements: Option<&Vec<serde_json::Value>>,
        bot_open_id: Option<&str>,
    ) -> String {
        let Some(elements) = elements else {
            return String::new();
        };
        elements
            .iter()
            .map(|e| match e["type"].as_str() {
                Some("text_run") => e["text_run"]["text"].as_str().unwrap_or("").to_string(),
                Some("docs_link") => e["docs_link"]["url"].as_str().unwrap_or("").to_string(),
                Some("person") => {
                    let uid = e["person"]["user_id"].as_str().unwrap_or("");
                    if !uid.is_empty() && Some(uid) == bot_open_id {
                        "@bot".to_string()
                    } else {
                        format!("@user:{uid}")
                    }
                }
                _ => String::new(),
            })
            .collect()
    }

    /// Feishu `create_time` is in milliseconds, but some v1.x events may be in
    /// seconds or microseconds. Normalise to seconds and format.
    pub(crate) fn parse_feishu_timestamp(value: &serde_json::Value) -> String {
        let ts = value
            .as_str()
            .and_then(|s| s.parse::<i64>().ok())
            .or_else(|| value.as_i64())
            .unwrap_or_else(|| chrono::Local::now().timestamp());

        let dt = if ts < 10_000_000_000 {
            chrono::DateTime::from_timestamp(ts, 0)
        } else if ts < 10_000_000_000_000 {
            chrono::DateTime::from_timestamp_millis(ts)
        } else {
            chrono::DateTime::from_timestamp_millis(ts / 1000)
        };
        dt.map_or_else(
            || chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            |dt| dt.format("%Y-%m-%d %H:%M:%S").to_string(),
        )
    }

    pub(crate) fn build_card(text: &str) -> String {
        // Platform-neutral `<@USER_ID>` contract → feishu <at> syntax.
        let text = super::utils::rewrite_mentions(text, &|id| format!("<at id={id}></at>"));
        json!({
            "schema": "2.0",
            "body": {
                "elements": [{ "tag": "markdown", "content": text }]
            }
        })
        .to_string()
    }
}

pub(crate) fn strip_bot_mention(
    text: &str,
    mentions: Option<&Vec<serde_json::Value>>,
    bot_open_id: Option<&str>,
) -> String {
    let Some(bot_open_id) = bot_open_id else {
        return text.trim().to_string();
    };
    mentions
        .into_iter()
        .flatten()
        .filter(|mention| mention["id"]["open_id"].as_str() == Some(bot_open_id))
        .filter_map(|mention| mention["key"].as_str())
        .fold(text.to_string(), |text, key| text.replace(key, ""))
        .trim()
        .to_string()
}

/// Rewrite a card's native `<at id=ou_x>name</at>` mention tags into the
/// platform-neutral `<@ou_x>name` contract (see the agent prompt's Mentions
/// section). The get-message API normalizes the tag to `id=` and drops the
/// display name, so `<at id=ou_x></at>` degrades to bare `<@ou_x>`. Fenced
/// code blocks and inline code spans are left untouched (shared walker) so
/// an example shown literally stays literal. The id attribute tolerates
/// optional quotes.
pub(crate) fn rewrite_card_at_tags(text: &str) -> String {
    super::utils::map_outside_code_spans(text, &mut |segment, out| {
        rewrite_at_tag_segment(segment, out);
    })
}

pub(crate) fn rewrite_at_tag_segment(segment: &str, out: &mut String) {
    use std::fmt::Write as _;
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    // `<at id=ou_x>name</at>` or `<at id="ou_x">name</at>` — id bounded like
    // the neutral contract (`{1,64}`); the name is any non-`<` text.
    let re = RE.get_or_init(|| {
        regex::Regex::new(
            r#"<at\s+id=(?:"([A-Za-z0-9_\-]{1,64})"|([A-Za-z0-9_\-]{1,64}))>([^<]*)</at>"#,
        )
        .unwrap()
    });
    let mut last = 0;
    for cap in re.captures_iter(segment) {
        let m = cap.get(0).unwrap();
        out.push_str(&segment[last..m.start()]);
        let id = cap.get(1).or_else(|| cap.get(2)).map_or("", |g| g.as_str());
        let name = cap.get(3).map_or("", |g| g.as_str());
        let _ = write!(out, "<@{id}>{name}");
        last = m.end();
    }
    out.push_str(&segment[last..]);
}
