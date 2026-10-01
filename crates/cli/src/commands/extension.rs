//! `yomi extension` — 扩展包管理（安装/清单/卸载）。
//!
//! 全走 KernelApi（remote daemon），与 cron 命令同轨道；包语义见
//! `yomi doc extension` 与 docs/design/ext-packages.md。

use crate::args::GlobalArgs;
use anyhow::{Context, Result};
use comfy_table::{ContentArrangement, Table};
use kernel::client::KernelApi;

async fn connect() -> Result<kernel::client::RemoteKernel> {
    crate::daemon::connect_strict().await
}

/// 安装：取货（clone/本地）+ 复制 + 挂载 + cron 收养，逐项报告。
/// `source` 是 GitHub 简写/URL 或本地目录。
pub async fn install(_global: &GlobalArgs, source: String) -> Result<()> {
    let kernel = connect().await?;
    let report: kernel::extension::InstallReport = kernel
        .extension_install(source)
        .await
        .context("Failed to install extension")?;
    println!(
        "Installed extension {} v{} (hash {})",
        report.name,
        report.version,
        &report.content_hash[..8.min(report.content_hash.len())]
    );
    for c in &report.cron {
        println!(
            "  cron {} ({})",
            c.name,
            if c.created {
                "created"
            } else {
                "exists, untouched"
            }
        );
    }
    for m in report.hooks.iter().chain(report.bins.iter()) {
        let status = match m.status {
            kernel::extension::MountStatus::Linked => "linked",
            kernel::extension::MountStatus::Already => "already linked",
        };
        println!("  {} ({status})", m.path);
    }
    for s in &report.snippets {
        println!("  snippet {s} (assembled into system prompt)");
    }
    Ok(())
}

/// 清单：name/version/source/health + 资源计数。
pub async fn list(_global: &GlobalArgs) -> Result<()> {
    let kernel = connect().await?;
    let rows = kernel
        .extension_list()
        .await
        .context("Failed to list extensions")?;
    if rows.is_empty() {
        println!("No extensions installed.");
        return Ok(());
    }
    let mut table = Table::new();
    table
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec!["NAME", "VERSION", "HEALTH", "SOURCE", "RESOURCES"]);
    table.load_preset(comfy_table::presets::NOTHING);
    for row in &rows {
        let resources = row["resources"]
            .as_object()
            .map(|r| {
                let n = |k: &str| r[k].as_array().map_or(0, Vec::len);
                format!(
                    "{} cron, {} hooks, {} bins, {} snippets",
                    n("cron"),
                    n("hooks"),
                    n("bins"),
                    n("snippets")
                )
            })
            .unwrap_or_default();
        table.add_row(vec![
            row["name"].as_str().unwrap_or("?"),
            row["version"].as_str().unwrap_or("?"),
            row["health"].as_str().unwrap_or("?"),
            row["source"].as_str().unwrap_or("?"),
            &resources,
        ]);
    }
    println!("{table}");
    Ok(())
}

/// 卸载：cron 前缀清扫 + 摘挂载（指向判定）+ 删包 + 删记录。
pub async fn remove(_global: &GlobalArgs, name: String) -> Result<()> {
    let kernel = connect().await?;
    let report = kernel
        .extension_remove(name.clone())
        .await
        .context("Failed to remove extension")?;
    // 全空且记录本就不在 = 从未装过/名字拼错——说清，不给假成功。
    let nothing_done = report.cron_removed.is_empty()
        && report.mounts_removed.is_empty()
        && !report.ext_dir_removed;
    if nothing_done {
        println!("Extension {name} is not installed (nothing to remove)");
        return Ok(());
    }
    println!("Removed extension {name}");
    for c in &report.cron_removed {
        println!("  cron {c} deleted");
    }
    for m in &report.mounts_removed {
        println!("  {m} unlinked");
    }
    for m in &report.mounts_left {
        println!("  {m} left in place (slot content changed)");
    }
    Ok(())
}
