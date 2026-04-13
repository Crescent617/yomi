use super::level::Level;
use crate::task::{
    TASK_CREATE_TOOL_NAME, TASK_GET_TOOL_NAME, TASK_LIST_TOOL_NAME, TASK_UPDATE_TOOL_NAME,
};
use crate::tools::{
    BASH_TOOL_NAME, EDIT_TOOL_NAME, GLOB_TOOL_NAME, GREP_TOOL_NAME, READ_TOOL_NAME,
    SUBAGENT_TOOL_NAME, WRITE_TOOL_NAME,
};
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
            // 修改工具, SubAgent, 任务创建/更新 - Caution
            EDIT_TOOL_NAME | WRITE_TOOL_NAME | SUBAGENT_TOOL_NAME | TASK_CREATE_TOOL_NAME | TASK_UPDATE_TOOL_NAME => Level::Caution,
            // Bash 根据命令内容判断
            BASH_TOOL_NAME => Self::resolve_bash_level(args),
            // 未知工具默认为 Dangerous（安全起见）
            _ => Level::Dangerous,
        }
    }

    /// 解析 Bash 命令的危险级别
    fn resolve_bash_level(args: &Value) -> Level {
        const DANGEROUS_PATTERNS: &[&str] = &[
            // Git 危险操作
            "git push",
            "git push --force",
            "git push -f",
            "git reset --hard",
            "git clean -fd",
            // 删除操作
            "rm -rf",
            "rm -fr",
            "rm -r /",
            "rm -rf /",
            // 系统操作
            "> /etc/",
            ">> /etc/",
            "> /var/",
            ">> /var/",
            "mkfs",
            "dd if=",
            // 权限修改
            "chmod -R",
            "chown -R",
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_resolve_safe_tools() {
        let empty_args = json!({});
        assert_eq!(
            ToolLevelResolver::resolve(READ_TOOL_NAME, &empty_args),
            Level::Safe
        );
        assert_eq!(
            ToolLevelResolver::resolve(GLOB_TOOL_NAME, &empty_args),
            Level::Safe
        );
        assert_eq!(
            ToolLevelResolver::resolve(GREP_TOOL_NAME, &empty_args),
            Level::Safe
        );
        assert_eq!(
            ToolLevelResolver::resolve(TASK_LIST_TOOL_NAME, &empty_args),
            Level::Safe
        );
        assert_eq!(
            ToolLevelResolver::resolve(TASK_GET_TOOL_NAME, &empty_args),
            Level::Safe
        );
    }

    #[test]
    fn test_resolve_caution_tools() {
        let empty_args = json!({});
        assert_eq!(
            ToolLevelResolver::resolve(EDIT_TOOL_NAME, &empty_args),
            Level::Caution
        );
        assert_eq!(
            ToolLevelResolver::resolve(WRITE_TOOL_NAME, &empty_args),
            Level::Caution
        );
        assert_eq!(
            ToolLevelResolver::resolve(TASK_CREATE_TOOL_NAME, &empty_args),
            Level::Caution
        );
        assert_eq!(
            ToolLevelResolver::resolve(TASK_UPDATE_TOOL_NAME, &empty_args),
            Level::Caution
        );
        assert_eq!(
            ToolLevelResolver::resolve(SUBAGENT_TOOL_NAME, &empty_args),
            Level::Caution
        );
    }

    #[test]
    fn test_resolve_bash_caution() {
        let args = json!({"command": "echo hello"});
        assert_eq!(
            ToolLevelResolver::resolve(BASH_TOOL_NAME, &args),
            Level::Caution
        );
    }

    #[test]
    fn test_resolve_bash_dangerous_git_push() {
        let args = json!({"command": "git push origin main"});
        assert_eq!(
            ToolLevelResolver::resolve(BASH_TOOL_NAME, &args),
            Level::Dangerous
        );
    }

    #[test]
    fn test_resolve_bash_dangerous_git_force() {
        let args = json!({"command": "git push --force"});
        assert_eq!(
            ToolLevelResolver::resolve(BASH_TOOL_NAME, &args),
            Level::Dangerous
        );
    }

    #[test]
    fn test_resolve_bash_dangerous_rm_rf() {
        let args = json!({"command": "rm -rf /some/path"});
        assert_eq!(
            ToolLevelResolver::resolve(BASH_TOOL_NAME, &args),
            Level::Dangerous
        );
    }

    #[test]
    fn test_resolve_bash_dangerous_reset_hard() {
        let args = json!({"command": "git reset --hard HEAD~1"});
        assert_eq!(
            ToolLevelResolver::resolve(BASH_TOOL_NAME, &args),
            Level::Dangerous
        );
    }

    #[test]
    fn test_resolve_unknown_tool() {
        let args = json!({});
        assert_eq!(
            ToolLevelResolver::resolve("UnknownTool", &args),
            Level::Dangerous
        );
    }
}
