//! File completion module for @-mentions in input
//!
//! Provides file scanning, caching, and fuzzy matching for file path completion.

use crate::components::CompletionList;

/// Maximum number of files to scan (prevents hanging on huge repos)
const MAX_FILES_TO_SCAN: usize = 1000;
/// Maximum number of files to display (performance)
const MAX_FILES_TO_DISPLAY: usize = 50;

/// Manages file completion state and caching
#[derive(Debug)]
pub struct FileCompletion {
    /// The completion list UI component
    completion: CompletionList<String>,
    /// Current query string (text after @)
    query: String,
    /// Position of '@' in the input
    query_start_pos: usize,
    /// Working directory for file scanning
    working_dir: std::path::PathBuf,
    /// Cached file list
    all_files: Vec<String>,
    /// Whether cache needs refresh
    cache_dirty: bool,
    /// Total files found (may exceed cache limit)
    total_files_scanned: usize,
    /// Whether scan hit `MAX_FILES_TO_SCAN` limit
    files_truncated: bool,
    /// Whether completion is currently active (independent of list visibility)
    active: bool,
}

impl Default for FileCompletion {
    fn default() -> Self {
        Self::new()
    }
}

impl FileCompletion {
    /// Create a new `FileCompletion` instance
    pub fn new() -> Self {
        Self {
            completion: CompletionList::new(),
            query: String::new(),
            query_start_pos: 0,
            working_dir: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            all_files: Vec::new(),
            cache_dirty: true,
            total_files_scanned: 0,
            files_truncated: false,
            active: false,
        }
    }

    /// Set the working directory for file scanning
    pub fn set_working_dir(&mut self, path: impl Into<std::path::PathBuf>) {
        self.working_dir = path.into();
        self.cache_dirty = true;
    }

    /// Start file completion at the given cursor position
    pub fn start(&mut self, cursor_pos: usize) {
        self.query.clear();
        self.query_start_pos = cursor_pos;
        self.active = true;
        self.ensure_cache();
        self.refresh_list();
        // Set completion list visible since we're now active
        // visibility will be checked via is_visible() which checks active && !is_empty()
    }

    /// Check if file completion is currently active
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Check if completion should be visible (active and has items)
    pub fn is_visible(&self) -> bool {
        self.active && !self.completion.is_empty()
    }

    /// Get the currently selected file path
    pub fn get_selected(&self) -> Option<&str> {
        self.completion.get_selected().map(|s| s.as_str())
    }

    /// Move selection to the next item
    pub fn next(&mut self) {
        self.completion.next();
    }

    /// Move selection to the previous item
    pub fn prev(&mut self) {
        self.completion.prev();
    }

    /// Handle input character during file completion
    /// Returns true if the input was consumed
    pub fn handle_input(&mut self, c: char, _cursor_pos: usize) -> bool {
        match c {
            '\n' | '\r' => {
                self.accept();
                true
            }
            '\x1b' | '\x03' => {
                // ESC or Ctrl-C
                self.cancel();
                true
            }
            '\t' => {
                self.prev();
                true
            }
            '\x08' => {
                // Backspace: remove last char from query and refresh
                self.query.pop();
                self.refresh_list();
                // Keep completion active even if query is empty
                true
            }
            ' ' if self.query.is_empty() => {
                // Space without query - cancel completion
                self.cancel();
                false // Allow space to be inserted
            }
            c => {
                self.query.push(c);
                self.refresh_list();
                true
            }
        }
    }

    /// Accept the current selection
    /// Returns the selected file path and the range to replace (start, end)
    pub fn accept(&mut self) -> Option<(String, usize, usize)> {
        self.completion.get_selected().cloned().map(|selected| {
            let start = self.query_start_pos;
            let end = self.query_start_pos + self.query.len();
            self.active = false;
            self.query.clear();
            self.completion.hide();
            (selected, start, end)
        })
    }

    /// Cancel file completion
    pub fn cancel(&mut self) {
        self.active = false;
        self.query.clear();
        self.completion.hide();
    }

    /// Hide the completion list (alias for cancel)
    pub fn hide(&mut self) {
        self.cancel();
    }

    /// Get items for rendering
    pub fn items(&self) -> &[String] {
        self.completion.items()
    }

    /// Get currently selected index
    pub fn selected_index(&self) -> usize {
        self.completion.selected_index()
    }

    /// Get the query start position
    pub fn query_start_pos(&self) -> usize {
        self.query_start_pos
    }

