//! `yomi session search <query>` — 会话历史全文检索（含工具文本），
//! 按会话分组输出带**行号**的命中片段，最近活跃的排前面；行号直接喂给
//! `yomi session cat --line <n>` 取全文。
//!
//! 性能：先用 `rg -ilF` 预筛命中文件（yomi 本就依赖 ripgrep），再只对
//! 候选文件逐行做**零分配** ASCII 大小写不敏感匹配——不要对全量语料
//! 做 `to_lowercase()`（近百 MB 语料含 MB 级 base64 图片行，分配即慢）。

use crate::args::GlobalArgs;
use anyhow::{Context, Result};
use std::io::BufRead;

/// 单个命中片段。
struct Hit {
    line: usize,
    role: String,
    snippet: String,
}

struct SessionHits {
    session_id: String,
    hits: Vec<Hit>,
    total: usize,
}

/// 零分配大小写不敏感子串匹配。`needle_lower` 必须已小写。
/// 纯 ASCII needle 走逐字节比较；含非 ASCII 时（如中文，无大小写概念）
/// 先直接 contains，命中不了再退回分配式小写化兜底（罕见路径）。
fn contains_ci(haystack: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    if !needle_lower.is_ascii() {
        return haystack.contains(needle_lower) || haystack.to_lowercase().contains(needle_lower);
    }
    let h = haystack.as_bytes();
    let n = needle_lower.as_bytes();
    if h.len() < n.len() {
        return false;
    }
    'outer: for w in 0..=h.len() - n.len() {
        for (i, &nb) in n.iter().enumerate() {
            if h[w + i].to_ascii_lowercase() != nb {
                continue 'outer;
            }
        }
        return true;
    }
    false
}

/// 大小写不敏感计数（非重叠匹配，对齐 `str::matches` 语义）。
/// 纯字节比较、不做任何切片——中文等多字节字符不会切出非法边界。
fn count_ci(haystack: &str, needle_lower: &str) -> usize {
    if needle_lower.is_empty() {
        return 0;
    }
    if !needle_lower.is_ascii() {
        return haystack.to_lowercase().matches(needle_lower).count();
    }
    let h = haystack.as_bytes();
    let n = needle_lower.as_bytes();
    let mut count = 0;
    let mut pos = 0;
    while pos + n.len() <= h.len() {
        if n.iter()
            .enumerate()
            .all(|(i, &b)| h[pos + i].to_ascii_lowercase() == b)
        {
            count += 1;
            // needle 纯 ASCII，命中区必为单字节字符，+n 后仍在边界上。
            pos += n.len();
        } else {
            pos += 1;
        }
    }
    count
}

/// `rg -iF --json` 一遍拿回全部命中（路径 + 行号 + 行内容）——行内容
/// 直接用于 JSONL 解析与片段提取，不再重读文件。rg 不可用/失败时返回
/// None，调用方退回逐行扫描的慢路径。
struct RgMatch {
    path: std::path::PathBuf,
    line: usize,
    text: String,
}

