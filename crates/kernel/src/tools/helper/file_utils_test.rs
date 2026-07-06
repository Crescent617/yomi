use super::*;

use std::io::Write;
use tempfile::TempDir;

#[tokio::test]
async fn test_get_mtime_existing_file() {
    let temp = TempDir::new().unwrap();
    let file_path = temp.path().join("test.txt");

    // Create file
    let mut file = std::fs::File::create(&file_path).unwrap();
    file.write_all(b"test").unwrap();
    drop(file);

    let mtime = get_mtime(&file_path).await;
    assert!(
        mtime.is_some() && mtime.unwrap() > 0,
        "mtime should be greater than 0 for existing file"
    );
}

#[tokio::test]
async fn test_get_mtime_nonexistent_file() {
    let mtime = get_mtime(Path::new("/nonexistent/file.txt")).await;
    assert_eq!(mtime, None, "mtime should be None for nonexistent file");
}

#[tokio::test]
async fn test_get_mtimes_concurrent() {
    let temp = TempDir::new().unwrap();
    let base_path = temp.path().to_path_buf();

    // Create multiple test files
    let file1 = base_path.join("file1.txt");
    let file2 = base_path.join("file2.txt");
    let file3 = base_path.join("file3.txt");

    std::fs::write(&file1, "content1").unwrap();
    std::fs::write(&file2, "content2").unwrap();
    // file3 doesn't exist

    let paths = vec![file1.clone(), file2.clone(), file3.clone()];

    let results = get_mtimes_concurrent(paths, None).await;

    // Should have 2 results (non-existent file skipped)
    assert_eq!(results.len(), 2);
    assert!(results[0].1 > 0); // file1 exists
    assert!(results[1].1 > 0); // file2 exists
}

#[tokio::test]
async fn test_get_mtimes_concurrent_with_limit() {
    let temp = TempDir::new().unwrap();
    let base_path = temp.path().to_path_buf();

    // Create test files
    for i in 0..10 {
        let file = base_path.join(format!("file{i}.txt"));
        std::fs::write(&file, format!("content{i}")).unwrap();
    }

    let paths: Vec<PathBuf> = (0..10)
        .map(|i| base_path.join(format!("file{i}.txt")))
        .collect();

    // Use a low concurrency limit
    let results = get_mtimes_concurrent(paths, Some(2)).await;

    // Should have 10 results (all files exist)
    assert_eq!(results.len(), 10);
}
