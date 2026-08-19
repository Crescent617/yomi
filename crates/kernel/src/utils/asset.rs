use std::path::Path;
use tokio::fs;

/// Parse a base64 data URL: `data:image/png;base64,xxxxx`
fn parse_data_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("data:")?;
    let (mime, b64) = rest.split_once(";base64,")?;
    Some((mime, b64))
}

fn ext_from_mime(mime: &str) -> Option<&str> {
    match mime.strip_prefix("image/")? {
        "png" => Some("png"),
        "jpeg" | "jpg" => Some("jpg"),
        "webp" => Some("webp"),
        "gif" => Some("gif"),
        "bmp" => Some("bmp"),
        "svg+xml" => Some("svg"),
        _ => Some("bin"),
    }
}

fn mime_from_ext(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// Extract an inline base64 image from a URL and store it in `data_dir/assets/{hash}.{ext}`.
/// Returns `Some("asset://{hash}.{ext}")` on success, `None` if the URL is not a data URL.
pub async fn store_inline_image(url: &str, data_dir: &Path) -> Option<String> {
    let (mime, b64_data) = parse_data_url(url)?;
    let bytes =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64_data).ok()?;
    let hash = blake3::hash(&bytes).to_hex().to_string();
    let ext = ext_from_mime(mime)?;
    let assets_dir = data_dir.join("assets");
    fs::create_dir_all(&assets_dir).await.ok()?;
    let path = assets_dir.join(format!("{hash}.{ext}"));
    if path.exists() {
        let touch_path = path.clone();
        tokio::task::spawn_blocking(move || {
            let file = std::fs::OpenOptions::new().write(true).open(touch_path)?;
            file.set_modified(std::time::SystemTime::now())
        })
        .await
        .ok()?
        .ok()?;
    } else {
        fs::write(&path, &bytes).await.ok()?;
    }
    Some(format!("asset://{hash}.{ext}"))
}

/// Resolve an `asset://{hash}.{ext}` URL back to a base64 data URL.
pub async fn resolve_asset_url(url: &str, data_dir: &Path) -> Option<String> {
    let hash_ext = url.strip_prefix("asset://")?;
    let path = data_dir.join("assets").join(hash_ext);
    let bytes = fs::read(&path).await.ok()?;
    let ext = hash_ext.rsplit_once('.').map_or("bin", |(_, e)| e);
    let mime = mime_from_ext(ext);
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);
    Some(format!("data:{mime};base64,{b64}"))
}

/// Replace `asset://` image references in a message with inline base64 data
/// URLs, for building model context. Each resolved image also gains an
/// absolute-path annotation block — see [`process_image_blocks`] for the
/// full semantics (pathological-asset guard, unavailable placeholders).
pub async fn inline_assets_in_message(msg: &mut crate::types::Message, data_dir: &Path) {
    let blocks = std::mem::take(&mut msg.content);
    msg.content = process_image_blocks(blocks, data_dir).await;
}

/// Extract inline base64 images from a single message and replace with `asset://` references.
/// Mutates the message in-place. Images are compressed BEFORE storing, so
/// assets on disk always fit provider limits regardless of what an
/// ingress let through.
pub async fn extract_inline_image(msg: &mut crate::types::Message, data_dir: &Path) {
    for block in &mut msg.content {
        if let crate::types::ContentBlock::ImageUrl { image_url } = block {
            if image_url.url.starts_with("data:") {
                if let Some(normalized) =
                    crate::utils::image::normalize_data_url_async(&image_url.url).await
                {
                    image_url.url = normalized;
                }
                if let Some(asset_url) = store_inline_image(&image_url.url, data_dir).await {
                    image_url.url = asset_url;
                }
            }
        }
    }
}

/// Get the absolute filesystem path for an asset URL.
///
/// Asset names are flat `{hash}.{ext}`: anything with path separators or
/// `..` components is rejected so a crafted URL cannot escape the assets
/// directory.
pub fn asset_path(url: &str, data_dir: &Path) -> Option<std::path::PathBuf> {
    let hash_ext = url.strip_prefix("asset://")?;
    let mut components = std::path::Path::new(hash_ext).components();
    match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(_)), None) => {
            Some(data_dir.join("assets").join(hash_ext))
        }
        _ => None,
    }
}

/// Unified image-block processing on the way INTO the model:
/// - `data:` inline images are normalized (compressed) and persisted to
///   `assets/` (blake3-deduped), staying inline for vision;
/// - `asset://` references resolve back to inline data URLs (pathological
///   oversized ones are refused, missing ones degrade to placeholders);
/// - every successfully resolved image gets a trailing text block with
///   its **absolute path** (`[image N: /…/assets/{hash}.{ext}]`), so the
///   model can Read it or use file tools on it (attachments, convert…).
/// Used by the jsonl read-back path and by the conductor's input entry,
/// so both the current turn and history carry paths.
pub async fn process_image_blocks(
    blocks: Vec<crate::types::ContentBlock>,
    data_dir: &Path,
) -> Vec<crate::types::ContentBlock> {
    use crate::types::ContentBlock;
    let mut out = Vec::with_capacity(blocks.len());
    let mut n = 0usize;
    for block in blocks {
        let ContentBlock::ImageUrl { image_url } = &block else {
            out.push(block);
            continue;
        };
        let url = image_url.url.clone();
        if url.starts_with("asset://") {
            // Refuse only pathological assets outright (decode-bomb guard).
            let pathological = match asset_path(&url, data_dir) {
                Some(path) => fs::metadata(&path)
                    .await
                    .is_ok_and(|m| m.len() > crate::utils::image::MAX_IMAGE_SIZE),
                None => false,
            };
            if pathological {
                out.push(ContentBlock::Text {
                    text: format!("[image omitted: too large, {url}]"),
                });
                continue;
            }
            match resolve_asset_url(&url, data_dir).await {
                Some(data_url) => {
                    let data_url = crate::utils::image::normalize_data_url_async(&data_url)
                        .await
                        .unwrap_or(data_url);
                    out.push(ContentBlock::ImageUrl {
                        image_url: data_url.into(),
                    });
                    if let Some(abs) = asset_path(&url, data_dir) {
                        n += 1;
                        out.push(ContentBlock::Text {
                            text: format!("[image {n}: {}]", abs.display()),
                        });
                    }
                }
                None => {
                    out.push(ContentBlock::Text {
                        text: format!("[image unavailable: {url}]"),
                    });
                }
            }
        } else if url.starts_with("data:") {
            let normalized = crate::utils::image::normalize_data_url_async(&url)
                .await
                .unwrap_or(url);
            match store_inline_image(&normalized, data_dir).await {
                Some(asset_url) => match asset_path(&asset_url, data_dir) {
                    Some(abs) => {
                        out.push(ContentBlock::ImageUrl {
                            image_url: normalized.into(),
                        });
                        n += 1;
                        out.push(ContentBlock::Text {
                            text: format!("[image {n}: {}]", abs.display()),
                        });
                    }
                    None => out.push(ContentBlock::ImageUrl {
                        image_url: normalized.into(),
                    }),
                },
                None => out.push(ContentBlock::ImageUrl {
                    image_url: normalized.into(),
                }),
            }
        } else {
            out.push(block);
        }
    }
    out
}

#[cfg(test)]
#[path = "asset_test.rs"]
mod tests;
