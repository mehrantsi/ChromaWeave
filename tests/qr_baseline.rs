use chromaweave::comparison::{
    QrErrorCorrection, decode_qr, decode_qr_zxing, encode_qr, qr_byte_capacity,
    qr_minimum_side_pixels,
};
use image::{Rgb, RgbImage, imageops};

#[test]
fn byte_mode_preserves_every_byte_at_both_error_levels() {
    let payload: Vec<u8> = (0..=255).collect();
    for ec in [QrErrorCorrection::M, QrErrorCorrection::H] {
        let side = qr_minimum_side_pixels(payload.len(), ec).unwrap() * 6;
        let encoded = encode_qr(&payload, side, ec).unwrap();
        assert_eq!(decode_qr(&encoded.image).unwrap(), payload);
        assert_eq!(decode_qr_zxing(&encoded.image).unwrap(), payload);
        assert_eq!(encoded.metadata.occupied_side_pixels, side);
        assert_eq!(encoded.metadata.module_pitch_pixels, 6);
    }
}

#[test]
fn numeric_bytes_use_byte_capacity_and_the_smallest_version() {
    let payload = b"123456789012345";
    let encoded = encode_qr(payload, 256, QrErrorCorrection::M).unwrap();
    assert_eq!(qr_byte_capacity(1, QrErrorCorrection::M).unwrap(), 14);
    assert_eq!(encoded.metadata.version, 2);
    assert_eq!(encoded.metadata.modules, 25);
    assert_eq!(decode_qr(&encoded.image).unwrap(), payload);
}

#[test]
fn automatic_scanner_finds_a_rotated_code_inside_a_larger_capture() {
    let payload = b"automatic localization without a supplied grid";
    let encoded = encode_qr(payload, 300, QrErrorCorrection::H).unwrap();
    let rotated = imageops::rotate90(&encoded.image);
    let mut capture = RgbImage::from_pixel(700, 500, Rgb([245; 3]));
    imageops::overlay(&mut capture, &rotated, 77, 131);
    assert_eq!(decode_qr(&capture).unwrap(), payload);
}

#[test]
fn quiet_zone_and_extra_canvas_padding_are_reported_separately() {
    let encoded = encode_qr(b"binary", 303, QrErrorCorrection::M).unwrap();
    assert_eq!(encoded.image.dimensions(), (303, 303));
    assert_eq!(encoded.metadata.modules, 21);
    assert_eq!(encoded.metadata.quiet_zone_modules, 4);
    assert_eq!(encoded.metadata.module_pitch_pixels, 10);
    assert_eq!(encoded.metadata.occupied_side_pixels, 290);
    for y in 0..46 {
        for x in 0..303 {
            assert_eq!(encoded.image.get_pixel(x, y), &Rgb([255; 3]));
        }
    }
}

#[test]
fn byte_capacity_boundaries_and_invalid_inputs_are_checked() {
    assert_eq!(qr_byte_capacity(1, QrErrorCorrection::H).unwrap(), 7);
    assert_eq!(qr_byte_capacity(40, QrErrorCorrection::M).unwrap(), 2331);
    assert_eq!(qr_byte_capacity(40, QrErrorCorrection::H).unwrap(), 1273);
    assert!(qr_byte_capacity(0, QrErrorCorrection::M).is_err());
    assert!(qr_byte_capacity(41, QrErrorCorrection::H).is_err());
    assert!(encode_qr(b"x", 28, QrErrorCorrection::M).is_err());
    assert!(encode_qr(&vec![0; 2332], 1000, QrErrorCorrection::M).is_err());
    assert!(decode_qr(&RgbImage::from_pixel(128, 128, Rgb([255; 3]))).is_err());
}

#[test]
fn zxing_localizes_perspective_captures_that_challenge_quirc() {
    use chromaweave::capture::{CaptureConfig, simulate_capture};
    use rand::{Rng, RngCore, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    for ec in [QrErrorCorrection::M, QrErrorCorrection::H] {
        for seed in 1..=5 {
            let mut payload = vec![0; 512];
            ChaCha8Rng::seed_from_u64(seed).fill_bytes(&mut payload);
            let source_side = qr_minimum_side_pixels(payload.len(), ec).unwrap() * 8;
            let encoded = encode_qr(&payload, source_side, ec).unwrap();
            assert_eq!(decode_qr(&encoded.image).unwrap(), payload);
            assert_eq!(decode_qr_zxing(&encoded.image).unwrap(), payload);
            let raster = imageops::resize(&encoded.image, 640, 640, imageops::FilterType::Triangle);
            let rotation = ChaCha8Rng::seed_from_u64(seed ^ 0xa63bc742).random_range(-180.0..180.0);
            let capture = simulate_capture(
                &raster,
                &CaptureConfig {
                    rotation_degrees: rotation,
                    perspective: 0.08,
                    frame_side: 1024,
                    code_fraction: 640.0 / 1024.0,
                    seed,
                    clutter: true,
                },
            )
            .unwrap();
            let quirc = decode_qr(&capture.image);
            let zxing = decode_qr_zxing(&capture.image);
            eprintln!(
                "ec={ec:?} seed={seed} rotation={rotation:.1}: Quirc={}, ZXing={}",
                quirc
                    .as_ref()
                    .map(|bytes| bytes == &payload)
                    .unwrap_or(false),
                zxing
                    .as_ref()
                    .map(|bytes| bytes == &payload)
                    .unwrap_or(false)
            );
            if let Err(error) = quirc {
                eprintln!("Quirc rejection: {error}");
            }
            assert_eq!(zxing.unwrap(), payload);
        }
    }
}
