use super::{ChannelConfig, Medium, simulate};
use image::{Rgb, RgbImage};

fn sample_image() -> RgbImage {
    RgbImage::from_fn(40, 32, |x, y| {
        Rgb([
            ((x * 7 + y * 3) % 256) as u8,
            ((x * 2 + y * 11) % 256) as u8,
            ((x * 13 + y * 5) % 256) as u8,
        ])
    })
}

fn config(medium: Medium, severity: f32, seed: u64) -> ChannelConfig {
    ChannelConfig {
        medium,
        severity,
        seed,
    }
}

#[test]
fn zero_severity_returns_the_exact_image() {
    let image = sample_image();
    let output = simulate(&image, &config(Medium::Print, 0.0, 17)).unwrap();
    assert_eq!(output, image);
}

#[test]
fn the_seed_makes_simulation_repeatable_and_selects_a_distinct_realization() {
    let image = sample_image();
    let first = simulate(&image, &config(Medium::Screen, 0.7, 17)).unwrap();
    let repeated = simulate(&image, &config(Medium::Screen, 0.7, 17)).unwrap();
    let different_seed = simulate(&image, &config(Medium::Screen, 0.7, 18)).unwrap();
    assert_eq!(first, repeated);
    assert_ne!(first, different_seed);
}

#[test]
fn severity_outside_the_supported_range_is_rejected() {
    let image = sample_image();
    assert!(simulate(&image, &config(Medium::Screen, -0.01, 1)).is_err());
    assert!(simulate(&image, &config(Medium::Print, 1.01, 1)).is_err());
    assert!(simulate(&image, &config(Medium::Screen, f32::NAN, 1)).is_err());
}

#[test]
fn screen_mode_changes_pixels() {
    let image = sample_image();
    let output = simulate(&image, &config(Medium::Screen, 0.6, 42)).unwrap();
    assert_ne!(output, image);
}

#[test]
fn print_mode_changes_pixels_with_halftone_coverage() {
    let image = sample_image();
    let output = simulate(&image, &config(Medium::Print, 0.8, 42)).unwrap();
    assert_ne!(output, image);
}

#[test]
fn print_halftoning_creates_dots_and_ink_expansion_darkens_them() {
    let gray = RgbImage::from_pixel(16, 16, Rgb([128, 128, 128]));
    let dots = super::halftone_cmy(&gray);
    let expanded = super::expand_ink(&dots);
    let first = dots.get_pixel(4, 4);
    assert!(dots.pixels().any(|pixel| pixel != first));
    assert!(
        expanded
            .pixels()
            .zip(dots.pixels())
            .any(|(expanded, dots)| (0..3).any(|channel| expanded[channel] < dots[channel]))
    );

    let printed = super::print_medium(&gray, 0.8);
    let first = printed.get_pixel(4, 4);
    assert!(printed.pixels().any(|pixel| pixel != first));
}
