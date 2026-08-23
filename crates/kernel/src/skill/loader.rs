//! Skill 热加载：磁盘即真相，按目录扫描结果缓存 `SCAN_TTL`，同一目录的
//! 并发 miss 由 moka 请求合并（request coalescing）合并为一次扫描。
//! 生效延迟上限 = 缓存时长。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use moka::future::Cache;

use super::{drop_manual_skills, merge_skills, Skill, SkillScanner};

/// 目录扫描结果的缓存时长：过期后下一次加载重新扫描。
pub const SCAN_TTL: Duration = Duration::from_secs(60);

/// Skill 热加载器：按目录扫描（结果缓存 `SCAN_TTL`），同目录并发扫描合
/// 并为一次。Clone 只复制句柄（缓存共享），放进 `AgentShared` 后所有
/// session spawn 共用同一份缓存。
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

    /// 分层扫描并合并 `folders`：目录按优先级从低到高传入，后面的目录
    /// 覆盖前面目录的同名 skill（保持首次出现的位置，新增按名追加）；
    /// 重复目录保序去重（留最后一次出现，即最高优先级）；手动 skill 在
    /// 合并后统一过滤——后层可禁用前层同名 skill。
    ///
    /// 顺序稳定：每层内部按 skill 名排序（`SkillScanner::load_all`），同
    /// 一磁盘状态 + 同一缓存代际下装配结果逐项一致。
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
        let mut merged = Vec::new();
        for folder in folders {
            merged = merge_skills(merged, self.load_dir(folder).await);
        }
        drop_manual_skills(&mut merged);
        merged
    }

    /// 扫描单个目录：命中缓存直接返回，并发 miss 合并为一次扫描。
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
