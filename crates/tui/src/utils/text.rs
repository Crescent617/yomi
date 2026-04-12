//! Text preprocessing utilities for TUI rendering

/// Preprocess text for display by:
/// - Converting tabs to 2 spaces for consistent width
pub fn preprocess(text: impl AsRef<str>) -> String {
    text.as_ref().replace('\t', "  ")
}
