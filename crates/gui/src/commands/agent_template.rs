use tauri::State;

use crate::error::GuiError;
use crate::state::AppState;

#[tauri::command(rename_all = "snake_case")]
pub async fn list_agent_templates(
    state: State<'_, AppState>,
    session_id: Option<String>,
) -> Result<Vec<kernel::agent_tmpl::AgentTemplate>, GuiError> {
    let coord = state.kernel_snapshot();
    let sid = session_id.map(kernel::types::SessionId::from);
    coord
        .list_agent_templates(sid.as_ref())
        .await
        .map_err(GuiError::kernel)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn save_agent_template(
    state: State<'_, AppState>,
    session_id: Option<String>,
    scope: kernel::agent_tmpl::TemplateScope,
    name: String,
    body: String,
) -> Result<(), GuiError> {
    let coord = state.kernel_snapshot();
    let sid = session_id.map(kernel::types::SessionId::from);
    coord
        .save_agent_template(sid.as_ref(), scope, &name, &body)
        .await
        .map_err(GuiError::kernel)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn delete_agent_template(
    state: State<'_, AppState>,
    session_id: Option<String>,
    scope: kernel::agent_tmpl::TemplateScope,
    name: String,
) -> Result<(), GuiError> {
    let coord = state.kernel_snapshot();
    let sid = session_id.map(kernel::types::SessionId::from);
    coord
        .delete_agent_template(sid.as_ref(), scope, &name)
        .await
        .map_err(GuiError::kernel)
}
