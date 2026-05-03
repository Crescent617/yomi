use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Version of the session state file format
pub const STATE_VERSION: u32 = 1;

/// A single entry in the session state file (JSONL line)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum StateEntry {
    /// Metadata header (first line)
    #[serde(rename = "meta")]
    Metadata { v: u32, created: String },

    /// File state entry
    #[serde(rename = "file")]
    FileState { p: PathBuf, m: u64 },
}

/// File state for tracking file modification times
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileState {
    pub path: PathBuf,
    pub mtime: u64,
}

impl FileState {
    pub fn new(path: PathBuf, mtime: u64) -> Self {
        Self { path, mtime }
    }
}

impl From<FileState> for StateEntry {
    fn from(fs: FileState) -> Self {
        StateEntry::FileState {
            p: fs.path,
            m: fs.mtime,
        }
    }
}

impl TryFrom<StateEntry> for FileState {
    type Error = &'static str;

    fn try_from(entry: StateEntry) -> Result<Self, Self::Error> {
        match entry {
            StateEntry::FileState { p, m } => Ok(FileState { path: p, mtime: m }),
            StateEntry::Metadata { .. } => Err("not a file state entry"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_serialization() {
        let meta = StateEntry::Metadata {
            v: 1,
            created: "2026-05-03T10:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&meta).unwrap();
        assert!(json.contains("\"t\":\"meta\""));
        assert!(json.contains("\"v\":1"));
    }

    #[test]
    fn test_file_state_serialization() {
        let file = StateEntry::FileState {
            p: PathBuf::from("/tmp/test.rs"),
            m: 1_234_567_890,
        };
        let json = serde_json::to_string(&file).unwrap();
        assert!(json.contains("\"t\":\"file\""));
        assert!(json.contains("\"p\":\"/tmp/test.rs\""));
        assert!(json.contains("\"m\":1234567890"));
    }

    #[test]
    fn test_file_state_roundtrip() {
        let original = FileState::new(PathBuf::from("/home/user/main.rs"), 1_714_723_200);
        let entry: StateEntry = original.clone().into();
        let back: FileState = entry.try_into().unwrap();
        assert_eq!(original.path, back.path);
        assert_eq!(original.mtime, back.mtime);
    }
}
