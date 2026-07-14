use super::StatusBar;

#[test]
fn activity_text_hides_zero_counts() {
    assert_eq!(StatusBar::activity_text(0, 0), "");
    assert_eq!(StatusBar::activity_text(2, 0), "Agents 2");
    assert_eq!(StatusBar::activity_text(0, 3), "Shells 3");
    assert_eq!(StatusBar::activity_text(2, 3), "Agents 2 · Shells 3");
}
