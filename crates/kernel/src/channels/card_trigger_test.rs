use super::*;
use crate::channels::ChannelError;
use crate::types::ContentBlock;
use std::sync::Mutex;

// ── Fixtures ───────────────────────────────────────────────────────

#[derive(Default)]
struct MockAdapter {
    sent_messages: Mutex<Vec<(String, String)>>, // chat_id, text
}

#[async_trait::async_trait]
impl PlatformAdapter for MockAdapter {
    async fn run_receiver(
        &self,
        _incoming: tokio::sync::mpsc::Sender<crate::channels::ChannelEvent>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<(), ChannelError> {
        cancel.cancelled().await;
        Ok(())
    }

    async fn send_message(
        &self,
        external_chat_id: &str,
        blocks: Vec<ContentBlock>,
        _reply_msg_id: Option<&str>,
    ) -> Result<Option<String>, ChannelError> {
        let text = crate::channels::blocks_to_text(&blocks);
        self.sent_messages
            .lock()
            .unwrap()
            .push((external_chat_id.to_string(), text));
        Ok(None)
    }
}

fn action(value: serde_json::Value) -> CardAction {
    CardAction {
        operator_open_id: "ou_op".to_string(),
        operator_union_id: Some("on_op".to_string()),
        chat_id: Some("oc_chat".to_string()),
        message_id: Some("om_card".to_string()),
        token: Some("c-tok".to_string()),
        value,
    }
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// 落一个记录契约的脚本：stdin、事件标识、env 残留、cwd 全写进
/// state 目录（`$YOMI_STATE_DIR` 由被测代码惰性创建）。
fn write_probe_script(path: &Path) {
    std::fs::write(
        path,
        "#!/bin/sh\n\
         cat > \"$YOMI_STATE_DIR/stdin.json\"\n\
         echo \"$YOMI_EVENT\" > \"$YOMI_STATE_DIR/event\"\n\
         pwd > \"$YOMI_STATE_DIR/pwd\"\n\
         { if [ -n \"$YOMI_SESSION_ID\" ]; then echo sid=set; else echo sid=unset; fi\n\
           if [ -n \"$YOMI_HOOK_EVENT\" ]; then echo hook=set; else echo hook=unset; fi\n\
         } > \"$YOMI_STATE_DIR/sanitized\"\n",
    )
    .unwrap();
    make_executable(path);
}

// ── valid_name ─────────────────────────────────────────────────────

#[test]
fn valid_name_accepts_tools_charset() {
    for name in ["a", "publish_v2", "A-1_x", &"a".repeat(MAX_NAME_LEN)] {
        assert_eq!(valid_name(name), Some(name), "accept {name}");
    }
}

#[test]
fn valid_name_rejects_path_and_charset_abuse() {
    for name in [
        "",        // 空（`ext_` 裸值）
        "1abc",    // 数字开头
        ".hidden", // 点开头（隐藏项/相对路径）
        "..",      // 父目录
        "a/b",     // 路径分隔
        "a.b",     // 点
        "名字",    // 非 ASCII
        &"a".repeat(MAX_NAME_LEN + 1),
    ] {
        assert_eq!(valid_name(name), None, "reject {name}");
    }
}

// ── resolve ────────────────────────────────────────────────────────

#[tokio::test]
async fn resolve_bare_file_form() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("channels").join(DIR_NAME);
    std::fs::create_dir_all(&base).unwrap();

    let bare = base.join("bare");
    std::fs::write(&bare, "#!/bin/sh\n").unwrap();
    make_executable(&bare);
    assert_eq!(resolve(tmp.path(), "bare").await, Some(bare));
}

#[tokio::test]
async fn resolve_misses() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("channels").join(DIR_NAME);
    std::fs::create_dir_all(&base).unwrap();

    // 未注册
    assert_eq!(resolve(tmp.path(), "ghost").await, None);

    // 文件无执行位 = 开关关
    let off = base.join("off");
    std::fs::write(&off, "#!/bin/sh\n").unwrap();
    assert_eq!(resolve(tmp.path(), "off").await, None);

    // 目录形态不收：只有裸文件是注册条目
    let dir = base.join("pack");
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(resolve(tmp.path(), "pack").await, None);

    // 破损符号链接不致命
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(base.join("nonexistent"), base.join("broken")).unwrap();
        assert_eq!(resolve(tmp.path(), "broken").await, None);
    }
}

