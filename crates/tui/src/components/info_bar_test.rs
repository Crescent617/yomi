use super::{InfoBar, InfoBarState, SPINNER_FRAMES};

fn streaming_bar(tick_frame: usize) -> InfoBar {
    let mut bar = InfoBar::new();
    bar.state = InfoBarState::Streaming;
    bar.tick_frame = tick_frame;
    bar
}

#[test]
fn active_state_shows_spinner_before_status_word() {
    let bar = streaming_bar(0);
    let line = bar.render_left_section();
    let first = line.spans[0].content.as_ref();
    assert!(
        SPINNER_FRAMES.iter().any(|f| first.starts_with(*f)),
        "first span should be a spinner frame, got {first:?}"
    );
}

#[test]
fn spinner_frame_cycles_with_tick() {
    // Frame advances every SPINNER_TICKS_PER_FRAME ticks.
    for (tick, expected_idx) in [(0, 0), (2, 0), (3, 1), (5, 1), (35, 11), (36, 0), (47, 3)] {
        let bar = streaming_bar(tick);
        let line = bar.render_left_section();
        let first = line.spans[0].content.as_ref();
        assert!(
            first.starts_with(SPINNER_FRAMES[expected_idx]),
            "tick {tick}: expected frame {expected_idx}, got {first:?}"
        );
    }
}

#[test]
fn terminal_states_show_static_glyph_not_spinner() {
    for state in [InfoBarState::Completed, InfoBarState::Cancelled] {
        let mut bar = InfoBar::new();
        bar.state = state;
        bar.tick_frame = 3;
        let line = bar.render_left_section();
        let first = line.spans[0].content.as_ref();
        assert!(
            !SPINNER_FRAMES.iter().any(|f| first.starts_with(*f)),
            "{state:?} should not show a spinner frame, got {first:?}"
        );
    }
}

#[test]
fn spinner_frames_are_single_column() {
    use unicode_width::UnicodeWidthChar;
    for frame in SPINNER_FRAMES {
        assert_eq!(
            UnicodeWidthChar::width(frame),
            Some(1),
            "spinner frame {frame:?} must be width 1 to keep the bar aligned"
        );
    }
}

#[test]
fn idle_without_activity_renders_nothing() {
    let bar = InfoBar::new();
    assert!(bar.render_left_section().spans.is_empty());
}
