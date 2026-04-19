use super::level::Level;
use crate::task::{TASK_GET_TOOL_NAME, TASK_LIST_TOOL_NAME};
use crate::tools::{BASH_TOOL_NAME, GLOB_TOOL_NAME, GREP_TOOL_NAME, READ_TOOL_NAME};
use serde_json::Value;

/// 判定工具危险级别（无状态，可全局共享）
pub struct ToolLevelResolver;

impl ToolLevelResolver {
    /// 根据工具名称和参数解析危险级别
    pub fn resolve(tool_name: &str, args: &Value) -> Level {
        match tool_name {
            // 只读工具 - Safe
            READ_TOOL_NAME | GLOB_TOOL_NAME | GREP_TOOL_NAME | TASK_LIST_TOOL_NAME
            | TASK_GET_TOOL_NAME => Level::Safe,
            BASH_TOOL_NAME => Self::resolve_bash_level(args),
            _ => Level::Caution,
        }
    }

    /// 解析 Bash 命令的危险级别
    fn resolve_bash_level(args: &Value) -> Level {
        const DANGEROUS_PATTERNS: &[&str] = &[
            // Git 危险操作
            "git push",
            "git reset",
            "git clean",
            "git checkout ",
            "git merge ",
            "git rebase ",
            "git revert ",
            "git cherry-pick ",
            // 删除操作
            "rm ",
        ];
        let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
        let cmd_lower = cmd.to_lowercase();
        for pattern in DANGEROUS_PATTERNS {
            if cmd_lower.contains(pattern) {
                return Level::Dangerous;
            }
        }
        Level::Caution
    }
}
