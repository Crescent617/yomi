//! Clipboard paste, image handling, and content block conversion

use crate::{msg::Msg, utils::text::truncate_by_chars};

use super::component::InputComponent;

impl InputComponent {
    /// Try to read image from clipboard and save to temp file
    pub(crate) fn try_paste_image(&mut self) -> Option<String> {
        self.try_paste_image_arboard()
    }

    /// Try to get image from clipboard
    fn try_paste_image_arboard(&mut self) -> Option<String> {
        use arboard::Clipboard;

        let mut clipboard = match Clipboard::new() {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!("Failed to create arboard clipboard: {}", e);
                return None;
            }
        };

        // Try to get image from clipboard
        let image = match clipboard.get_image() {
            Ok(img) => img,
            Err(e) => {
                tracing::debug!("No image in arboard clipboard: {}", e);
                return None;
            }
        };

        tracing::debug!(
            "Got image from arboard: {}x{}, {} bytes",
            image.width,
            image.height,
            image.bytes.len()
        );

        self.save_image_to_temp(image.width, image.height, &image.bytes)
    }

    /// Save image bytes to temp file and return placeholder
    fn save_image_to_temp(&mut self, width: usize, height: usize, bytes: &[u8]) -> Option<String> {
        // Create temp file
        let temp_dir = std::env::temp_dir().join("yomi_images");
        if let Err(e) = std::fs::create_dir_all(&temp_dir) {
            tracing::warn!("Failed to create temp dir: {}", e);
            return None;
        }

        self.placeholder_counter += 1;
        let filename = format!(
            "paste_{}_{}.png",
            std::process::id(),
            self.placeholder_counter
        );
        let filepath = temp_dir.join(&filename);

        // Check if bytes length is valid for RGBA
        let expected_len = width * height * 4;
        if bytes.len() != expected_len {
            tracing::warn!(
                "Image bytes length mismatch: got {}, expected {} ({}x{}x4)",
                bytes.len(),
                expected_len,
                width,
                height
            );
            return None;
        }

        // Save image as PNG using image crate
        let img = match image::RgbaImage::from_raw(width as u32, height as u32, bytes.to_vec()) {
            Some(img) => img,
            None => {
                tracing::warn!("Failed to create RgbaImage from raw bytes");
                return None;
            }
        };

        if let Err(e) = img.save(&filepath) {
            tracing::warn!("Failed to save image: {}", e);
            return None;
        }

        tracing::info!("Saved pasted image to: {:?}", filepath);

        // Create placeholder and store mapping
        let placeholder = format!("[Pasted #{} image]", self.placeholder_counter);
        self.image_paths.insert(placeholder.clone(), filepath);

        Some(placeholder)
    }

    /// Handle text paste by creating a placeholder
    pub(crate) fn handle_text_paste(&mut self, text: &str) -> Msg {
        // If there's a selection, delete it first
        if self.component.has_selection() {
            self.component.delete_selection();
        }

        // If text is small (< 1k), insert directly without placeholder
        if text.len() < 1024 {
            let cleaned = text.replace('\r', "");
            self.component.insert_str(&cleaned);
            self.update_completion();
            return Msg::InputChanged(self.component.content().to_string());
        }

        // Large text: use placeholder
        self.placeholder_counter += 1;
        let placeholder = format!("[Pasted #{} text]", self.placeholder_counter);
        let cleaned = text.replace('\r', "");
        self.pasted_contents.insert(placeholder.clone(), cleaned);
        self.component.insert_str(&placeholder);
        self.update_completion();
        Msg::InputChanged(self.component.content().to_string())
    }

    /// Get current input as content blocks (with image and paste placeholders converted)
    pub fn get_content_blocks(&self) -> Vec<kernel::types::ContentBlock> {
        let text = self.component.content();
        tracing::debug!(
            "get_content_blocks: text='{}', image_paths={:?}, pasted_contents={:?}",
            text,
            self.image_paths,
            self.pasted_contents
        );
        let blocks = self.convert_to_content_blocks(text);
        tracing::info!("Converted to {} content blocks", blocks.len());
        for (i, block) in blocks.iter().enumerate() {
            match block {
                kernel::types::ContentBlock::Text { text } => {
                    tracing::debug!("Block {}: Text ({} chars)", i, text.len());
                }
                kernel::types::ContentBlock::ImageUrl { image_url } => {
                    let preview = if image_url.url.chars().count() > 60 {
                        let truncated = truncate_by_chars(&image_url.url, 50);
                        format!("{truncated}...({} chars)", image_url.url.chars().count())
                    } else {
                        image_url.url.clone()
                    };
                    tracing::info!("Block {}: ImageUrl {}", i, preview);
                }
                _ => {
                    tracing::debug!("Block {}: Other", i);
                }
            }
        }
        blocks
    }

    /// Convert input content with placeholders to content blocks
    /// Images are converted to base64 data URLs for LLM API compatibility
    /// Paste placeholders [Pasted #N image/text] are replaced with actual content
    fn convert_to_content_blocks(&self, text: &str) -> Vec<kernel::types::ContentBlock> {
        use kernel::types::{ContentBlock, ImageUrl};

        let mut blocks = Vec::new();
        let mut remaining = text;

        // Find all placeholders (both image and paste) and split text
        while let Some(start) = remaining.find('[') {
            // Add text before placeholder
            if start > 0 {
                blocks.push(ContentBlock::Text {
                    text: remaining[..start].to_string(),
                });
            }

            // Find placeholder end
            if let Some(end) = remaining[start..].find(']') {
                let end_idx = start + end;
                let potential_placeholder = &remaining[start..=end_idx];

                // Check if it's a known placeholder
                if let Some(path) = self.image_paths.get(potential_placeholder) {
                    // Image placeholder
                    match Self::image_to_base64_url(path) {
                        Some(base64_url) => blocks.push(ContentBlock::ImageUrl {
                            image_url: ImageUrl {
                                url: base64_url,
                                detail: Some("auto".to_string()),
                            },
                        }),
                        None => blocks.push(ContentBlock::Text {
                            text: format!("[Error: Failed to process {potential_placeholder}]"),
                        }),
                    }
                    remaining = &remaining[end_idx + 1..];
                } else if let Some(pasted_text) = self.pasted_contents.get(potential_placeholder) {
                    // Text placeholder
                    blocks.push(ContentBlock::Text {
                        text: pasted_text.clone(),
                    });
                    remaining = &remaining[end_idx + 1..];
                }
                // Not a recognized placeholder, treat '[' as regular text
                else {
                    blocks.push(ContentBlock::Text {
                        text: "[".to_string(),
                    });
                    remaining = &remaining[start + 1..];
                }
            } else {
                // No closing ']', treat as regular text
                break;
            }
        }

        // Add remaining text
        if !remaining.is_empty() {
            blocks.push(ContentBlock::Text {
                text: remaining.to_string(),
            });
        }

        if blocks.is_empty() {
            blocks.push(ContentBlock::Text {
                text: text.to_string(),
            });
        }

        blocks
    }

    /// Convert image file to base64 data URL
    /// OpenAI/Anthropic expect format: `data:image/{format};base64,{base64_data`}
    fn image_to_base64_url(path: &std::path::Path) -> Option<String> {
        // Read image file
        let image_data = match std::fs::read(path) {
            Ok(data) => data,
            Err(e) => {
                tracing::warn!("Failed to read image file {:?}: {}", path, e);
                return None;
            }
        };

        // Detect MIME type from file magic bytes
        let mime_type = match Self::detect_image_mime_type(&image_data) {
            Ok(mime) => mime,
            Err(e) => {
                tracing::error!("Failed to detect image format: {}", e);
                return None;
            }
        };

        // Encode to base64
        let base64_data =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &image_data);

        // Remove any newlines that might be in the base64 output
        let base64_clean: String = base64_data.chars().filter(|c| !c.is_whitespace()).collect();

        // Create data URL with correct MIME type
        let data_url = format!("data:{mime_type};base64,{base64_clean}");

        tracing::debug!(
            "Converted image {:?} to {} base64 ({} bytes -> {} chars)",
            path,
            mime_type,
            image_data.len(),
            base64_clean.len()
        );

        Some(data_url)
    }

    /// Detect image MIME type from file magic bytes
    /// Returns error for unsupported formats
    fn detect_image_mime_type(data: &[u8]) -> Result<&'static str, String> {
        if data.starts_with(b"\x89PNG\r\n\x1a\n") {
            Ok("image/png")
        } else if data.starts_with(b"\xff\xd8\xff") {
            Ok("image/jpeg")
        } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
            Ok("image/gif")
        } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
            Ok("image/webp")
        } else {
            let magic: String = data.iter().take(16).fold(String::new(), |mut acc, b| {
                use std::fmt::Write;
                let _ = write!(acc, "{b:02x}");
                acc
            });
            Err(format!("Unsupported image format (magic bytes: {magic})"))
        }
    }
}
