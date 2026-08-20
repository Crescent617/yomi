//! `yomi session cat` — read a session's JSONL message log.
//!
//! Default output is a friendly transcript: user/assistant text blocks, with
//! image blocks rendered as the real asset file path on disk. `--tools` adds
//! tool calls (name/args/result) to the transcript.
//! `--raw` dumps the JSONL file (large inline base64 payloads elided).

use crate::args::GlobalArgs;
use anyhow::{Context, Result};
use kernel::storage::MessageStore;
use kernel::types::{ContentBlock, Message, MessageId, Role, SessionMessage};
use kernel::utils::strs;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;
use tokio::io::AsyncWriteExt;

pub async fn run(
    global: &GlobalArgs,
    session: Option<String>,
    raw: bool,
    tools: bool,
    verbose: bool,
    line: Option<usize>,
    context: usize,
) -> Result<()> {
    let session_id = super::resolve_session_id(global, session).await?;
    let data_dir = crate::utils::data_dir(global)?;

    // Read the JSONL log directly — no sqlite / daemon needed, so this also
    // works when the db is locked or the daemon is down.
    let store = kernel::storage::JsonlMessageStore::new(data_dir.join("sessions"), &data_dir);
    let path = store.file_path(&session_id);
    if !path.exists() {
        anyhow::bail!("Session log not found: {}", path.display());
    }

    if raw {
        return dump_raw(&path).await;
    }

    // `--line <n>`：按 JSONL 行号取消息（`session search` 输出的坐标），
    // `--context <k>` 附带前后 k 行；section 头标行号。
    if let Some(n) = line {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let total = content.lines().count();
        anyhow::ensure!(n >= 1 && n <= total, "Line {n} out of range (1..={total})");
        let lo = n.saturating_sub(context).max(1);
        let hi = (n + context).min(total);
        // 全量解析再按行过滤：tool 配对靠跨消息索引，切片先转会让
        // "结果在窗内、调用在窗外"的 tool 消息整条消失。
        let mut parsed: Vec<(Option<usize>, Message)> = Vec::new();
        for (idx, raw_line) in content.lines().enumerate() {
            let line_no = idx + 1;
            match serde_json::from_str::<Message>(raw_line) {
                Ok(msg) => parsed.push((Some(line_no), msg)),
                Err(e) => tracing::warn!(line = line_no, error = %e, "skipping unparsable line"),
            }
        }
        let transcript =
            format_transcript_with_lines(parsed, &data_dir, tools, verbose, Some((lo, hi)));
        if transcript.is_empty() {
            let hint = if tools {
                ""
            } else {
                " (tool messages need --tools)"
            };
            println!("No displayable messages at L{lo}..L{hi} in session {session_id}{hint}");
        } else {
            print!("{transcript}");
        }
        return Ok(());
    }

    let messages = store.get(&session_id).await?;
    let transcript = format_transcript(messages, &data_dir, tools, verbose);
    if transcript.is_empty() {
        println!("No displayable messages found in session {session_id}");
    } else {
        print!("{transcript}");
    }
    Ok(())
}

async fn dump_raw(path: &Path) -> Result<()> {
    use tokio::io::AsyncBufReadExt;

    let file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("Failed to open {}", path.display()))?;
    let mut lines = tokio::io::BufReader::new(file).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines
        .next_line()
        .await
        .with_context(|| format!("Failed to read {}", path.display()))?
    {
        stdout.write_all(redact_base64(&line).as_bytes()).await?;
        stdout.write_all(b"\n").await?;
    }
    stdout.flush().await?;
    Ok(())
}

