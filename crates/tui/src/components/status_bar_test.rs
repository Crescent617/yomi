use super::{AppMode, StatusBar};
use crate::theme::colors;
use kernel::permission::Level;
use tuirealm::ratatui::style::Modifier;

#[test]
fn mode_section_uses_colored_foreground_without_background() {
    let mut status = StatusBar::default();
    status.set_mode(AppMode::Browse);

    let mode = &status.render_left_section().spans[0];
    assert_eq!(mode.content, " BROWSE ");
    assert_eq!(mode.style.fg, Some(colors::accent_system()));
    assert_eq!(mode.style.bg, None);
    assert!(mode.style.add_modifier.contains(Modifier::BOLD));

    let mut status = StatusBar::default();
    status.set_permission_level(Level::Dangerous);

    let mode = &status.render_left_section().spans[0];
    assert_eq!(mode.content, " YOLO ");
    assert_eq!(mode.style.fg, Some(colors::accent_warning()));
    assert_eq!(mode.style.bg, None);
}

#[test]
fn activity_indicators_use_colored_foregrounds_without_backgrounds() {
    let status = StatusBar {
        activity_counts: (2, 3),
        ..StatusBar::default()
    };
    let line = status.render_activity_section();

    assert_eq!(line.spans.len(), 2);
    assert_eq!(line.spans[0].content, " 󰚩 2 Agents ");
    assert_eq!(line.spans[1].content, "  3 Shells ");
    assert_eq!(line.spans[0].style.fg, Some(colors::accent_info()));
    assert_eq!(line.spans[1].style.fg, Some(colors::accent_warning()));
    assert_eq!(line.spans[0].style.bg, None);
    assert_eq!(line.spans[1].style.bg, None);
    assert!(line.spans[0].style.add_modifier.contains(Modifier::BOLD));
    assert!(line.spans[1].style.add_modifier.contains(Modifier::BOLD));
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
    assert_eq!(line.spans[1].content, " 󰚩 1 Agent ");
    assert_eq!(line.spans[2].content, "  1 Shell ");
}
