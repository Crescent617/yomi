//! Shared utilities for platform adapters.

/// Maximum retry delay for platform connection failures (shared across adapters).
#[cfg(feature = "feishu")]
#[allow(clippy::duration_suboptimal_units)]
pub(crate) const MAX_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(300);

/// Map each text segment outside fenced code blocks and inline code spans
/// through `f`, leaving code untouched so an example stays literal. An
/// unbalanced backtick only affects its own line — the `in_code` state
/// resets per line so a stray backtick never suppresses later lines.
pub(crate) fn map_outside_code_spans(text: &str, f: &mut dyn FnMut(&str, &mut String)) -> String {
    crate::utils::markdown::map_outside_fences(text, |run, out| {
        for line in run.split_inclusive('\n') {
            let mut in_code = false;
            for (i, segment) in line.split('`').enumerate() {
                if i > 0 {
                    out.push('`');
                    in_code = !in_code;
                }
                if in_code {
                    out.push_str(segment);
                } else {
                    f(segment, out);
                }
            }
        }
    })
}

/// Rewrite the platform-neutral `<@USER_ID>` mention contract (see the
/// agent prompt's Mentions section) into native syntax via `render`.
/// Fenced code blocks and inline code spans are left untouched so the
/// agent can show the syntax literally. The id pattern is bounded
/// (`{1,64}`) — feishu open_ids run ~36 chars, telegram ids are numeric —
/// so a runaway `<@...>` never matches.
pub(crate) fn rewrite_mentions(text: &str, render: &dyn Fn(&str) -> String) -> String {
    map_outside_code_spans(text, &mut |segment, out| {
        rewrite_mention_segment(segment, render, out);
    })
}

fn rewrite_mention_segment(segment: &str, render: &dyn Fn(&str) -> String, out: &mut String) {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"<@([A-Za-z0-9_\-]{1,64})>").unwrap());
    let mut last = 0;
    for cap in re.captures_iter(segment) {
        let m = cap.get(0).unwrap();
        out.push_str(&segment[last..m.start()]);
        out.push_str(&render(cap.get(1).unwrap().as_str()));
        last = m.end();
    }
    out.push_str(&segment[last..]);
}

/// Whether `text` carries a `<@USER_ID>` mention — same fence/inline-code
/// skipping as the rewrite (an example in a code block does not count).
pub(crate) fn contains_mention(text: &str) -> bool {
    let found = std::cell::Cell::new(false);
    let mark = |_: &str| {
        found.set(true);
        String::new()
    };
    let _ = rewrite_mentions(text, &mark);
    found.get()
}

/// A file read and validated for platform upload (see [`read_upload`]).
#[cfg(any(feature = "feishu", feature = "telegram"))]
pub(crate) struct UploadFile {
    pub bytes: Vec<u8>,
    pub file_name: String,
    pub is_image: bool,
}

/// Read `path` and validate it against platform upload caps, shared by the
/// platform adapters: empty uploads and oversize files are rejected by the
/// platforms with a generic error, so fail fast here with a precise reason.
/// `image_kind`/`file_kind` name the two classes in user-facing errors
/// (Feishu: image/file; Telegram: photo/document). The delivery file name
/// falls back to `file` when the path has no usable name.
#[cfg(any(feature = "feishu", feature = "telegram"))]
pub(crate) async fn read_upload(
    path: &std::path::Path,
    image_max_bytes: usize,
    file_max_bytes: usize,
    image_kind: &'static str,
    file_kind: &'static str,
) -> Result<UploadFile, super::ChannelError> {
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| super::ChannelError::Platform(format!("read file: {e}")))?;
    let is_image = mime_guess::from_path(path).first_or_octet_stream().type_() == "image";
    let (limit, kind) = if is_image {
        (image_max_bytes, image_kind)
    } else {
        (file_max_bytes, file_kind)
    };
    if bytes.is_empty() {
        return Err(super::ChannelError::Platform(format!("empty {kind}")));
    }
    if bytes.len() > limit {
        return Err(super::ChannelError::Platform(format!(
            "{kind} exceeds the {}MB platform limit",
            limit / 1024 / 1024
        )));
    }
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string();
    Ok(UploadFile {
        bytes,
        file_name,
        is_image,
    })
}

#[cfg(test)]
#[path = "utils_test.rs"]
mod tests;