    /// Check if files were truncated during scanning
    pub fn was_truncated(&self) -> bool {
        self.files_truncated
    }

    /// Get total number of files scanned
    pub fn total_scanned(&self) -> usize {
        self.total_files_scanned
    }

    /// Get the number of items in the completion list
    pub fn len(&self) -> usize {
        self.completion.len()
    }

    /// Check if the completion list is empty
    pub fn is_empty(&self) -> bool {
        self.completion.len() == 0
    }

    /// Get current scroll offset (for sticky scrolling)
    pub fn scroll_offset(&self) -> usize {
        self.completion.scroll_offset()
    }

    /// Ensure selected item is visible within the given max visible count
    pub fn ensure_visible(&mut self, max_visible: usize) {
        self.completion.ensure_visible(max_visible);
    }

    /// Get mutable access to the underlying completion list
    pub fn completion_list_mut(&mut self) -> &mut CompletionList<String> {
        &mut self.completion
    }

    /// Refresh the file list based on current query
    fn refresh_list(&mut self) {
        let filtered = if self.query.is_empty() {
            self.all_files
                .iter()
                .take(MAX_FILES_TO_DISPLAY)
                .cloned()
                .collect()
        } else {
            let query_lower = self.query.to_lowercase();
            let mut filtered: Vec<(String, i32)> = self
                .all_files
                .iter()
                .filter_map(|file| {
                    let file_lower = file.to_lowercase();
                    // Use fuzzy match first, fallback to substring match
                    if let Some(score) = Self::fuzzy_match(file, &query_lower) {
                        Some((file.clone(), score))
                    } else if file_lower.contains(&query_lower) {
                        // Fallback: simple substring match with lower score
                        let pos = file_lower.find(&query_lower).unwrap_or(0);
                        Some((file.clone(), 10 - pos as i32))
                    } else {
                        None
                    }
                })
                .collect();
            // Sort by score descending
            filtered.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
            filtered
                .into_iter()
                .map(|(file, _)| file)
                .take(MAX_FILES_TO_DISPLAY)
                .collect()
        };
        // Use set_items to avoid changing visibility - visibility is controlled by active flag
        self.completion.set_items(filtered);
    }

    /// Ensure file cache is populated (lazy loading)
    fn ensure_cache(&mut self) {
        if self.cache_dirty || self.all_files.is_empty() {
            let (files, count, truncated) = self.scan_files();
            self.all_files = files;
            self.total_files_scanned = count;
            self.files_truncated = truncated;
            self.cache_dirty = false;
        }
    }

    /// Scan files recursively with limits, respecting .gitignore
    /// Returns (files, `total_count`, `was_truncated`)
    fn scan_files(&self) -> (Vec<String>, usize, bool) {
        let gitignore = self.load_gitignore();
        let mut files = Vec::with_capacity(MAX_FILES_TO_SCAN);
        let mut count = 0usize;
        Self::scan_recursive(&self.working_dir, "", &gitignore, &mut files, &mut count);
        let truncated = count >= MAX_FILES_TO_SCAN;
        // Sort: shorter paths first, then alphabetically
        files.sort_by(|a, b| {
            let a_parts = a.matches('/').count();
            let b_parts = b.matches('/').count();
            match a_parts.cmp(&b_parts) {
                std::cmp::Ordering::Equal => a.cmp(b),
                other => other,
            }
        });
        (files, count, truncated)
    }

    /// Load .gitignore patterns
    fn load_gitignore(&self) -> Vec<String> {
        let gitignore_path = self.working_dir.join(".gitignore");
        let mut patterns = vec![
            ".git".to_string(),
            ".gitignore".to_string(),
            "target/".to_string(),
            "node_modules/".to_string(),
        ];
        if let Ok(content) = std::fs::read_to_string(&gitignore_path) {
            for line in content.lines() {
                let line = line.trim();
                if !line.is_empty() && !line.starts_with('#') {
                    patterns.push(line.to_string());
                }
            }
        }
        patterns
    }

