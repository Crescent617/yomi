use crate::types::Message;
use std::path::Path;
use std::sync::Arc;
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

/// Extract inline base64 images from a single message and replace with `asset://` references.
/// Mutates the message in-place.
pub async fn extract_inline_image(msg: &mut crate::types::Message, data_dir: &Path) {
    for block in &mut msg.content {
        if let crate::types::ContentBlock::ImageUrl { image_url } = block {
            if image_url.url.starts_with("data:") {
                if let Some(asset_url) = store_inline_image(&image_url.url, data_dir).await {
                    image_url.url = asset_url;
                }
            }
        }
    }
}

/// Read asset bytes by `asset://{hash}.{ext}` URL.
pub async fn read_asset(url: &str, data_dir: &Path) -> Option<Vec<u8>> {
    let hash_ext = url.strip_prefix("asset://")?;
    let path = data_dir.join("assets").join(hash_ext);
    fs::read(&path).await.ok()
}

/// Get the absolute filesystem path for an asset URL.
pub fn asset_path(url: &str, data_dir: &Path) -> Option<std::path::PathBuf> {
    let hash_ext = url.strip_prefix("asset://")?;
    Some(data_dir.join("assets").join(hash_ext))
}

/// Resolve asset URLs in a slice of messages for provider requests.
/// Clones only messages that contain `asset://` images.
pub async fn resolve_messages(messages: &[Arc<Message>], data_dir: &Path) -> Vec<Arc<Message>> {
    let mut result = Vec::with_capacity(messages.len());
    for m in messages {
        let needs_resolve = m.content.iter().any(|c| {
            matches!(c, crate::types::ContentBlock::ImageUrl { image_url } if image_url.url.starts_with("asset://"))
        });
        if !needs_resolve {
            result.push(Arc::clone(m));
            continue;
        }
        let mut resolved = (**m).clone();
        for block in &mut resolved.content {
            if let crate::types::ContentBlock::ImageUrl { image_url } = block {
                if image_url.url.starts_with("asset://") {
                    if let Some(data_url) = resolve_asset_url(&image_url.url, data_dir).await {
                        image_url.url = data_url;
                    }
                }
            }
        }
        result.push(Arc::new(resolved));
    }
    result
}
