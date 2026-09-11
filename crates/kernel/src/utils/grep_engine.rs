//! 进程内文本搜索引擎：grep 工具的执行后端。用 ripgrep 的核心库
//! （grep-regex 匹配、grep-searcher 搜索、ignore 遍历）在进程内完成，
//! 替代外部 rg 子进程——去除对系统 rg 的依赖（Windows/精简容器尤其
//! 受益），并省去每次调用的进程启动开销。
//!
//! 语义对齐原 rg 调用：含隐藏文件但遵守 gitignore（`--hidden`）、排除
//! VCS 目录（`.git`/`.svn`/`.hg`，含同名文件）、截止时限、glob 白/黑
//! 名单（gitignore 语义，支持 `!` 与 `{a,b}`）、文件类型过滤（与 rg
//! 同一份类型表）、multiline（`-U --multiline-dotall`）。二进制处理：
//! 遍历到的文件探测到 NUL 即停（NUL 所在缓冲块内的前置匹配丢弃，
//! 与 rg 口径相同）；被截断的文件有命中时经 `file_errors` 给出
//! 「binary file matches」信号，无命中则与 rg 一样静默；显式单文件
//! root 按 convert 把 NUL 换行符化后继续搜，与 rg 对显式文件的策略
//! 一致。
//!
//! 与 rg 二进制的已知偏差（对 agent 工具场景无害，刻意接受）：
//! - 非 UTF-8 内容按 lossy 读（rg 会转码 UTF-16 等编码）；
//! - count + multiline：按 sink 回调次数计（相邻 multiline 匹配被
//!   grep-searcher 并块，可能比 rg 按正则匹配数的口径少；非
//!   multiline 时两口径一致）；
//! - count 遇二进制截断：已收集的部分计数照常返回（rg 对该文件
//!   整体抑制计数）；
//! - 单线程遍历：跨文件的匹配顺序是目录序（rg 并行遍历本无序），
//!   filename 模式下游本按 mtime 重排，无影响；
//! - 匹配全量收集后由调用方分页（与原「全量读 rg stdout 再分页」的
//!   内存敞口一致）。

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use grep_searcher::{Searcher, Sink, SinkContext, SinkMatch};

use crate::utils::grep_output::{GrepMatch, GrepResult};

/// 一次搜索的全部参数（模式无关部分；模式见 [`SearchMode`]）。
pub struct SearchParams<'a> {
    pub pattern: &'a str,
    pub case_insensitive: bool,
    pub multiline: bool,
    pub context_before: usize,
    pub context_after: usize,
    /// 已解析好的 glob 列表（gitignore 语义，`!` 为黑名单）。
    pub glob_patterns: &'a [String],
    pub file_type: Option<&'a str>,
    /// 截止时刻（超时返回 [`SearchError::Timeout`]）。
    pub deadline: Option<Instant>,
}

/// 输出模式（与工具 `output_mode` 参数对应）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SearchMode {
    /// 有匹配的文件列表。
    Files,
    /// 每文件匹配行数。
    Count,
    /// 匹配与上下文行全文。
    Content,
}

/// 搜索产出 + 逐文件错误（无法读取/解码的文件不致命，等价 rg 打到
/// stderr 的报错：调用方汇总展示）。
pub struct SearchReport {
    pub outcome: SearchOutcome,
    pub file_errors: Vec<String>,
}

/// 三种模式的产出。
pub enum SearchOutcome {
    Files(Vec<PathBuf>),
    Counts(Vec<(PathBuf, usize)>),
    Content(GrepResult),
}

/// 搜索失败（与「搜完但无匹配」分层：无匹配不是错误）。
#[derive(Debug)]
pub enum SearchError {
    /// 正则编译失败。
    Pattern(String),
    /// glob 编译失败。
    Glob(String),
    /// 未知文件类型（对齐 rg 的 unrecognized file type）。
    FileType(String),
    /// 超过截止时刻。
    Timeout(Duration),
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pattern(e) => write!(f, "invalid pattern: {e}"),
            Self::Glob(e) => write!(f, "invalid glob: {e}"),
            Self::FileType(e) => write!(f, "{e}"),
            Self::Timeout(d) => write!(f, "search timed out after {} seconds", d.as_secs()),
        }
    }
}

