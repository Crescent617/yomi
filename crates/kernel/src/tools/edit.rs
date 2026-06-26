use crate::tools::helper::{
    g_lock_timeout, get_mtime, FileStateStore, DEFAULT_LOCK_TIMEOUT, MAX_FILE_SIZE,
};
use crate::tools::{FileStateAwareTool, Tool, ToolExecCtx};
use crate::types::{KernelError, Result, ToolOutput};
use crate::utils::path::expand_tilde;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub const EDIT_TOOL_NAME: &str = "edit";

#[derive(Default)]
pub struct EditTool {
    file_state_store: Option<Arc<FileStateStore>>,
}

impl EditTool {
    /// Create a new `EditTool` with optional file state store
    pub fn new(store: impl Into<Option<Arc<FileStateStore>>>) -> Self {
        Self {
            file_state_store: store.into(),
        }
    }
}

impl FileStateAwareTool for EditTool {
    fn file_state_store(&self) -> Option<&Arc<FileStateStore>> {
        self.file_state_store.as_ref()
    }
}
struct Normalized {
    text: String,
    // 规范化文本中每个字符位置对应的原始文本字节起始位置
    byte_map: Vec<usize>,
}

impl Normalized {
    fn build(content: &str, normalize: impl Fn(char) -> char) -> Self {
        let mut text = String::with_capacity(content.len());
        let mut byte_map = Vec::with_capacity(content.len());
        let mut chars = content.char_indices().peekable();

        while let Some((start_byte, c)) = chars.next() {
            match c {
                // CRLF 处理：把 \r\n 映射为 \n，但记录 \r 的起始位置
                '\r' if matches!(chars.peek(), Some(&(_, '\n'))) => {
                    chars.next(); // consume '\n'
                    text.push('\n');
                    byte_map.push(start_byte);
                }
                c => {
                    let nc = normalize(c);
                    text.push(nc);
                    for _ in 0..nc.len_utf8() {
                        byte_map.push(start_byte);
                    }
                }
            }
        }

        Self { text, byte_map }
    }

    fn map_range(&self, start: usize, end: usize, orig_len: usize) -> Option<(usize, usize)> {
        if self.byte_map.is_empty() {
            return Some((0, 0));
        }
        let orig_start = self.byte_map[start.min(self.byte_map.len() - 1)];
        let orig_end = if end >= self.byte_map.len() {
            orig_len
        } else {
            self.byte_map[end]
        };
        if orig_end <= orig_start {
            return None;
        }
        Some((orig_start, orig_end))
    }
}

fn normalize_char_quotes(c: char) -> char {
    match c {
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{2032}' | '\u{FF07}' => '\'',
        '\u{201c}' | '\u{201d}' | '\u{201E}' | '\u{2033}' | '\u{FF02}' => '"',
        _ => c,
    }
}

fn normalize_char_full(c: char) -> char {
    match c {
        '\n' | '\r' | '\t' => c,
        c if is_unicode_whitespace(c) => ' ',
        c => normalize_char_quotes(c),
    }
}

fn is_unicode_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{00A0}'      // NBSP
        | '\u{2000}'
            ..='\u{200A}'  // En Quad / Em Quad / En Space / Em Space / Three-Per-Em / Four-Per-Em / Six-Per-Em / Figure / Punctuation / Thin / Hair
        | '\u{202F}'     // Narrow No-Break Space
        | '\u{205F}'     // Medium Mathematical Space
        | '\u{3000}' // Ideographic Space
    )
}

/// Check if a string contains any characters that could be normalized
/// (CRLF, curly quotes, or unicode whitespace).
fn has_normalizable_chars(s: &str) -> bool {
    s.contains('\r')
        || s.chars()
            .any(|c| normalize_char_quotes(c) != c || is_unicode_whitespace(c))
}

