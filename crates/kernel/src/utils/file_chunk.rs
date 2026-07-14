use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Raw, unformatted UTF-8 byte range from a file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FileChunk {
    pub content: String,
    pub path: String,
    pub file_size: u64,
    pub start_offset: u64,
    pub end_offset: u64,
    pub has_earlier: bool,
}

pub fn read_utf8_file_chunk(
    path: &Path,
    before_offset: Option<u64>,
    after_offset: Option<u64>,
    max_bytes: u64,
    missing_is_empty: bool,
) -> std::io::Result<FileChunk> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if missing_is_empty && error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(empty_chunk(path));
        }
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "debug source must be a regular file",
        ));
    }

    let mut file = std::fs::File::open(path)?;
    let file_size = file.metadata()?.len();
    let (requested_start, requested_end) = if let Some(after_offset) = after_offset {
        if after_offset > file_size {
            (file_size.saturating_sub(max_bytes), file_size)
        } else {
            (after_offset, (after_offset + max_bytes).min(file_size))
        }
    } else {
        let end = before_offset.unwrap_or(file_size).min(file_size);
        (end.saturating_sub(max_bytes), end)
    };
    file.seek(SeekFrom::Start(requested_start))?;
    let mut bytes = vec![0; (requested_end - requested_start) as usize];
    file.read_exact(&mut bytes)?;

    let (leading_skip, valid_len) = valid_utf8_window(&bytes, requested_start > 0)?;
    let candidate = &bytes[leading_skip..leading_skip + valid_len];
    let start_offset = requested_start + leading_skip as u64;
    let end_offset = start_offset + valid_len as u64;
    let content = std::str::from_utf8(candidate)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?
        .to_string();

    Ok(FileChunk {
        content,
        path: path.to_string_lossy().to_string(),
        file_size,
        start_offset,
        end_offset,
        has_earlier: start_offset > 0,
    })
}

fn empty_chunk(path: &Path) -> FileChunk {
    FileChunk {
        content: String::new(),
        path: path.to_string_lossy().to_string(),
        file_size: 0,
        start_offset: 0,
        end_offset: 0,
        has_earlier: false,
    }
}

fn valid_utf8_window(bytes: &[u8], may_start_mid_char: bool) -> std::io::Result<(usize, usize)> {
    let max_skip = if may_start_mid_char {
        bytes.len().min(3)
    } else {
        0
    };
    for skip in 0..=max_skip {
        let candidate = &bytes[skip..];
        match std::str::from_utf8(candidate) {
            Ok(_) => return Ok((skip, candidate.len())),
            Err(error) if error.error_len().is_none() => {
                return Ok((skip, error.valid_up_to()));
            }
            Err(_) => {}
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "debug source is not UTF-8",
    ))
}

#[cfg(test)]
#[path = "file_chunk_test.rs"]
mod tests;
