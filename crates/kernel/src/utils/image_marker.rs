//! `yomi://image/` marker 协议——CLI/skill 脚本把图片直接交回模型的通道。
//!
//! 契约（面向 skill 作者）：
//! - 一行一个 marker：`yomi://image/<路径原文>`，必须独占一行（行首不允许
//!   前导空白，无 caption 等后缀语法）。
//! - 路径即行尾原文：不做 trim、不做 URL 解码——空格、中文、`&`、`%`
//!   都是合法文件名字符。相对路径按进程 spawn 时的工作目录解析（脚本
//!   内部自行 chdir 过的，应打印绝对路径），`~` 展开对齐 read 工具。
//! - 每次工具结果最多 [`MAX_IMAGES_PER_RESULT`] 张，超出的 marker 换成
//!   omitted 说明行。
//! - 加载失败不算工具错误：marker 行替换为 `[Image unavailable: ...]`，
//!   模型可据此改用 read/shell 自查。不认识协议的旧内核看到的是一行
//!   文本路径，模型仍可自行 read——协议向后兼容。

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::types::ToolOutputBlock;
use crate::utils::image::{
    bytes_to_data_url_async, detect_mime_type, gif_first_frame_to_data_url, probe_gif_info,
    MAX_IMAGE_SIZE,
};
use crate::utils::path::expand_tilde;

/// Marker 行前缀。放 `yomi://` scheme 下，与 `yomi://post_message/` 等
/// 已有约定同族。
pub const IMAGE_MARKER_PREFIX: &str = "yomi://image/";

/// 单次工具结果允许带入的图片上限。每张图约 1.1-1.6k token（1568px 档），
/// 上限把"脚本失控刷屏"的成本锁在十余千 token 内；超出的 marker 只留
/// omitted 说明，不算错误。
pub const MAX_IMAGES_PER_RESULT: usize = 10;

/// 收集结果：文本（marker 行已原位替换为说明行）+ 图片 block（按 marker
/// 出现顺序）。
pub struct CollectedOutput {
    pub text: String,
    pub images: Vec<ToolOutputBlock>,
}

/// 扫描 CLI 输出文本，提取 image marker 并加载图片。
///
/// `base_dir` 是相对路径的解析基准（= 进程 spawn 时的工作目录）。
pub async fn collect_image_markers(text: &str, base_dir: &Path) -> CollectedOutput {
    // 快路径：无 marker 时免逐行解析（仅一次文本拷贝）。
    if !text.contains(IMAGE_MARKER_PREFIX) {
        return CollectedOutput {
            text: text.to_string(),
            images: Vec::new(),
        };
    }
    let mut images = Vec::new();
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let Some(raw) = line.strip_prefix(IMAGE_MARKER_PREFIX) else {
            out.push_str(line);
            continue;
        };
        // 只剥 Windows CRLF 的 \r；路径其余部分逐字节保留（契约=行尾原文）。
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if images.len() >= MAX_IMAGES_PER_RESULT {
            let _ = write!(
                out,
                "[Image omitted: {raw} | image limit {MAX_IMAGES_PER_RESULT} per tool result]"
            );
            continue;
        }
        match load_image(raw, base_dir).await {
            Ok(loaded) => {
                out.push_str(&loaded.note);
                images.push(ToolOutputBlock::Image {
                    url: loaded.url,
                    mime_type: None,
                });
            }
            Err(reason) => {
                let _ = write!(out, "[Image unavailable: {raw} | {reason}]");
            }
        }
    }
    CollectedOutput { text: out, images }
}

struct LoadedImage {
    url: String,
    note: String,
}

/// 加载一张 marker 指向的图片：大小检查 → magic 嗅探 → GIF 拍平 → 归一化
/// （长边/像素/字节上限，见 `utils::image`）。Err 是给模型看的一行原因。
async fn load_image(raw_path: &str, base_dir: &Path) -> Result<LoadedImage, String> {
    if raw_path.is_empty() {
        return Err("empty path".to_string());
    }
    let expanded = expand_tilde(raw_path);
    let path: PathBuf = if expanded.is_absolute() {
        expanded
    } else {
        base_dir.join(expanded)
    };
    let display = path.display().to_string();

    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "not found".to_string(),
            _ => format!("stat failed: {e}"),
        })?;
    if !metadata.is_file() {
        return Err("not a regular file".to_string());
    }
    let size = metadata.len();
    if size > MAX_IMAGE_SIZE {
        return Err(format!("too large ({size} bytes, limit {MAX_IMAGE_SIZE})"));
    }
    // stat→read 竞争：读到一半文件被改大时按上限截断读取并复检。
    let data = {
        use tokio::io::AsyncReadExt as _;
        let file = tokio::fs::File::open(&path)
            .await
            .map_err(|e| format!("read failed: {e}"))?;
        let mut buf = Vec::new();
        file.take(MAX_IMAGE_SIZE + 1)
            .read_to_end(&mut buf)
            .await
            .map_err(|e| format!("read failed: {e}"))?;
        buf
    };
    if data.len() as u64 > MAX_IMAGE_SIZE {
        return Err(format!(
            "too large ({} bytes, limit {MAX_IMAGE_SIZE})",
            data.len()
        ));
    }
    let Some(mime) = detect_mime_type(&data) else {
        return Err("not a supported image (png/jpeg/gif/webp)".to_string());
    };

    // 与 read 工具同规则：动图拍平首帧（vision API 本来就只看首帧）。
    let (url, flattened) =
        if mime == "image/gif" && probe_gif_info(&data).is_none_or(|i| i.frames > 1) {
            let url = gif_first_frame_to_data_url(&data)
                .map_err(|e| format!("gif decode failed: {e}"))?;
            (url, true)
        } else {
            let url = bytes_to_data_url_async(data)
                .await
                .map_err(|e| e.to_string())?;
            (url, false)
        };

    let mut note = format!("[Image: {display} | Size: {size} bytes]");
    if flattened {
        note.push_str(" | animated GIF: frame 1 shown");
    }
    Ok(LoadedImage { url, note })
}

#[cfg(test)]
#[path = "image_marker_test.rs"]
mod tests;
