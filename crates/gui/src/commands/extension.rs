use kernel::extension::{ExtensionInfo, InstallReport, RemoveReport};
use tauri::State;

use crate::error::GuiError;
use crate::state::AppState;

/// 已装扩展清单（`extensions/` 目录即注册表）。
#[tauri::command(rename_all = "snake_case")]
pub async fn list_extensions(state: State<'_, AppState>) -> Result<Vec<ExtensionInfo>, GuiError> {
    state
        .kernel_snapshot()
        .extension_list()
        .await
        .map_err(|e| GuiError::kernel(format!("list extensions failed: {e}")))
}

/// 安装/更新扩展包。`source` = GitHub 简写/URL 或本地目录路径；
/// 取货（clone 上限 3 分钟）+ 复制 + 挂载 + cron 收养，逐项报告。
#[tauri::command(rename_all = "snake_case")]
pub async fn install_extension(
    state: State<'_, AppState>,
    source: String,
) -> Result<InstallReport, GuiError> {
    state
        .kernel_snapshot()
        .extension_install(source)
        .await
        .map_err(|e| GuiError::kernel(format!("install extension failed: {e}")))
}

/// 卸载扩展包（cron 前缀清扫 + 挂载指向判定回滚）。
#[tauri::command(rename_all = "snake_case")]
pub async fn remove_extension(
    state: State<'_, AppState>,
    name: String,
) -> Result<RemoveReport, GuiError> {
    state
        .kernel_snapshot()
        .extension_remove(name)
        .await
        .map_err(|e| GuiError::kernel(format!("remove extension failed: {e}")))
}
