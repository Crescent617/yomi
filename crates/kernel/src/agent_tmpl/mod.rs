//! Agent templates（`<name>/ROLE.md`）：subagent 的角色定义资产。
//!
//! 纯 markdown：全文即角色系统提示，名字取自目录名，无 frontmatter。
//! 三层合并：内置（`include_str!`，地板层）→ 全局 `<data_dir>/agents/`
//! → workspace `<cwd>/.yomi/agents/`（最高层），同名后者覆盖前者。
//!
//! 位置说明：模板正文是 yomi 的提示词约定，与别家 subagent 格式互不兼容，
//! 所以放 yomi 私有的 `.yomi/agents/` 而非跨厂商的 `.agents/`——
//! 共享目录的前提是共享格式。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// workspace 模板目录（相对 `working_dir`）。
pub const WORKSPACE_DIR: &str = ".yomi/agents";
/// 全局模板目录（相对 `data_dir`）。
pub const GLOBAL_DIR: &str = "agents";
/// 模板主文件名。
pub const ROLE_FILE: &str = "ROLE.md";

/// 模板来源层。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateSource {
    Builtin,
    Global,
    Workspace,
}

impl TemplateSource {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Global => "global",
            Self::Workspace => "workspace",
        }
    }
}

/// 一个角色模板：名字 + 正文（即 subagent 的系统提示）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTemplate {
    pub name: String,
    pub body: String,
    pub source: TemplateSource,
}

/// 可写层（builtin 随二进制发布，只读）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateScope {
    Global,
    Workspace,
}

fn parse(name: &str, content: &str, source: TemplateSource) -> AgentTemplate {
    let body = content.trim().to_string();
    if body.is_empty() {
        tracing::warn!("agent template '{name}' has empty body");
    }
    AgentTemplate {
        name: name.to_string(),
        body,
        source,
    }
}

/// 内置模板（官方地板层）。文件存于本模块目录下，随二进制版本对齐。
const BUILTIN: &[(&str, &str)] = &[
    ("planner", include_str!("planner/ROLE.md")),
    ("verifier", include_str!("verifier/ROLE.md")),
    ("explorer", include_str!("explorer/ROLE.md")),
    ("reviewer", include_str!("reviewer/ROLE.md")),
];

/// 内置模板清单。
pub fn builtin() -> Vec<AgentTemplate> {
    BUILTIN
        .iter()
        .map(|(name, content)| parse(name, content, TemplateSource::Builtin))
        .collect()
}

/// 全局模板目录（`data_dir` 下的 `agents/`）。
pub fn global_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(GLOBAL_DIR)
}

/// 扫描一个模板目录（`<dir>/<name>/ROLE.md`，一层），符号链接跟随。
/// 目录不存在或条目损坏均跳过——资产加载永远 best-effort。
async fn load_dir(dir: &Path, source: TemplateSource) -> Vec<AgentTemplate> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut templates = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let role = entry.path().join(ROLE_FILE);
        let Ok(content) = tokio::fs::read_to_string(&role).await else {
            continue;
        };
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        templates.push(parse(&name, &content, source));
    }
    templates
}

/// 同名后者覆盖前者，按 name 排序输出（稳定）。
fn merge(layers: Vec<Vec<AgentTemplate>>) -> Vec<AgentTemplate> {
    let mut merged: Vec<AgentTemplate> = Vec::new();
    for layer in layers {
        for t in layer {
            match merged.iter_mut().find(|m| m.name == t.name) {
                Some(existing) => *existing = t,
                None => merged.push(t),
            }
        }
    }
    merged.sort_by(|a, b| a.name.cmp(&b.name));
    merged
}

/// 三层合并后的模板清单：builtin → 全局 → workspace。
pub async fn list(global_dir: &Path, working_dir: Option<&Path>) -> Vec<AgentTemplate> {
    let workspace_dir = working_dir.map(|d| d.join(WORKSPACE_DIR));
    let global = load_dir(global_dir, TemplateSource::Global).await;
    let workspace = match &workspace_dir {
        Some(dir) => load_dir(dir, TemplateSource::Workspace).await,
        None => Vec::new(),
    };
    merge(vec![builtin(), global, workspace])
}

/// 按名解析（实时读盘，spawn/调用时的 ground truth）。
pub async fn resolve(
    name: &str,
    global_dir: &Path,
    working_dir: Option<&Path>,
) -> Option<AgentTemplate> {
    list(global_dir, working_dir)
        .await
        .into_iter()
        .find(|t| t.name == name)
}

/// 校验模板名：`^[a-z0-9][a-z0-9-]{0,63}$`。同时杜绝路径穿越。
pub fn validate_name(name: &str) -> Result<(), String> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "invalid template name '{name}': expected ^[a-z0-9][a-z0-9-]{{0,63}}$"
        ))
    }
}

fn invalid_input(msg: String) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, msg)
}

/// 写入 `<root>/<name>/ROLE.md`（自动建目录），返回文件路径。
pub async fn save(root: &Path, name: &str, body: &str) -> std::io::Result<PathBuf> {
    validate_name(name).map_err(invalid_input)?;
    if body.trim().is_empty() {
        return Err(invalid_input(format!(
            "template '{name}' body must not be empty"
        )));
    }
    let dir = root.join(name);
    tokio::fs::create_dir_all(&dir).await?;
    let file = dir.join(ROLE_FILE);
    tokio::fs::write(&file, body).await?;
    Ok(file)
}

/// 删除 `<root>/<name>/` 目录（builtin 不在磁盘上，天然不可达）。
pub async fn delete(root: &Path, name: &str) -> std::io::Result<()> {
    validate_name(name).map_err(invalid_input)?;
    tokio::fs::remove_dir_all(root.join(name))
        .await
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                invalid_input(format!("template '{name}' does not exist at this scope"))
            }
            _ => e,
        })
}

/// 错误信息用的可用模板概览：`planner (builtin), my-role (workspace), ...`
pub async fn available_summary(global_dir: &Path, working_dir: Option<&Path>) -> String {
    list(global_dir, working_dir)
        .await
        .iter()
        .map(|t| format!("{} ({})", t.name, t.source.label()))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
#[path = "agent_tmpl_test.rs"]
mod tests;