/// Elide large inline base64 payloads in data URLs:
/// `data:image/png;base64,<blob>` → `data:image/png;base64,[omitted:N]`.
/// Only data URLs are touched — plain text mentioning `;base64,` stays
/// verbatim. Small payloads (≤ 256 chars, e.g. 1x1 placeholders) are kept.
/// The payload runs to the closing quote of the JSON string (base64 never
/// contains `"`), so redacted lines stay valid JSON.
fn redact_base64(line: &str) -> String {
    const MAX_PAYLOAD: usize = 256;
    const MARKER: &str = ";base64,";
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(idx) = rest.find(MARKER) {
        // A data URL starts right after the opening quote of its JSON string.
        let seg_start = rest[..idx].rfind('"').map_or(0, |i| i + 1);
        let is_data_url = rest[seg_start..idx].starts_with("data:");
        let payload_start = idx + MARKER.len();
        out.push_str(&rest[..payload_start]);
        let after = &rest[payload_start..];
        let payload_len = after.find('"').unwrap_or(after.len());
        let payload = &after[..payload_len];
        if is_data_url && payload.len() > MAX_PAYLOAD {
            let _ = write!(out, "[omitted:{}]", payload.len());
        } else {
            out.push_str(payload);
        }
        rest = &after[payload_len..];
    }
    out.push_str(rest);
    out
}

/// Render a friendly transcript of user/assistant messages.
/// Tool calls are included only when `show_tools` is set; thinking blocks
/// only when `verbose` is set.
/// Pure function so it stays unit-testable.
fn format_transcript(
    messages: Vec<Message>,
    data_dir: &Path,
    show_tools: bool,
    verbose: bool,
) -> String {
    let wrapped = messages.into_iter().map(|m| (None, m)).collect();
    format_transcript_with_lines(wrapped, data_dir, show_tools, verbose, None)
}

/// `format_transcript` 的行号版：`(line_no, message)` 对，section 头标
/// `· L<n>`（`--line` 路径使用；line_no 为 None 时不标）。
/// `range` 给定时只渲染行号落在 `[lo, hi]` 内的消息（配对索引用全量，
/// 显示按窗口）。
fn format_transcript_with_lines(
    messages: Vec<(Option<usize>, Message)>,
    data_dir: &Path,
    show_tools: bool,
    verbose: bool,
    range: Option<(usize, usize)>,
) -> String {
    let mut line_of: HashMap<MessageId, usize> = HashMap::new();
    let raw: Vec<Message> = messages
        .into_iter()
        .map(|(line_no, m)| {
            if let Some(n) = line_no {
                line_of.insert(m.id.clone(), n);
            }
            m
        })
        .collect();
    let dangling = if show_tools {
        dangling_tool_calls(&raw)
    } else {
        HashMap::new()
    };
    // 批量转换（tool 配对靠跨消息索引，必须整批）；转换会丢弃 system/
    // internal 与坏损消息，所以行号按消息 id 回查，不靠位置对齐。
    let converted = SessionMessage::from_storage(raw);
    let mut out = String::new();
    let mut section =
        |label: &str, ts: chrono::DateTime<chrono::Utc>, line_no: Option<usize>, body: &str| {
            if body.trim().is_empty() {
                return;
            }
            let ts = ts.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S");
            let coord = line_no.map_or_else(String::new, |n| format!(" · L{n}"));
            let _ = writeln!(out, "=== {label} · {ts}{coord} ===\n{body}\n");
        };
    for msg in converted {
        let msg_line = match &msg {
            SessionMessage::User(m) => line_of.get(&m.id).copied(),
            SessionMessage::Steer(m) => line_of.get(&m.id).copied(),
            SessionMessage::Interrupted(m) => line_of.get(&m.id).copied(),
            SessionMessage::Assistant(m) => line_of.get(&m.id).copied(),
            SessionMessage::Tool(m) => line_of.get(&m.id).copied(),
        };
        if let (Some((lo, hi)), Some(l)) = (range, msg_line) {
            if l < lo || l > hi {
                continue;
            }
        }
        match msg {
            SessionMessage::User(m) => {
                section(
                    "user",
                    m.created_at,
                    msg_line,
                    &render_blocks(&m.content, data_dir, verbose),
                );
            }
            SessionMessage::Steer(m) => {
                section(
                    "user (steer)",
                    m.created_at,
                    msg_line,
                    &render_blocks(&m.content, data_dir, verbose),
                );
            }
            SessionMessage::Interrupted(m) => {
                section(
                    "interrupted",
                    m.created_at,
                    msg_line,
                    &render_blocks(&m.content, data_dir, verbose),
                );
            }
            SessionMessage::Assistant(m) => {
                section(
                    "assistant",
                    m.created_at,
                    msg_line,
                    &render_blocks(&m.content, data_dir, verbose),
                );
                // Tool calls without a result (cancel/crash mid-tool) have no
                // ToolMsg — surface them here or they'd vanish entirely.
                if let Some(missing) = dangling.get(&m.id) {
                    for (name, args) in missing {
                        let body = format!(
                            "args: {}\n(no result — interrupted?)",
                            strs::truncate_with_suffix(args, 300, "...")
                        );
                        section(&format!("tool · {name}"), m.created_at, msg_line, &body);
                    }
                }
            }
            SessionMessage::Tool(m) => {
                if !show_tools {
                    continue;
                }
                let mut body = format!("args: {}", strs::truncate_with_suffix(&m.args, 300, "..."));
                let result = render_blocks(&m.result, data_dir, verbose);
                if !result.trim().is_empty() {
                    let _ = write!(
                        body,
                        "\n{}",
                        strs::truncate_with_suffix(&result, 4000, "\n...[truncated]")
                    );
                }
                section(&format!("tool · {}", m.name), m.created_at, msg_line, &body);
            }
        }
    }
    out
}