impl std::error::Error for SearchError {}

/// 在 `root`（文件或目录）下按模式搜索。
pub fn search(
    root: &Path,
    mode: SearchMode,
    params: &SearchParams,
) -> Result<SearchReport, SearchError> {
    let matcher = grep_regex::RegexMatcherBuilder::new()
        .case_insensitive(params.case_insensitive)
        .multi_line(params.multiline)
        .dot_matches_new_line(params.multiline)
        .build(params.pattern)
        .map_err(|e| SearchError::Pattern(e.to_string()))?;

    // 显式单文件 root：与 rg 对显式文件的策略一致（convert 后静默
    // 搜索，不发二进制信号）。
    let explicit_file = root.is_file();
    let mut builder = grep_searcher::SearcherBuilder::new();
    builder
        .line_number(true)
        .multi_line(params.multiline)
        // 与 rg 同策略：显式单文件 convert（NUL 换行符化，全文可搜）；
        // 遍历到的文件 quit（探测到 NUL 即停）。
        .binary_detection(if explicit_file {
            grep_searcher::BinaryDetection::convert(0)
        } else {
            grep_searcher::BinaryDetection::quit(0)
        });
    if mode == SearchMode::Content {
        builder
            .before_context(params.context_before)
            .after_context(params.context_after);
    }
    let mut searcher = builder.build();

    // 超时面额：以首次检查为基准的剩余时长（错误信息用）。
    let timeout_dur = params
        .deadline
        .map(|dl| dl.saturating_duration_since(Instant::now()));
    // sink 内命中截止时刻的标记（跨每文件 sink 共享）。
    let hit_deadline = Cell::new(false);
    let deadline_passed = || params.deadline.is_some_and(|dl| Instant::now() >= dl);

    let mut files = Vec::new();
    let mut counts = Vec::new();
    let mut content = GrepResult::default();
    let mut file_errors = Vec::new();

    for entry in build_walker(root, params)? {
        if deadline_passed() {
            break;
        }
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                file_errors.push(e.to_string());
                continue;
            }
        };
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();
        match mode {
            SearchMode::Files => {
                let mut found = false;
                let mut sink = grep_searcher::sinks::Lossy(|_line_number, _line| {
                    found = true;
                    Ok(false) // 首个匹配即停
                });
                if let Err(e) = searcher.search_path(&matcher, path, &mut sink) {
                    file_errors.push(format!("{}: {e}", path.display()));
                } else if found {
                    files.push(path.to_path_buf());
                }
                // 无匹配文件搜索过程无回调，无法中途打断；读完即查，
                // 避免一连串无匹配大文件读穿时间预算。
                if deadline_passed() {
                    hit_deadline.set(true);
                }
            }
            SearchMode::Count => {
                let hit = &hit_deadline;
                let n = Cell::new(0usize);
                let count = &n;
                let mut sink = grep_searcher::sinks::Lossy(move |_line_number, _line| {
                    if deadline_passed() {
                        hit.set(true);
                        return Ok(false);
                    }
                    // 按 sink 回调计匹配块数；相邻 multiline 匹配并块
                    // 的口径偏差见模块文档。
                    count.set(count.get() + 1);
                    Ok(true)
                });
                match searcher.search_path(&matcher, path, &mut sink) {
                    Ok(()) if n.get() > 0 => counts.push((path.to_path_buf(), n.get())),
                    Ok(()) => {}
                    Err(e) => file_errors.push(format!("{}: {e}", path.display())),
                }
            }
            SearchMode::Content => {
                // 与 rg 一致：二进制信号以「该文件有命中」为前提，搜索前
                // 记录基数用于判定。
                let matches_before = content.matches.len();
                let mut sink = CollectSink {
                    matches: &mut content.matches,
                    path,
                    deadline: params.deadline,
                    hit_deadline: &hit_deadline,
                    binary_hit: false,
                    quit_on_binary: !explicit_file,
                };
                if let Err(e) = searcher.search_path(&matcher, path, &mut sink) {
                    file_errors.push(format!("{}: {e}", path.display()));
                }
                // 与 rg 的 "binary file matches (found "\0" byte ...)" 对齐：
                // 遍历到的二进制文件被截断且有命中时给出信号而非静默；
                // 无命中文件与显式单文件 root（convert 策略）与 rg 一样
                // 静默。
                let binary_hit = sink.binary_hit;
                if binary_hit && !explicit_file && content.matches.len() > matches_before {
                    file_errors.push(format!(
                        "{}: binary file matches (NUL byte detected); results may be incomplete",
                        path.display()
                    ));
                }
            }
        }
        if hit_deadline.get() || deadline_passed() {
            break;
        }
    }

    if hit_deadline.get() || deadline_passed() {
        return Err(SearchError::Timeout(timeout_dur.unwrap_or(Duration::ZERO)));
    }

    let outcome = match mode {
        SearchMode::Files => SearchOutcome::Files(files),
        SearchMode::Count => SearchOutcome::Counts(counts),
        SearchMode::Content => SearchOutcome::Content(content),
    };
    Ok(SearchReport {
        outcome,
        file_errors,
    })
}

