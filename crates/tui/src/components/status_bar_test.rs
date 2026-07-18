use super::StatusBar;
use tuirealm::ratatui::style::Modifier;

#[test]
fn activity_text_hides_zero_counts() {
    assert_eq!(StatusBar::activity_text(0, 0), "");
    assert_eq!(StatusBar::activity_text(1, 0), "󰚩 1 AGENT");
    assert_eq!(StatusBar::activity_text(2, 0), "󰚩 2 AGENTS");
    assert_eq!(StatusBar::activity_text(0, 1), " 1 BG TASK");
    assert_eq!(StatusBar::activity_text(2, 3), "󰚩 2 AGENTS   3 BG TASKS");
}

#[test]
fn activity_badges_use_distinct_fancy_styles() {
    let status = StatusBar {
        activity_counts: (2, 3),
        ..StatusBar::default()
    };
    let line = status.render_activity_section();

    assert_eq!(line.spans.len(), 3);
    assert_eq!(line.spans[0].content, " 󰚩 2 AGENTS ");
    assert_eq!(line.spans[1].content, " ");
    assert_eq!(line.spans[2].content, "  3 BG TASKS ");
    assert_ne!(line.spans[0].style.bg, line.spans[2].style.bg);
    assert!(line.spans[0].style.add_modifier.contains(Modifier::BOLD));
    assert!(line.spans[2].style.add_modifier.contains(Modifier::BOLD));
}
