//! `/cron` — cron 面板卡：定时任务管理（**全部 job、不含已完成**
//! ——hrli 2026-08-22 决策：completed 纯属历史残留，列表只该看
//! "还活着的"；failed 保留显示以呈现 `last_error`），每行 ⏸暂停 /
//! ▶️恢复 / 🗑删除（无状态两段确认），底部 🔄 Refresh。`cron_*` 回
//! 调执行后原地刷新（settings 同款："执行 → 重读状态 →
//! `update_card`"）。命令与全部按钮回调限 admin（与 `/settings`
//! 同档）；路由层 user 门限对所有按钮生效。
//!
//! 不提供触发按钮（行太挤——hrli 决策；触发走 cron tool）。

use std::sync::Arc;

use serde_json::json;
use tracing::warn;

use crate::cron::{CronJob, CronJobStatus, CronStore, UpdateCronJobInput};
use crate::kernel::Kernel;
use crate::types::{ContentBlock, Result as KernelResult};

use crate::channels::hub_deliver::info_card_envelope;
use crate::channels::{CardAction, ChannelConfig, ChannelMessage, PlatformAdapter};

async fn read_jobs(cron_store: &Arc<dyn CronStore>) -> KernelResult<Vec<CronJob>> {
    let mut jobs = cron_store
        // 上限是"全库最近 1000 条"的软箍（list 按创建时间倒序截
        // 断）：超出部分静默缺席面板——个人部署远在量级之下，真要
        // 支持需加分页。
        .list(None, 1000)
        .await
        .map_err(|e| crate::types::KernelError::storage(format!("list cron jobs: {e}")))?
        .into_iter()
        // completed 不显示（纯属历史残留——hrli 2026-08-22）；failed
        // 保留（last_error 需要可见）。
        .filter(|j| !matches!(j.status, CronJobStatus::Completed))
        .collect::<Vec<_>>();
    // 活跃在前，其余按下次运行时间升序（None 排尾）。
    jobs.sort_by_key(|j| {
        (
            u8::from(!matches!(j.status, CronJobStatus::Active)),
            j.next_run_at.map_or(i64::MAX, |t| t.timestamp()),
        )
    });
    Ok(jobs)
}

fn status_icon(status: CronJobStatus) -> &'static str {
    match status {
        CronJobStatus::Active => "🟢",
        CronJobStatus::Paused => "⏸",
        CronJobStatus::Completed => "✅",
        CronJobStatus::Failed => "❌",
    }
}

fn fmt_next(job: &CronJob) -> String {
    match job.next_run_at {
        Some(t) => t
            .with_timezone(&chrono::Local)
            .format("%m-%d %H:%M")
            .to_string(),
        None => "—".to_string(),
    }
}

fn job_info_line(job: &CronJob) -> String {
    let runs = if job.has_max_runs() {
        format!(" · {}/{}次", job.run_count, job.max_runs)
    } else if job.run_count > 0 {
        format!(" · {}次", job.run_count)
    } else {
        String::new()
    };
    let mut line = format!(
        "**{}**\n{} `{}` · next {}{}",
        crate::channels::reply::md_safe(&crate::channels::reply::flatten_ws(&job.name)),
        status_icon(job.status),
        job.schedule,
        fmt_next(job),
        runs
    );
    if let Some(err) = &job.last_error {
        use std::fmt::Write as _;
        let _ = write!(
            line,
            "\n<font color='red'>last error: {}</font>",
            crate::channels::reply::md_safe(&crate::channels::reply::flatten_ws(&tail(err, 60)))
        );
    }
    line
}

fn tail(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        format!("{}…", s.chars().take(max).collect::<String>())
    } else {
        s.to_string()
    }
}

fn small_button(text: &str, value: &serde_json::Value) -> serde_json::Value {
    json!({
        "tag": "button",
        "text": { "tag": "plain_text", "content": text },
        "type": "default",
        "size": "small",
        "behaviors": [{ "type": "callback", "value": value }],
    })
}

