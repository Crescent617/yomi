//! grep 工具的输出结构与格式化：匹配记录（`GrepMatch`）、结果集
//! （`GrepResult`）与分页/格式化/文件提取。匹配的产生见
//! `utils::grep_engine`（进程内搜索引擎）。

use std::fmt::Write;
use std::path::PathBuf;

/// A parsed search result containing matches and metadata
#[derive(Debug, Default)]
pub struct GrepResult {
    /// All matches found
    pub matches: Vec<GrepMatch>,
}

impl GrepResult {
    /// Returns true if there are no matches
    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    /// Apply pagination to matches
    pub fn paginate(&self, limit: usize, offset: usize) -> (Vec<GrepMatch>, bool) {
        paginate_matches(&self.matches, limit, offset)
    }

    /// Format matches with pagination applied
    pub fn format_paginated(&self, limit: usize, offset: usize, show_line_numbers: bool) -> String {
        let (paginated, _) = self.paginate(limit, offset);
        format_matches(&paginated, show_line_numbers)
    }

    /// Get unique file paths from paginated matches
    pub fn unique_files_paginated(&self, limit: usize, offset: usize) -> Vec<PathBuf> {
        let (paginated, _) = self.paginate(limit, offset);
        extract_file_paths(&paginated)
    }
}

/// A single match
#[derive(Debug, Clone)]
pub struct GrepMatch {
    /// Absolute or relative path to the file
    pub path: PathBuf,
    /// Line number (1-indexed)
    pub line_number: usize,
    /// The matched line content
    pub lines: String,
}

/// Apply limit and offset to matches, return the subset and whether it was truncated
pub fn paginate_matches(
    matches: &[GrepMatch],
    limit: usize,
    offset: usize,
) -> (Vec<GrepMatch>, bool) {
    if matches.is_empty() {
        return (Vec::new(), false);
    }

    let skip = offset.min(matches.len());
    let remaining = matches.len() - skip;

    let take = if limit == 0 {
        remaining
    } else {
        remaining.min(limit)
    };

    let was_truncated = limit > 0 && remaining > limit;
    let paginated: Vec<GrepMatch> = matches.iter().skip(skip).take(take).cloned().collect();

    (paginated, was_truncated)
}

/// Format matches as human-readable text（按文件分组、可选行号）
///
/// Format:
/// ```text
/// path/to/file.rs
/// 12:    matched line content
/// 34:    another match
///
/// path/to/another.rs
/// 56:    matched line
/// ```
pub fn format_matches(matches: &[GrepMatch], show_line_numbers: bool) -> String {
    if matches.is_empty() {
        return "No matches found".to_string();
    }

    let mut result = String::new();
    let mut current_path: Option<&std::path::Path> = None;

    for (i, m) in matches.iter().enumerate() {
        // Print file path when it changes
        if current_path != Some(&m.path) {
            if i > 0 {
                result.push('\n'); // Empty line between files
            }
            current_path = Some(&m.path);
            result.push_str(&m.path.display().to_string());
            result.push('\n');
        }

        // Handle multiline content - split and number each line
        // Use split('\n') instead of lines() to preserve empty lines
        let split_lines: Vec<&str> = m.lines.split('\n').collect();
        // If the original ends with \n, split gives an empty string at the end - skip it
        let lines_to_print = if m.lines.ends_with('\n') && split_lines.last() == Some(&"") {
            &split_lines[..split_lines.len().saturating_sub(1)]
        } else {
            &split_lines[..]
        };

        for (line_idx, line_content) in lines_to_print.iter().enumerate() {
            if show_line_numbers && m.line_number > 0 {
                let current_line_num = m.line_number + line_idx;
                let _ = write!(result, "{current_line_num}:{line_content}");
            } else {
                result.push_str(line_content);
            }
            result.push('\n');
        }
    }

    result
}

/// Extract unique file paths from matches (preserves order of first appearance)
pub fn extract_file_paths(matches: &[GrepMatch]) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    let mut paths = Vec::new();

    for m in matches {
        if seen.insert(m.path.clone()) {
            paths.push(m.path.clone());
        }
    }

    paths
}

#[cfg(test)]
#[path = "grep_output_test.rs"]
mod tests;
