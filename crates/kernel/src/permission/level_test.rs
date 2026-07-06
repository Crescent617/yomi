use super::*;

#[test]
fn test_level_ordering() {
    assert!(Level::Safe < Level::Caution);
    assert!(Level::Caution < Level::Dangerous);
}

#[test]
fn test_exceeds_threshold() {
    // Safe threshold: only Safe passes
    assert!(!exceeds_threshold(Level::Safe, Level::Safe));
    assert!(exceeds_threshold(Level::Caution, Level::Safe));
    assert!(exceeds_threshold(Level::Dangerous, Level::Safe));

    // Caution threshold: Safe and Caution pass
    assert!(!exceeds_threshold(Level::Safe, Level::Caution));
    assert!(!exceeds_threshold(Level::Caution, Level::Caution));
    assert!(exceeds_threshold(Level::Dangerous, Level::Caution));

    // Dangerous threshold: all pass
    assert!(!exceeds_threshold(Level::Safe, Level::Dangerous));
    assert!(!exceeds_threshold(Level::Caution, Level::Dangerous));
    assert!(!exceeds_threshold(Level::Dangerous, Level::Dangerous));
}

#[test]
fn test_from_str() {
    use std::str::FromStr;

    assert_eq!(Level::from_str("safe"), Ok(Level::Safe));
    assert_eq!(Level::from_str("CAUTION"), Ok(Level::Caution));
    assert_eq!(Level::from_str("Dangerous"), Ok(Level::Dangerous));
    assert!(Level::from_str("invalid").is_err());
}
