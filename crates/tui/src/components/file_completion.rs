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

/// Manages async file completion with nucleo-powered fuzzy matching
pub struct FileCompletion {
    completion: CompletionList<String>,
    query: String,
    query_start_pos: usize,
    working_dir: std::path::PathBuf,
    nucleo: Option<Nucleo<String>>,
    /// Whether scan is complete
    scan_complete: Arc<AtomicBool>,
    scan_handle: Option<JoinHandle<()>>,
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
            nucleo: None,
            scan_complete: Arc::new(AtomicBool::new(false)),
            scan_handle: None,
            active: false,
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
    pub fn start(&mut self, cursor_pos: usize) {
        self.query.clear();
        self.query_start_pos = cursor_pos;
        self.active = true;

        self.cancel_scan();

        // Initialize nucleo matcher with default config
        let config = nucleo::Config::DEFAULT;
        self.nucleo = Some(Nucleo::new(config, Arc::new(|| {}), None, 1));

        // Launch async background scan
        self.start_async_scan();

        // Show scanning message immediately
        self.set_placeholder(PLACEHOLDER_SCANNING);
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
                self.query.pop();
                true
            }
            ' ' if self.query.is_empty() => {
                self.cancel();
                false
            }
            c => {
                self.query.push(c);
                true
            }
        }
    }

    /// Accept the current selection
    pub fn accept(&mut self) -> Option<(String, usize, usize)> {
        self.completion.get_selected().cloned().and_then(|selected| {
            if selected == PLACEHOLDER_SCANNING || selected == PLACEHOLDER_NO_MATCHES {
                return None;
            }
            let start = self.query_start_pos;
            let end = self.query_start_pos + self.query.len();
            self.active = false;
            self.query.clear();
            self.completion.hide();
            Some((selected, start, end))
        })
    }

    /// Cancel file completion
    pub fn cancel(&mut self) {
        self.active = false;
        self.query.clear();
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

    /// Sync query from current input content and cursor position
    pub fn sync_query(&mut self, content: &str, cursor_pos: usize) {
        if cursor_pos >= self.query_start_pos {
            let new_query = &content[self.query_start_pos..cursor_pos];
            if new_query != self.query {
                self.query = new_query.to_string();
            }
        }
    }

    /// Get total number of files indexed
    pub fn total_files(&self) -> usize {
        self.nucleo
            .as_ref()
            .map(|n| n.snapshot().item_count() as usize)
            .unwrap_or(0)
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
                self.completion.set_items(items);
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

        self.completion.set_items(items);
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

        fc.completion.set_items(vec![PLACEHOLDER_SCANNING.to_string()]);
        assert!(fc.accept().is_none());

        fc.completion
            .set_items(vec![PLACEHOLDER_NO_MATCHES.to_string()]);
        assert!(fc.accept().is_none());

        fc.completion.set_items(vec!["src/main.rs".to_string()]);
        assert!(fc.accept().is_some());
    }
}
