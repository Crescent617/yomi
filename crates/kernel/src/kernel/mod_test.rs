use super::{duration_until_next_midnight, mark_user_steer};
use crate::storage::NewSession;
use crate::types::{ContentBlock, ImageUrl};

#[test]
fn duration_until_next_midnight_is_within_a_day() {
    let d = duration_until_next_midnight();
    assert!(d > std::time::Duration::ZERO);
    assert!(d <= std::time::Duration::from_hours(24));
}

#[test]
fn user_steer_prefixes_the_first_text_block() {
    assert_eq!(
        mark_user_steer(vec![ContentBlock::Text {
            text: "change direction".to_string(),
        }]),
        vec![ContentBlock::Text {
            text: "[From User] change direction".to_string(),
        }]
    );
}

#[test]
fn user_steer_inserts_prefix_before_non_text_content() {
    let image = ContentBlock::ImageUrl {
        image_url: ImageUrl {
            url: "data:image/png;base64,abc".to_string(),
            detail: None,
        },
    };

    assert_eq!(
        mark_user_steer(vec![image.clone()]),
        vec![
            ContentBlock::Text {
                text: "[From User] ".to_string(),
            },
            image,
        ]
    );
}

/// `gc.auto = true` + `Kernel::start` runs a gc pass at startup that purges
/// expired sessions (`dry_run` is never set for the daemon's auto gc).
#[tokio::test]
async fn auto_gc_collects_expired_sessions_on_start() {
    use crate::storage::StorageSet;

    let tmp = tempfile::TempDir::new().unwrap();
    let mut config = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    config.gc.auto = true;
    // Keep the pass minimal: this test exercises scheduling, not the sweep.
    config.gc.sweep_orphans = false;
    config.finalize();

    // Create and age a session before the kernel starts so the immediate
    // first gc pass collects it.
    let storage = StorageSet::open(tmp.path().to_path_buf()).await.unwrap();
    let id = crate::types::SessionId::new();
    storage
        .session_store()
        .create(NewSession {
            working_dir: Some("/test".into()),
            ..NewSession::new(id.clone())
        })
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET updated_at = datetime('now', '-100 days') WHERE id = ?")
        .bind(&*id.0)
        .execute(storage.pool())
        .await
        .unwrap();

    let kernel = crate::build_kernel(&config, false).await.unwrap();
    kernel.start();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if storage.session_store().get(&id).await.unwrap().is_none() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "auto gc did not collect the expired session in time"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    kernel.stop();
}

/// With `gc.auto = false` (the default) `Kernel::start` spawns no gc pass.
#[tokio::test]
async fn auto_gc_disabled_by_default() {
    use crate::storage::StorageSet;

    let tmp = tempfile::TempDir::new().unwrap();
    let mut config = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    config.finalize();

    let storage = StorageSet::open(tmp.path().to_path_buf()).await.unwrap();
    let id = crate::types::SessionId::new();
    storage
        .session_store()
        .create(NewSession {
            working_dir: Some("/test".into()),
            ..NewSession::new(id.clone())
        })
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET updated_at = datetime('now', '-100 days') WHERE id = ?")
        .bind(&*id.0)
        .execute(storage.pool())
        .await
        .unwrap();

    let kernel = crate::build_kernel(&config, false).await.unwrap();
    kernel.start();

    // Give any (unexpected) gc pass a chance to run; the session must survive.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(storage.session_store().get(&id).await.unwrap().is_some());

    kernel.stop();
}
