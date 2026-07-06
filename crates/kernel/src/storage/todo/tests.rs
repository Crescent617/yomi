use super::*;

#[test]
fn test_strip_system_reminders() {
    let text = "Hello\n<system_reminder>\nReminder\n</system_reminder>\nWorld";
    let result = strip_system_reminders(text);
    assert!(!result.contains("<system_reminder>"));
    assert!(result.contains("Hello"));
    assert!(result.contains("World"));
}

#[test]
fn test_strip_system_reminders_multiple() {
    let text = "A<system_reminder>1</system_reminder>B<system_reminder>2</system_reminder>C";
    let result = strip_system_reminders(text);
    assert_eq!(result, "ABC");
}

#[test]
fn test_strip_system_reminders_unclosed() {
    let text = "Hello<system_reminder>no end";
    let result = strip_system_reminders(text);
    assert_eq!(result, "Hello<system_reminder>no end");
}
