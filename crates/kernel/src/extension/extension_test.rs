use super::*;
use crate::permission::Level;
use crate::tools::Tool as _;

fn def(name: &str) -> ExtToolDef {
    ExtToolDef {
        name: name.to_string(),
        desc: format!("{name} desc"),
        schema: serde_json::json!({"type": "object"}),
        level: Level::Caution,
    }
}

#[tokio::test]
async fn register_dispatch_pull_result_roundtrip() {
    let registry = Arc::new(ExtensionRegistry::new());
    let reg_id = registry
        .register_tool("conn_a", def("stock_quote"))
        .unwrap();

    // proxies 可见（spawn 合并入口）
    let proxies = registry.tool_proxies();
    assert_eq!(proxies.len(), 1);
    assert_eq!(proxies[0].name(), "stock_quote");

    // 调用方派单（后台等待回执）
    let registry_call = Arc::clone(&registry);
    let reg_call = reg_id.clone();
    let call = tokio::spawn(async move {
        registry_call
            .dispatch(&reg_call, serde_json::json!({"symbol": "600519"}), None)
            .await
    });

    // provider 领单
    let work = registry.pull("conn_a", &reg_id).await.unwrap().unwrap();
    assert_eq!(work.name, "stock_quote");
    assert_eq!(work.args["symbol"], "600519");

    // 交付回执 → 调用方收
    registry
        .submit_result("conn_a", &work.call_id, "1900.00".to_string(), false)
        .unwrap();
    let outcome = call.await.unwrap().unwrap();
    assert_eq!(outcome.output, "1900.00");
    assert!(!outcome.is_error);
}

#[tokio::test]
async fn duplicate_name_rejected() {
    let registry = ExtensionRegistry::new();
    registry.register_tool("conn_a", def("x")).unwrap();
    let err = registry.register_tool("conn_b", def("x")).unwrap_err();
    assert!(err.contains("already registered"), "{err}");
}

#[tokio::test]
async fn pull_single_worker_conflict() {
    let registry = Arc::new(ExtensionRegistry::new());
    let reg_id = registry.register_tool("conn_a", def("x")).unwrap();
    let registry2 = Arc::clone(&registry);
    let reg_pending = reg_id.clone();
    let pending = tokio::spawn(async move { registry2.pull("conn_a", &reg_pending).await });
    tokio::task::yield_now().await;
    let err = registry.pull("conn_a", &reg_id).await.unwrap_err();
    assert!(err.contains("already pending"), "{err}");
    pending.abort();
}

#[tokio::test]
async fn pull_wrong_conn_rejected() {
    let registry = ExtensionRegistry::new();
    let reg_id = registry.register_tool("conn_a", def("x")).unwrap();
    let err = registry.pull("conn_b", &reg_id).await.unwrap_err();
    assert!(err.contains("another connection"), "{err}");
}

#[tokio::test]
async fn sweep_disconnects_pending_and_hides_tool() {
    let registry = Arc::new(ExtensionRegistry::new());
    let reg_id = registry.register_tool("conn_a", def("x")).unwrap();
    let registry_call = Arc::clone(&registry);
    let reg_call = reg_id.clone();
    let call = tokio::spawn(async move {
        registry_call
            .dispatch(&reg_call, serde_json::json!({}), None)
            .await
    });
    tokio::task::yield_now().await;

    registry.sweep("conn_a");

    let outcome = call.await.unwrap().unwrap();
    assert!(outcome.is_error);
    assert!(
        outcome.output.contains("disconnected"),
        "{}",
        outcome.output
    );
    assert!(registry.tool_proxies().is_empty());

    // sweep 后同一名称可重新注册（重连重注册是契约）。
    registry.register_tool("conn_b", def("x")).unwrap();
}

#[tokio::test]
async fn result_wrong_conn_errors() {
    let registry = Arc::new(ExtensionRegistry::new());
    let reg_id = registry.register_tool("conn_a", def("x")).unwrap();
    let registry_call = Arc::clone(&registry);
    let reg_call = reg_id.clone();
    let call = tokio::spawn(async move {
        registry_call
            .dispatch(&reg_call, serde_json::json!({}), None)
            .await
    });
    let work = registry.pull("conn_a", &reg_id).await.unwrap().unwrap();

    let err = registry
        .submit_result("conn_b", &work.call_id, "hack".to_string(), false)
        .unwrap_err();
    assert!(err.contains("another connection"), "{err}");
    // 串线者的"结果"被作为错误回执送回调用方。
    let outcome = call.await.unwrap().unwrap();
    assert!(outcome.is_error);
}

#[tokio::test]
async fn late_result_discarded_quietly() {
    let registry = ExtensionRegistry::new();
    // 未存在过的 call_id：静默丢弃不报错（过期/取消后的迟到场景）。
    registry
        .submit_result("conn_a", "c_nope", "late".to_string(), false)
        .unwrap();
}

#[tokio::test]
async fn route_memory_roundtrip() {
    let registry = ExtensionRegistry::new();
    let sid = SessionId::from("sess_x".to_string());
    assert!(registry.route_get("gitlab-ci", "p1").is_none());
    registry.route_set("gitlab-ci", "p1", sid.clone());
    assert_eq!(registry.route_get("gitlab-ci", "p1"), Some(sid));
}

/// 代理 Tool 的 exec：调派 → 回执映射为 ToolOutput（成功/错误/断开三路）。
#[tokio::test]
async fn ext_tool_exec_maps_outcomes() {
    use crate::tools::ToolExecCtx;

    let registry = Arc::new(ExtensionRegistry::new());
    let reg_id = registry.register_tool("conn_a", def("quote")).unwrap();
    let tool = Arc::new(registry.tool_proxies().into_iter().next().unwrap());

    // 起一个在飞的 exec（future 惰性，必须 spawn 驱动；tc 取字面量保 'static）。
    let spawn_call = |tc: &'static str| {
        let tool = Arc::clone(&tool);
        let ctx = ToolExecCtx::new(tc, "/tmp", "sess_x");
        tokio::spawn(async move { tool.exec(serde_json::json!({}), ctx).await })
    };

    // 成功回执 → text
    let call = spawn_call("tc_1");
    let work = registry.pull("conn_a", &reg_id).await.unwrap().unwrap();
    registry
        .submit_result("conn_a", &work.call_id, "42".to_string(), false)
        .unwrap();
    let out = call.await.unwrap().unwrap();
    let text = format!("{:?}", out.contents);
    assert!(text.contains("42"), "{text}");
    assert!(!out.is_error);

    // is_error 回执（串线 conn → 错误回执送回调用方）→ error output
    let call = spawn_call("tc_2");
    let work = registry.pull("conn_a", &reg_id).await.unwrap().unwrap();
    registry
        .submit_result("conn_b", &work.call_id, "boom".to_string(), true)
        .unwrap_err();
    let out = call.await.unwrap().unwrap();
    let text = format!("{:?}", out.contents);
    assert!(text.contains("wrong connection"), "{text}");
    assert!(out.is_error);

    // provider 断开 → error output
    let call = spawn_call("tc_3");
    registry.sweep("conn_a");
    let out = call.await.unwrap().unwrap();
    let text = format!("{:?}", out.contents);
    assert!(text.contains("disconnected"), "{text}");
    assert!(out.is_error);
}
