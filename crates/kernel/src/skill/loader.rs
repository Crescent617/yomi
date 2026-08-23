//! Skill 热加载：spawn 时按目录现场扫描，结果缓存 `SCAN_TTL`，生效延
//! 迟上限 = TTL。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use moka::future::Cache;

use super::{drop_manual_skills, Skill, SkillScanner};

/// 目录扫描结果的缓存时长。
pub const SCAN_TTL: Duration = Duration::from_secs(60);

/// 按目录的热加载器：TTL 缓存 + moka 请求合并（同目录并发扫描只跑一
/// 次）。Clone 共享缓存。
#[derive(Clone)]
pub struct SkillLoader {
    cache: Cache<PathBuf, Vec<Arc<Skill>>>,
}

impl SkillLoader {
    pub fn new() -> Self {
        Self::with_ttl(SCAN_TTL)
    }

    /// 自定义缓存时长（测试用）。
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            cache: Cache::builder().time_to_live(ttl).build(),
        }
    }

    /// 分层合并 `folders`（低→高优先级）：后层覆盖前层同名（保持首次出
    /// 现位置），重复目录留最后出现，最后过滤手动 skill（后层可禁用前
    /// 层同名项）。每层内部按名排序，合并用 name→位置索引，整体 O(n)。
    pub async fn load(&self, folders: Vec<PathBuf>) -> Vec<Arc<Skill>> {
        let mut seen = std::collections::HashSet::new();
        let folders: Vec<PathBuf> = folders
            .into_iter()
            .rev()
            .filter(|f| seen.insert(f.clone()))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let mut position: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let mut merged: Vec<Arc<Skill>> = Vec::new();
        for folder in folders {
            for skill in self.load_dir(folder).await {
                match position.get(&skill.name) {
                    Some(&i) => merged[i] = skill,
                    None => {
                        position.insert(skill.name.clone(), merged.len());
                        merged.push(skill);
                    }
                }
            }
        }
        drop_manual_skills(&mut merged);
        merged
    }

    /// 扫描单个目录：缓存命中直接返回，并发 miss 合并为一次扫描。
    async fn load_dir(&self, dir: PathBuf) -> Vec<Arc<Skill>> {
        self.cache
            .get_with(dir.clone(), async move {
                SkillScanner::new(vec![dir]).load_all().await
            })
            .await
    }
}

impl Default for SkillLoader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "loader_test.rs"]
mod tests;
