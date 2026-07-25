use super::{InfoBar, InfoBarState};

fn streaming_bar(tick_frame: usize) -> InfoBar {
    let mut bar = InfoBar::new();
    bar.state = InfoBarState::Streaming;
    bar.tick_frame = tick_frame;
    bar
}

#[test]
fn active_state_verb_has_leading_space() {
    let bar = streaming_bar(0);
    let line = bar.render_left_section();
    assert_eq!(
        line.spans[0].content.as_ref(),
        " ",
        "status word should start with a leading space"
    );
}

#[test]
fn terminal_states_show_static_glyph_first() {
    for state in [InfoBarState::Completed, InfoBarState::Cancelled] {
        let mut bar = InfoBar::new();
        bar.state = state;
        bar.tick_frame = 3;
        let line = bar.render_left_section();
        let first = line.spans[0].content.as_ref();
        assert_ne!(
            first, " ",
            "{state:?} should show its static glyph, not the leading space"
        );
    }
}

#[test]
fn idle_without_activity_renders_nothing() {
    let bar = InfoBar::new();
    assert!(bar.render_left_section().spans.is_empty());
}