    /// Check if path matches gitignore pattern
    fn matches_gitignore(path: &str, patterns: &[String]) -> bool {
        let path = path.trim_end_matches('/');
        for pattern in patterns {
            let pattern = pattern.trim();
            if pattern.is_empty() {
                continue;
            }
            // Handle directory patterns (ending with /)
            let is_dir_pattern = pattern.ends_with('/');
            let clean_pattern = pattern.trim_end_matches('/');
            // Exact match
            if path == clean_pattern {
                return true;
            }
            // Check if any component matches
            if path.split('/').any(|part| {
                if is_dir_pattern {
                    part == clean_pattern
                } else {
                    part == clean_pattern || path.ends_with(&format!("/{clean_pattern}"))
                }
            }) {
                return true;
            }
            // Simple glob-like matching for *
            if clean_pattern.contains('*') {
                let parts: Vec<&str> = clean_pattern.split('*').collect();
                if parts.len() == 2 {
                    let prefix = parts[0];
                    let suffix = parts[1];
                    if (prefix.is_empty() || path.contains(prefix))
                        && (suffix.is_empty() || path.contains(suffix))
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Recursively scan directory with limit
    fn scan_recursive(
        base_dir: &std::path::Path,
        prefix: &str,
        gitignore: &[String],
        files: &mut Vec<String>,
        count: &mut usize,
    ) {
        if *count >= MAX_FILES_TO_SCAN {
            return;
        }
        let current_dir = if prefix.is_empty() {
            base_dir.to_path_buf()
        } else {
            base_dir.join(prefix)
        };
        if let Ok(entries) = std::fs::read_dir(&current_dir) {
            for entry in entries.flatten() {
                if *count >= MAX_FILES_TO_SCAN {
                    break;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                let relative_path = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}/{name}")
                };
                // Check gitignore
                if Self::matches_gitignore(&relative_path, gitignore) {
                    continue;
                }
                if let Ok(metadata) = entry.metadata() {
                    if metadata.is_dir() {
                        files.push(format!("{relative_path}/"));
                        *count += 1;
                        // Recurse into subdirectory
                        Self::scan_recursive(base_dir, &relative_path, gitignore, files, count);
                    } else if metadata.is_file() {
                        files.push(relative_path);
                        *count += 1;
                    }
                }
            }
        }
    }

    /// Case-insensitive fuzzy matching
    fn fuzzy_match(text: &str, pattern: &str) -> Option<i32> {
        if pattern.is_empty() {
            return Some(0);
        }
        let pattern_lower = pattern.to_lowercase();
        let pattern_chars: Vec<char> = pattern_lower.chars().collect();
        let mut pattern_idx = 0;
        let mut score = 0i32;
        let mut consecutive_bonus = 0;
        let mut prev_match_idx: Option<usize> = None;

        for (text_idx, c) in text.chars().enumerate() {
            if pattern_idx < pattern_chars.len() {
                let pc = pattern_chars[pattern_idx];
                let c_lower = c.to_lowercase().next()?;
                if c_lower == pc {
                    if let Some(prev) = prev_match_idx {
                        if text_idx == prev + 1 {
                            consecutive_bonus += 1;
                            score += consecutive_bonus;
                        }
                    }
                    if let Some(prev) = prev_match_idx {
                        if text_idx > 0 {
                            let prev_char = text.chars().nth(prev)?;
                            if prev_char == '/'
                                || prev_char == '-'
                                || prev_char == '_'
                                || prev_char == '.'
                            {
                                score += 3;
                            }
                        }
                    }
                    score += 1;
                    prev_match_idx = Some(text_idx);
                    pattern_idx += 1;
                }
            }
        }

        if pattern_idx == pattern_chars.len() {
            score -= (text.len().saturating_sub(pattern_lower.len())) as i32;
            Some(score)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuzzy_match() {
        assert!(FileCompletion::fuzzy_match("hello.rs", "hr").is_some());
        assert!(FileCompletion::fuzzy_match("hello.rs", "HR").is_some()); // case insensitive
        assert!(FileCompletion::fuzzy_match("hello.rs", "xyz").is_none());

        // Test scoring
        let score1 = FileCompletion::fuzzy_match("src/main.rs", "main").unwrap();
        let score2 = FileCompletion::fuzzy_match("src/main.rs", "smr").unwrap();
        assert!(score1 > score2); // exact match scores higher
    }

    #[test]
    fn test_gitignore_matching() {
        let patterns = vec![
            "target/".to_string(),
            "*.log".to_string(),
            "secret".to_string(),
        ];

        assert!(FileCompletion::matches_gitignore("target/", &patterns));
        assert!(FileCompletion::matches_gitignore("target/debug", &patterns));
        assert!(FileCompletion::matches_gitignore("app.log", &patterns));
        assert!(FileCompletion::matches_gitignore("secret", &patterns));
        assert!(!FileCompletion::matches_gitignore("src/main.rs", &patterns));
    }
}
