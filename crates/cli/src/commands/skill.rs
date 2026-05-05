use crate::args::GlobalArgs;
use crate::commands::tui::resolve_skill_folders;
use crate::utils::load_config;
use anyhow::Result;
use kernel::skill::SkillLoader;

#[allow(clippy::needless_pass_by_value)]
pub async fn list(global: GlobalArgs) -> Result<()> {
    let working_dir = global
        .dir
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let working_dir = working_dir.canonicalize()?;

    let config = load_config(global.config.as_ref(), &working_dir)?;
    let skill_folders = resolve_skill_folders(&config, &working_dir);

    let loader = SkillLoader::new(skill_folders);
    let skills = loader.load_all().unwrap_or_default();

    if skills.is_empty() {
        println!("No skills found.");
        return Ok(());
    }

    let name_width = skills
        .iter()
        .map(|s| s.name.len())
        .max()
        .unwrap_or(10)
        .max(10);
    let loc_width = skills
        .iter()
        .map(|s| s.source_path.display().to_string().len())
        .max()
        .unwrap_or(20)
        .max(20);

    println!(
        "{:<name_width$}  {:<loc_width$}  DESCRIPTION",
        "NAME",
        "LOCATION",
        name_width = name_width,
        loc_width = loc_width
    );

    for skill in skills {
        let location = skill.source_path.display().to_string();
        let desc = if skill.description.is_empty() {
            "(no description)"
        } else {
            &skill.description
        };
        println!(
            "{:<name_width$}  {:<loc_width$}  {}",
            skill.name,
            location,
            desc,
            name_width = name_width,
            loc_width = loc_width
        );
    }

    Ok(())
}
