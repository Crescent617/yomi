use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn write_skill(dir: &std::path::Path, description: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\ndescription: {description}\n---\n"),
    )
    .unwrap();
}

fn write_manual_skill(dir: &std::path::Path, description: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\ndescription: {description}\ndisable-model-invocation: true\n---\n"),
    )
    .unwrap();
}

fn names(skills: &[Arc<Skill>]) -> Vec<&str> {
    skills.iter().map(|s| s.name.as_str()).collect()
}

#[tokio::test]
async fn concurrent_misses_of_same_dir_coalesce_into_one_scan() {
    let loader = SkillLoader::new();
    let scan_count = Arc::new(AtomicUsize::new(0));
    let dir = PathBuf::from("/virtual/skill-dir");

    let mut handles = Vec::new();
    for _ in 0..8 {
        let loader = loader.clone();
        let scan_count = Arc::clone(&scan_count);
        let dir = dir.clone();
        handles.push(tokio::spawn(async move {
            loader
                .cache
                .get_with(dir, {
                    let scan_count = Arc::clone(&scan_count);
                    async move {
                        scan_count.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        Vec::new()
                    }
                })
                .await
        }));
    }
    for handle in handles {
        handle.await.unwrap();
    }

    assert_eq!(scan_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn load_dir_caches_within_ttl_and_rescans_after_expiry() {
    let root = tempfile::tempdir().unwrap();
    write_skill(&root.path().join("alpha"), "v1");
    let dir = root.path().to_path_buf();
    let loader = SkillLoader::with_ttl(Duration::from_millis(80));

    assert_eq!(loader.load_dir(dir.clone()).await[0].description, "v1");

    // TTL 内：命中缓存，磁盘改动不可见
    write_skill(&root.path().join("alpha"), "v2");
    assert_eq!(loader.load_dir(dir.clone()).await[0].description, "v1");

    // 过期后重扫，看到新内容
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert_eq!(loader.load_dir(dir).await[0].description, "v2");
}

#[tokio::test]
async fn load_is_layered_and_stable() {
    let global_root = tempfile::tempdir().unwrap();
    write_skill(&global_root.path().join("beta"), "global beta");
    write_skill(&global_root.path().join("alpha"), "global alpha");
    let workspace_root = tempfile::tempdir().unwrap();
    write_skill(&workspace_root.path().join("beta"), "workspace beta");
    write_skill(&workspace_root.path().join("gamma"), "workspace gamma");

    let loader = SkillLoader::new();
    let folders = || {
        vec![
            global_root.path().to_path_buf(),
            workspace_root.path().to_path_buf(),
        ]
    };
    let first = loader.load(folders()).await;
    let second = loader.load(folders()).await;

    // 前层序（按名）保留，后层覆盖同名项，后层新增按名追加
    assert_eq!(names(&first), vec!["alpha", "beta", "gamma"]);
    assert_eq!(first[1].description, "workspace beta");
    // 同一磁盘状态两次装配逐项一致
    assert_eq!(first, second);
}

#[tokio::test]
async fn later_layer_can_disable_same_named_earlier_skill() {
    let global_root = tempfile::tempdir().unwrap();
    write_skill(&global_root.path().join("x"), "global auto");
    let workspace_root = tempfile::tempdir().unwrap();
    write_manual_skill(&workspace_root.path().join("x"), "workspace disables it");

    let loader = SkillLoader::new();
    let skills = loader
        .load(vec![
            global_root.path().to_path_buf(),
            workspace_root.path().to_path_buf(),
        ])
        .await;

    assert!(names(&skills).is_empty());
}

#[tokio::test]
async fn three_layers_override_in_place() {
    let l1 = tempfile::tempdir().unwrap();
    write_skill(&l1.path().join("x"), "l1 x");
    write_skill(&l1.path().join("y"), "l1 y");
    let l2 = tempfile::tempdir().unwrap();
    write_skill(&l2.path().join("x"), "l2 x");
    let l3 = tempfile::tempdir().unwrap();
    write_skill(&l3.path().join("x"), "l3 x");

    let loader = SkillLoader::new();
    let skills = loader
        .load(vec![
            l1.path().to_path_buf(),
            l2.path().to_path_buf(),
            l3.path().to_path_buf(),
        ])
        .await;

    // 三层同名 x：最末层胜且保持首次出现的位置；未触及的 y 保前层
    assert_eq!(names(&skills), vec!["x", "y"]);
    assert_eq!(skills[0].description, "l3 x");
    assert_eq!(skills[1].description, "l1 y");
}

#[tokio::test]
async fn later_auto_layer_re_enables_earlier_manual_skill() {
    // flag 随胜者走：前层 manual + 后层 auto → 该项重新进入索引
    let earlier = tempfile::tempdir().unwrap();
    write_manual_skill(&earlier.path().join("x"), "earlier manual");
    let later = tempfile::tempdir().unwrap();
    write_skill(&later.path().join("x"), "later auto");

    let loader = SkillLoader::new();
    let skills = loader
        .load(vec![
            earlier.path().to_path_buf(),
            later.path().to_path_buf(),
        ])
        .await;

    assert_eq!(names(&skills), vec!["x"]);
    assert_eq!(skills[0].description, "later auto");
}

#[tokio::test]
async fn duplicate_folders_are_deduped_keeping_last_occurrence() {
    let dir_a = tempfile::tempdir().unwrap();
    write_skill(&dir_a.path().join("x"), "from a");
    let dir_b = tempfile::tempdir().unwrap();
    write_skill(&dir_b.path().join("x"), "from b");

    let loader = SkillLoader::new();
    // 无脑 push 的重复目录：保留最后一次出现（= 最高优先级）
    let skills = loader
        .load(vec![
            dir_a.path().to_path_buf(),
            dir_b.path().to_path_buf(),
            dir_a.path().to_path_buf(),
        ])
        .await;

    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].description, "from a");
}

#[tokio::test]
async fn manual_skills_are_dropped() {
    let root = tempfile::tempdir().unwrap();
    write_skill(&root.path().join("auto"), "auto");
    write_manual_skill(&root.path().join("manual"), "manual only");

    let loader = SkillLoader::new();
    let skills = loader.load(vec![root.path().to_path_buf()]).await;

    assert_eq!(names(&skills), vec!["auto"]);
}
