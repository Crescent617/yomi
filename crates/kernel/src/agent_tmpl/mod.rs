//! Agent templates（`<name>/ROLE.md`）：subagent 的角色定义资产。
//!
//! 三层合并：内置（`include_str!`，地板层）→ 全局 `<data_dir>/agents/`
//! → workspace `<cwd>/.yomi/agents/`（最高层），同名后者覆盖前者。
//! 模板只收敛不扩权：`tools_block` 只会追加进 blocklist。
//!
//! 位置说明：模板 frontmatter 是 yomi 方言（`tools_block`/`model_key`/`skills`），
//! 与别家 subagent 格式互不兼容，所以放 yomi 私有的 `.yomi/agents/` 而非
//! 跨厂商的 `.agents/`——共享目录的前提是共享格式。

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// workspace 模板目录（相对 `working_dir`）。
pub const WORKSPACE_DIR: &str = ".yomi/agents";
/// 全局模板目录（相对 `data_dir`）。
pub const GLOBAL_DIR: &str = "agents";
/// 模板主文件名。
pub const ROLE_FILE: &str = "ROLE.md";

/// 全局模板目录（`data_dir` 下的 `agents/`）。
pub fn global_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(GLOBAL_DIR)
}

/// 模板来源层。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// 一个解析完毕的角色模板。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTemplate {
    pub name: String,
    pub description: String,
    /// 追加进 subagent 工具 blocklist 的工具名（只能收窄父 agent 的工具集）。
    pub tools_block: Vec<String>,
    /// 角色系统提示（frontmatter 之后的正文）。
    pub body: String,
    pub source: TemplateSource,
}

#[derive(Debug, Default, Deserialize)]
struct Frontmatter {
    #[serde(default)]
    description: String,
    #[serde(default)]
    tools_block: Vec<String>,
    // model_key / skills 等未知字段刻意容忍（解析忽略）——全继承是当前的
    // 有意简化，将来引入时不破坏存量文件。
}

/// 解析 `<name>/ROLE.md` 内容：YAML frontmatter + 正文。
/// 无 frontmatter 时整体作为 body、元数据全默认；frontmatter 未闭合同样
/// 按整体正文处理——宁可降级也不产出空 body 的静默残次模板。
fn parse(name: &str, content: &str, source: TemplateSource) -> AgentTemplate {
    let mut lines = content.lines();
    let (fm, body) = if lines.next().map(str::trim) == Some("---") {
        let mut yaml = String::new();
        let mut rest = String::new();
        let mut closed = false;
        for line in lines {
            if !closed && line.trim() == "---" {
                closed = true;
                continue;
            }
            if closed {
                rest.push_str(line);
                rest.push('\n');
            } else {
                yaml.push_str(line);
                yaml.push('\n');
            }
        }
        if closed {
            (
                serde_yaml::from_str(&yaml).unwrap_or_else(|e| {
                    // 闭合法定但 YAML 非法：默认化会静默清空 tools_block（放宽
                    // 工具集）——必须留痕。
                    tracing::warn!("agent template '{name}' frontmatter parse failed: {e}");
                    Frontmatter::default()
                }),
                rest.trim().to_string(),
            )
        } else {
            (Frontmatter::default(), content.trim().to_string())
        }
    } else {
        (Frontmatter::default(), content.trim().to_string())
    };

    if body.is_empty() {
        tracing::warn!("agent template '{name}' has empty body");
    }

    AgentTemplate {
        name: name.to_string(),
        description: fm.description,
        tools_block: fm.tools_block,
        body,
        source,
    }
}

/// 内置模板（官方地板层）。文件存于本模块目录下，随二进制版本对齐。
const BUILTIN: &[(&str, &str)] = &[
    ("planner", include_str!("planner/ROLE.md")),
    ("reviewer", include_str!("reviewer/ROLE.md")),
    ("explorer", include_str!("explorer/ROLE.md")),
];

/// 内置模板清单。
pub fn builtin() -> Vec<AgentTemplate> {
    BUILTIN
        .iter()
        .map(|(name, content)| parse(name, content, TemplateSource::Builtin))
        .collect()
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

/// 同名后者覆盖前者，保持有序（按 name 排序，输出稳定）。
///
/// 例外：`tools_block` 跨层取**并集**——约束只能逐层加码、不能被上层
/// 覆盖悄悄放宽（"权限只能收窄"在层间同样成立；workspace 放一个省略
/// `tools_block` 的同名文件不该让内置 reviewer 失去只读约束）。
fn merge(layers: Vec<Vec<AgentTemplate>>) -> Vec<AgentTemplate> {
    let mut merged: Vec<AgentTemplate> = Vec::new();
    for layer in layers {
        for t in layer {
            match merged.iter_mut().find(|m| m.name == t.name) {
                Some(existing) => {
                    let mut tools_block = t.tools_block.clone();
                    for tool in &existing.tools_block {
                        if !tools_block.contains(tool) {
                            tools_block.push(tool.clone());
                        }
                    }
                    tools_block.sort();
                    *existing = AgentTemplate { tools_block, ..t };
                }
                None => merged.push(t),
            }
        }
    }
    merged.sort_by(|a, b| a.name.cmp(&b.name));
    merged
}

/// 三层合并后的模板清单：builtin → 全局（`global_dir`）→ workspace。
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
