use super::{compute_panel_frame, PanelPlacement, MAX_PANEL_HEIGHT, MIN_PANEL_HEIGHT, PANEL_WIDTH};
use tauri::{LogicalPosition, LogicalSize};

const EPSILON: f64 = 1e-9;

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < EPSILON,
        "expected {actual} ≈ {expected}"
    );
}

fn sprite(size: f64) -> LogicalSize<f64> {
    LogicalSize::new(size, size)
}

fn work_area(x: f64, y: f64, w: f64, h: f64) -> (LogicalPosition<f64>, LogicalSize<f64>) {
    (LogicalPosition::new(x, y), LogicalSize::new(w, h))
}

#[test]
fn panel_opens_above_with_sprite_anchor_fixed() {
    let sprite_size = sprite(192.0);
    let sprite_pos = LogicalPosition::new(1000.0, 800.0);
    let frame = compute_panel_frame(
        sprite_size,
        sprite_pos,
        Some(work_area(0.0, 0.0, 2560.0, 1440.0)),
        MAX_PANEL_HEIGHT,
    );

    assert_eq!(frame.placement, PanelPlacement::Above);
    assert_close(frame.size.width, PANEL_WIDTH);
    assert_close(frame.size.height, MAX_PANEL_HEIGHT + 192.0);
    // Sprite keeps its center x and bottom y: the pet does not jump.
    assert_close(frame.position.x + frame.size.width / 2.0, 1000.0 + 96.0);
    assert_close(frame.position.y + frame.size.height, 800.0 + 192.0);
}

#[test]
fn short_panel_keeps_the_same_anchor() {
    // 内容少时面板收缩，精灵锚点不变（高度自适应的核心不变量）。
    let sprite_size = sprite(192.0);
    let sprite_pos = LogicalPosition::new(1000.0, 800.0);
    let frame = compute_panel_frame(
        sprite_size,
        sprite_pos,
        Some(work_area(0.0, 0.0, 2560.0, 1440.0)),
        MIN_PANEL_HEIGHT,
    );

    assert_eq!(frame.placement, PanelPlacement::Above);
    assert_close(frame.size.height, MIN_PANEL_HEIGHT + 192.0);
    assert_close(frame.position.x + frame.size.width / 2.0, 1000.0 + 96.0);
    assert_close(frame.position.y + frame.size.height, 800.0 + 192.0);
}

#[test]
fn panel_flips_below_when_no_room_above() {
    let sprite_size = sprite(192.0);
    let sprite_pos = LogicalPosition::new(1000.0, 40.0);
    let frame = compute_panel_frame(
        sprite_size,
        sprite_pos,
        Some(work_area(0.0, 0.0, 2560.0, 1440.0)),
        MAX_PANEL_HEIGHT,
    );

    assert_eq!(frame.placement, PanelPlacement::Below);
    // Sprite pinned to the frame's top edge, position unchanged.
    assert_close(frame.position.y, 40.0);
    assert_close(frame.position.x + frame.size.width / 2.0, 1000.0 + 96.0);
}

#[test]
fn panel_clamps_horizontally_into_work_area_with_margin() {
    let sprite_size = sprite(192.0);
    // Sprite hugging the right screen edge.
    let sprite_pos = LogicalPosition::new(2468.0, 800.0);
    let frame = compute_panel_frame(
        sprite_size,
        sprite_pos,
        Some(work_area(0.0, 0.0, 2560.0, 1440.0)),
        MAX_PANEL_HEIGHT,
    );

    // 8px screen margin: the frame stops short of the physical edge.
    assert_close(frame.position.x + frame.size.width, 2560.0 - 8.0);
}

#[test]
fn panel_widens_to_sprite_when_sprite_is_wider_than_panel() {
    let sprite_size = sprite(480.0);
    let sprite_pos = LogicalPosition::new(1000.0, 900.0);
    let frame = compute_panel_frame(
        sprite_size,
        sprite_pos,
        Some(work_area(0.0, 0.0, 2560.0, 1440.0)),
        MAX_PANEL_HEIGHT,
    );

    assert_close(frame.size.width, 480.0);
    assert_close(frame.size.height, MAX_PANEL_HEIGHT + 480.0);
}

#[test]
fn panel_without_monitor_stays_above_unclamped() {
    let sprite_size = sprite(192.0);
    let sprite_pos = LogicalPosition::new(50.0, 800.0);
    let frame = compute_panel_frame(sprite_size, sprite_pos, None, MAX_PANEL_HEIGHT);

    assert_eq!(frame.placement, PanelPlacement::Above);
    assert_close(frame.position.y, 800.0 + 192.0 - (MAX_PANEL_HEIGHT + 192.0));
}

#[test]
fn below_flip_still_clamps_into_work_area() {
    // 贴顶精灵：上方翻不开翻到下面，且整体夹在工作区内。
    let sprite_size = sprite(146.0);
    let sprite_pos = LogicalPosition::new(770.0, 40.0);
    let frame = compute_panel_frame(
        sprite_size,
        sprite_pos,
        Some(work_area(0.0, 30.0, 1920.0, 986.0)),
        MAX_PANEL_HEIGHT,
    );

    assert_eq!(frame.placement, PanelPlacement::Below);
    assert_close(frame.position.y, 40.0);
}

#[test]
fn reframe_keeps_placement_and_anchor_on_height_change() {
    use super::reframe_open;

    let sprite_size = sprite(146.0);
    let sprite_pos = LogicalPosition::new(770.0, 303.0);
    let wa = Some(work_area(0.0, 30.0, 1920.0, 986.0));

    let taller = reframe_open(
        sprite_size,
        sprite_pos,
        PanelPlacement::Below,
        wa,
        MAX_PANEL_HEIGHT,
    );
    let shorter = reframe_open(
        sprite_size,
        sprite_pos,
        PanelPlacement::Below,
        wa,
        MIN_PANEL_HEIGHT,
    );

    // below：精灵钉在框架顶；更高的框架被底边夹住 1px，矮的保持原位。
    assert_eq!(taller.placement, PanelPlacement::Below);
    assert_close(taller.position.y, 302.0);
    assert_close(shorter.position.y, 303.0);
    assert_close(
        taller.size.height - shorter.size.height,
        MAX_PANEL_HEIGHT - MIN_PANEL_HEIGHT,
    );

    // above：精灵钉在框架底（bottom 对齐），高度变化只改框架顶。
    let above = reframe_open(
        sprite_size,
        sprite_pos,
        PanelPlacement::Above,
        wa,
        MIN_PANEL_HEIGHT,
    );
    assert_close(above.position.y + above.size.height, 303.0 + 146.0);
}
