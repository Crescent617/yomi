use super::*;
use std::io::Write as _;

fn write_file(dir: &tempfile::TempDir, name: &str, content: &str) {
    let path = dir.path().join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = std::fs::File::create(&path).unwrap();
    write!(f, "{content}").unwrap();
}

fn params<'a>(pattern: &'a str) -> SearchParams<'a> {
    SearchParams {
        pattern,
        case_insensitive: false,
        multiline: false,
        context_before: 0,
        context_after: 0,
        glob_patterns: &[],
        file_type: None,
        max_columns: 500,
        deadline: None,
    }
}

#[test]
fn files_mode_finds_matching_files_only() {
    let dir = tempfile::TempDir::new().unwrap();
    write_file(&dir, "a.rs", "fn main() { println!(\"1\"); }\n");
    write_file(&dir, "b.rs", "fn foo() {}\n");
    let report = search(dir.path(), SearchMode::Files, &params("println!")).unwrap();
    let SearchOutcome::Files(files) = report.outcome else {
        panic!("wrong outcome");
    };
    assert_eq!(files.len(), 1);
    assert!(files[0].ends_with("a.rs"));
}

#[test]
fn content_mode_collects_matches_with_line_numbers() {
    let dir = tempfile::TempDir::new().unwrap();
    write_file(&dir, "a.rs", "line 1\nfn main() {\nline 3\n");
    let report = search(dir.path(), SearchMode::Content, &params("fn main")).unwrap();
    let SearchOutcome::Content(result) = report.outcome else {
        panic!("wrong outcome");
    };
    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].line_number, 2);
    assert_eq!(result.matches[0].lines, "fn main() {\n");
    assert_eq!(result.files_searched.len(), 1);
}

#[test]
fn content_mode_includes_context_lines() {
    let dir = tempfile::TempDir::new().unwrap();
    write_file(
        &dir,
        "a.rs",
        "line 1\nline 2\nfn main() {\nline 4\nline 5\n",
    );
    let mut p = params("fn main");
    p.context_before = 2;
    p.context_after = 2;
    let report = search(dir.path(), SearchMode::Content, &p).unwrap();
    let SearchOutcome::Content(result) = report.outcome else {
        panic!("wrong outcome");
    };
    let lines: Vec<&str> = result.matches.iter().map(|m| m.lines.trim_end()).collect();
    assert_eq!(
        lines,
        vec!["line 1", "line 2", "fn main() {", "line 4", "line 5"]
    );
    let nums: Vec<usize> = result.matches.iter().map(|m| m.line_number).collect();
    assert_eq!(nums, vec![1, 2, 3, 4, 5]);
}

#[test]
fn count_mode_counts_matching_lines() {
    let dir = tempfile::TempDir::new().unwrap();
    write_file(&dir, "a.rs", "x\nx y\nz\n");
    let report = search(dir.path(), SearchMode::Count, &params("x")).unwrap();
    let SearchOutcome::Counts(counts) = report.outcome else {
        panic!("wrong outcome");
    };
    assert_eq!(counts.len(), 1);
    assert_eq!(counts[0].1, 2, "lines containing x");
}

#[test]
fn hidden_files_are_included() {
    let dir = tempfile::TempDir::new().unwrap();
    write_file(&dir, ".hidden.rs", "fn secret() {}\n");
    let report = search(dir.path(), SearchMode::Files, &params("secret")).unwrap();
    let SearchOutcome::Files(files) = report.outcome else {
        panic!("wrong outcome");
    };
    assert_eq!(files.len(), 1);
}

#[test]
fn gitignore_is_respected() {
    let dir = tempfile::TempDir::new().unwrap();
    // .gitignore 规则只在 git 仓库内生效（rg 与 ignore 库同一语义）。
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    write_file(&dir, ".gitignore", "ignored.rs\n");
    write_file(&dir, "ignored.rs", "secret\n");
    write_file(&dir, "kept.rs", "secret\n");
    let report = search(dir.path(), SearchMode::Files, &params("secret")).unwrap();
    let SearchOutcome::Files(files) = report.outcome else {
        panic!("wrong outcome");
    };
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(files[0].ends_with("kept.rs"));
}

#[test]
fn glob_filter_selects_files() {
    let dir = tempfile::TempDir::new().unwrap();
    write_file(&dir, "a.rs", "main\n");
    write_file(&dir, "b.js", "main\n");
    let globs = vec!["*.rs".to_string()];
    let mut p = params("main");
    p.glob_patterns = &globs;
    let report = search(dir.path(), SearchMode::Files, &p).unwrap();
    let SearchOutcome::Files(files) = report.outcome else {
        panic!("wrong outcome");
    };
    assert_eq!(files.len(), 1);
    assert!(files[0].ends_with("a.rs"));
}

#[test]
fn file_type_filter_selects_files() {
    let dir = tempfile::TempDir::new().unwrap();
    write_file(&dir, "a.rs", "main\n");
    write_file(&dir, "b.js", "main\n");
    let mut p = params("main");
    p.file_type = Some("rust");
    let report = search(dir.path(), SearchMode::Files, &p).unwrap();
    let SearchOutcome::Files(files) = report.outcome else {
        panic!("wrong outcome");
    };
    assert_eq!(files.len(), 1);
    assert!(files[0].ends_with("a.rs"));

    let mut p = params("main");
    p.file_type = Some("not-a-real-type");
    assert!(matches!(
        search(dir.path(), SearchMode::Files, &p),
        Err(SearchError::FileType(_))
    ));
}

#[test]
fn invalid_pattern_is_pattern_error() {
    let dir = tempfile::TempDir::new().unwrap();
    assert!(matches!(
        search(dir.path(), SearchMode::Files, &params("(unclosed")),
        Err(SearchError::Pattern(_))
    ));
}

#[test]
fn case_insensitive_and_binary_skip() {
    let dir = tempfile::TempDir::new().unwrap();
    write_file(&dir, "a.rs", "fn MAIN() {}\n");
    let bin = dir.path().join("b.bin");
    std::fs::write(&bin, b"main\0binary\n").unwrap();

    let mut p = params("main");
    p.case_insensitive = true;
    let report = search(dir.path(), SearchMode::Files, &p).unwrap();
    let SearchOutcome::Files(files) = report.outcome else {
        panic!("wrong outcome");
    };
    assert_eq!(files.len(), 1, "binary file must be skipped: {files:?}");
    assert!(files[0].ends_with("a.rs"));
}

#[test]
fn deadline_returns_timeout() {
    let dir = tempfile::TempDir::new().unwrap();
    write_file(&dir, "a.rs", "x\n");
    let mut p = params("x");
    // 已过的截止时刻：立刻超时。
    p.deadline = Some(Instant::now() - Duration::from_secs(1));
    assert!(matches!(
        search(dir.path(), SearchMode::Content, &p),
        Err(SearchError::Timeout(_))
    ));
}
