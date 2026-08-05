use super::*;

#[test]
fn message_from_args_joined() {
    let args = vec!["hello".to_string(), "world".to_string()];
    assert_eq!(resolve_message(&args, None).unwrap(), "hello world");
}

#[test]
fn message_from_args_ignores_stdin() {
    let args = vec!["hi".to_string()];
    assert_eq!(
        resolve_message(&args, Some("piped".to_string())).unwrap(),
        "hi"
    );
}

#[test]
fn message_from_stdin_trimmed() {
    assert_eq!(
        resolve_message(&[], Some("  piped text\n".to_string())).unwrap(),
        "piped text"
    );
}

#[test]
fn empty_message_errors() {
    assert!(resolve_message(&[], None).is_err());
    assert!(resolve_message(&[], Some("  \n".to_string())).is_err());
    assert!(resolve_message(&["  ".to_string()], None).is_err());
}