/// Tool calls whose result message never landed (cancel/crash mid-tool),
/// keyed by the assistant message id. `from_storage` drops these entirely.
fn dangling_tool_calls(messages: &[Message]) -> HashMap<MessageId, Vec<(String, String)>> {
    let answered: HashSet<&str> = messages
        .iter()
        .filter_map(|m| m.tool_call_id.as_deref())
        .collect();
    let mut dangling: HashMap<MessageId, Vec<(String, String)>> = HashMap::new();
    for m in messages {
        if m.role != Role::Assistant {
            continue;
        }
        let Some(calls) = &m.tool_calls else {
            continue;
        };
        let missing: Vec<(String, String)> = calls
            .iter()
            .filter(|c| !answered.contains(c.id.as_str()))
            .map(|c| (c.name.clone(), c.arguments.to_string()))
            .collect();
        if !missing.is_empty() {
            dangling.insert(m.id.clone(), missing);
        }
    }
    dangling
}

/// Render content blocks for the transcript: text as-is, images as paths.
/// thinking / redacted thinking / audio are skipped (text-only transcript).
fn render_blocks(blocks: &[ContentBlock], data_dir: &Path, verbose: bool) -> String {
    let mut body = String::new();
    for block in blocks {
        match block {
            ContentBlock::Text { text } => {
                if !body.is_empty() {
                    body.push('\n');
                }
                body.push_str(text);
            }
            ContentBlock::ImageUrl { image_url } => {
                if !body.is_empty() {
                    body.push('\n');
                }
                let _ = write!(body, "[image: {}]", image_display(&image_url.url, data_dir));
            }
            ContentBlock::Thinking { thinking, .. } if verbose => {
                if !body.is_empty() {
                    body.push('\n');
                }
                let _ = write!(body, "[thinking]\n{thinking}");
            }
            _ => {}
        }
    }
    body
}

/// How an image block should appear in the transcript: `asset://` resolves to
/// the real file path under `{data_dir}/assets/`; anything else prints as-is
/// (inline base64 is summarized to avoid flooding the terminal).
fn image_display(url: &str, data_dir: &Path) -> String {
    if let Some(path) = kernel::utils::asset::asset_path(url, data_dir) {
        return path.display().to_string();
    }
    if url.starts_with("data:") {
        return "(inline base64 image)".to_string();
    }
    url.to_string()
}

#[cfg(test)]
#[path = "cat_test.rs"]
mod tests;
