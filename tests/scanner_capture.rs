use chromaweave::{
    capture::{CaptureConfig, Homography, bilinear, simulate_capture},
    channel::{ChannelConfig, Medium, simulate},
    scanner,
};
use image::{Rgb, RgbImage};

fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 131 + 47) as u8).collect()
}

#[test]
fn discovers_grid_and_cell_size_without_hints() {
    for (len, cell) in [(0, 4), (211, 8), (1024, 16), (4096, 6)] {
        let data = payload(len);
        let encoded = scanner::encode(&data, cell).unwrap();
        assert_eq!(encoded.image.width(), encoded.layout.image_side_pixels);
        let expected_density = data.len() as f64 * 8.0 / f64::from(encoded.image.width()).powi(2);
        assert_eq!(encoded.layout.net_bits_per_pixel, expected_density);
        let decoded = scanner::decode(&encoded.image).unwrap();
        assert_eq!(decoded.payload, data);
        assert_eq!(decoded.layout.grid_cells, encoded.layout.grid_cells);
    }
}

#[test]
fn localizes_rotated_projective_captures_in_clutter() {
    let data = payload(1024);
    let encoded = scanner::encode(&data, 8).unwrap();
    for (rotation, perspective, seed) in [
        (17.0, 0.12, 1),
        (91.0, 0.22, 41),
        (183.0, 0.15, 731),
        (274.0, 0.2, 19),
        (43.0, 0.18, 71),
    ] {
        let captured = simulate_capture(
            &encoded.image,
            &CaptureConfig {
                rotation_degrees: rotation,
                perspective,
                frame_side: 900,
                code_fraction: 0.75,
                seed,
                clutter: true,
            },
        )
        .unwrap();
        let decoded = scanner::decode(&captured.image)
            .unwrap_or_else(|error| panic!("rotation {rotation}, seed {seed}: {error}"));
        assert_eq!(decoded.payload, data);
    }
}

#[test]
fn scans_independently_specified_non_square_projective_image() {
    let data = payload(500);
    let encoded = scanner::encode(&data, 10).unwrap();
    let side = f64::from(encoded.image.width());
    let mapping = Homography::from_quad(
        [
            [-0.5, -0.5],
            [side - 0.5, -0.5],
            [side - 0.5, side - 0.5],
            [-0.5, side - 0.5],
        ],
        [[109.0, 87.0], [650.0, 41.0], [719.0, 587.0], [62.0, 549.0]],
    )
    .unwrap()
    .inverse()
    .unwrap();
    let mut capture = RgbImage::from_pixel(820, 640, Rgb([174, 161, 145]));
    for (x, y, pixel) in capture.enumerate_pixels_mut() {
        let [sx, sy] = mapping.transform([f64::from(x), f64::from(y)]).unwrap();
        if sx >= 0.0 && sy >= 0.0 && sx < side && sy < side {
            *pixel = bilinear(&encoded.image, sx, sy);
        }
    }
    assert_eq!(scanner::decode(&capture).unwrap().payload, data);
}

#[test]
fn scans_mild_print_and_screen_channels_after_localization() {
    let data = payload(700);
    let encoded = scanner::encode(&data, 12).unwrap();
    for medium in [Medium::Print, Medium::Screen] {
        let observed = simulate(
            &encoded.image,
            &ChannelConfig {
                medium,
                severity: 0.35,
                seed: 41,
            },
        )
        .unwrap();
        let capture = simulate_capture(
            &observed,
            &CaptureConfig {
                frame_side: 1000,
                code_fraction: 0.8,
                ..CaptureConfig::default()
            },
        )
        .unwrap();
        assert_eq!(scanner::decode(&capture.image).unwrap().payload, data);
    }
}

#[test]
fn rejects_images_without_a_complete_frame() {
    assert!(scanner::decode(&RgbImage::from_pixel(300, 220, Rgb([230, 230, 230]))).is_err());
    let mut encoded = scanner::encode(b"not a valid frame after erasing a finder", 8).unwrap();
    for y in 0..80 {
        for x in 0..80 {
            encoded.image.put_pixel(x, y, Rgb([255; 3]));
        }
    }
    assert!(scanner::decode(&encoded.image).is_err());
}

#[test]
fn corrects_one_damaged_bit_in_each_metadata_copy() {
    let data = payload(500);
    let cell = 12;
    let mut encoded = scanner::encode(&data, cell).unwrap();
    let side = encoded.image.width();
    let center = 5.5 * f64::from(cell);
    let span = f64::from(side) - 2.0 * center;
    for x in 0..side {
        let t = ((f64::from(x) + 0.5 - center) / span - 0.15) / 0.7;
        if !(0.0..1.0).contains(&t) {
            continue;
        }
        let bit = (t * 32.0) as u32;
        for y in 4 * cell..7 * cell {
            if bit == 5 {
                let value = 255 - encoded.image.get_pixel(x, y)[0];
                encoded.image.put_pixel(x, y, Rgb([value; 3]));
            }
            if bit == 23 {
                let target = side - 1 - y;
                let value = 255 - encoded.image.get_pixel(x, target)[0];
                encoded.image.put_pixel(x, target, Rgb([value; 3]));
            }
        }
    }
    assert_eq!(scanner::decode(&encoded.image).unwrap().payload, data);
}

#[test]
fn framed_capacity_accounts_for_finders_before_allocating() {
    for cell in [4, 8, 16, 64] {
        let framed = scanner::maximum_payload_bytes(cell).unwrap();
        assert_eq!(
            framed,
            chromaweave::codec::maximum_payload_bytes(cell).unwrap()
        );
        assert!(scanner::encode(&vec![0; framed + 1], cell).is_err());
    }
    assert!(scanner::maximum_payload_bytes(0).is_err());
    assert!(scanner::maximum_payload_bytes(5).is_err());
}

#[test]
#[ignore = "manual fixed-image latency measurement"]
fn fixed_capture_decode_latency() {
    let data = payload(1024);
    let encoded = scanner::encode(&data, 8).unwrap();
    let captured = simulate_capture(
        &encoded.image,
        &CaptureConfig {
            frame_side: 1024,
            code_fraction: 0.5,
            rotation_degrees: 17.0,
            perspective: 0.08,
            seed: 41,
            clutter: true,
        },
    )
    .unwrap();
    for _ in 0..10 {
        assert_eq!(scanner::decode(&captured.image).unwrap().payload, data);
    }
    let mut elapsed = Vec::new();
    for _ in 0..100 {
        let start = std::time::Instant::now();
        let decoded = scanner::decode(std::hint::black_box(&captured.image)).unwrap();
        elapsed.push(start.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(decoded.payload, data);
    }
    elapsed.sort_by(f64::total_cmp);
    eprintln!(
        "fixed 1024-byte/512px/1024px frame: p50={:.3}ms p95={:.3}ms",
        elapsed[50], elapsed[95]
    );
}