fn rg_matches(query: &str, sessions_dir: &std::path::Path) -> Option<Vec<RgMatch>> {
    let out = std::process::Command::new("rg")
        .args([
            "-iF",
            "--json",
            "--glob",
            "*.jsonl",
            "--",
            query,
            &sessions_dir.to_string_lossy(),
        ])
        .output()
        .ok()?;
    // rg: 0=有命中, 1=无命中（合法，空列表）, 其他=错误（回退慢路径）
    if !out.status.success() && out.status.code() != Some(1) {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut matches = Vec::new();
    for line in stdout.lines() {
        let Ok(ev) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if ev["type"] != "match" {
            continue;
        }
        let data = &ev["data"];
        let (Some(path), Some(line_no), Some(text)) = (
            data["path"]["text"].as_str(),
            data["line_number"].as_u64(),
            data["lines"]["text"].as_str(),
        ) else {
            continue;
        };
        matches.push(RgMatch {
            path: std::path::PathBuf::from(path),
            line: line_no as usize,
            text: text.trim_end_matches('\n').to_string(),
        });
    }
    Some(matches)
}

/// 折叠空白并截取 match 前后各 ±ctx 字符的片段（char 安全，仅命中行调
/// 用，分配可接受）。
fn snippet(text: &str, needle_lower: &str, ctx: usize) -> Option<String> {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let flat_lower = flat.to_lowercase();
    let byte_pos = flat_lower.find(needle_lower)?;
    let chars: Vec<char> = flat.chars().collect();
    let approx = flat_lower[..byte_pos].chars().count();
    let start = approx.saturating_sub(ctx);
    let end = (approx + needle_lower.chars().count() + ctx).min(chars.len());
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(chars[start..end].iter());
    if end < chars.len() {
        out.push('…');
    }
    Some(out)
}

/// 从一行 JSONL 中提取全部可搜索文本（user/assistant 消息的 text 块、
/// 工具调用的 name/args、工具结果的 text）。
fn harvest_text(line: &serde_json::Value, verbose: bool, out: &mut Vec<(String, String)>) {
    let role = line["role"].as_str().unwrap_or("?").to_string();
    let mut buf = String::new();
    harvest_strs(&line["content"], verbose, &mut buf);
    harvest_strs(&line["tool_calls"], verbose, &mut buf);
    if !buf.trim().is_empty() {
        out.push((role, buf));
    }
}

fn harvest_strs(v: &serde_json::Value, verbose: bool, buf: &mut String) {
    match v {
        serde_json::Value::String(s) => {
            buf.push_str(s);
            buf.push('\n');
        }
        serde_json::Value::Array(items) => {
            for item in items {
                harvest_strs(item, verbose, buf);
            }
        }
        serde_json::Value::Object(map) => {
            // 只采文本类字段，跳过 base64 图片等重负载；thinking 默认不采
            // （--verbose 才纳入，与 cat 的口径一致）。
            let mut keys: Vec<&str> = vec!["text", "name", "arguments", "input"];
            if verbose {
                keys.push("thinking");
            }
            for key in keys {
                if let Some(val) = map.get(key) {
                    harvest_strs(val, verbose, buf);
                }
            }
        }
        _ => {}
    }
}

fn search_file(
    path: &std::path::Path,
    needle_lower: &str,
    max_snippets: usize,
    verbose: bool,
) -> SessionHits {
    let session_id = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut acc = SessionHits {
        session_id,
        hits: Vec::new(),
        total: 0,
    };
    let Ok(file) = std::fs::File::open(path) else {
        return acc;
    };
    for (idx, line) in std::io::BufReader::new(file).lines().enumerate() {
        let Ok(line) = line else { continue };
        if !contains_ci(&line, needle_lower) {
            continue; // 行级粗筛（零分配），省下 JSON 解析
        }
        process_line(
            &mut acc,
            idx + 1,
            &line,
            needle_lower,
            max_snippets,
            verbose,
        );
    }
    acc
}

/// 一条命中行的处理：解析 JSONL → 提取文本 → 计数/片段/行号。
fn process_line(
    acc: &mut SessionHits,
    line_no: usize,
    raw: &str,
    needle_lower: &str,
    max_snippets: usize,
    verbose: bool,
) {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(raw) else {
        return;
    };
    let mut texts = Vec::new();
    harvest_text(&json, verbose, &mut texts);
    for (role, text) in texts {
        if !contains_ci(&text, needle_lower) {
            continue;
        }
        acc.total += count_ci(&text, needle_lower);
        if acc.hits.len() < max_snippets {
            if let Some(s) = snippet(&text, needle_lower, 60) {
                // 同一片段（不同工具调用复读同一内容）只留一条。
                if acc.hits.iter().all(|h: &Hit| h.snippet != s) {
                    acc.hits.push(Hit {
                        line: line_no,
                        role,
                        snippet: s,
                    });
                }
            }
        }
    }
}

pub async fn run(
    global: &GlobalArgs,
    query: &str,
    session: Option<String>,
    limit: usize,
    snippets: usize,
    json_out: bool,
    verbose: bool,
) -> Result<()> {
    let data_dir = crate::utils::data_dir(global)?;
    let sessions_dir = data_dir.join("sessions");
    if !sessions_dir.is_dir() {
        println!("No sessions directory at {}", sessions_dir.display());
        return Ok(());
    }
    let needle_lower = query.to_lowercase();

    let mut results: Vec<SessionHits> = Vec::new();
    let mut scoped_files: Option<Vec<std::path::PathBuf>> = None;
    if let Some(sid) = &session {
        let safe = sid.replace(['/', '\\'], "_");
        let p = sessions_dir.join(format!("{safe}.jsonl"));
        scoped_files = Some(if p.exists() { vec![p] } else { vec![] });
    }

    // 快路径：rg 一遍拿回全部命中行；慢路径（rg 不可用或单文件 scope）
    // 逐文件扫描。
    let rg_result = if scoped_files.is_none() {
        rg_matches(query, &sessions_dir)
    } else {
        None
    };
    match rg_result {
        Some(matches) => {
            let mut by_file: std::collections::HashMap<std::path::PathBuf, SessionHits> =
                std::collections::HashMap::new();
            for m in matches {
                let acc = by_file
                    .entry(m.path.clone())
                    .or_insert_with(|| SessionHits {
                        session_id: m
                            .path
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default(),
                        hits: Vec::new(),
                        total: 0,
                    });
                process_line(acc, m.line, &m.text, &needle_lower, snippets, verbose);
            }
            results = by_file.into_values().filter(|r| r.total > 0).collect();
        }
        None => {
            let files = scoped_files.unwrap_or_else(|| {
                std::fs::read_dir(&sessions_dir)
                    .map(|rd| {
                        rd.filter_map(|e| e.ok())
                            .map(|e| e.path())
                            .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
                            .collect()
                    })
                    .unwrap_or_default()
            });
            for path in &files {
                let hits = search_file(path, &needle_lower, snippets, verbose);
                if hits.total > 0 {
                    results.push(hits);
                }
            }
        }
    }

    if results.is_empty() {
        println!("No matches for \"{query}\".");
        return Ok(());
    }

    // 标题与活跃度（best-effort；storage 打不开也不影响检索）。
    let storage = crate::utils::open_storage(global).await.ok();
    let mut meta = std::collections::HashMap::new();
    if let Some(storage) = &storage {
        if let Ok((sessions, _)) = storage
            .session_store()
            .list(
                None,
                kernel::storage::session::SessionListScope::All,
                None,
                1000,
            )
            .await
        {
            for s in sessions {
                meta.insert(s.id.0.to_string(), (s.title, s.updated_at));
            }
        }
    }
    results.sort_by_key(|r| {
        meta.get(&r.session_id)
            .map(|(_, ts)| *ts)
            .unwrap_or_default()
    });
    results.reverse();
    results.truncate(limit);

    if json_out {
        let out: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                serde_json::json!({
                    "session_id": r.session_id,
                    "title": meta.get(&r.session_id).and_then(|(t, _)| t.clone()),
                    "matches": r.total,
                    "snippets": r.hits.iter().map(|h| serde_json::json!({
                        "line": h.line, "role": h.role, "snippet": h.snippet,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    for r in &results {
        let (title, age) = meta
            .get(&r.session_id)
            .map(|(t, ts)| {
                let title = t.clone().unwrap_or_else(|| "(no title)".to_string());
                let secs = (chrono::Utc::now() - *ts).num_seconds().max(0);
                let age = if secs < 3600 {
                    format!("{}m", secs / 60)
                } else if secs < 86400 {
                    format!("{}h", secs / 3600)
                } else {
                    format!("{}d", secs / 86400)
                };
                (title, age)
            })
            .unwrap_or_else(|| ("(unknown session)".to_string(), "?".to_string()));
        let title: String = title.chars().take(60).collect();
        println!(
            "{}  {:<6} {:<60} ({} match{})",
            r.session_id,
            age,
            title,
            r.total,
            if r.total == 1 { "" } else { "es" }
        );
        for h in &r.hits {
            println!("    L{:<6} [{}] {}", h.line, h.role, h.snippet);
        }
        println!();
    }
    Ok(())
}

/// 供 main.rs 路由调用。
pub async fn run_cli(args: &SearchArgs) -> Result<()> {
    run(
        &args.global,
        &args.query,
        args.session.clone(),
        args.limit,
        args.snippets,
        args.json,
        args.verbose,
    )
    .await
    .with_context(|| "session search failed".to_string())
}

#[derive(clap::Parser)]
pub struct SearchArgs {
    #[command(flatten)]
    pub global: GlobalArgs,

    /// 搜索词（大小写不敏感的子串匹配）
    pub query: String,

    /// 只搜指定 session
    #[arg(short, long)]
    pub session: Option<String>,

    /// 最多显示几个 session
    #[arg(long, default_value = "10")]
    pub limit: usize,

    /// 每个 session 最多显示几条片段
    #[arg(long, default_value = "3")]
    pub snippets: usize,

    /// JSON 输出
    #[arg(long)]
    pub json: bool,

    /// thinking 内容也纳入检索（默认排除，与 cat 口径一致）
    #[arg(long)]
    pub verbose: bool,
}
