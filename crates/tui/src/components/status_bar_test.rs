use super::{AppMode, StatusBar};
use crate::theme::colors;
use tuirealm::ratatui::style::Modifier;

#[test]
fn activity_text_hides_zero_counts() {
    assert_eq!(StatusBar::activity_text(0, 0), "");
    assert_eq!(StatusBar::activity_text(1, 0), "󰚩 1 Agent");
    assert_eq!(StatusBar::activity_text(2, 0), "󰚩 2 Agents");
    assert_eq!(StatusBar::activity_text(0, 1), " 1 Shell");
    assert_eq!(StatusBar::activity_text(2, 3), "󰚩 2 Agents   3 Shells");
}

#[test]
fn activity_indicators_use_colored_foregrounds_without_backgrounds() {
    let status = StatusBar {
        activity_counts: (2, 3),
        ..StatusBar::default()
    };
    let line = status.render_activity_section();

    assert_eq!(line.spans.len(), 3);
    assert_eq!(line.spans[0].content, "󰚩 2 Agents");
    assert_eq!(line.spans[1].content, "  ");
    assert_eq!(line.spans[2].content, " 3 Shells");
    assert_eq!(line.spans[0].style.fg, Some(colors::accent_info()));
    assert_eq!(line.spans[2].style.fg, Some(colors::accent_warning()));
    assert_eq!(line.spans[0].style.bg, None);
    assert_eq!(line.spans[2].style.bg, None);
    assert!(line.spans[0].style.add_modifier.contains(Modifier::BOLD));
    assert!(line.spans[2].style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn activity_indicators_immediately_follow_mode_in_left_section() {
    let mut status = StatusBar {
        activity_counts: (1, 1),
        ..StatusBar::default()
    };
    status.set_mode(AppMode::Browse);

    let line = status.render_left_section();

    assert_eq!(line.spans[0].content, " BROWSE ");
    assert_eq!(line.spans[1].content, "󰚩 1 Agent");
    assert_eq!(line.spans[2].content, "  ");
    assert_eq!(line.spans[3].content, " 1 Shell");
}
