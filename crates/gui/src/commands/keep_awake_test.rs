#![cfg(not(any(target_os = "android", target_os = "ios")))]

use super::imp;

#[test]
fn set_is_idempotent_and_toggles() {
    assert!(!imp::get());
    // Headless Linux has no D-Bus for logind: creating the assertion fails
    // there — the error must surface and the state must stay off.
    if imp::set(true).is_err() {
        assert!(!imp::get());
        return;
    }
    assert!(imp::get());
    // Same state again: no-op, still on.
    assert_eq!(imp::set(true), Ok(true));
    assert!(imp::get());
    assert_eq!(imp::set(false), Ok(false));
    assert!(!imp::get());
    assert_eq!(imp::set(false), Ok(false));
    assert!(!imp::get());
}
