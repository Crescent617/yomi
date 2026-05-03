//! Environment variable utilities for the kernel crate

/// Get environment variable - inlined for performance
#[inline]
pub fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Try multiple env vars in order, return first set value
#[inline]
pub fn env_first(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| env_var(name))
}

/// Parse environment variable as a specific type
#[inline]
pub fn env_parse<T: std::str::FromStr>(name: &str) -> Option<T> {
    env_var(name).and_then(|s| s.parse().ok())
}

/// Parse boolean from environment variable
#[inline]
pub fn env_bool(name: &str) -> bool {
    std::env::var(name).is_ok_and(|s| {
        matches!(
            s.as_bytes(),
            b"true" | b"1" | b"yes" | b"TRUE" | b"YES" | b"on"
        )
    })
}

/// Parse optional boolean from environment variable
#[inline]
pub fn env_bool_opt(name: &str) -> Option<bool> {
    std::env::var(name).ok().map(|s| {
        matches!(
            s.as_bytes(),
            b"true" | b"1" | b"yes" | b"TRUE" | b"YES" | b"on"
        )
    })
}

/// Parse number with unit suffix (k/m) from string
/// Supports formats like "131072", "128k", "200k", "1m"
pub fn parse_number_with_unit(s: &str) -> Option<u32> {
    let s = s.trim().to_lowercase();

    // Check for 'k' suffix (thousands)
    if let Some(num_str) = s.strip_suffix('k') {
        let num: f32 = num_str.parse().ok()?;
        return Some((num * 1000.0) as u32);
    }

    // Check for 'm' suffix (millions)
    if let Some(num_str) = s.strip_suffix('m') {
        let num: f32 = num_str.parse().ok()?;
        return Some((num * 1_000_000.0) as u32);
    }

    // Plain number
    s.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_env_bool_parsing() {
        // Test via actual env var manipulation
        std::env::set_var("TEST_BOOL_TRUE", "true");
        std::env::set_var("TEST_BOOL_1", "1");
        std::env::set_var("TEST_BOOL_YES", "yes");
        std::env::set_var("TEST_BOOL_UPPER", "TRUE");
        std::env::set_var("TEST_BOOL_FALSE", "false");
        std::env::set_var("TEST_BOOL_0", "0");
        std::env::set_var("TEST_BOOL_EMPTY", "");

        assert!(env_bool("TEST_BOOL_TRUE"));
        assert!(env_bool("TEST_BOOL_1"));
        assert!(env_bool("TEST_BOOL_YES"));
        assert!(env_bool("TEST_BOOL_UPPER"));
        assert!(!env_bool("TEST_BOOL_FALSE"));
        assert!(!env_bool("TEST_BOOL_0"));
        assert!(!env_bool("TEST_BOOL_EMPTY"));
        assert!(!env_bool("TEST_BOOL_NONEXISTENT"));

        // Cleanup
        for key in [
            "TEST_BOOL_TRUE",
            "TEST_BOOL_1",
            "TEST_BOOL_YES",
            "TEST_BOOL_UPPER",
            "TEST_BOOL_FALSE",
            "TEST_BOOL_0",
            "TEST_BOOL_EMPTY",
        ] {
            std::env::remove_var(key);
        }
    }

    #[test]
    fn test_env_var_and_first() {
        std::env::set_var("TEST_VAR_1", "value1");
        std::env::set_var("TEST_VAR_2", "value2");

        assert_eq!(env_var("TEST_VAR_1"), Some("value1".to_string()));
        assert_eq!(env_var("TEST_VAR_2"), Some("value2".to_string()));
        assert_eq!(env_var("TEST_VAR_NONEXISTENT"), None);

        // Test env_first
        assert_eq!(
            env_first(&["TEST_VAR_NONEXISTENT", "TEST_VAR_1"]),
            Some("value1".to_string())
        );
        assert_eq!(
            env_first(&["TEST_VAR_1", "TEST_VAR_2"]),
            Some("value1".to_string())
        );
        assert_eq!(env_first(&["TEST_VAR_NONEXISTENT"]), None);

        // Cleanup
        std::env::remove_var("TEST_VAR_1");
        std::env::remove_var("TEST_VAR_2");
    }

    #[test]
    fn test_env_parse() {
        std::env::set_var("TEST_PARSE_INT", "42");
        std::env::set_var("TEST_PARSE_FLOAT", "3.14");
        std::env::set_var("TEST_PARSE_INVALID", "not_a_number");

        assert_eq!(env_parse::<i32>("TEST_PARSE_INT"), Some(42));
        assert_eq!(env_parse::<f64>("TEST_PARSE_FLOAT"), Some(3.14));
        assert_eq!(env_parse::<i32>("TEST_PARSE_INVALID"), None);
        assert_eq!(env_parse::<i32>("TEST_PARSE_NONEXISTENT"), None);

        // Cleanup
        for key in ["TEST_PARSE_INT", "TEST_PARSE_FLOAT", "TEST_PARSE_INVALID"] {
            std::env::remove_var(key);
        }
    }

    #[test]
    fn test_env_bool_opt() {
        std::env::set_var("TEST_OPT_TRUE", "true");
        std::env::set_var("TEST_OPT_FALSE", "false");

        assert_eq!(env_bool_opt("TEST_OPT_TRUE"), Some(true));
        assert_eq!(env_bool_opt("TEST_OPT_FALSE"), Some(false));
        assert_eq!(env_bool_opt("TEST_OPT_NONEXISTENT"), None);

        // Cleanup
        std::env::remove_var("TEST_OPT_TRUE");
        std::env::remove_var("TEST_OPT_FALSE");
    }

    #[test]
    fn test_parse_number_with_unit() {
        // Plain numbers
        assert_eq!(parse_number_with_unit("131072"), Some(131_072));
        assert_eq!(parse_number_with_unit("200000"), Some(200_000));
        assert_eq!(parse_number_with_unit("1000"), Some(1000));

        // k suffix
        assert_eq!(parse_number_with_unit("128k"), Some(128_000));
        assert_eq!(parse_number_with_unit("200k"), Some(200_000));
        assert_eq!(parse_number_with_unit("1.5k"), Some(1500));

        // m suffix
        assert_eq!(parse_number_with_unit("1m"), Some(1_000_000));
        assert_eq!(parse_number_with_unit("2m"), Some(2_000_000));

        // With whitespace
        assert_eq!(parse_number_with_unit(" 128k "), Some(128_000));

        // Invalid values
        assert_eq!(parse_number_with_unit("invalid"), None);
        assert_eq!(parse_number_with_unit(""), None);
    }
}