fn job_row(chat_id: &str, job: &CronJob) -> serde_json::Value {
    let id = &job.id.0;
    let mut action_cols = Vec::new();
    match job.status {
        CronJobStatus::Active => action_cols.push(json!({
            "tag": "column", "width": "auto", "vertical_align": "center",
            "elements": [small_button("⏸", &json!({ "action": "cron_pause", "scope": chat_id, "id": id }))],
        })),
        CronJobStatus::Paused => action_cols.push(json!({
            "tag": "column", "width": "auto", "vertical_align": "center",
            "elements": [small_button("▶️", &json!({ "action": "cron_resume", "scope": chat_id, "id": id }))],
        })),
        _ => {}
    }
    action_cols.push(json!({
        "tag": "column", "width": "auto", "vertical_align": "center",
        "elements": [small_button("🗑", &json!({ "action": "cron_del_ask", "scope": chat_id, "id": id, "name": job.name }))],
    }));
    let mut columns = vec![json!({
        "tag": "column", "width": "weighted", "weight": 1, "vertical_align": "center",
        "elements": [{ "tag": "markdown", "text_size": "notation", "content": job_info_line(job) }],
    })];
    columns.extend(action_cols);
    json!({ "tag": "column_set", "columns": columns })
}

/// 面板卡片：`confirming` 携带待确认的删除（无状态两段确认）。
fn cron_card(chat_id: &str, jobs: &[CronJob], confirming: Option<(&str, &str)>) -> String {
    let mut elements = Vec::new();
    if let Some((id, name)) = confirming {
        elements.push(json!({
            "tag": "column_set",
            "columns": [
                {
                    "tag": "column", "width": "weighted", "weight": 1, "vertical_align": "center",
                    "elements": [{ "tag": "markdown", "text_size": "notation",
                        "content": format!("确认删除「{}」？此操作不可撤销。", crate::channels::reply::md_safe(&crate::channels::reply::flatten_ws(name))) }],
                },
                {
                    "tag": "column", "width": "auto", "vertical_align": "center",
                    "elements": [small_button("确认删除", &json!({ "action": "cron_del_do", "scope": chat_id, "id": id }))],
                },
                {
                    "tag": "column", "width": "auto", "vertical_align": "center",
                    "elements": [small_button("取消", &json!({ "action": "cron_refresh", "scope": chat_id }))],
                },
            ],
        }));
    }
    if jobs.is_empty() {
        elements.push(json!({
            "tag": "markdown", "text_size": "notation",
            "content": "暂无定时任务。让我帮你建：「每天早上 8 点给我发 AI 日报」即可。",
        }));
    } else {
        elements.extend(jobs.iter().map(|j| job_row(chat_id, j)));
    }
    elements.push(json!({
        "tag": "column_set",
        "columns": [{
            "tag": "column", "width": "weighted", "weight": 1,
            "elements": [small_button("🔄 Refresh", &json!({ "action": "cron_refresh", "scope": chat_id }))],
        }],
    }));
    info_card_envelope("⏰ Cron jobs", elements)
}

/// `/cron` 命令主体（admin 门槛在命令臂，此处只管执行）。
pub(crate) async fn handle_cron_command(
    kernel: &Arc<Kernel>,
    adapter: &Arc<dyn PlatformAdapter>,
    msg: &ChannelMessage,
    reply_msg_id: Option<String>,
) -> KernelResult<Option<String>> {
    let chat_id = &msg.external_chat_id;
    let Some(cron_store) = kernel.cron_store() else {
        adapter
            .send_message(
                chat_id,
                vec![ContentBlock::Text {
                    text: "cron 未启用（config 中未配置 cron store）。".to_string(),
                }],
                reply_msg_id.as_deref(),
            )
            .await?;
        return Ok(None);
    };
    let jobs = read_jobs(&cron_store).await?;
    adapter
        .send_card(
            chat_id,
            &cron_card(chat_id, &jobs, None),
            reply_msg_id.as_deref(),
        )
        .await?;
    Ok(None)
}

fn notify_scheduler(kernel: &Arc<Kernel>) {
    let slot = kernel.cron_scheduler_slot();
    let guard = slot.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ref scheduler) = *guard {
        scheduler.reload();
    }
}

