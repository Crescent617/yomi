use super::level::Level;
use crate::tools::task::{TASK_GET_TOOL_NAME, TASK_LIST_TOOL_NAME};
use crate::tools::{
    GLOB_TOOL_NAME, GREP_TOOL_NAME, READ_TOOL_NAME, REMINDER_TOOL_NAME, SHELL_TOOL_NAME,
    SKILL_TOOL_NAME, TODO_TOOL_NAME, WEBSEARCH_TOOL_NAME,
};
use serde_json::Value;

/// 权限级别解析的唯一收口：ext 注册表优先（扩展声明的 level），
/// 查不到才落内建名表（内建语义稳定、ext 与内建不撞名由 spawn 保证）。
pub fn resolve_level(
    extension_registry: Option<&crate::extension::ExtensionRegistry>,
    tool_name: &str,
    args: &Value,
) -> Level {
    if let Some(level) = extension_registry.and_then(|r| r.registered_level(tool_name)) {
        return level;
    }
    match tool_name {
        // 只读工具 - Safe
        READ_TOOL_NAME | GLOB_TOOL_NAME | GREP_TOOL_NAME | TASK_LIST_TOOL_NAME
        | TASK_GET_TOOL_NAME | TODO_TOOL_NAME | REMINDER_TOOL_NAME | WEBSEARCH_TOOL_NAME
        | SKILL_TOOL_NAME => Level::Safe,
        SHELL_TOOL_NAME => ToolLevelResolver::resolve_bash_level(args),
        _ => Level::Caution,
    }
}

/// 判定工具危险级别（无状态，可全局共享）
pub struct ToolLevelResolver;

impl ToolLevelResolver {
    /// 根据工具名称和参数解析危险级别
    pub fn resolve(tool_name: &str, args: &Value) -> Level {
        resolve_level(None, tool_name, args)
    }

    /// 解析 Bash 命令的危险级别
    fn resolve_bash_level(args: &Value) -> Level {
        const DANGEROUS_PATTERNS: &[&str] = &[
            // Git 危险操作 (推送、强制推送、破坏性重置)
            "git push",
            "git reset --hard",
            "git clean",
            "git checkout -f",
            "git merge ",
            "git rebase ",
            "git revert ",
            "git cherry-pick ",
            // 文件系统破坏性操作
            "rm ",
            "rmdir ",
            // 磁盘/分区操作
            "mkfs.",
            "mkfs ",
            "dd if=",
            "dd of=",
            "fdisk ",
            "parted ",
            // 权限提升
            "sudo ",
            "su -",
            "su root",
            // 管道执行远程脚本 (极其危险)
            "| sh",
            "| bash",
            "| zsh",
            "| /bin/sh",
            "| /bin/bash",
            // Docker 破坏性操作
            "docker system prune",
            "docker rmi",
            "docker rm -f",
            "docker rm --force",
            // kubectl 破坏性操作
            "kubectl delete",
            "kubectl apply",
            // 系统关机/重启
            "shutdown ",
            "reboot",
            "poweroff",
            "halt",
            // 系统服务管理 (可能影响系统稳定性)
            "systemctl stop",
            "systemctl restart",
            "systemctl disable",
            "service ",
            // 包管理器 (修改系统状态)
            "apt install",
            "apt remove",
            "apt purge",
            "yum install",
            "yum remove",
            "dnf install",
            "dnf remove",
            "pacman -S",
            "pacman -R",
            "brew install",
            "brew uninstall",
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
    use crate::extension::{ExtToolDef, ExtensionRegistry};
    use std::sync::Arc;

    fn registry_with(name: &str, level: Level) -> Arc<ExtensionRegistry> {
        let registry = ExtensionRegistry::new();
        registry
            .register_tool(
                "conn_x",
                ExtToolDef {
                    name: name.to_string(),
                    desc: "d".to_string(),
                    schema: serde_json::json!({}),
                    level,
                },
            )
            .unwrap();
        Arc::new(registry)
    }

    #[test]
    fn ext_declared_level_wins_over_default_caution() {
        let registry = registry_with("danger_op", Level::Dangerous);
        // ext 声明 dangerous 必须生效（此前的死字段会被静默降档为 caution）。
        assert_eq!(
            resolve_level(Some(&registry), "danger_op", &Value::Null),
            Level::Dangerous
        );
        let registry = registry_with("safe_lookup", Level::Safe);
        assert_eq!(
            resolve_level(Some(&registry), "safe_lookup", &Value::Null),
            Level::Safe
        );
    }

    #[test]
    fn builtin_table_unaffected_by_registry() {
        let registry = registry_with("danger_op", Level::Dangerous);
        assert_eq!(
            resolve_level(Some(&registry), READ_TOOL_NAME, &Value::Null),
            Level::Safe
        );
        assert_eq!(
            resolve_level(Some(&registry), "unknown_tool", &Value::Null),
            Level::Caution
        );
    }

    #[test]
    fn no_registry_falls_back_to_builtin_table() {
        assert_eq!(
            resolve_level(None, READ_TOOL_NAME, &Value::Null),
            Level::Safe
        );
        assert_eq!(
            resolve_level(None, "unknown_tool", &Value::Null),
            Level::Caution
        );
    }
}
