//! `yomi doctor` — harness 健康自检：daemon 握手、渠道连通、cron 积压、
//! 存储与配置。任何一项 ❌ 时退出码为 1（可接进重启自检 cron 做门禁）。

use crate::args::GlobalArgs;
use anyhow::Result;
use kernel::client::KernelApi;
use kernel::wire::ReqMethod;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    Ok,
    Warn,
    Fail,
}

impl Level {
    fn icon(&self) -> &'static str {
        match self {
            Level::Ok => "✅",
            Level::Warn => "⚠️",
            Level::Fail => "❌",
        }
    }
}

struct Check {
    level: Level,
    label: String,
    detail: String,
}

fn check(level: Level, label: &str, detail: impl Into<String>) -> Check {
    Check {
        level,
        label: label.to_string(),
        detail: detail.into(),
    }
}

/// 未 finalize 的原始配置：finalize 会静默补默认 model、重置无效
/// `default_model`——健康检查必须在归一化之前看配置。
fn load_config_unfinalized(global: &GlobalArgs) -> Result<kernel::config::Config> {
    let mut config = if let Some(path) = global.config.as_ref() {
        kernel::config::Config::from_file(path)?
    } else {
        kernel::config::Config::discover_file()
            .map(|path| kernel::config::Config::from_file(&path))
            .transpose()?
            .unwrap_or_default()
    };
    config.inject_env()?;
    config.apply_env_overrides();
    Ok(config)
}