/// 遍历器：含隐藏文件、遵守 gitignore 系规则（含父目录与全局）、排除
/// VCS 目录、按 glob/类型过滤。
fn build_walker(root: &Path, params: &SearchParams) -> Result<ignore::Walk, SearchError> {
    let mut wb = ignore::WalkBuilder::new(root);
    wb.hidden(false)
        .follow_links(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .parents(true)
        .filter_entry(|e| {
            // 排除任意名为 .git/.svn/.hg 的条目（目录与同名文件，对齐
            // 原 `!.git` 等 glob 的 basename 语义）。
            !matches!(e.file_name().to_str(), Some(".git" | ".svn" | ".hg"))
        });

    if !params.glob_patterns.is_empty() {
        let mut ob = ignore::overrides::OverrideBuilder::new(root);
        for pat in params.glob_patterns {
            ob.add(pat).map_err(|e| SearchError::Glob(e.to_string()))?;
        }
        let overrides = ob.build().map_err(|e| SearchError::Glob(e.to_string()))?;
        wb.overrides(overrides);
    }

    if let Some(ft) = params.file_type.filter(|t| !t.is_empty()) {
        let mut tb = ignore::types::TypesBuilder::new();
        tb.add_defaults();
        if !tb.definitions().iter().any(|def| def.name() == ft) {
            return Err(SearchError::FileType(format!(
                "unrecognized file type: {ft}"
            )));
        }
        tb.select(ft);
        wb.types(
            tb.build()
                .map_err(|e| SearchError::FileType(e.to_string()))?,
        );
    }

    Ok(wb.build())
}

/// content 模式的收集 sink：匹配行与上下文行同构收集（`GrepMatch`
/// 的展示不区分两者，与原 JSON 解析路径一致）。
struct CollectSink<'a> {
    matches: &'a mut Vec<GrepMatch>,
    path: &'a Path,
    deadline: Option<Instant>,
    hit_deadline: &'a Cell<bool>,
    binary_hit: bool,
    /// 探测到二进制字节时是否停止该文件（quit 策略）；convert 策略
    /// （显式单文件）下须返回继续，转换才会生效。
    quit_on_binary: bool,
}

impl CollectSink<'_> {
    fn push(&mut self, line_number: Option<u64>, bytes: &[u8]) -> bool {
        if self.deadline.is_some_and(|dl| Instant::now() >= dl) {
            self.hit_deadline.set(true);
            return false;
        }
        self.matches.push(GrepMatch {
            path: self.path.to_path_buf(),
            line_number: line_number.unwrap_or(0) as usize,
            lines: String::from_utf8_lossy(bytes).into_owned(),
        });
        true
    }
}

impl Sink for CollectSink<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        Ok(self.push(mat.line_number(), mat.bytes()))
    }

    fn context(
        &mut self,
        _searcher: &Searcher,
        ctx: &SinkContext<'_>,
    ) -> Result<bool, Self::Error> {
        Ok(self.push(ctx.line_number(), ctx.bytes()))
    }

    fn binary_data(&mut self, _searcher: &Searcher, _offset: u64) -> Result<bool, Self::Error> {
        self.binary_hit = true;
        Ok(!self.quit_on_binary)
    }
}

#[cfg(test)]
#[path = "grep_engine_test.rs"]
mod tests;
