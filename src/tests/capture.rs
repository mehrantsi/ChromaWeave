use super::*;

#[test]
fn chroma420_preserves_luminance_detail_and_constant_colors() {
    let gray = RgbImage::from_fn(7, 5, |x, y| Rgb([((x * 43 + y * 19) % 256) as u8; 3]));
    assert_eq!(subsample_chroma420(&gray).unwrap(), gray);
    for (width, height) in [(1, 1), (1, 7), (7, 1), (7, 5), (8, 6)] {
        for color in [[0, 0, 0], [255, 255, 255], [23, 117, 231], [254, 17, 91]] {
            let input = RgbImage::from_pixel(width, height, Rgb(color));
            assert_eq!(subsample_chroma420(&input).unwrap(), input);
        }
    }
}

#[test]
fn chroma420_removes_pixel_frequency_color_checkerboards() {
    let image = RgbImage::from_fn(8, 8, |x, y| {
        Rgb(if (x + y) % 2 == 0 {
            [187, 98, 128]
        } else {
            [69, 158, 128]
        })
    });
    let observed = subsample_chroma420(&image).unwrap();
    assert!(observed.pixels().all(|pixel| *pixel == Rgb([128; 3])));
    assert!(subsample_chroma420(&RgbImage::new(0, 1)).is_err());
}

#[test]
fn maps_quad_and_inverse() {
    let source = [[0.0, 0.0], [100.0, 0.0], [100.0, 80.0], [0.0, 80.0]];
    let destination = [[12.3, 7.8], [115.0, 20.0], [92.0, 103.0], [2.0, 70.0]];
    let mapping = Homography::from_quad(source, destination).unwrap();
    let inverse = mapping.inverse().unwrap();
    for (source, destination) in source.into_iter().zip(destination) {
        let mapped = mapping.transform(source).unwrap();
        let restored = inverse.transform(mapped).unwrap();
        for axis in 0..2 {
            assert!((mapped[axis] - destination[axis]).abs() < 1e-8);
            assert!((restored[axis] - source[axis]).abs() < 1e-8);
        }
    }
}

#[test]
fn deterministic_capture_and_bounds() {
    let source = RgbImage::from_pixel(32, 32, Rgb([255, 255, 255]));
    for rotation in [0.0, 17.5, 93.0, 181.0, -360.0] {
        let config = CaptureConfig {
            frame_side: 96,
            rotation_degrees: rotation,
            perspective: 0.3,
            code_fraction: 0.9,
            ..CaptureConfig::default()
        };
        let first = simulate_capture(&source, &config).unwrap();
        let second = simulate_capture(&source, &config).unwrap();
        assert_eq!(first.image, second.image);
        assert_eq!(first.corners, second.corners);
        assert_eq!(first.image.dimensions(), (96, 96));
        for point in first.corners {
            assert!(
                point
                    .iter()
                    .all(|v| v.is_finite() && *v >= -0.5 && *v <= 95.5)
            );
        }
    }
}

#[test]
fn affine_capture_preserves_source_and_white_border() {
    let source = RgbImage::from_fn(20, 20, |x, y| {
        if (5..15).contains(&x) && (5..15).contains(&y) {
            Rgb([20, 50, 100])
        } else {
            Rgb([255, 255, 255])
        }
    });
    let config = CaptureConfig {
        frame_side: 100,
        rotation_degrees: 0.0,
        perspective: 0.0,
        code_fraction: 0.8,
        clutter: false,
        ..CaptureConfig::default()
    };
    let capture = simulate_capture(&source, &config).unwrap();
    assert!((capture.footprint_area_pixels() - 6400.0).abs() < 1e-8);
    for (point, expected) in [
        ([9.5, 9.5], Rgb([20, 50, 100])),
        ([1.5, 9.5], Rgb([255, 255, 255])),
    ] {
        let [x, y] = capture.source_to_frame.transform(point).unwrap();
        assert_eq!(
            *capture.image.get_pixel(x.round() as u32, y.round() as u32),
            expected
        );
    }
    assert!(Homography::from_quad([[0.0, 0.0]; 4], [[1.0, 1.0]; 4]).is_err());
}

#[test]
fn illumination_is_spatial_and_identity_at_zero() {
    let source = RgbImage::from_pixel(32, 24, Rgb([128; 3]));
    assert_eq!(illuminate(&source, 0.0).unwrap(), source);
    let lit = illuminate(&source, 0.7).unwrap();
    assert_eq!(lit.dimensions(), source.dimensions());
    assert_eq!(lit, illuminate(&source, 0.7).unwrap());
    assert!(lit.get_pixel(0, 0)[1] < lit.get_pixel(31, 23)[1]);
    assert!(lit.get_pixel(0, 0)[0] < lit.get_pixel(0, 0)[2]);
    assert!(lit.get_pixel(31, 23)[0] > lit.get_pixel(31, 23)[2]);
    assert!(illuminate(&source, f64::NAN).is_err());
    assert!(illuminate(&source, -0.1).is_err());
    assert!(illuminate(&source, 1.01).is_err());
    assert!(illuminate(&RgbImage::new(0, 0), 0.0).is_err());
}

#[test]
fn jpeg_roundtrip_preserves_dimensions_and_changes_fine_color_detail() {
    let source = RgbImage::from_fn(32, 24, |x, y| {
        if (x + y) % 2 == 0 {
            Rgb([225, 20, 30])
        } else {
            Rgb([10, 70, 240])
        }
    });
    let compressed = jpeg_roundtrip(&source, 35).unwrap();
    assert_eq!(compressed.dimensions(), source.dimensions());
    assert_ne!(compressed, source);
    assert_eq!(compressed, jpeg_roundtrip(&source, 35).unwrap());
    assert!(jpeg_roundtrip(&source, 1).is_ok());
    assert!(jpeg_roundtrip(&source, 100).is_ok());
    assert!(jpeg_roundtrip(&source, 0).is_err());
    assert!(jpeg_roundtrip(&source, 101).is_err());
    assert!(jpeg_roundtrip(&RgbImage::new(0, 0), 80).is_err());
}

#[test]
fn rejects_invalid_configuration() {
    let source = RgbImage::new(10, 10);
    let config = CaptureConfig {
        perspective: f64::NAN,
        ..CaptureConfig::default()
    };
    assert!(simulate_capture(&source, &config).is_err());
    assert!(simulate_capture(&RgbImage::new(0, 0), &CaptureConfig::default()).is_err());
    assert!(simulate_capture(&RgbImage::new(8193, 1), &CaptureConfig::default()).is_err());
    let config = CaptureConfig {
        frame_side: 8193,
        ..CaptureConfig::default()
    };
    assert!(simulate_capture(&source, &config).is_err());
}
