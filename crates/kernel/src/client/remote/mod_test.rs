//! Tests for the remote kernel client's pure helpers.

use super::resolve_auth_token;

#[test]
fn explicit_token_wins_over_env() {
    assert_eq!(
        resolve_auth_token(Some("explicit".to_string()), Some("env".to_string())),
        Some("explicit".to_string())
    );
}

#[test]
fn missing_explicit_falls_back_to_env() {
    assert_eq!(
        resolve_auth_token(None, Some("env".to_string())),
        Some("env".to_string())
    );
}

#[test]
fn blank_explicit_falls_back_to_env() {
    for blank in ["", "   ", "\t\n"] {
        assert_eq!(
            resolve_auth_token(Some(blank.to_string()), Some("env".to_string())),
            Some("env".to_string()),
            "blank token {blank:?} should fall back to env"
        );
    }
}

#[test]
fn no_token_anywhere_yields_none() {
    assert_eq!(resolve_auth_token(None, None), None);
    assert_eq!(resolve_auth_token(Some(" ".to_string()), None), None);
}
