//! Helper for parsing ripgrep JSON output
//!
//! Ripgrep's `--json` flag produces structured output that can be parsed
//! to extract matches, file paths, and metadata without parsing text output.
//!
//! Example JSON lines:
//! ```json
//! {"type":"begin","data":{"path":{"text":"src/main.rs"}}}
//! {"type":"match","data":{"path":{"text":"src/main.rs"},"lines":{"text":"fn main()"},"line_number":1}}
//! {"type":"end","data":{"path":{"text":"src/main.rs"}}}
//! ```

use serde::Deserialize;
use std::fmt::Write;
use std::path::PathBuf;

/// A parsed ripgrep result containing matches and metadata
#[derive(Debug, Default)]
pub struct RipgrepResult {
    /// All matches found
    pub matches: Vec<RgMatch>,
    /// Files that were searched (whether they had matches or not)
    pub files_searched: Vec<PathBuf>,
}

impl RipgrepResult {
    /// Returns true if there are no matches
    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    /// Apply pagination to matches
    pub fn paginate(&self, limit: usize, offset: usize) -> (Vec<RgMatch>, bool) {
        paginate_matches(&self.matches, limit, offset)
    }

    /// Format matches with pagination applied
    pub fn format_paginated(&self, limit: usize, offset: usize, show_line_numbers: bool) -> String {
        let (paginated, _) = self.paginate(limit, offset);
        format_matches(&paginated, show_line_numbers)
    }

    /// Get unique file paths from all matches (preserves order of first appearance)
    pub fn unique_files(&self) -> Vec<PathBuf> {
        extract_file_paths(&self.matches)
    }

    /// Get unique file paths from paginated matches
    pub fn unique_files_paginated(&self, limit: usize, offset: usize) -> Vec<PathBuf> {
        let (paginated, _) = self.paginate(limit, offset);
        extract_file_paths(&paginated)
    }
}

/// A single match from ripgrep
#[derive(Debug, Clone)]
pub struct RgMatch {
    /// Absolute or relative path to the file
    pub path: PathBuf,
    /// Line number (1-indexed)
    pub line_number: usize,
    /// The matched line content
    pub lines: String,
    /// Column byte offset (if available)
    pub column: Option<usize>,
    /// Submatches within the line
    pub submatches: Vec<RgSubmatch>,
}

/// A submatch within a line
#[derive(Debug, Clone)]
pub struct RgSubmatch {
    /// The matched text
    pub text: String,
    /// Start byte offset
    pub start: usize,
    /// End byte offset
    pub end: usize,
}

/// Raw JSON types for deserialization
#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
#[allow(dead_code)]
enum RgMessage {
    #[serde(rename = "begin")]
    Begin { data: BeginData },
    #[serde(rename = "match")]
    Match { data: MatchData },
    #[serde(rename = "context")]
    Context { data: ContextData },
    #[serde(rename = "end")]
    End { data: EndData },
}

#[allow(dead_code)]
#[derive(Deserialize, Debug)]
struct BeginData {
    path: TextField,
}

#[derive(Deserialize, Debug)]
struct MatchData {
    path: TextField,
    lines: TextField,
    line_number: Option<usize>,
    absolute_offset: Option<usize>,
    #[serde(default)]
    submatches: Vec<SubmatchData>,
}

#[derive(Deserialize, Debug)]
struct ContextData {
    path: TextField,
    lines: TextField,
    line_number: Option<usize>,
    absolute_offset: Option<usize>,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct EndData {
    path: TextField,
}

#[derive(Deserialize, Debug)]
struct TextField {
    text: String,
}

#[derive(Deserialize, Debug)]
struct SubmatchData {
    #[serde(rename = "match")]
    match_field: TextField,
    start: usize,
    end: usize,
}

/// Parse ripgrep JSON output
///
/// Each line of the output should be a separate JSON object.
/// Returns a `RipgrepResult` containing all matches and metadata.
pub fn parse_json_output(json_lines: &str) -> RipgrepResult {
    let mut result = RipgrepResult::default();

    for line in json_lines.lines() {
        if line.trim().is_empty() {
            continue;
        }

        match serde_json::from_str::<RgMessage>(line) {
            Ok(RgMessage::Begin { data }) => {
                result.files_searched.push(PathBuf::from(data.path.text));
            }
            Ok(RgMessage::Match { data }) => {
                let submatches = data
                    .submatches
                    .into_iter()
                    .map(|sm| RgSubmatch {
                        text: sm.match_field.text,
                        start: sm.start,
                        end: sm.end,
                    })
                    .collect();

                result.matches.push(RgMatch {
                    path: PathBuf::from(data.path.text),
                    line_number: data.line_number.unwrap_or(0),
                    lines: data.lines.text,
                    column: data.absolute_offset,
                    submatches,
                });
            }
            Ok(RgMessage::Context { data }) => {
                // Context lines are treated like matches for display purposes
                // but have no submatches
                result.matches.push(RgMatch {
                    path: PathBuf::from(data.path.text),
                    line_number: data.line_number.unwrap_or(0),
                    lines: data.lines.text,
                    column: data.absolute_offset,
                    submatches: vec![],
                });
            }
            Ok(RgMessage::End { .. }) => {
                // Track file completion if needed
            }
            Err(e) => {
                tracing::debug!("Failed to parse ripgrep JSON line: {}", e);
            }
        }
    }

    result
}

/// Apply limit and offset to matches, return the subset and whether it was truncated
pub fn paginate_matches(matches: &[RgMatch], limit: usize, offset: usize) -> (Vec<RgMatch>, bool) {
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
    let paginated: Vec<RgMatch> = matches.iter().skip(skip).take(take).cloned().collect();

    (paginated, was_truncated)
}

/// Format matches as human-readable text (ripgrep style)
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
pub fn format_matches(matches: &[RgMatch], show_line_numbers: bool) -> String {
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
pub fn extract_file_paths(matches: &[RgMatch]) -> Vec<PathBuf> {
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
#[path = "rg_helper_test.rs"]
mod tests;
