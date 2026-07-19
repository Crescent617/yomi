use super::{normalize_pet_scale, pet_window_size, MAX_PET_SCALE, MIN_PET_SCALE};

#[test]
fn pet_scale_accepts_bounds_and_midpoint() {
    assert_eq!(normalize_pet_scale(MIN_PET_SCALE), Some(MIN_PET_SCALE));
    assert_eq!(normalize_pet_scale(1.0), Some(1.0));
    assert_eq!(normalize_pet_scale(MAX_PET_SCALE), Some(MAX_PET_SCALE));
}

#[test]
fn pet_scale_rejects_out_of_range_and_non_finite() {
    assert_eq!(normalize_pet_scale(MIN_PET_SCALE - 0.01), None);
    assert_eq!(normalize_pet_scale(MAX_PET_SCALE + 0.01), None);
    assert_eq!(normalize_pet_scale(f64::NAN), None);
    assert_eq!(normalize_pet_scale(f64::INFINITY), None);
    assert_eq!(normalize_pet_scale(f64::NEG_INFINITY), None);
}

#[test]
fn pet_window_size_scales_the_sprite_cell() {
    let unit = pet_window_size(1.0);
    assert_eq!((unit.width, unit.height), (192.0, 208.0));
    let doubled = pet_window_size(2.0);
    assert_eq!((doubled.width, doubled.height), (384.0, 416.0));
}
