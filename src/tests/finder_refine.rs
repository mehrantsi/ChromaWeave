use super::*;
use image::Rgb;

fn fixture(pitch: f64, angle: f64) -> (RgbImage, [[f64; 2]; 4], [f64; 2]) {
    let center = [47.2, 45.8];
    let (sin, cos) = angle.sin_cos();
    let mut image = RgbImage::new(96, 96);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let mut value = 0.0;
        for sy in 0..8 {
            for sx in 0..8 {
                let dx = f64::from(x) + (f64::from(sx) + 0.5) / 8.0 - 0.5 - center[0];
                let dy = f64::from(y) + (f64::from(sy) + 0.5) / 8.0 - 0.5 - center[1];
                let u = (cos * dx + sin * dy) / pitch;
                let v = (-sin * dx + cos * dy) / pitch;
                let radius = u.abs().max(v.abs());
                let black = radius < 1.5 || (2.5..3.5).contains(&radius);
                value += if black { 28.0 } else { 225.0 };
            }
        }
        let luminance = (value / 64.0 + f64::from(x) * 0.15 - f64::from(y) * 0.11).round() as u8;
        *pixel = Rgb([luminance; 3]);
    }
    let rough_quad = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]].map(|[u, v]| {
        let radius = 3.5 * pitch - 0.5;
        [
            center[0] + radius * (u * cos - v * sin) + pitch * 0.6,
            center[1] + radius * (u * sin + v * cos) - pitch * 0.4,
        ]
    });
    (image::imageops::blur(&image, 0.55), rough_quad, center)
}

#[test]
fn registration_recovers_subpixel_translation_rotation_and_scale() {
    for pitch in [1.8, 2.7, 4.2] {
        for angle in [0.0, 0.37, 1.22] {
            let (image, quad, expected) = fixture(pitch, angle);
            let center = refine_center(&image, quad).expect("valid finder");
            let error = (center[0] - expected[0]).hypot(center[1] - expected[1]);
            assert!(error < 0.15, "pitch {pitch}, angle {angle}, error {error}");
        }
    }
}

#[test]
fn coarse_hull_bias_can_exceed_one_module_inside_the_finder_core() {
    let (image, mut quad, expected) = fixture(1.8, 0.37);
    for point in &mut quad {
        point[0] += 0.9;
    }
    let center = refine_center(&image, quad).expect("center remains in the black core");
    assert!((center[0] - expected[0]).hypot(center[1] - expected[1]) < 0.15);
}

#[test]
fn chooses_converged_cost_when_start_cost_prefers_a_wrong_basin() {
    let image = image::load_from_memory(include_bytes!("../../tests/fixtures/finder-basin.png"))
        .unwrap()
        .to_rgb8();
    let quad = [[13.0, 16.0], [31.0, 12.0], [36.0, 33.0], [16.0, 35.0]];
    let expected = [24.16740444151654, 24.3962588555578];
    let center = refine_center(&image, quad).expect("valid finder");
    assert!((center[0] - expected[0]).hypot(center[1] - expected[1]) < 0.08);
}

#[test]
fn uniform_or_degenerate_input_has_no_registration_evidence() {
    let image = RgbImage::from_pixel(32, 32, Rgb([128; 3]));
    assert!(refine_center(&image, [[5., 5.], [25., 5.], [25., 25.], [5., 25.]]).is_none());
    assert!(refine_center(&image, [[5., 5.]; 4]).is_none());
}