pub async fn run(global: &GlobalArgs) -> Result<()> {
    let mut checks = Vec::new();

    // ── Config ────────────────────────────────────────────────────────
    let config = match load_config_unfinalized(global) {
        Ok(raw) => {
            if raw.models.is_empty() {
                checks.push(check(Level::Fail, "config", "no [[models]] configured"));
            } else if !raw.models.iter().any(|m| m.name == raw.agent.default_model) {
                checks.push(check(
                    Level::Fail,
                    "config",
                    format!(
                        "default_model '{}' not found in [[models]]",
                        raw.agent.default_model
                    ),
                ));
            } else {
                let mut detail = format!("{} model(s) configured", raw.models.len());
                if raw.channels.is_empty() {
                    detail.push_str(", no channels");
                }
                checks.push(check(Level::Ok, "config", detail));
            }
            // 后续检查用完整归一化配置（data_dir 等由 finalize 定型）。
            crate::utils::load_config(global.config.as_ref()).ok()
        }
        Err(e) => {
            checks.push(check(Level::Fail, "config", format!("failed to load: {e}")));
            None
        }
    };

    // ── Daemon（握手含 wire 协议版本校验）──────────────────────────
    // 不用 connect_strict：它的 context 会把"协议不匹配"和"daemon 没起"
    // 抹成同一句，而这两者处置完全不同。
    let client = match kernel::client::RemoteKernel::connect(&crate::daemon::socket_addr()).await {
        Ok(c) => {
            checks.push(check(
                Level::Ok,
                "daemon",
                format!(
                    "running (wire protocol v{})",
                    kernel::wire::WIRE_PROTOCOL_VERSION
                ),
            ));
            Some(c)
        }
        Err(e) => {
            checks.push(check(
                Level::Fail,
                "daemon",
                format!("{e} — fix: `yomi daemon restart`"),
            ));
            None
        }
    };

    // ── Channels / cron / sessions（依赖 daemon）──────────────────
    if let Some(client) = &client {
        match client.call(ReqMethod::ListChannels).await {
            Ok(v) => {
                let infos: Vec<kernel::channels::ChannelInfo> =
                    serde_json::from_value(v).unwrap_or_default();
                if infos.is_empty() {
                    checks.push(check(Level::Ok, "channels", "none configured"));
                }
                for info in &infos {
                    // STATUS_CONNECTING 的语义是"receiver 活着"（ws 在收），
                    // 即健康运行态；Idle 才是"receiver 已退出"（不在收）。
                    let (level, note) = match info.status {
                        kernel::channels::ChannelStatus::Error => (Level::Fail, "error"),
                        kernel::channels::ChannelStatus::Connecting => (Level::Ok, "receiving"),
                        kernel::channels::ChannelStatus::Idle => (Level::Warn, "not receiving"),
                    };
                    checks.push(check(level, &format!("channel:{}", info.name), note));
                }
            }
            Err(e) => checks.push(check(Level::Warn, "channels", format!("query failed: {e}"))),
        }

        match client.list_cron_jobs(None, 100).await {
            Ok(jobs) => {
                let active = jobs
                    .iter()
                    .filter(|j| j.status == kernel::cron::CronJobStatus::Active)
                    .count();
                let failed = jobs
                    .iter()
                    .filter(|j| j.status == kernel::cron::CronJobStatus::Failed)
                    .count();
                // "下一次触发"只统计 active job——completed/paused 的
                // next_run_at 是历史值，会误报 overdue。
                let next = jobs
                    .iter()
                    .filter(|j| j.status == kernel::cron::CronJobStatus::Active)
                    .filter_map(|j| j.next_run_at)
                    .min()
                    .map_or_else(
                        || "none scheduled".to_string(),
                        |t| {
                            let mins = (t - chrono::Utc::now()).num_minutes();
                            if mins < 0 {
                                "overdue".to_string()
                            } else {
                                format!("next in {mins}m")
                            }
                        },
                    );
                let level = if failed > 0 { Level::Warn } else { Level::Ok };
                checks.push(check(
                    level,
                    "cron",
                    format!("{active} active, {failed} failed, {next}"),
                ));
            }
            Err(e) => checks.push(check(Level::Warn, "cron", format!("query failed: {e}"))),
        }

        match client.list_running_sessions().await {
            Ok(running) => {
                checks.push(check(
                    Level::Ok,
                    "sessions",
                    format!("{} running", running.len()),
                ));
            }
            Err(e) => checks.push(check(Level::Warn, "sessions", format!("query failed: {e}"))),
        }
    }

    // ── Storage（本地）─────────────────────────────────────────────
    if let Some(config) = &config {
        match kernel::StorageSet::open_with_config(&config.data_dir, config).await {
            Ok(storage) => {
                match storage
                    .session_store()
                    .list(
                        None,
                        kernel::storage::session::SessionListScope::All,
                        None,
                        500,
                    )
                    .await
                {
                    Ok((v, _)) => {
                        let db = config.data_dir.join("yomi.db");
                        let db_mb = std::fs::metadata(&db)
                            .map(|m| m.len() as f64 / 1_048_576.0)
                            .unwrap_or(0.0);
                        let level = if db_mb > 1024.0 {
                            Level::Warn
                        } else {
                            Level::Ok
                        };
                        checks.push(check(
                            level,
                            "storage",
                            format!("{} session(s), db {db_mb:.1} MiB", v.len()),
                        ));
                    }
                    // 查询失败不能报成"0 会话"的假健康。
                    Err(e) => checks.push(check(
                        Level::Warn,
                        "storage",
                        format!("session list failed: {e}"),
                    )),
                }
            }
            Err(e) => checks.push(check(Level::Fail, "storage", format!("open failed: {e}"))),
        }
    }

    // ── Report ───────────────────────────────────────────────────────
    let worst = checks
        .iter()
        .map(|c| &c.level)
        .max()
        .cloned()
        .unwrap_or(Level::Ok);
    for c in &checks {
        println!("{} {:<16} {}", c.level.icon(), c.label, c.detail);
    }
    match worst {
        Level::Ok => println!("\nAll checks passed."),
        Level::Warn => println!("\nHealthy with warnings."),
        Level::Fail => println!("\nUnhealthy — see ❌ above."),
    }
    if worst == Level::Fail {
        std::process::exit(1);
    }
    Ok(())
}
