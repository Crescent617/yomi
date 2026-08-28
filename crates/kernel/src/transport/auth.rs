//! Socket auth for network transports (ws/wss).
//!
//! Model: the daemon is configured with a password *hash* via the
//! `YOMI_SOCKET_AUTH_HASH` env var; clients present the plaintext
//! password as `Authorization: Bearer <password>` during the WebSocket
//! upgrade handshake. Unix sockets never use this — they rely on
//! filesystem permissions instead.
//!
//! blake3 is appropriate here because the password is expected to be a
//! high-entropy machine token, not a human passphrase. Generate one with
//! `yomi daemon auth-hash`.

use std::sync::Arc;

/// Transport-layer credential check, injected into the ws listener.
/// Returns `true` if the presented password may proceed.
pub type AuthVerifier = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// `blake3:<hex>` of a password — the value put in `YOMI_SOCKET_AUTH_HASH`.
pub fn hash_password(password: &str) -> String {
    format!("blake3:{}", blake3::hash(password.as_bytes()).to_hex())
}

/// Extract the token from an `Authorization: Bearer <token>` header
/// value. The scheme is case-insensitive per RFC 6750.
pub(crate) fn bearer_token(value: &str) -> Option<&str> {
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim_start())
}

/// Build a verifier from the configured hash string.
///
/// Accepts `blake3:<hex>` or a bare hex digest; surrounding whitespace and
/// hex case are tolerated. An unrecognized (non-hex, wrong-length) value
/// yields a verifier that rejects everything — fail closed.
pub fn auth_verifier(configured_hash: &str) -> AuthVerifier {
    let expected = configured_hash
        .trim()
        .strip_prefix("blake3:")
        .unwrap_or(configured_hash.trim())
        .trim()
        .to_ascii_lowercase();
    Arc::new(move |presented: &str| {
        let actual = blake3::hash(presented.as_bytes()).to_hex();
        constant_time_eq(expected.as_bytes(), actual.as_str().as_bytes())
    })
}

/// Length leaks nothing here: both sides are fixed-length hex digests.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
#[path = "auth_test.rs"]
mod tests;
