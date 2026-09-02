//! Tests for the watch tee's content assembly.

use super::*;

fn msg(content: Vec<ContentBlock>, image_keys: Vec<&str>) -> ChannelMessage {
    ChannelMessage {
        external_chat_id: "oc_chat".to_string(),
        external_user_id: "ou_user".to_string(),
        external_message_id: Some("om_msg".to_string()),
        is_mention: false,
        raw_text: None,
        content,
        image_keys: image_keys.into_iter().map(str::to_string).collect(),
        thread_id: None,
        root_id: None,
        parent_id: None,
        is_group: true,
        create_time: None,
        doc_comment: None,
    }
}

#[test]
fn mirror_content_keeps_message_blocks_verbatim() {
    let blocks = vec![ContentBlock::Text {
        text:
            "[ts][from_user_id: ou_user][chat_id: oc_chat][msg_id: om_msg][platform: feishu]\nhello"
                .to_string(),
    }];
    let out = mirror_content(&msg(blocks.clone(), vec![]));
    assert_eq!(out, blocks);
}

#[test]
fn mirror_content_appends_image_refs_as_text() {
    let out = mirror_content(&msg(
        vec![ContentBlock::Text {
            text: "header".to_string(),
        }],
        vec!["img_k1", "img_k2"],
    ));
    assert_eq!(out.len(), 2);
    let ContentBlock::Text { text } = &out[1] else {
        panic!("expected text block");
    };
    assert_eq!(text, "[image: img_k1] [image: img_k2]");
}

#[test]
fn mirror_content_truncates_oversized_text_blocks() {
    let out = mirror_content(&msg(
        vec![ContentBlock::Text {
            text: "汉".repeat(5000),
        }],
        vec![],
    ));
    let ContentBlock::Text { text } = &out[0] else {
        panic!("expected text block");
    };
    let marker = "…(已截断)";
    assert!(text.ends_with(marker), "{text}");
    assert_eq!(
        text.chars().count(),
        MIRROR_TEXT_CAP + marker.chars().count(),
        "kept chars + marker"
    );
    assert!(text.starts_with(&"汉".repeat(100)), "head preserved");

    // Exactly at the cap: untouched, no marker (multibyte-safe either
    // way — `truncate_chars` works on chars, never bytes).
    let exact = "a".repeat(MIRROR_TEXT_CAP);
    let out = mirror_content(&msg(
        vec![ContentBlock::Text {
            text: exact.clone(),
        }],
        vec![],
    ));
    assert_eq!(
        out,
        vec![ContentBlock::Text { text: exact }],
        "cap-boundary text stays verbatim"
    );
}

#[test]
fn mapping_kind_roundtrip() {
    assert_eq!(MappingKind::Watch.as_str(), "watch");
    assert_eq!(MappingKind::Normal.as_str(), "normal");
    assert_eq!(MappingKind::from_str_lossy("watch"), MappingKind::Watch);
    assert_eq!(MappingKind::from_str_lossy("normal"), MappingKind::Normal);
    // Unknown/legacy values degrade to Normal.
    assert_eq!(MappingKind::from_str_lossy(""), MappingKind::Normal);
    assert_eq!(MappingKind::from_str_lossy("whatever"), MappingKind::Normal);
}

/// epoch 守卫的集成 harness：内存 store + 黑洞模型 kernel（run 挂在
/// 模型调用上，steer 的 user 消息照常落盘）。`hub_test` 的同款 harness
/// 是 sibling 模块私有，这里自建。
async fn epoch_harness() -> (Arc<dyn ChannelStore>, Arc<Kernel>, tempfile::TempDir) {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    crate::storage::migrations::run_migrations(&pool)
        .await
        .unwrap();
    let store: Arc<dyn ChannelStore> =
        Arc::new(crate::channels::store::SqliteChannelStore::new(pool));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((s, _)) = listener.accept().await {
            held.push(s);
        }
    });
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        models: vec![crate::provider::ModelConfig {
            name: "blackhole".into(),
            endpoint: format!("http://{addr}"),
            ..Default::default()
        }],
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();
    kernel.start();
    (store, kernel, tmp)
}

/// epoch 守卫：批被 take 之后、flush 之前若发生真翻转（off→on 双翻
/// 转竞态，drain 推进 epoch），陈旧批整批丢弃——绝不让旧模式的镜像
/// 漏进新模式。
#[tokio::test]
async fn flush_batch_drops_stale_epoch_after_drain() {
    let (store, kernel, _tmp) = epoch_harness().await;
    set_channel_watch_by_name(&store, &kernel, "mock", "oc_epoch", true)
        .await
        .unwrap();
    let sid = store
        .find_mapping("mock", "oc_epoch")
        .await
        .unwrap()
        .expect("watch on created the session");

    // 入队一条（窗口 30s，任务睡着），手动 take——模拟窗口任务刚好
    // 取出批、尚未 flush。
    let mut m = msg(
        vec![ContentBlock::Text {
            text: "旧模式的遗物".to_string(),
        }],
        vec![],
    );
    m.external_chat_id = "oc_epoch".to_string();
    mirror_enqueue(
        "mock",
        &store,
        &kernel,
        &m,
        std::time::Duration::from_secs(30),
    )
    .await;
    let (batch, epoch, entry) = {
        let entry = PENDING
            .get(&pending_key(&kernel, "mock", "oc_epoch"))
            .map(|r| Arc::clone(r.value()))
            .expect("pending entry exists");
        let (batch, epoch) = {
            let mut state = entry.lock().await;
            (std::mem::take(&mut state.items), state.epoch)
        };
        (batch, epoch, entry)
    };
    assert_eq!(batch.len(), 1, "one message taken");

    // take 之后 epoch 前进（生产上是 off→on 双翻转间的 drain；此处直
    // 接 drain_pending 精确隔离守卫本身——kind 保持 Watch，没有 epoch
    // 守卫时陈旧批会照常 steer 落盘，测试才不放空枪）。
    drain_pending(&kernel, "mock", "oc_epoch").await;
    // 陈旧批 flush：epoch 不符 → 整批丢弃。
    flush_batch("mock", &store, &kernel, "oc_epoch", batch, epoch, entry).await;

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let blob = format!("{:?}", kernel.list_messages(&sid).await.unwrap_or_default());
    assert!(
        !blob.contains("旧模式的遗物"),
        "stale batch must not land: {blob}"
    );
    kernel.stop().await;
}
