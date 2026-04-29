use super::level::Level;
use crate::task::{TASK_GET_TOOL_NAME, TASK_LIST_TOOL_NAME};
use crate::tools::{
    GLOB_TOOL_NAME, GREP_TOOL_NAME, READ_TOOL_NAME, REMINDER_TOOL_NAME, SHELL_TOOL_NAME,
    SKILL_TOOL_NAME, TODO_READ_TOOL_NAME, WEBFETCH_TOOL_NAME, WEBSEARCH_TOOL_NAME,
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
            | TASK_GET_TOOL_NAME | TODO_READ_TOOL_NAME | REMINDER_TOOL_NAME
            | WEBFETCH_TOOL_NAME | WEBSEARCH_TOOL_NAME | SKILL_TOOL_NAME => Level::Safe,
            SHELL_TOOL_NAME => Self::resolve_bash_level(args),
            _ => Level::Caution,
        }
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
            // 修改系统文件权限
            "chmod 777",
            "chmod -R 777",
            "chown -R",
            // 文件重定向覆盖 (可能导致数据丢失)
            " > /",
            ">/",
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
