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
