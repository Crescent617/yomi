use super::*;

#[test]
fn test_paginate_matches() {
    let matches: Vec<GrepMatch> = (1..=10)
        .map(|i| GrepMatch {
            path: PathBuf::from("test.rs"),
            line_number: i,
            lines: format!("line {i}"),
        })
        .collect();

    // limit=3, offset=0
    let (paginated, truncated) = paginate_matches(&matches, 3, 0);
    assert_eq!(paginated.len(), 3);
    assert!(truncated);
    assert_eq!(paginated[0].line_number, 1);
    assert_eq!(paginated[2].line_number, 3);

    // limit=3, offset=5
    let (paginated, truncated) = paginate_matches(&matches, 3, 5);
    assert_eq!(paginated.len(), 3);
    assert!(truncated);
    assert_eq!(paginated[0].line_number, 6);

    // limit=0 (no limit), offset=5
    let (paginated, truncated) = paginate_matches(&matches, 0, 5);
    assert_eq!(paginated.len(), 5);
    assert!(!truncated);
}

#[test]
fn test_format_matches() {
    let matches = vec![
        GrepMatch {
            path: PathBuf::from("src/main.rs"),
            line_number: 1,
            lines: "fn main()".to_string(),
        },
        GrepMatch {
            path: PathBuf::from("src/lib.rs"),
            line_number: 10,
            lines: "pub fn foo()".to_string(),
        },
    ];

    let formatted = format_matches(&matches, true);
    // Should show file path on its own line, then line number and content
    assert!(formatted.contains("src/main.rs\n1:fn main()"));
    assert!(formatted.contains("src/lib.rs\n10:pub fn foo()"));
    // Should have empty line between different files
    assert!(formatted.contains("fn main()\n\nsrc/lib.rs"));
}

#[test]
fn test_format_matches_multiline() {
    let matches = vec![GrepMatch {
        path: PathBuf::from("src/main.rs"),
        line_number: 1,
        lines: "fn main() {\n    println!(\"hello\");\n}".to_string(),
    }];

    let formatted = format_matches(&matches, true);
    // File path should be on its own line
    assert!(formatted.starts_with("src/main.rs\n"));
    // Each line should have its line number
    assert!(formatted.contains("1:fn main() {"));
    assert!(formatted.contains("2:    println!(\"hello\");"));
    assert!(formatted.contains("3:}"));
}

#[test]
fn test_extract_file_paths() {
    let matches = vec![
        GrepMatch {
            path: PathBuf::from("src/main.rs"),
            line_number: 1,
            lines: "line 1".to_string(),
        },
        GrepMatch {
            path: PathBuf::from("src/main.rs"),
            line_number: 2,
            lines: "line 2".to_string(),
        },
        GrepMatch {
            path: PathBuf::from("src/lib.rs"),
            line_number: 1,
            lines: "line 1".to_string(),
        },
    ];

    let paths = extract_file_paths(&matches);
    assert_eq!(paths.len(), 2);
    assert_eq!(paths[0], PathBuf::from("src/main.rs"));
    assert_eq!(paths[1], PathBuf::from("src/lib.rs"));
}

#[test]
fn test_ripgrep_result_is_empty() {
    let empty = GrepResult::default();
    assert!(empty.is_empty());

    let result = GrepResult {
        matches: vec![GrepMatch {
            path: PathBuf::from("src/main.rs"),
            line_number: 1,
            lines: "hello".to_string(),
        }],
    };
    assert!(!result.is_empty());
}

#[test]
fn test_ripgrep_result_paginate() {
    let matches: Vec<GrepMatch> = (1..=10)
        .map(|i| GrepMatch {
            path: PathBuf::from("test.rs"),
            line_number: i,
            lines: format!("line {i}"),
        })
        .collect();

    let result = GrepResult { matches };

    let (paginated, truncated) = result.paginate(3, 0);
    assert_eq!(paginated.len(), 3);
    assert!(truncated);
}

#[test]
fn test_ripgrep_result_unique_files() {
    let matches = vec![
        GrepMatch {
            path: PathBuf::from("src/main.rs"),
            line_number: 1,
            lines: "line 1".to_string(),
        },
        GrepMatch {
            path: PathBuf::from("src/main.rs"),
            line_number: 2,
            lines: "line 2".to_string(),
        },
        GrepMatch {
            path: PathBuf::from("src/lib.rs"),
            line_number: 1,
            lines: "line 1".to_string(),
        },
    ];

    let result = GrepResult { matches };

    let files = result.unique_files();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0], PathBuf::from("src/main.rs"));
    assert_eq!(files[1], PathBuf::from("src/lib.rs"));
}
