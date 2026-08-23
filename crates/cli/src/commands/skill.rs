use crate::args::GlobalArgs;
use crate::utils::load_config;
use anyhow::Result;
use kernel::skill::{SkillLoader, SkillScanner};
use std::path::PathBuf;

pub async fn list(global: &GlobalArgs) -> Result<()> {
    let config = load_config(global.config.as_ref())?;
    // 与会话同语义解析目录：展开 ~、相对路径相对 data_dir（对齐 lib.rs）
    let mut skill_folders: Vec<PathBuf> = config
        .skill_folders()
        .iter()
        .map(|p| kernel::expand_tilde(p))
        .map(|p| {
            if p.is_relative() {
                config.data_dir.join(p)
            } else {
                p
            }
        })
        .collect();
    // 与当前目录的会话视图一致：项目层存在时叠加（最高优先级）
    skill_folders = kernel::skill::session_skill_folders(
        &skill_folders,
        kernel::skill::workspace_skill_dir(&std::env::current_dir()?).await,
    );

    // 与会话装配同一语义：分层合并后只显示胜者（同名 skill 高优先级层胜出）
    let skills = SkillLoader::new().load(skill_folders.clone()).await;
    // 手动 skill 不进索引——统计并提示（先于空表判断，全是手动项时不能谎报为空）
    let all = SkillScanner::new(skill_folders).load_all().await;
    let indexed: std::collections::HashSet<_> = skills.iter().map(|s| &s.name).collect();
    let manual_names: std::collections::HashSet<_> = all
        .iter()
        .filter(|s| s.disable_model_invocation)
        .map(|s| &s.name)
        .collect();
    let manual = manual_names.difference(&indexed).count();

    if skills.is_empty() && manual == 0 {
        println!("No skills found.");
        return Ok(());
    }

    if !skills.is_empty() {
        let name_width = skills
            .iter()
            .map(|s| s.name.len())
            .max()
            .unwrap_or(10)
            .max(10);

        println!("{:<name_width$}  LOCATION", "NAME", name_width = name_width);

        for skill in &skills {
            println!(
                "{:<name_width$}  {}",
                skill.name,
                skill.source_path.display(),
                name_width = name_width
            );
        }
    }

    if manual > 0 {
        println!(
            "\n(+{manual} manual skill(s) excluded from the index by disable-model-invocation)"
        );
    }

    Ok(())
}
