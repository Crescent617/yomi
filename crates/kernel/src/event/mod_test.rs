use super::format_retry_delay;

#[test]
fn format_retry_delay_renders_seconds_rounded_up() {
    assert_eq!(format_retry_delay(34_000).as_deref(), Some("in 34s"));
    // Sub-second remainders round up — never claim "0s" of waiting.
    assert_eq!(format_retry_delay(1_001).as_deref(), Some("in 2s"));
    assert_eq!(format_retry_delay(1).as_deref(), Some("in 1s"));
}

#[test]
fn format_retry_delay_none_without_wait() {
    assert_eq!(format_retry_delay(0), None);
}
