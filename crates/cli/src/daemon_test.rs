use super::*;
use kernel::types::KernelError;

#[test]
fn config_not_applied_matches_structurally() {
    // 正是这个错误把 sanity 误判引入过死代码：Display 带
    // "Configuration error: " 前缀，裸串比较恒 false。
    let e = KernelError::config(kernel::client::RESTART_CONFIG_NOT_APPLIED);
    assert!(is_config_not_applied(&e));
    // 防御：若有人改回 Display 裸串比较，这个断言会失败
    assert_ne!(e.to_string(), kernel::client::RESTART_CONFIG_NOT_APPLIED);
}

#[test]
fn config_not_applied_rejects_other_errors() {
    assert!(!is_config_not_applied(&KernelError::config(
        "some other config problem"
    )));
    assert!(!is_config_not_applied(&KernelError::Io(
        kernel::client::RESTART_CONFIG_NOT_APPLIED.to_string()
    )));
    assert!(!is_config_not_applied(&KernelError::storage("nope")));
}