/// Find the actual string in file content, with quote/whitespace normalization.
///
/// Returns the original file substring if found, or None otherwise.
fn find_actual_string(file_content: &str, search_string: &str) -> Option<String> {
    if file_content.contains(search_string) {
        return Some(search_string.to_string());
    }

    // Quick exit: if neither string contains any normalizable characters,
    // no fallback stage will succeed (both are already in normalized form).
    if !has_normalizable_chars(file_content) && !has_normalizable_chars(search_string) {
        return None;
    }

    // Stage 1: quotes + newlines (CRLF handling)
    let norm_file = Normalized::build(file_content, normalize_char_quotes);
    let norm_search = Normalized::build(search_string, normalize_char_quotes);
    if let Some(pos) = norm_file.text.find(norm_search.text.as_str()) {
        let end = pos + norm_search.text.len();
        if let Some((start, end)) = norm_file.map_range(pos, end, file_content.len()) {
            return Some(file_content[start..end].to_string());
        }
    }

    // Stage 2: + unicode whitespace
    let norm_file = Normalized::build(file_content, normalize_char_full);
    let norm_search = Normalized::build(search_string, normalize_char_full);
    if let Some(pos) = norm_file.text.find(norm_search.text.as_str()) {
        let end = pos + norm_search.text.len();
        if let Some((start, end)) = norm_file.map_range(pos, end, file_content.len()) {
            return Some(file_content[start..end].to_string());
        }
    }

    None
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &'static str {
        EDIT_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        "Replace text in a file. Use this instead of sed. Provide old_str to locate the text (should be unique enough) and new_str to replace it. Supports replace_all=true to replace all occurrences."
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative to the working directory or absolute path"
                },
                "old_str": {
                    "type": "string",
                    "description": "The text to find and replace. Should be unique enough to identify the location."
                },
                "new_str": {
                    "type": "string",
                    "description": "The new text to replace old_str with"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "If true, replace all occurrences. Default false (replace first only).",
                    "default": false
                }
            },
            "required": ["path", "old_str", "new_str"]
        })
    }

    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| KernelError::tool("Missing 'path' argument"))?;
        let old_str = args["old_str"]
            .as_str()
            .ok_or_else(|| KernelError::tool("Missing 'old_str' argument"))?;
        let new_str = args["new_str"]
            .as_str()
            .ok_or_else(|| KernelError::tool("Missing 'new_str' argument"))?;
        let replace_all = args["replace_all"].as_bool().unwrap_or(false);

        let path = expand_tilde(path_str);
        let path = if path.is_absolute() {
            path
        } else {
            ctx.working_dir.join(path)
        };

        tracing::debug!("Edit: replace in {}", path.display());

        // Check if file exists
        if !tokio::fs::try_exists(&path).await? {
            return Ok(ToolOutput::error(format!(
                "File does not exist: {path_str}"
            )));
        }
        // Check file size
        if tokio::fs::metadata(&path).await?.len() > MAX_FILE_SIZE {
            return Ok(ToolOutput::error(format!(
                "File is too large to edit: {path_str}"
            )));
        }

        // Check if file has been read before editing
        // Skip for simple single-line edits (old_str and new_str are both single lines)
        let is_simple_edit = !old_str.contains('\n') && !new_str.contains('\n') && !replace_all;

        if let Some(ref store) = self.file_state_store {
            // Simple edits don't require read-first check
            if !is_simple_edit {
                if !store.has_recorded(&path) {
                    return Ok(ToolOutput::error(format!(
                        "File has not been read yet. Read it first before editing: {path_str}"
                    )));
                }
                if let Err(error) = self.check_staleness(&path).await {
                    return Ok(ToolOutput::error(error));
                }
            }
        }

        // Track file for checkpoint before modification
        ctx.track_edit(&path).await;

        // Acquire lock to serialize concurrent tool calls
        let _guard = g_lock_timeout(path.to_string_lossy(), DEFAULT_LOCK_TIMEOUT).await?;

        // Read file content (now protected by exclusive lock)
        let content = tokio::fs::read_to_string(&path).await?;

        // Validate old_str is not empty (except for creating new files)
        if old_str.is_empty() && !content.is_empty() {
            return Ok(ToolOutput::error(
                "Cannot use empty old_str on existing file with content. Provide the text to replace."
            ));
        }

        // Check if old_str and new_str are the same
        if old_str == new_str {
            return Ok(ToolOutput::error(
                "No changes to make: old_str and new_str are exactly the same.",
            ));
        }

        // Find the actual string in the file (handles quote normalization)
        let Some(actual_old_str) = find_actual_string(&content, old_str) else {
            return Ok(ToolOutput::error(format!(
                "Could not find 'old_str' in file. String not found:\n{old_str}"
            )));
        };

        // Count occurrences
        let occurrences = content.matches(&actual_old_str).count();
        if occurrences == 0 {
            return Ok(ToolOutput::error(format!(
                "Could not find 'old_str' in file. String not found:\n{old_str}"
            )));
        }

        // Check for multiple matches when replace_all is false
        if occurrences > 1 && !replace_all {
            return Ok(ToolOutput::error(format!(
                "Found {occurrences} matches of the string to replace, but replace_all is false. \
                 To replace all occurrences, set replace_all to true. \
                 To replace only one occurrence, please provide more context to uniquely identify the instance."
            )));
        }

        // Perform the replacement
        let new_content = if replace_all {
            content.replace(&actual_old_str, new_str)
        } else {
            content.replacen(&actual_old_str, new_str, 1)
        };

        // Write the new content (exclusive lock still held)
        tokio::fs::write(&path, &new_content).await?;

        // Update file mtime in store
        if let Some(ref store) = self.file_state_store {
            if let Some(mtime) = get_mtime(&path).await {
                store.record(path.clone(), mtime).await;
            }
        }

        // Build success message
        let resp = if replace_all {
            format!("Replaced all {occurrences} occurrences")
        } else {
            "Replaced 1 occurrence".to_string()
        };

        Ok(ToolOutput::text_with_summary(resp, ""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_edit_tool_basic() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "hello world").unwrap();
        let path = temp_file.path().parent().unwrap();
        let file_name = temp_file.path().file_name().unwrap().to_str().unwrap();
        // Use canonicalized path for file state store
        let full_path = path.join(file_name).canonicalize().unwrap();

        // First, simulate a read by setting file state with actual file's mtime
        let store = Arc::new(FileStateStore::new());
        let _content = "hello world".to_string();

        // Get actual file mtime
        let mtime = crate::tools::helper::get_mtime(&full_path).await.unwrap();

        store.record(full_path.clone(), mtime).await;

        let tool = EditTool::new(store);

        let args = serde_json::json!({
            "path": file_name,
            "old_str": "hello",
            "new_str": "goodbye"
        });

        let ctx = ToolExecCtx::new("test_tool_call", path, "test-session");
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.text_content().contains("Replaced"));

        let new_content = tokio::fs::read_to_string(temp_file.path()).await.unwrap();
        assert_eq!(new_content, "goodbye world\n");
    }

    #[tokio::test]
    async fn test_edit_tool_no_read_first_multiline() {
        // Multi-line edit should require read first
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "line 1").unwrap();
        writeln!(temp_file, "line 2").unwrap();
        let path = temp_file.path().parent().unwrap();
        let file_name = temp_file.path().file_name().unwrap().to_str().unwrap();

        let store = Arc::new(FileStateStore::new());
        let tool = EditTool::new(store);

        // Multi-line old_str requires read first
        let args = serde_json::json!({
            "path": file_name,
            "old_str": "line 1\nline 2",
            "new_str": "replaced"
        });

        let ctx = ToolExecCtx::new("test_tool_call", path, "test-session");
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.is_error);
        assert!(result.error_text().contains("not been read"));
    }

    #[tokio::test]
    async fn test_edit_tool_simple_edit_no_read() {
        // Simple single-line edit should work without read
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "hello world").unwrap();
        let path = temp_file.path().parent().unwrap();
        let file_name = temp_file.path().file_name().unwrap().to_str().unwrap();

        let store = Arc::new(FileStateStore::new());
        let tool = EditTool::new(store);

        let args = serde_json::json!({
            "path": file_name,
            "old_str": "hello",
            "new_str": "goodbye"
        });

        let ctx = ToolExecCtx::new("test_tool_call", path, "test-session");
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(!result.is_error);
        assert!(result.text_content().contains("Replaced"));

        let new_content = tokio::fs::read_to_string(temp_file.path()).await.unwrap();
        assert_eq!(new_content, "goodbye world\n");
    }

    #[tokio::test]
    async fn test_edit_crlf() {
        let mut temp_file = NamedTempFile::new().unwrap();
        write!(temp_file, "line1\r\nline2\r\n").unwrap();
        let path = temp_file.path().parent().unwrap();
        let file_name = temp_file.path().file_name().unwrap().to_str().unwrap();

        let store = Arc::new(FileStateStore::new());
        let mtime = get_mtime(&temp_file.path().canonicalize().unwrap())
            .await
            .unwrap();
        store
            .record(temp_file.path().canonicalize().unwrap(), mtime)
            .await;

        let tool = EditTool::new(store);
        let args = serde_json::json!({
            "path": file_name,
            "old_str": "line1\nline2",
            "new_str": "foo\nbar"
        });

        let ctx = ToolExecCtx::new("test_tool_call", path, "test-session");
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(!result.is_error);

        let new_content = tokio::fs::read_to_string(temp_file.path()).await.unwrap();
        assert_eq!(new_content, "foo\nbar\r\n");
    }

    #[tokio::test]
    async fn test_edit_curly_quotes() {
        let mut temp_file = NamedTempFile::new().unwrap();
        write!(temp_file, "println!(\u{2018}hello\u{2019});").unwrap();
        let path = temp_file.path().parent().unwrap();
        let file_name = temp_file.path().file_name().unwrap().to_str().unwrap();

        let store = Arc::new(FileStateStore::new());
        let mtime = get_mtime(&temp_file.path().canonicalize().unwrap())
            .await
            .unwrap();
        store
            .record(temp_file.path().canonicalize().unwrap(), mtime)
            .await;

        let tool = EditTool::new(store);
        let args = serde_json::json!({
            "path": file_name,
            "old_str": "println!('hello');",
            "new_str": "println!('world');"
        });

        let ctx = ToolExecCtx::new("test_tool_call", path, "test-session");
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(!result.is_error);

        let new_content = tokio::fs::read_to_string(temp_file.path()).await.unwrap();
        assert_eq!(new_content, "println!('world');");
    }

    #[tokio::test]
    async fn test_edit_unicode_whitespace() {
        let mut temp_file = NamedTempFile::new().unwrap();
        write!(temp_file, "let\u{00a0}x = 1;").unwrap();
        let path = temp_file.path().parent().unwrap();
        let file_name = temp_file.path().file_name().unwrap().to_str().unwrap();

        let store = Arc::new(FileStateStore::new());
        let mtime = get_mtime(&temp_file.path().canonicalize().unwrap())
            .await
            .unwrap();
        store
            .record(temp_file.path().canonicalize().unwrap(), mtime)
            .await;

        let tool = EditTool::new(store);
        let args = serde_json::json!({
            "path": file_name,
            "old_str": "let x = 1;",
            "new_str": "let y = 2;"
        });

        let ctx = ToolExecCtx::new("test_tool_call", path, "test-session");
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(!result.is_error);

        let new_content = tokio::fs::read_to_string(temp_file.path()).await.unwrap();
        assert_eq!(new_content, "let y = 2;");
    }

    #[tokio::test]
    async fn test_edit_crlf_multi() {
        // 多个 CRLF 连续：文件 a\r\nb\r\nc\r\n，搜索 b\nc，替换 X\nY
        // 验证映射不会越界，只替换中间一段
        let mut temp_file = NamedTempFile::new().unwrap();
        write!(temp_file, "a\r\nb\r\nc\r\n").unwrap();
        let path = temp_file.path().parent().unwrap();
        let file_name = temp_file.path().file_name().unwrap().to_str().unwrap();

        let store = Arc::new(FileStateStore::new());
        let mtime = get_mtime(&temp_file.path().canonicalize().unwrap())
            .await
            .unwrap();
        store
            .record(temp_file.path().canonicalize().unwrap(), mtime)
            .await;

        let tool = EditTool::new(store);
        let args = serde_json::json!({
            "path": file_name,
            "old_str": "b\nc",
            "new_str": "X\nY"
        });

        let ctx = ToolExecCtx::new("test_tool_call", path, "test-session");
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(!result.is_error);

        let new_content = tokio::fs::read_to_string(temp_file.path()).await.unwrap();
        assert_eq!(new_content, "a\r\nX\nY\r\n");
    }

    #[tokio::test]
    async fn test_edit_crlf_mixed_lf() {
        // CRLF + 纯 LF 混合：文件 a\r\nb\nc，搜索 b\nc
        // 验证匹配到纯 LF 段，映射提取的是 b\nc 而非 b\r\nc
        let mut temp_file = NamedTempFile::new().unwrap();
        write!(temp_file, "a\r\nb\nc").unwrap();
        let path = temp_file.path().parent().unwrap();
        let file_name = temp_file.path().file_name().unwrap().to_str().unwrap();

        let store = Arc::new(FileStateStore::new());
        let mtime = get_mtime(&temp_file.path().canonicalize().unwrap())
            .await
            .unwrap();
        store
            .record(temp_file.path().canonicalize().unwrap(), mtime)
            .await;

        let tool = EditTool::new(store);
        let args = serde_json::json!({
            "path": file_name,
            "old_str": "b\nc",
            "new_str": "X\nY"
        });

        let ctx = ToolExecCtx::new("test_tool_call", path, "test-session");
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(!result.is_error);

        let new_content = tokio::fs::read_to_string(temp_file.path()).await.unwrap();
        assert_eq!(new_content, "a\r\nX\nY");
    }

    #[tokio::test]
    async fn test_edit_crlf_trailing() {
        // 文件末尾 CRLF：a\r\n，搜索 a\n，替换 X
        // 验证 end == byte_map.len() 的 map_range 边界（orig_end = orig_len）
        let mut temp_file = NamedTempFile::new().unwrap();
        write!(temp_file, "a\r\n").unwrap();
        let path = temp_file.path().parent().unwrap();
        let file_name = temp_file.path().file_name().unwrap().to_str().unwrap();

        let store = Arc::new(FileStateStore::new());
        let mtime = get_mtime(&temp_file.path().canonicalize().unwrap())
            .await
            .unwrap();
        store
            .record(temp_file.path().canonicalize().unwrap(), mtime)
            .await;

        let tool = EditTool::new(store);
        let args = serde_json::json!({
            "path": file_name,
            "old_str": "a\n",
            "new_str": "X"
        });

        let ctx = ToolExecCtx::new("test_tool_call", path, "test-session");
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(!result.is_error);

        let new_content = tokio::fs::read_to_string(temp_file.path()).await.unwrap();
        assert_eq!(new_content, "X");
    }

    #[tokio::test]
    async fn test_edit_fullwidth_quotes() {
        // 全角引号：\u{FF07} (') \u{FF02} (")
        let mut temp_file = NamedTempFile::new().unwrap();
        write!(temp_file, "say\u{FF07}hello\u{FF02}world\u{FF07}").unwrap();
        let path = temp_file.path().parent().unwrap();
        let file_name = temp_file.path().file_name().unwrap().to_str().unwrap();

        let store = Arc::new(FileStateStore::new());
        let mtime = get_mtime(&temp_file.path().canonicalize().unwrap())
            .await
            .unwrap();
        store
            .record(temp_file.path().canonicalize().unwrap(), mtime)
            .await;

        let tool = EditTool::new(store);
        let args = serde_json::json!({
            "path": file_name,
            "old_str": "say'hello\"world'",
            "new_str": "print'goodbye\"earth'"
        });

        let ctx = ToolExecCtx::new("test_tool_call", path, "test-session");
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(!result.is_error);

        let new_content = tokio::fs::read_to_string(temp_file.path()).await.unwrap();
        assert_eq!(new_content, "print'goodbye\"earth'");
    }
}
