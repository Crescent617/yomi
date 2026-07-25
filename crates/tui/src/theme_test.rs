use super::*;

#[test]
fn test_hex_color() {
    assert_eq!(hex("#FF5733"), Color::Rgb(255, 87, 51));
    assert_eq!(hex("#000000"), Color::Rgb(0, 0, 0));
    assert_eq!(hex("#FFFFFF"), Color::Rgb(255, 255, 255));
}

#[test]
fn test_styles_use_current_theme() {
    set_theme(ThemeConfig::default());
    let style = Styles::user_header();
    assert_eq!(style.fg, Some(colors::accent_user()));
}

#[test]
fn lerp_color_hits_endpoints() {
    let base = Color::Rgb(0, 0, 0);
    let peak = Color::Rgb(200, 100, 50);
    assert_eq!(lerp_color(base, peak, 0.0), base);
    assert_eq!(lerp_color(base, peak, 1.0), peak);
    assert_eq!(lerp_color(base, peak, 0.5), Color::Rgb(100, 50, 25));
}

#[test]
fn lerp_color_tolerates_non_rgb_and_out_of_range_t() {
    let peak = Color::Rgb(255, 255, 255);
    // Color::Reset falls back to the default secondary tone instead of panicking
    assert_eq!(lerp_color(Color::Reset, peak, 1.0), peak);
    assert_eq!(lerp_color(Color::Reset, peak, 2.0), peak); // clamped
}

#[test]
fn shimmer_spans_emit_one_span_per_char() {
    let spans = shimmer_spans(
        "Running...",
        0.5,
        Color::Rgb(10, 10, 10),
        Color::Rgb(200, 200, 200),
    );
    assert_eq!(spans.len(), "Running...".chars().count());
    assert_eq!(
        spans.iter().map(|s| s.content.as_ref()).collect::<String>(),
        "Running..."
    );
}

#[test]
fn shimmer_wave_centers_peak_at_phase_midpoint() {
    let base = Color::Rgb(10, 10, 10);
    let peak = Color::Rgb(200, 200, 200);
    let text = "01234567";
    let spans = shimmer_spans(text, 0.5, base, peak);
    // phase 0.5 puts the wave center at 0.5 * (8 + 5) - 2.5 = 4.0 → char 4
    assert_eq!(spans[4].style.fg, Some(peak));
    // far from the center the wave decays to the base color
    assert_eq!(spans[0].style.fg, Some(base));
}

#[test]
fn shimmer_handles_empty_text() {
    assert!(shimmer_spans("", 0.3, Color::Rgb(0, 0, 0), Color::Rgb(1, 1, 1)).is_empty());
}