// ── run ────────────────────────────────────────────────────────────

#[tokio::test]
async fn run_feeds_contract_and_sanitizes_env() {
    let tmp = tempfile::tempdir().unwrap();
    let script = tmp.path().join("probe");
    write_probe_script(&script);

    let act = action(serde_json::json!({"action": "ext_publish", "id": 1}));
    // 种残留：显式移除语义必须在父进程带着这两个变量时也成立
    // （对照 hook_test.rs 的 stale-env 模式）。
    std::env::set_var("YOMI_SESSION_ID", "sess_stale");
    std::env::set_var("YOMI_HOOK_EVENT", "pre_tool_use");
    run(tmp.path(), "feishu", "publish", &script, &act).await;
    std::env::remove_var("YOMI_SESSION_ID");
    std::env::remove_var("YOMI_HOOK_EVENT");

    let state = tmp
        .path()
        .join("state")
        .join("channels")
        .join(DIR_NAME)
        .join("publish");
    let stdin: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(state.join("stdin.json")).unwrap()).unwrap();
    assert_eq!(
        stdin,
        serde_json::json!({
            "event": "card_trigger",
            "name": "publish",
            "channel": "feishu",
            "operator_open_id": "ou_op",
            "operator_union_id": "on_op",
            "chat_id": "oc_chat",
            "message_id": "om_card",
            "token": "c-tok",
            "value": {"action": "ext_publish", "id": 1},
        })
    );
    assert_eq!(
        std::fs::read_to_string(state.join("event")).unwrap().trim(),
        "card_trigger"
    );
    // 无会话语义：父进程种过残留，子进程也拿不到（显式移除而非继承）
    assert_eq!(
        std::fs::read_to_string(state.join("sanitized"))
            .unwrap()
            .trim(),
        "sid=unset\nhook=unset"
    );
    // macOS /var → /private/var：子进程 pwd 是物理路径，按规范化比较
    let pwd = std::fs::read_to_string(state.join("pwd")).unwrap();
    assert_eq!(
        std::path::PathBuf::from(pwd.trim()).canonicalize().unwrap(),
        tmp.path().canonicalize().unwrap()
    );
}

#[tokio::test]
async fn run_failure_is_contained() {
    let tmp = tempfile::tempdir().unwrap();
    let script = tmp.path().join("boom");
    std::fs::write(&script, "#!/bin/sh\necho oops >&2\nexit 1\n").unwrap();
    make_executable(&script);
    // 非零退出只 warn 留痕：调用不 panic、不传播
    run(
        tmp.path(),
        "feishu",
        "boom",
        &script,
        &action(serde_json::json!({})),
    )
    .await;
}

// ── dispatch ───────────────────────────────────────────────────────

#[tokio::test]
async fn dispatch_invalid_name_denies_without_echo() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    dispatch(
        tmp.path(),
        "feishu",
        &adapter,
        &action(serde_json::json!({"action": "ext_.."})),
    )
    .await;
    let msgs = mock.sent_messages.lock().unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].0, "oc_chat");
    assert!(msgs[0].1.contains("Unknown card trigger"), "{}", msgs[0].1);
    assert!(!msgs[0].1.contains(".."), "不 echo 非法名: {}", msgs[0].1);
}

#[tokio::test]
async fn dispatch_unknown_trigger_denies_with_name() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    dispatch(
        tmp.path(),
        "feishu",
        &adapter,
        &action(serde_json::json!({"action": "ext_ghost"})),
    )
    .await;
    let msgs = mock.sent_messages.lock().unwrap();
    assert_eq!(msgs.len(), 1);
    assert!(msgs[0].1.contains("ghost"), "{}", msgs[0].1);
}

#[tokio::test]
async fn dispatch_registered_trigger_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("channels").join(DIR_NAME);
    std::fs::create_dir_all(&base).unwrap();
    write_probe_script(&base.join("publish"));

    let mock = Arc::new(MockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    dispatch(
        tmp.path(),
        "feishu",
        &adapter,
        &action(serde_json::json!({"action": "ext_publish"})),
    )
    .await;

    let state = tmp
        .path()
        .join("state")
        .join("channels")
        .join(DIR_NAME)
        .join("publish");
    assert!(state.join("stdin.json").exists(), "脚本被喂了 stdin");
    assert!(
        mock.sent_messages.lock().unwrap().is_empty(),
        "注册命中不发拒绝提示"
    );
}
