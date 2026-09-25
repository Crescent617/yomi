//! `yomi doc` —— 内置手册：话题索引 + 全文输出。
//!
//! 内容以 markdown 文件存于 `crates/cli/docs/`，`include_str!` 编译进
//! 二进制：文档版本与代码严格一致，无网络、无漂移源。索引页的一行简介
//! 兼作触发词载体——任务词面对不上 system prompt 那行话题名单时，
//! agent 靠这一页找回召回率，简介要当 description 写。

use anyhow::{bail, Result};
use std::fmt::Write as _;

struct Topic {
    name: &'static str,
    summary: &'static str,
    content: &'static str,
}

const TOPICS: &[Topic] = &[
    Topic {
        name: "skills",
        summary: "skill 体系：三层目录与优先级、SKILL.md 格式、安装/调试 skill",
        content: include_str!("../../docs/skills.md"),
    },
    Topic {
        name: "config",
        summary: "配置：config show/get/set/schema、改后须重启、[env] 注入、socket 鉴权",
        content: include_str!("../../docs/config.md"),
    },
    Topic {
        name: "sessions",
        summary: "会话：检索/查看/驱动（cat/search/send/wait/mailbox/cancel）、运行态、规则文件",
        content: include_str!("../../docs/sessions.md"),
    },
    Topic {
        name: "cron",
        summary: "定时任务：create/update/trigger、一次性任务、precheck 门、退出码 42",
        content: include_str!("../../docs/cron.md"),
    },
    Topic {
        name: "daemon",
        summary: "daemon：status/restart/stop、doctor 自检、自杀式重启、日志位置",
        content: include_str!("../../docs/daemon.md"),
    },
    Topic {
        name: "extensions",
        summary: "扩展点总览：hook/外挂 tool/飞书卡片触发器的注册约定",
        content: include_str!("../../docs/extensions.md"),
    },
    Topic {
        name: "hooks",
        summary: "hook 契约：pre_tool_use 否决闸、turn/daemon 生命周期通知、stdin schema、退出码",
        content: include_str!("../../docs/hooks.md"),
    },
    Topic {
        name: "tools",
        summary: "外挂 tool 契约：tool.json manifest、调用协议、图片回传、审批级别",
        content: include_str!("../../docs/tools.md"),
    },
    Topic {
        name: "card-triggers",
        summary: "飞书卡片触发器契约：ext_ 路由、stdin schema、改卡路径",
        content: include_str!("../../docs/card-triggers.md"),
    },
    Topic {
        name: "debug",
        summary: "调试：headless 运行与退出码、events 流、rpc 逃生舱口、usage 用量、gc 清理、日志",
        content: include_str!("../../docs/debug.md"),
    },
    Topic {
        name: "websearch",
        summary: "web_search 工具：引擎 fallback 顺序、env 配置、SearXNG 的 json 格式坑",
        content: include_str!("../../docs/websearch.md"),
    },
    Topic {
        name: "deployment",
        summary: "部署：容器/K8s readiness 探针",
        content: include_str!("../../docs/deployment.md"),
    },
];

fn render_index() -> String {
    let mut out = String::from("yomi 内置手册。查看主题：yomi doc <topic>\n\n");
    let width = TOPICS.iter().map(|t| t.name.len()).max().unwrap_or(0);
    for t in TOPICS {
        let _ = writeln!(out, "{:<width$}  {}", t.name, t.summary, width = width);
    }
    out
}

fn render_topic(name: &str) -> Option<&'static str> {
    TOPICS.iter().find(|t| t.name == name).map(|t| t.content)
}

pub fn run(topic: Option<&str>) -> Result<()> {
    match topic {
        None => print!("{}", render_index()),
        Some(name) => match render_topic(name) {
            Some(content) => print!("{content}"),
            None => {
                let valid = TOPICS.iter().map(|t| t.name).collect::<Vec<_>>().join(", ");
                bail!("unknown doc topic '{name}'. Topics: {valid}");
            }
        },
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_topic_has_non_empty_markdown_with_heading() {
        for t in TOPICS {
            assert!(!t.content.trim().is_empty(), "topic {} is empty", t.name);
            assert!(
                t.content.trim_start().starts_with("# "),
                "topic {} must start with a markdown heading",
                t.name
            );
            assert!(!t.summary.is_empty(), "topic {} missing summary", t.name);
        }
    }

    #[test]
    fn topic_names_are_unique() {
        let mut names: Vec<_> = TOPICS.iter().map(|t| t.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), TOPICS.len());
    }

    #[test]
    fn index_lists_every_topic() {
        let index = render_index();
        for t in TOPICS {
            assert!(index.contains(t.name), "index missing {}", t.name);
        }
        assert!(index.contains("yomi doc <topic>"));
    }

    #[test]
    fn sp_line_major_topics_exist() {
        // system prompt 的 Environment 行列出的话题名必须都能解析——
        // 改名/删话题时这里先红，SP 文案同步更新（kernel prompt/mod.rs）。
        for major in [
            "skills",
            "config",
            "sessions",
            "cron",
            "daemon",
            "extensions",
            "debug",
        ] {
            assert!(
                render_topic(major).is_some(),
                "SP-line topic '{major}' missing"
            );
        }
    }
}
