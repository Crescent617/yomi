//! File completion module for @-mentions in input
//!
//! Provides async file scanning with high-performance fzf-style fuzzy matching via nucleo.

use crate::components::CompletionList;
use ignore::WalkBuilder;
use nucleo::Nucleo;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::task::JoinHandle;

/// Maximum number of files to index (safety limit for huge repos)
const MAX_FILES: usize = 100_000;
/// Maximum number of files to display (performance)
const MAX_FILES_TO_DISPLAY: usize = 50;

const PLACEHOLDER_SCANNING: &str = "Scanning files...";
const PLACEHOLDER_NO_MATCHES: &str = "No matches";

/// Completion mode for file completion
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionMode {
    /// Normal fuzzy matching mode (default)
    Normal,
    /// Root directory listing mode (triggered by @/ or @/~)
    RootDir,
}

/// Manages async file completion with nucleo-powered fuzzy matching
pub struct FileCompletion {
    completion: CompletionList<String>,
    query: String,
    query_start_pos: usize,
    working_dir: std::path::PathBuf,
    home_dir: Option<std::path::PathBuf>,
    nucleo: Option<Nucleo<String>>,
    /// Whether scan is complete
    scan_complete: Arc<AtomicBool>,
    scan_handle: Option<JoinHandle<()>>,
    active: bool,
    /// Current completion mode
    mode: CompletionMode,
    /// Cached root directory entries for root dir mode
    root_dir_entries: Vec<String>,
    /// Last processed query to avoid redundant refreshes
    last_refresh_query: String,
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
            home_dir: std::env::var("HOME").ok().map(std::path::PathBuf::from),
            nucleo: None,
            scan_complete: Arc::new(AtomicBool::new(false)),
            scan_handle: None,
            active: false,
            mode: CompletionMode::Normal,
            root_dir_entries: Vec::new(),
            last_refresh_query: String::new(),
        }
    }

    /// Set the working directory for file scanning
    pub fn set_working_dir(&mut self, path: impl Into<std::path::PathBuf>) {
        self.working_dir = path.into();
        self.cancel_scan();
    }

    /// Cancel any running background scan
    fn cancel_scan(&mut self) {
        if let Some(handle) = self.scan_handle.take() {
            handle.abort();
        }
        self.nucleo = None;
        self.scan_complete.store(false, Ordering::SeqCst);
    }

    /// Start file completion at the given cursor position
    ///
    /// If `initial_content` starts with "/" or "~", enters root directory mode
    /// which shows first-level directory contents for progressive completion.
    pub fn start(&mut self, cursor_pos: usize) {
        self.start_with_content(cursor_pos, "");
    }

    /// Start file completion with initial content
    ///
    /// # Arguments
    /// * `cursor_pos` - The position in the input where @ was typed
    /// * `initial_content` - The content after @ (e.g., "/", "~/", "src/")
    pub fn start_with_content(&mut self, cursor_pos: usize, initial_content: &str) {
        self.query = initial_content.to_string();
        self.query_start_pos = cursor_pos;
        self.active = true;
        self.mode = CompletionMode::Normal;
        self.root_dir_entries.clear();
        self.last_refresh_query.clear();

        self.cancel_scan();

        // Check if we should enter root directory mode (absolute paths or ~)
        if initial_content.starts_with('/')
            || initial_content == "~"
            || initial_content.starts_with("~/")
        {
            self.mode = CompletionMode::RootDir;
            self.populate_root_dir_entries(initial_content);
            return;
        }

        // Initialize nucleo matcher with default config
        let config = nucleo::Config::DEFAULT;
        self.nucleo = Some(Nucleo::new(config, Arc::new(|| {}), None, 1));

        // Launch async background scan
        self.start_async_scan();

        // Show scanning message immediately
        self.set_placeholder(PLACEHOLDER_SCANNING);
    }

    /// Populate root directory entries for root dir mode
    ///
    /// Note: `~user` syntax is not supported; only `~` and `~/` are expanded
    /// to the current user's home directory.
    fn populate_root_dir_entries(&mut self, initial_content: &str) {
        let target_dir = if initial_content.starts_with('~') {
            // Handle ~/path or ~ (not ~user)
            if let Some(home) = &self.home_dir {
                if initial_content == "~" || initial_content == "~/" {
                    home.clone()
                } else {
                    home.join(&initial_content[2..])
                }
            } else {
                self.set_placeholder("Home directory not found");
                return;
            }
        } else if initial_content == "/" {
            // Handle /
            std::path::PathBuf::from("/")
        } else {
            // Handle /path (absolute path)
            std::path::PathBuf::from(initial_content)
        };

        let mut entries = Vec::new();

        if let Ok(reader) = std::fs::read_dir(&target_dir) {
            for entry in reader.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();

                    // Skip hidden files (starting with .)
                    if name_str.starts_with('.') {
                        continue;
                    }

                    if file_type.is_dir() {
                        entries.push(format!("{name_str}/"));
                    } else if file_type.is_file() {
                        entries.push(name_str.to_string());
                    }
                }
            }
        }

        // Sort directories first, then files
        entries.sort_by(|a, b| {
            let a_is_dir = a.ends_with('/');
            let b_is_dir = b.ends_with('/');
            match (a_is_dir, b_is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.cmp(b),
            }
        });

        self.root_dir_entries.clone_from(&entries);
        self.last_refresh_query = self.query.clone();
        self.completion.show(entries);
    }

    /// Start async background file scan
    fn start_async_scan(&mut self) {
        let working_dir = self.working_dir.clone();
        let scan_complete = Arc::clone(&self.scan_complete);

        // Get injector from nucleo for adding items
        let injector = self.nucleo.as_ref().unwrap().injector();

        let handle = tokio::task::spawn_blocking(move || {
            let walker = WalkBuilder::new(&working_dir)
                .hidden(false)
                .git_ignore(true)
                .git_global(true)
                .git_exclude(true)
                .follow_links(false)
                .max_depth(None)
                .build();

            let mut count = 0usize;
            for entry in walker.flatten() {
                // Cap at MAX_FILES + 1 (100001)
                if count > MAX_FILES {
                    break;
                }

                // Skip .git directory
                if entry.path().components().any(|c| c.as_os_str() == ".git") {
                    continue;
                }

                let path_str = if entry.file_type().is_some_and(|ft| ft.is_file()) {
                    entry
                        .path()
                        .strip_prefix(&working_dir)
                        .ok()
                        .map(|p| p.to_string_lossy().into_owned())
                } else if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                    entry.path().strip_prefix(&working_dir).ok().and_then(|p| {
                        let s = p.to_string_lossy();
                        if s.is_empty() {
                            None
                        } else {
                            Some(format!("{s}/"))
                        }
                    })
                } else {
                    None
                };

                if let Some(path) = path_str {
                    let _ = injector.push(path, |s, cols| {
                        cols[0] = s.clone().into();
                    });
                    count += 1;
                }
            }

            scan_complete.store(true, Ordering::SeqCst);
        });

        self.scan_handle = Some(handle);
    }

    /// Check if file completion is currently active
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Get the current completion mode
    pub fn mode(&self) -> CompletionMode {
        self.mode
    }

    /// Get the currently selected file path
    pub fn get_selected(&self) -> Option<&str> {
        self.completion.get_selected().map(|s| s.as_str())
    }

    /// Build full path for root dir mode from current query and selected item.
    fn build_full_path(&self, selected: &str) -> String {
        if self.query.ends_with('/') || self.query.is_empty() {
            format!("{}{}", self.query, selected)
        } else if let Some(idx) = self.query.rfind('/') {
            format!("{}{}", &self.query[..=idx], selected)
        } else {
            format!("{}{}", self.query, selected)
        }
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
    pub fn handle_input(&mut self, c: char, _cursor_pos: usize) -> bool {
        match c {
            '\n' | '\r' => {
                self.accept();
                true
            }
            '\x1b' | '\x03' => {
                self.cancel();
                true
            }
            '\t' => {
                self.prev();
                true
            }
            '\x08' => {
                // Backspace - remove last char and check for mode transition
                self.query.pop();
                self.update_mode_based_on_query();
                true
            }
            ' ' if self.query.is_empty() => {
                self.cancel();
                false
            }
            c => {
                self.query.push(c);
                // Check if we need to switch to/from root dir mode
                self.update_mode_based_on_query();
                true
            }
        }
    }

    /// Accept the current selection
    pub fn accept(&mut self) -> Option<(String, usize, usize)> {
        self.completion
            .get_selected()
            .cloned()
            .and_then(|selected| {
                if Self::is_placeholder(&selected) {
                    return None;
                }
                let result = self.build_result_path(&selected);
                let start = self.query_start_pos;
                let end = self.query_start_pos + self.query.len();
                self.active = false;
                self.query.clear();
                self.completion.hide();
                Some((result, start, end))
            })
    }

    /// Check if the currently selected item is a directory
    pub fn is_selected_dir(&self) -> bool {
        self.completion
            .get_selected()
            .is_some_and(|s| s.ends_with('/'))
    }

    /// Check if the given item is a placeholder (not a real file/directory)
    fn is_placeholder(item: &str) -> bool {
        item == PLACEHOLDER_SCANNING || item == PLACEHOLDER_NO_MATCHES
    }

    /// Build result path based on current mode
    fn build_result_path(&self, selected: &str) -> String {
        match self.mode {
            CompletionMode::RootDir => self.build_full_path(selected),
            CompletionMode::Normal => selected.to_string(),
        }
    }

    /// Get the full path of the currently selected item without closing completion.
    /// Returns `None` for placeholder items.
    pub fn selected_full_path(&self) -> Option<String> {
        let selected = self.completion.get_selected().cloned()?;
        if Self::is_placeholder(&selected) {
            return None;
        }
        Some(self.build_result_path(&selected))
    }

    /// Reset query and `last_refresh_query` so completion stays open for continued searching.
    /// Note: `query_start_pos` is NOT updated to allow continued path completion from the original @ position.
    pub fn reset_for_continue(&mut self) {
        self.query.clear();
        self.last_refresh_query.clear();
    }

    /// Accept and continue into the selected directory (for progressive completion)
    /// Returns the path to append to the current query
    pub fn accept_and_continue(&mut self) -> Option<String> {
        self.completion
            .get_selected()
            .cloned()
            .and_then(|selected| {
                if Self::is_placeholder(&selected) {
                    return None;
                }

                // Only continue if it's a directory
                if !selected.ends_with('/') {
                    // It's a file, accept normally and exit
                    self.active = false;
                    self.query.clear();
                    self.completion.hide();
                    return Some(selected);
                }

                // It's a directory - update query and refresh
                let new_query = self.build_full_path(&selected);
                self.query.clone_from(&new_query);
                self.populate_root_dir_entries(&new_query);
                Some(selected)
            })
    }

    /// Cancel file completion
    pub fn cancel(&mut self) {
        self.active = false;
        self.query.clear();
        self.mode = CompletionMode::Normal;
        self.root_dir_entries.clear();
        self.last_refresh_query.clear();
        self.completion.hide();
        self.cancel_scan();
    }

    /// Hide the completion list
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

    /// Get the current query string
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Sync query from current input content and cursor position
    pub fn sync_query(&mut self, content: &str, cursor_pos: usize) {
        if cursor_pos >= self.query_start_pos {
            let new_query = &content[self.query_start_pos..cursor_pos];
            if new_query != self.query {
                self.query = new_query.to_string();
                // Check if we need to transition to/from root dir mode
                self.update_mode_based_on_query();
            }
        }
    }

    /// Update completion mode based on current query
    fn update_mode_based_on_query(&mut self) {
        // Root dir mode: absolute paths starting with / or ~ (including multi-level)
        let should_be_root_mode =
            self.query.starts_with('/') || self.query == "~" || self.query.starts_with("~/");

        match (self.mode, should_be_root_mode) {
            (CompletionMode::Normal, true) => {
                // Transition to root dir mode
                self.mode = CompletionMode::RootDir;
                self.cancel_scan();
                let query = self.query.clone();
                self.populate_root_dir_entries(&query);
            }
            (CompletionMode::RootDir, false) => {
                // Transition back to normal mode
                self.mode = CompletionMode::Normal;
                self.root_dir_entries.clear();
                // Restart normal scan
                let config = nucleo::Config::DEFAULT;
                self.nucleo = Some(Nucleo::new(config, Arc::new(|| {}), None, 1));
                self.start_async_scan();
                self.set_placeholder(PLACEHOLDER_SCANNING);
            }
            (CompletionMode::RootDir, true)
                // Still in root dir mode, only reload if path ends with '/'
                // Otherwise just refresh the filter without reloading
                if self.query.ends_with('/') =>
            {
                let query = self.query.clone();
                self.populate_root_dir_entries(&query);
            }
            _ => {} // No change needed
        }
    }

    /// Get total number of files indexed
    pub fn total_files(&self) -> usize {
        self.nucleo
            .as_ref()
            .map_or(0, |n| n.snapshot().item_count() as usize)
    }

    /// Check if file list was truncated at `MAX_FILES`
    pub fn is_truncated(&self) -> bool {
        self.total_files() > MAX_FILES
    }

    /// Check if scan is complete
    pub fn is_scan_complete(&self) -> bool {
        self.scan_complete.load(Ordering::SeqCst)
    }

    /// Get the number of items in the completion list
    pub fn len(&self) -> usize {
        self.completion.len()
    }

    /// Check if the completion list is empty
    pub fn is_empty(&self) -> bool {
        self.completion.len() == 0
    }

    /// Get current scroll offset
    pub fn scroll_offset(&self) -> usize {
        self.completion.scroll_offset()
    }

    /// Ensure selected item is visible
    pub fn ensure_visible(&mut self, max_visible: usize) {
        self.completion.ensure_visible(max_visible);
    }

    /// Get mutable access to the underlying completion list
    pub fn completion_list_mut(&mut self) -> &mut CompletionList<String> {
        &mut self.completion
    }

    /// Update the file list based on current query
    /// Call this periodically (e.g., every frame) for real-time updates
    pub fn refresh_list(&mut self) {
        match self.mode {
            CompletionMode::RootDir => self.refresh_root_dir_list(),
            CompletionMode::Normal => self.refresh_normal_list(),
        }
    }

    /// Refresh list for root directory mode with filtering
    fn refresh_root_dir_list(&mut self) {
        // Avoid redundant refreshes if query hasn't changed
        if self.query == self.last_refresh_query {
            return;
        }
        self.last_refresh_query.clone_from(&self.query);

        // Check if query ends with '/' (user just entered a directory)
        if self.query.ends_with('/') {
            // Re-populate entries for the new directory
            let query = self.query.clone();
            self.populate_root_dir_entries(&query);
            return;
        }

        // Check if query is a directory path without trailing slash
        // e.g., "/Applications" - check if it's an existing directory
        if !self.query.is_empty() {
            let path = std::path::PathBuf::from(&self.query);
            if path.is_dir() {
                // It's a directory but without trailing slash
                let query_with_slash = format!("{}/", self.query);
                self.populate_root_dir_entries(&query_with_slash);
                return;
            }
        }

        // For filtering, extract the last path component
        let filter = self.query.rsplit('/').next().unwrap_or(&self.query);

        if filter.is_empty() {
            // Show all entries without filtering
            if self.root_dir_entries.is_empty() {
                self.set_placeholder(PLACEHOLDER_NO_MATCHES);
            } else {
                // Preserve selection when showing all entries
                self.completion
                    .update_items_preserving_selection(self.root_dir_entries.clone());
            }
            return;
        }

        // Filter entries based on the last path component
        let filter_lc = filter.to_lowercase();
        let filtered: Vec<String> = self
            .root_dir_entries
            .iter()
            .filter(|entry| {
                let entry_name = entry.trim_end_matches('/');
                entry_name.to_lowercase().starts_with(&filter_lc)
            })
            .take(MAX_FILES_TO_DISPLAY)
            .cloned()
            .collect();

        if filtered.is_empty() {
            self.set_placeholder(PLACEHOLDER_NO_MATCHES);
        } else {
            self.completion.update_items_preserving_selection(filtered);
        }
    }

    /// Refresh list for normal mode using nucleo fuzzy matching
    fn refresh_normal_list(&mut self) {
        let nucleo_mut = match self.nucleo.as_mut() {
            Some(n) => n,
            None => return,
        };

        // Process newly injected files and pattern matching
        nucleo_mut.tick(0);

        // Update nucleo pattern (last bool: invert match)
        nucleo_mut.pattern.reparse(
            0,
            &self.query,
            nucleo::pattern::CaseMatching::Smart,
            nucleo::pattern::Normalization::Smart,
            false,
        );

        // Tick again for pattern matching to take effect
        nucleo_mut.tick(0);

        let snapshot = nucleo_mut.snapshot();
        let total_scanned = snapshot.item_count() as usize;
        let match_count = snapshot.matched_item_count();

        // Still scanning and no files yet
        if total_scanned == 0 {
            self.set_placeholder(PLACEHOLDER_SCANNING);
            return;
        }

        if self.query.is_empty() {
            if match_count > 0 {
                let limit = (MAX_FILES_TO_DISPLAY as u32).min(match_count);
                let items: Vec<String> = snapshot
                    .matched_items(0..limit)
                    .map(|item| item.data.clone())
                    .collect();
                self.completion.update_items_preserving_selection(items);
            } else {
                self.set_placeholder(PLACEHOLDER_SCANNING);
            }
            return;
        }

        if match_count == 0 {
            if total_scanned == 0 {
                self.set_placeholder(PLACEHOLDER_SCANNING);
            } else {
                self.set_placeholder(PLACEHOLDER_NO_MATCHES);
            }
            return;
        }

        let limit = (MAX_FILES_TO_DISPLAY as u32).min(match_count);
        let items: Vec<String> = snapshot
            .matched_items(0..limit)
            .map(|item| item.data.clone())
            .collect();

        self.completion.update_items_preserving_selection(items);
    }

    fn set_placeholder(&mut self, msg: &str) {
        self.completion.set_items(vec![msg.to_string()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_completion_new() {
        let fc = FileCompletion::new();
        assert!(!fc.is_active());
        assert_eq!(fc.total_files(), 0);
    }

    #[test]
    fn test_nucleo_basic() {
        // Create nucleo matcher
        let config = nucleo::Config::DEFAULT;
        let mut nucleo = Nucleo::<String>::new(config, Arc::new(|| {}), None, 1);

        // Inject some test files
        let injector = nucleo.injector();
        for file in ["src/main.rs", "src/lib.rs", "Cargo.toml", "README.md"] {
            let _ = injector.push(file.to_string(), |s, cols| {
                cols[0] = s.clone().into();
            });
        }

        // Process injected items
        for _ in 0..20 {
            nucleo.tick(10);
        }

        // Get snapshot with empty pattern
        let snapshot = nucleo.snapshot();
        let item_count = snapshot.item_count();
        let match_count = snapshot.matched_item_count();

        println!("Total items: {item_count}, Matched: {match_count}");

        // With empty pattern, should match all items
        assert!(item_count > 0, "Should have items in nucleo");

        // Now test with pattern
        nucleo.pattern.reparse(
            0,
            "main",
            nucleo::pattern::CaseMatching::Smart,
            nucleo::pattern::Normalization::Smart,
            false,
        );

        // Tick again for pattern matching
        for _ in 0..10 {
            nucleo.tick(10);
        }

        let snapshot = nucleo.snapshot();
        let match_count = snapshot.matched_item_count();
        println!("After 'main' pattern: {match_count} matches");

        // Should find at least src/main.rs
        assert!(match_count > 0, "Should find 'main' pattern matches");
    }

    #[test]
    fn test_accept_rejects_placeholders() {
        let mut fc = FileCompletion::new();
        // Manually set up without spawning tokio tasks
        fc.active = true;
        fc.query_start_pos = 0;
        fc.query.clear();

        fc.completion
            .set_items(vec![PLACEHOLDER_SCANNING.to_string()]);
        assert!(fc.accept().is_none());

        fc.completion
            .set_items(vec![PLACEHOLDER_NO_MATCHES.to_string()]);
        assert!(fc.accept().is_none());

        fc.completion.set_items(vec!["src/main.rs".to_string()]);
        assert!(fc.accept().is_some());
    }
}