/// `cron_*` 按钮回调：执行变更后原地刷新这张卡片（settings 同款约定
/// ——不自动跟踪变更，别处改了任务点 🔄 Refresh）。
pub(crate) async fn handle_card_action(
    channel_name: &str,
    config: &ChannelConfig,
    kernel: &Arc<Kernel>,
    adapter: &Arc<dyn PlatformAdapter>,
    action: CardAction,
) {
    if let Err(e) = handle_card_action_inner(channel_name, config, kernel, adapter, &action).await {
        warn!(channel = %channel_name, error = %e, "cron card action failed");
    }
}

/// 变更动作的执行结果（handler 据此通知 scheduler 与刷新卡片）。
struct ActionOutcome {
    /// 删除两段确认的待确认项（(id, name)）。
    confirming: Option<(String, String)>,
    /// 任务集已变更，需要通知 scheduler reload。
    scheduler_dirty: bool,
}

/// 变更动作的纯逻辑面（测试缝）：按 action 值执行 store 变更并返回
/// 结果。scheduler 通知与卡片刷新由调用方（handler）负责。
async fn apply_action(
    cron_store: &Arc<dyn CronStore>,
    value: &serde_json::Value,
) -> KernelResult<ActionOutcome> {
    let id = value["id"].as_str().unwrap_or_default();
    let clean = ActionOutcome {
        confirming: None,
        scheduler_dirty: false,
    };
    match value["action"].as_str() {
        Some("cron_pause") => {
            cron_store
                .update(
                    &crate::types::CronJobId::from(id.to_string()),
                    &UpdateCronJobInput {
                        status: Some(CronJobStatus::Paused),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|e| crate::types::KernelError::storage(format!("pause cron job: {e}")))?;
            Ok(ActionOutcome {
                scheduler_dirty: true,
                ..clean
            })
        }
        Some("cron_resume") => {
            cron_store
                .update(
                    &crate::types::CronJobId::from(id.to_string()),
                    &UpdateCronJobInput {
                        status: Some(CronJobStatus::Active),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|e| crate::types::KernelError::storage(format!("resume cron job: {e}")))?;
            Ok(ActionOutcome {
                scheduler_dirty: true,
                ..clean
            })
        }
        Some("cron_del_ask") => {
            let name = value["name"].as_str().unwrap_or_default();
            Ok(ActionOutcome {
                confirming: Some((id.to_string(), name.to_string())),
                ..clean
            })
        }
        Some("cron_del_do") => {
            cron_store
                .delete(&crate::types::CronJobId::from(id.to_string()))
                .await
                .map_err(|e| crate::types::KernelError::storage(format!("delete cron job: {e}")))?;
            Ok(ActionOutcome {
                scheduler_dirty: true,
                ..clean
            })
        }
        Some("cron_refresh") => Ok(clean),
        other => {
            warn!(value = %value, "unrecognized cron card action {other:?}");
            Ok(clean)
        }
    }
}

async fn handle_card_action_inner(
    _channel_name: &str,
    config: &ChannelConfig,
    kernel: &Arc<Kernel>,
    adapter: &Arc<dyn PlatformAdapter>,
    action: &CardAction,
) -> KernelResult<()> {
    let value = &action.value;
    let chat_id = value["scope"].as_str().unwrap_or_default();
    if chat_id.is_empty() {
        warn!(value = %value, "cron card action missing scope");
        return Ok(());
    }
    if let Some(deny) = crate::channels::approval::check_admin(config, &action.operator_open_id) {
        crate::channels::approval::send_action_denial(adapter, action, deny).await;
        return Ok(());
    }
    let Some(cron_store) = kernel.cron_store() else {
        return Ok(());
    };
    let outcome = apply_action(&cron_store, value).await?;
    if outcome.scheduler_dirty {
        notify_scheduler(kernel);
    }
    if let Some(message_id) = &action.message_id {
        let jobs = read_jobs(&cron_store).await?;
        let confirming = outcome
            .confirming
            .as_ref()
            .map(|(id, name)| (id.as_str(), name.as_str()));
        adapter
            .update_card(message_id, &cron_card(chat_id, &jobs, confirming))
            .await?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "cron_card_test.rs"]
mod tests;
