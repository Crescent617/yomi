use rand::RngExt;

const BASE56_CHARS: &[u8] = b"23456789abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ";

/// Generate a random base56 ID of specified length
pub fn gen_base56_id(len: usize) -> String {
    let mut rng = rand::rng();
    (0..len)
        .map(|_| {
            let idx = rng.random_range(0..BASE56_CHARS.len());
            BASE56_CHARS[idx] as char
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gen_base56_id_length() {
        // Test various lengths
        assert_eq!(gen_base56_id(0).len(), 0);
        assert_eq!(gen_base56_id(1).len(), 1);
        assert_eq!(gen_base56_id(8).len(), 8);
        assert_eq!(gen_base56_id(16).len(), 16);
        assert_eq!(gen_base56_id(32).len(), 32);
    }

    #[test]
    fn test_gen_base56_id_charset() {
        // Verify only valid characters are used
        let id = gen_base56_id(100);
        for ch in id.chars() {
            assert!(
                BASE56_CHARS.contains(&(ch as u8)),
                "Character '{ch}' is not in BASE56_CHARS"
            );
        }
    }

    #[test]
    fn test_gen_base56_id_no_ambiguous_chars() {
        // Verify no ambiguous characters (0, 1, O, I, l) are used
        let id = gen_base56_id(1000);
        assert!(!id.contains('0'));
        assert!(!id.contains('1'));
        assert!(!id.contains('O'));
        assert!(!id.contains('I'));
        assert!(!id.contains('l'));
    }

    #[test]
    fn test_gen_base56_id_uniqueness() {
        // Generate many IDs and verify they're not all the same (probabilistic test)
        let ids: std::collections::HashSet<String> = (0..100).map(|_| gen_base56_id(8)).collect();
        // With 56^8 possible combinations, collisions should be extremely rare
        assert_eq!(ids.len(), 100, "Expected 100 unique IDs");
    }

    #[test]
    fn test_gen_base56_id_contains_valid_chars() {
        // Specific valid characters that should appear
        let id = gen_base56_id(1000);
        let has_digit = id.chars().any(|c| c.is_ascii_digit());
        let has_lower = id.chars().any(|c| c.is_ascii_lowercase());
        let has_upper = id.chars().any(|c| c.is_ascii_uppercase());

        // With 1000 chars from a set of 56, we should have all three categories
        // This is probabilistic but extremely likely
        assert!(has_digit, "Should contain digits");
        assert!(has_lower, "Should contain lowercase letters");
        assert!(has_upper, "Should contain uppercase letters");
    }
}
