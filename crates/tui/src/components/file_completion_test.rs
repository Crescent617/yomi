use super::*;

#[test]
fn test_file_completion_new() {
    let fc = FileCompletion::new();
    assert!(!fc.is_active());
    assert_eq!(fc.total_files(), 0);
}

#[test]
fn test_nucleo_basic() {
    // Create nucleo matcher
    let config = nucleo::Config::DEFAULT;
    let mut nucleo = Nucleo::<String>::new(config, Arc::new(|| {}), None, 1);

    // Inject some test files
    let injector = nucleo.injector();
    for file in ["src/main.rs", "src/lib.rs", "Cargo.toml", "README.md"] {
        let _ = injector.push(file.to_string(), |s, cols| {
            cols[0] = s.clone().into();
        });
    }

    // Process injected items
    for _ in 0..20 {
        nucleo.tick(10);
    }

    // Get snapshot with empty pattern
    let snapshot = nucleo.snapshot();
    let item_count = snapshot.item_count();
    let match_count = snapshot.matched_item_count();

    println!("Total items: {item_count}, Matched: {match_count}");

    // With empty pattern, should match all items
    assert!(item_count > 0, "Should have items in nucleo");

    // Now test with pattern
    nucleo.pattern.reparse(
        0,
        "main",
        nucleo::pattern::CaseMatching::Smart,
        nucleo::pattern::Normalization::Smart,
        false,
    );

    // Tick again for pattern matching
    for _ in 0..10 {
        nucleo.tick(10);
    }

    let snapshot = nucleo.snapshot();
    let match_count = snapshot.matched_item_count();
    println!("After 'main' pattern: {match_count} matches");

    // Should find at least src/main.rs
    assert!(match_count > 0, "Should find 'main' pattern matches");
}

#[test]
fn test_accept_rejects_placeholders() {
    let mut fc = FileCompletion::new();
    // Manually set up without spawning tokio tasks
    fc.active = true;
    fc.query_start_pos = 0;
    fc.query.clear();

    fc.completion
        .set_items(vec![PLACEHOLDER_SCANNING.to_string()]);
    assert!(fc.accept().is_none());

    fc.completion
        .set_items(vec![PLACEHOLDER_NO_MATCHES.to_string()]);
    assert!(fc.accept().is_none());

    fc.completion.set_items(vec!["src/main.rs".to_string()]);
    assert!(fc.accept().is_some());
}
