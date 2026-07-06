use super::*;

#[test]
fn test_format_answers() {
    let mut answers = HashMap::new();
    answers.insert("Which library?".to_string(), "chrono".to_string());

    let text = format_answers(&answers);
    assert!(text.contains("chrono"));
    assert!(text.contains("Which library?"));
}
