//! 权限管理系统
//!
//! 提供工具权限检查的分级支持：
//! - `Level`: 工具危险级别 / 自动批准阈值（Safe/Caution/Dangerous）
//! - `PermissionChecker`: 权限检查器
//! - `ToolLevelResolver`: 工具级别判定器

mod checker;
mod level;
mod resolver;

pub use checker::{
    check_tool_permissions, Checker, PermissionCheckResult, PermissionState, Responder, Response,
};
pub use level::{exceeds_threshold, Level};
pub use resolver::ToolLevelResolver;
