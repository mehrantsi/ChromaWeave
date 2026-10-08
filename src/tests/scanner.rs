use super::*;

#[test]
fn finder_cores_survive_many_larger_data_components() {
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    for distractors in [0_u32, 600, 1400] {
        let columns = (f64::from(distractors).sqrt().ceil() as u32).max(1);
        let side = columns * 10 + 60;
        let mut image = RgbImage::from_pixel(side, side, Rgb([255; 3]));
        let mut rng = ChaCha8Rng::seed_from_u64(83);
        for index in 0..distractors {
            let x = 30 + index % columns * 10;
            let y = 30 + index / columns * 10;
            let width = rng.random_range(7..=9);
            let height = rng.random_range(7..=9);
            for dy in 0..height {
                for dx in 0..width {
                    image.put_pixel(x + dx, y + dy, Rgb([0; 3]));
                }
            }
        }
        let origins = [
            [8, 8],
            [side - 22, 8],
            [side - 22, side - 22],
            [8, side - 22],
        ];
        for [x, y] in origins {
            for my in 0..7 {
                for mx in 0..7 {
                    if mx == 0
                        || mx == 6
                        || my == 0
                        || my == 6
                        || ((2..=4).contains(&mx) && (2..=4).contains(&my))
                    {
                        for dy in 0..2 {
                            for dx in 0..2 {
                                image.put_pixel(x + mx * 2 + dx, y + my * 2 + dy, Rgb([0; 3]));
                            }
                        }
                    }
                }
            }
        }
        let centers = finders(&image);
        assert_eq!(centers.len(), 4, "{distractors} distractors");
        for [x, y] in origins {
            assert!(centers.iter().any(|center| {
                (center[0] - (f64::from(x) + 6.5)).hypot(center[1] - (f64::from(y) + 6.5)) < 0.01
            }));
        }
    }
}

#[test]
fn finders_survive_capture_brightness_and_illumination_gradients() {
    let data: Vec<u8> = (0..1024).map(|i| (i * 131 + 47) as u8).collect();
    let encoded = encode(&data, 8).unwrap();
    let captured = crate::capture::simulate_capture(
        &encoded.image,
        &crate::capture::CaptureConfig {
            frame_side: 1024,
            code_fraction: 0.5,
            rotation_degrees: 17.0,
            perspective: 0.08,
            seed: 41,
            clutter: true,
        },
    )
    .unwrap();
    let low = FINDER_CENTER * 8.0 - 0.5;
    let high = f64::from(encoded.image.width()) - FINDER_CENTER * 8.0 - 0.5;
    let expected = [[low, low], [high, low], [high, high], [low, high]]
        .map(|point| captured.source_to_frame.transform(point).unwrap());
    for (gain, offset, gradient) in [
        (1.0, 0.0, 0.0),
        (0.45, 140.0, 20.0),
        (0.65, 80.0, 55.0),
        (0.3, 170.0, 10.0),
    ] {
        let mut image = captured.image.clone();
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            let illumination = offset + gradient * (f64::from(x + y) / 1023.0 - 1.0);
            for channel in &mut pixel.0 {
                *channel = (f64::from(*channel) * gain + illumination)
                    .clamp(0.0, 255.0)
                    .round() as u8;
            }
        }
        let centers = finders(&image);
        for point in expected {
            assert!(
                centers
                    .iter()
                    .any(|center| (center[0] - point[0]).hypot(center[1] - point[1]) < 1.0),
                "gain {gain}, offset {offset}, gradient {gradient}; expected {point:?}, found {centers:?}"
            );
        }
    }
}

#[test]
#[ignore = "manual external large-capture regression"]
fn recovers_large_external_captures() {
    let directory = std::env::var("CW_FINDER_DIAGNOSTIC_DIR").unwrap();
    let payload = std::fs::read(format!("{directory}/payload.bin")).unwrap();
    eprintln!(
        "retained component storage: {} bytes",
        std::mem::size_of::<Component>()
    );
    for name in ["screen-4096", "print-4096", "screen-6144", "print-6144"] {
        let image = image::open(format!("{directory}/{name}.png"))
            .unwrap()
            .to_rgb8();
        let centers = finders(&image);
        eprintln!("{name}: {} finder candidates {centers:?}", centers.len());
        let decoded = decode(&image).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(decoded.payload, payload, "{name}");
        eprintln!("{name}: exact recovery, {:?}", decoded.stats);
    }
}

#[test]
#[ignore = "manual external camera finder regression"]
fn camera_finder_regression() {
    let path = std::env::var("CW_CAMERA_FRAME").unwrap();
    let grid = std::env::var("CW_CAMERA_GRID")
        .unwrap()
        .parse::<u32>()
        .unwrap();
    let image = image::open(path).unwrap().to_rgb8();
    let start = std::time::Instant::now();
    let centers = finders(&image);
    eprintln!(
        "finder time {:?}; {} centers: {centers:?}",
        start.elapsed(),
        centers.len()
    );
    assert!(centers.len() >= 4);
    match decode(&image) {
        Ok(decoded) => {
            assert_eq!(decoded.layout.grid_cells, grid);
            eprintln!("decoded {} payload bytes", decoded.payload.len());
        }
        Err(error) => {
            let message = error.to_string();
            eprintln!("{error:#}");
            assert!(
                message.contains(&format!(": grid {grid}, profile ")),
                "{error:#}"
            );
            assert!(
                message.starts_with("insufficient resolution: grid ")
                    || message.starts_with("spatial equalizer failed: grid ")
                    || message.starts_with("payload decode failed: grid "),
                "{error:#}"
            );
        }
    }
}

#[test]
fn lattice_sampling_includes_sensor_pixel_support() {
    let image = RgbImage::from_fn(4, 3, |x, y| Rgb([(x + y * 4) as u8; 3]));
    assert_eq!(sample_lattice(&image, [-0.25, 0.0], false), Some([0.0; 3]));
    assert_eq!(sample_lattice(&image, [3.25, 2.25], false), Some([11.0; 3]));
    assert!(sample_lattice(&image, [-0.51, 0.0], false).is_none());
    assert!(sample_lattice(&image, [3.0, 2.51], false).is_none());
    assert_eq!(sample_lattice(&image, [-4.0, 2.0], true), Some([8.0; 3]));
    assert!(sample_lattice(&image, [f64::INFINITY, 0.0], true).is_none());
    for position in [
        [-0.25, 0.0],
        [3.25, 2.25],
        [1.2, 1.7],
        [-1.0, 0.0],
        [f64::NAN, 1.0],
    ] {
        for padding in [false, true] {
            for channel in 0..3 {
                assert_eq!(
                    sample_channel_lattice(&image, position, padding, channel),
                    sample_lattice(&image, position, padding).map(|v| v[channel])
                );
            }
        }
    }
}

#[test]
fn equalized_lattice_extends_padding_but_rejects_cropped_grid() {
    let data: Vec<u8> = (0..1024).map(|i| (i * 131 + 47) as u8).collect();
    for modulation in [codec::Modulation::Luma10, codec::Modulation::Luma10LowEcc] {
        let cell_pixels = 4;
        let encoded = encode_with_modulation(&data, cell_pixels, modulation).unwrap();
        let grid = encoded.layout.grid_cells;
        let metadata = FrameMetadata { grid, modulation };
        let crop = 2 * cell_pixels;
        let image = image::imageops::crop_imm(
            &encoded.image,
            crop,
            crop,
            grid * cell_pixels,
            grid * cell_pixels,
        )
        .to_image();
        let low = FINDER_CENTER * f64::from(cell_pixels) - 0.5 - f64::from(crop);
        let high = f64::from(encoded.image.width())
            - FINDER_CENTER * f64::from(cell_pixels)
            - 0.5
            - f64::from(crop);
        let transform = Homography::from_quad(
            [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            [[low, low], [high, low], [high, high], [low, high]],
        )
        .unwrap();
        let features = equalized_features(&image, &transform, metadata).unwrap();
        let decoded = codec::decode_from_feature_slice_with_modulation(
            grid,
            cell_pixels,
            modulation,
            &features,
        )
        .unwrap();
        assert_eq!(decoded.payload, data, "{modulation:?}");

        let clipped =
            image::imageops::crop_imm(&image, 0, 0, image.width(), image.height() - 2).to_image();
        let cell_x = grid / 2;
        let cell_y = grid - 1;
        assert!(!crate::integrated_layout::reserved(grid, cell_x, cell_y));
        let origin = 2.0 - FINDER_CENTER;
        let span = finder_span(grid);
        let position = transform
            .transform([
                (origin + f64::from(cell_x) + 0.75) / span,
                (origin + f64::from(cell_y) + 0.75) / span,
            ])
            .unwrap();
        assert!(sample_lattice(&clipped, position, false).is_none());
        assert_eq!(
            equalized_features(&clipped, &transform, metadata)
                .unwrap_err()
                .to_string(),
            "pilot lattice extends outside image"
        );
    }
}

#[test]
fn metadata_corrects_every_single_bit_and_rejects_every_double_bit() {
    for grid in [40, 51, 115, 255, 510] {
        let word = encode_metadata(grid);
        assert_eq!(decode_metadata(word), Some(grid));
        for first in 0..32 {
            assert_eq!(decode_metadata(word ^ (1 << first)), Some(grid));
            for second in first + 1..32 {
                assert_eq!(decode_metadata(word ^ (1 << first) ^ (1 << second)), None);
            }
        }
    }
}

#[test]
fn rejects_large_grid_metadata_in_a_small_frame_before_rectification() {
    let cell = 4;
    let mut encoded = encode(b"small frame", cell).unwrap();
    let side = encoded.image.width();
    let center = FINDER_CENTER * f64::from(cell);
    let span = f64::from(side) - 2.0 * center;
    let word = encode_metadata(510);
    for x in 0..side {
        let t = ((f64::from(x) + 0.5 - center) / span - 0.15) / 0.7;
        if !(0.0..1.0).contains(&t) {
            continue;
        }
        let value = if word & (1 << (31 - (t * 32.0) as u32)) == 0 {
            255
        } else {
            0
        };
        for y in 4 * cell..7 * cell {
            encoded.image.put_pixel(x, y, Rgb([value; 3]));
            encoded.image.put_pixel(x, side - 1 - y, Rgb([value; 3]));
        }
    }
    let low = center - 0.5;
    let high = low + span;
    let quad = [[low, low], [high, low], [high, high], [low, high]];
    let transform =
        Homography::from_quad([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]], quad).unwrap();
    let advertised = read_grid(&encoded.image, &transform, 0.0).unwrap();
    assert_eq!(advertised, 510);
    assert!(!grid_is_resolved(&quad, advertised));
    assert!(grid_is_resolved(&quad, encoded.layout.grid_cells));
    let error = format!("{:#}", decode(&encoded.image).err().unwrap());
    assert!(error.contains("insufficient resolution"), "{error}");
    assert!(error.contains("grid 510, profile Luma10"), "{error}");
    assert!(error.contains("px/cell shortest edge"), "{error}");
    assert!(error.contains("1.50 px/cell decoding minimum"), "{error}");
}

#[test]
fn payload_failure_keeps_ecc_cause_and_clean_frame_still_decodes() {
    let modulation = codec::Modulation::Luma10LowEcc;
    let payload: Vec<_> = (0..2048).map(|i| (i * 131 + 47) as u8).collect();
    let mut encoded = encode_with_modulation(&payload, 2, modulation).unwrap();
    assert_eq!(decode(&encoded.image).unwrap().payload, payload);
    let grid = encoded.layout.grid_cells;
    for y in 0..grid {
        for x in 0..grid {
            if crate::integrated_layout::reserved(grid, x, y) {
                continue;
            }
            for py in 0..2 {
                for px in 0..2 {
                    encoded
                        .image
                        .put_pixel((x + 2) * 2 + px, (y + 2) * 2 + py, Rgb([128; 3]));
                }
            }
        }
    }
    let attempt = scan(&encoded.image);
    assert!(attempt.code_detected);
    let error = format!("{:#}", attempt.result.err().unwrap());
    assert!(error.contains("payload decode failed"), "{error}");
    assert!(error.contains("Luma10LowEcc"), "{error}");
    assert!(error.contains("Reed-Solomon block"), "{error}");
    assert!(error.contains("px/cell shortest edge"), "{error}");
    let (attempt, observation) = scan_with_observation(&encoded.image);
    assert!(attempt.code_detected);
    assert!(attempt.result.is_err());
    let shared_error = observation.err().expect("flat data cannot register");
    let separate_error = observe_luma(&encoded.image)
        .err()
        .expect("flat data cannot register");
    assert_eq!(shared_error.to_string(), separate_error.to_string());
}

#[test]
fn blank_frame_does_not_trigger_capture_recovery() {
    let attempt = scan(&RgbImage::from_pixel(256, 256, Rgb([255; 3])));
    assert!(!attempt.code_detected);
    assert!(attempt.result.is_err());
}

fn place_code_on_4k(code: &RgbImage, translation: [f64; 2]) -> RgbImage {
    let (width, height) = (f64::from(code.width()), f64::from(code.height()));
    let source = [
        [-0.5, -0.5],
        [width - 0.5, -0.5],
        [width - 0.5, height - 0.5],
        [-0.5, height - 0.5],
    ];
    let destination = source.map(|point| {
        let local = [point[0] + 0.5 - width * 0.5, point[1] + 0.5 - height * 0.5];
        [
            2048.25 + translation[0] + local[0],
            2047.75 + translation[1] + local[1],
        ]
    });
    let source_to_frame = Homography::from_quad(source, destination).unwrap();
    let frame_to_source = source_to_frame.inverse().unwrap();
    let mut frame = RgbImage::from_pixel(4096, 4096, Rgb([255; 3]));
    let min_x = destination
        .iter()
        .map(|point| point[0])
        .fold(f64::INFINITY, f64::min)
        .floor()
        .max(0.0) as u32;
    let max_x = destination
        .iter()
        .map(|point| point[0])
        .fold(f64::NEG_INFINITY, f64::max)
        .ceil()
        .min(4095.0) as u32;
    let min_y = destination
        .iter()
        .map(|point| point[1])
        .fold(f64::INFINITY, f64::min)
        .floor()
        .max(0.0) as u32;
    let max_y = destination
        .iter()
        .map(|point| point[1])
        .fold(f64::NEG_INFINITY, f64::max)
        .ceil()
        .min(4095.0) as u32;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let Some([source_x, source_y]) =
                frame_to_source.transform([f64::from(x), f64::from(y)])
            else {
                continue;
            };
            if source_x >= -0.5
                && source_y >= -0.5
                && source_x < width - 0.5
                && source_y < height - 0.5
            {
                frame.put_pixel(x, y, crate::capture::bilinear(code, source_x, source_y));
            }
        }
    }
    frame
}

#[test]
fn native_two_pixel_integrated_codes_scan_in_4k_frames() {
    let payload: Vec<u8> = (0..32).map(|index| (index * 71 + 29) as u8).collect();
    for modulation in [codec::Modulation::Luma10LowEcc, codec::Modulation::Luma10] {
        let encoded = encode_with_modulation(&payload, 2, modulation).unwrap();
        let image = place_code_on_4k(&encoded.image, [0.31, -0.27]);

        assert_eq!(decode(&image).unwrap().payload, payload, "{modulation:?}");

        let mut recovery = crate::recovery::PhotoRecovery::new();
        let progress = recovery.push(&image).unwrap();
        assert_eq!(progress.decoded.unwrap().payload, payload, "{modulation:?}");
    }

    let mut recovery = crate::recovery::PhotoRecovery::new();
    assert!(
        recovery
            .push(&RgbImage::from_pixel(4096, 4096, Rgb([255; 3])))
            .is_err()
    );
    assert!(!recovery.code_detected());
}

#[test]
fn reads_color_warp_with_periodic_false_color() {
    use rand::{Rng, SeedableRng};
    let mut random = rand_chacha::ChaCha8Rng::seed_from_u64(831);
    let payload: Vec<u8> = (0..8192).map(|_| random.random()).collect();
    let encoded = encode_with_modulation(&payload, 8, codec::Modulation::Luma10LowEcc).unwrap();
    let source = &encoded.image;
    let center = (f64::from(source.width()) - 1.0) * 0.5;
    for phase in [0.0, 1.1, 2.4] {
        let mut image = source.clone();
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            let nx = (f64::from(x) - center) / center;
            let ny = (f64::from(y) - center) / center;
            let interference =
                (std::f64::consts::TAU * (f64::from(x) / 19.0 + f64::from(y) / 37.0) + phase).sin()
                    * 5.0;
            for channel in 0..3 {
                let shift = [-0.7, 0.0, 0.9][channel];
                let position = [f64::from(x) + shift * nx, f64::from(y) + shift * ny * 0.6];
                let value = sample_lattice(source, position, true).unwrap()[channel]
                    + interference * [-0.5, 0.0, 1.0][channel];
                pixel[channel] = value.round().clamp(0.0, 255.0) as u8;
            }
        }
        let decoded = decode(&image).unwrap_or_else(|error| panic!("phase {phase}: {error:#}"));
        assert_eq!(decoded.payload, payload, "phase {phase}");
    }
}

#[test]
fn failure_selection_prefers_stage_then_largest_candidate() {
    let metadata = FrameMetadata {
        grid: 137,
        modulation: codec::Modulation::Luma10LowEcc,
    };
    let quad = [[0.0, 0.0], [520.0, 0.0], [520.0, 260.0], [0.0, 260.0]];
    let mut failure = None;
    for (stage, area, cause) in [
        (FailureStage::Resolution, 1000.0, "underresolved"),
        (FailureStage::Payload, 500.0, "payload largest"),
        (FailureStage::Equalizer, 2000.0, "earlier stage"),
        (FailureStage::Payload, 100.0, "payload smaller"),
    ] {
        retain_failure(&mut failure, stage, area, &quad, metadata, anyhow!(cause));
    }
    let error = format!("{:#}", failure.unwrap().error);
    assert!(error.contains("payload largest"), "{error}");
    assert!(
        error.contains("2.00 px/cell shortest edge (3.00 mean)"),
        "{error}"
    );
}

#[test]
#[ignore = "requires external camera capture and expected payload"]
fn camera_payload_variants() {
    let image = image::open(std::env::var("CW_CAMERA_FRAME").unwrap())
        .unwrap()
        .to_rgb8();
    let expected = std::fs::read(std::env::var("CW_EXPECTED_PAYLOAD").unwrap()).unwrap();
    let variants = [
        image.clone(),
        image::imageops::rotate90(&image),
        image::imageops::flip_horizontal(&image),
        image::imageops::resize(
            &image,
            image.width() * 3 / 4,
            image.height() * 3 / 4,
            image::imageops::FilterType::Triangle,
        ),
    ];
    for (index, variant) in variants.iter().enumerate() {
        let start = std::time::Instant::now();
        let decoded = decode(variant).unwrap();
        assert_eq!(decoded.payload, expected);
        eprintln!(
            "variant {index}: exact {} bytes in {:?}",
            expected.len(),
            start.elapsed()
        );
    }
}

#[test]
#[ignore = "manual camera stage latency measurement"]
fn camera_stage_latency() {
    let path = std::env::var("CW_CAMERA_FRAME").unwrap();
    let image = image::open(path).unwrap().to_rgb8();
    let now = std::time::Instant::now();
    let candidates = candidate_quads(&image).unwrap();
    eprintln!("locate: {:?}", now.elapsed());
    for (_, quad) in candidates {
        for mirrored in [false, true] {
            for rotation in 0..4 {
                let destination = std::array::from_fn(|i| {
                    quad[(rotation + if mirrored { 4 - i } else { i }) % 4]
                });
                let transform = Homography::from_quad(
                    [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                    destination,
                )
                .unwrap();
                let Some(metadata) = read_frame_metadata(&image, &transform, 0.0)
                    .or_else(|| read_frame_metadata(&image, &transform, 1.0))
                else {
                    continue;
                };
                if pilot_luma_correlation(&image, &transform, metadata).unwrap_or(0.0) < 0.5 {
                    continue;
                }
                let now = std::time::Instant::now();
                let fields = color_registration(&image, &transform, metadata);
                eprintln!(
                    "register: {:?}; fields={}",
                    now.elapsed(),
                    fields.iter().flatten().count()
                );
                let now = std::time::Instant::now();
                let features = equalized_features(&image, &transform, metadata).unwrap();
                eprintln!("features incl registration: {:?}", now.elapsed());
                let now = std::time::Instant::now();
                let result = codec::decode_from_feature_slice_with_modulation(
                    metadata.grid,
                    8,
                    metadata.modulation,
                    &features,
                );
                eprintln!(
                    "classify/ECC: {:?}; result={:?}",
                    now.elapsed(),
                    result.map(|v| v.payload.len())
                );
                return;
            }
        }
    }
    panic!("No oriented integrated code found");
}

#[test]
fn boundary_projections_preserve_component_geometry() {
    let directions = std::array::from_fn(|i| {
        let angle = i as f64 * std::f64::consts::TAU / 16.0;
        [angle.cos(), angle.sin()]
    });
    for angle in [0.0_f64, 33.0, 45.0, 87.0] {
        let (sin, cos) = angle.to_radians().sin_cos();
        let contains = |x: u32, y: u32| {
            let x = f64::from(x) - 48.0;
            let y = f64::from(y) - 41.0;
            let u = x * cos + y * sin;
            let v = -x * sin + y * cos;
            u.abs() <= 28.0 && v.abs() <= 23.0 && !(u.abs() < 18.0 && v.abs() < 13.0)
        };
        let mut all = ComponentBuilder::new();
        let mut boundary_only = ComponentBuilder::new();
        for y in 0..83 {
            for x in 0..97 {
                if !contains(x, y) {
                    continue;
                }
                let boundary = [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)]
                    .into_iter()
                    .any(|(nx, ny)| !contains(nx, ny));
                all.add(x, y, true, &directions);
                boundary_only.add(x, y, boundary, &directions);
            }
        }
        assert_eq!(all.projections, boundary_only.projections);
        let all = all.component;
        let boundary_only = boundary_only.component;
        assert_eq!(all.area, boundary_only.area);
        assert_eq!(all.min, boundary_only.min);
        assert_eq!(all.max, boundary_only.max);
        assert_eq!(all.extremes, boundary_only.extremes);
        assert_eq!(all.center(), boundary_only.center());
    }
}

#[test]
fn profile_metadata_corrects_bits_without_trial_profile_selection() {
    for modulation in [codec::Modulation::Luma10, codec::Modulation::Luma10LowEcc] {
        let metadata = FrameMetadata {
            grid: 51,
            modulation,
        };
        let word = encode_metadata_with_modulation(metadata.grid, modulation);
        assert_eq!(decode_metadata_with_modulation(word), Some(metadata));
        for first in 0..32 {
            assert_eq!(
                decode_metadata_with_modulation(word ^ (1 << first)),
                Some(metadata)
            );
            for second in first + 1..32 {
                assert_eq!(
                    decode_metadata_with_modulation(word ^ (1 << first) ^ (1 << second)),
                    None
                );
            }
        }
    }
}

#[test]
fn integrated_metadata_has_distinct_orientation_and_bounded_grid() {
    let modulation = codec::Modulation::Luma10;
    for grid in [39, 511] {
        assert_eq!(
            decode_metadata_with_modulation(encode_metadata_with_modulation(grid, modulation)),
            None
        );
    }
    for grid in [40, 510] {
        assert_eq!(
            decode_metadata_with_modulation(encode_metadata_with_modulation(grid, modulation)),
            Some(FrameMetadata { grid, modulation })
        );
    }
    for length in [0, 2048] {
        let encoded = encode_with_modulation(&vec![0x53; length], 2, modulation).unwrap();
        let low = 10.5;
        let high = f64::from(encoded.image.width()) - 11.5;
        let corners = [[low, low], [high, low], [high, high], [low, high]];
        let mut accepted = 0;
        for mirrored in [false, true] {
            for rotation in 0..4 {
                let destination = std::array::from_fn(|i| {
                    corners[(rotation + if mirrored { 4 - i } else { i }) % 4]
                });
                let transform = Homography::from_quad(
                    [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                    destination,
                )
                .unwrap();
                let metadata = read_frame_metadata(&encoded.image, &transform, 0.0)
                    .or_else(|| read_frame_metadata(&encoded.image, &transform, 1.0));
                if metadata.is_some() {
                    assert_eq!(
                        metadata,
                        Some(FrameMetadata {
                            grid: encoded.layout.grid_cells,
                            modulation
                        })
                    );
                    assert_eq!(destination[0][0], low);
                    assert_eq!(destination[1][0], high);
                    accepted += 1;
                }
            }
        }
        assert_eq!(accepted, 2);
    }
}

#[test]
fn low_ecc_metadata_is_distinct_from_high_ecc() {
    let grid = 54;
    let high = codec::Modulation::Luma10;
    let low = codec::Modulation::Luma10LowEcc;
    assert_eq!(high.metadata_magic(), 0xcd);
    assert_eq!(low.metadata_magic(), 0xce);
    assert_ne!(
        encode_metadata_with_modulation(grid, high),
        encode_metadata_with_modulation(grid, low)
    );
    assert_eq!(
        decode_metadata_with_modulation(encode_metadata_with_modulation(grid, low)),
        Some(FrameMetadata {
            grid,
            modulation: low
        })
    );
    for grid in [39, 511] {
        assert_eq!(
            decode_metadata_with_modulation(encode_metadata_with_modulation(grid, low)),
            None
        );
    }
}

#[test]
fn metadata_rejects_invalid_grid_bounds() {
    for grid in [0, 15, 2023, 2047] {
        assert_eq!(decode_metadata(encode_metadata(grid)), None);
    }
    let modulation = codec::Modulation::Luma10;
    let grid = crate::luma::MAX_GRID_CELLS;
    assert_eq!(
        decode_metadata_with_modulation(encode_metadata_with_modulation(grid, modulation)),
        Some(FrameMetadata { grid, modulation })
    );
    assert_eq!(
        decode_metadata_with_modulation(encode_metadata_with_modulation(grid + 1, modulation)),
        None
    );
}
#[test]
fn rejects_retired_format_metadata() {
    for word in [
        0x63839c04, 0x64039f61, 0x64831cd6, 0xe5031999, 0xe5839a2e, 0x66031a80,
    ] {
        assert_eq!(decode_metadata_with_modulation(word), None);
    }
}
fn transfer_tracking_fixture() -> Vec<(Vec<u8>, RgbImage)> {
    use rand::{RngCore, SeedableRng};
    let mut data = vec![0u8; 3 * 10240];
    rand_chacha::ChaCha8Rng::seed_from_u64(0x6075).fill_bytes(&mut data);
    let transfer = crate::transfer::RepairTransfer::with_id(&data, 10240, 20, [0x7b; 16]).unwrap();
    transfer
        .group_symbols(0)
        .unwrap()
        .into_iter()
        .take(3)
        .map(|symbol| {
            let packet = symbol.envelope().unwrap();
            let encoded = encode_with_options(
                &packet,
                8,
                codec::Modulation::Luma10LowEcc,
                codec::EncodeOptions {
                    compression: codec::Compression::None,
                },
            )
            .unwrap();
            (packet, encoded.image)
        })
        .collect()
}

fn place_transfer_image(image: &RgbImage, x: u32, y: u32) -> RgbImage {
    let mut canvas = RgbImage::from_pixel(image.width() + 256, image.height() + 256, Rgb([255; 3]));
    image::imageops::replace(&mut canvas, image, i64::from(x), i64::from(y));
    canvas
}

#[test]
fn transfer_scanner_tracks_distinct_packets_and_preserves_normal_fast_path() {
    let frames = transfer_tracking_fixture();
    let mut session = TransferScanner::new();
    for (step, index) in [2, 0, 1, 2].into_iter().enumerate() {
        let (packet, source) = &frames[index];
        let image = place_transfer_image(
            source,
            32 + if index == 1 { 8 } else { 0 },
            32 + if index == 1 { 5 } else { 0 },
        );
        if step > 0 {
            assert!(
                session
                    .geometry
                    .as_ref()
                    .unwrap()
                    .candidate(&image)
                    .is_some()
            );
        }
        let normal = scan(&image).result.unwrap();
        let decoded = session.scan(&image).result.unwrap();
        assert_eq!(&decoded.payload, packet);
        assert_eq!(decoded.payload, normal.payload);
    }
    session.reset();
    assert!(session.geometry.is_none());
    let tiny = encode(b"ordinary tiny normal fast path", 2).unwrap();
    let normal = scan(&tiny.image).result.unwrap();
    let decoded = session.scan(&tiny.image).result.unwrap();
    assert_eq!(decoded.payload, normal.payload);
    assert_eq!(decoded.stats.corrected_bytes, normal.stats.corrected_bytes);
    assert_eq!(decoded.stats.erased_bytes, normal.stats.erased_bytes);
    assert_eq!(
        decoded.stats.mean_symbol_confidence.to_bits(),
        normal.stats.mean_symbol_confidence.to_bits()
    );
}

#[test]
fn transfer_scanner_incomplete_frame_clears_geometry_and_next_frame_reacquires() {
    let frames = transfer_tracking_fixture();
    let mut session = TransferScanner::new();
    let first = place_transfer_image(&frames[0].1, 32, 32);
    assert_eq!(session.scan(&first).result.unwrap().payload, frames[0].0);
    let mixed = RgbImage::from_fn(frames[0].1.width(), frames[0].1.height(), |x, y| {
        *frames[usize::from(x >= frames[0].1.width() / 2)]
            .1
            .get_pixel(x, y)
    });
    let mixed = place_transfer_image(&mixed, 32, 32);
    assert!(
        session
            .geometry
            .as_ref()
            .unwrap()
            .candidate(&mixed)
            .is_some()
    );
    let failed = session.scan(&mixed);
    assert!(failed.code_detected);
    assert!(failed.result.is_err());
    assert!(session.geometry.is_none());
    let moved = place_transfer_image(&frames[1].1, 112, 82);
    assert_eq!(session.scan(&moved).result.unwrap().payload, frames[1].0);
    assert!(session.geometry.is_some());
    session.reset();
    let black = RgbImage::new(first.width(), first.height());
    let rejected = session.scan(&black);
    assert!(rejected.result.is_err());
    assert!(!rejected.code_detected);
    assert!(session.geometry.is_none());
}

#[test]
fn transfer_recovery_origin_matches_retained_observation_not_later_candidate() {
    let first = FrameMetadata {
        grid: 108,
        modulation: codec::Modulation::Luma10LowEcc,
    };
    let second = FrameMetadata {
        grid: 149,
        modulation: codec::Modulation::Luma10,
    };
    let first_quad = [[10.0, 10.0], [90.0, 10.0], [90.0, 90.0], [10.0, 90.0]];
    let observation = crate::recovery::FrameObservation {
        grid: first.grid,
        modulation: first.modulation,
        raw: vec![[2.0; 12]],
        mixed: vec![[3.0; 12]],
    };
    let selected = Homography::from_quad(
        [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        [
            [100.0, 100.0],
            [190.0, 100.0],
            [190.0, 190.0],
            [100.0, 190.0],
        ],
    )
    .unwrap();
    let retained = RecoveryCandidates {
        observation: Some((observation, (first_quad, first))),
        selected: Some((0.99, selected, second)),
        ..RecoveryCandidates::default()
    };
    assert_eq!(retained.origin(), Some((first_quad, first)));
    let observed = retained.finish(&RgbImage::new(1, 1)).unwrap();
    assert_eq!(observed.grid, first.grid);
    assert_eq!(observed.modulation, first.modulation);
    assert_eq!(observed.raw[0], [2.0; 12]);
}

#[cfg(test)]
mod finder_downsample_tests {
    use super::*;

    #[test]
    fn row_slices_preserve_border_replication_and_rounding() {
        for (width, height) in [
            (0, 0),
            (0, 7),
            (7, 0),
            (1, 1),
            (1, 7),
            (7, 1),
            (2, 2),
            (3, 3),
            (15, 16),
            (16, 15),
            (17, 18),
            (18, 17),
            (31, 32),
            (32, 31),
        ] {
            for offset in 0..4 {
                let image = RgbImage::from_fn(width, height, |x, y| {
                    Rgb(std::array::from_fn(|channel| {
                        ((x * 31 + y * 127 + channel as u32 * 151 + offset + (offset + 1) * x * y)
                            % 256) as u8
                    }))
                });
                let expected = RgbImage::from_fn(
                    image.width().div_ceil(2),
                    image.height().div_ceil(2),
                    |x, y| {
                        let x0 = x * 2;
                        let y0 = y * 2;
                        let x1 = (x0 + 1).min(image.width() - 1);
                        let y1 = (y0 + 1).min(image.height() - 1);
                        Rgb(std::array::from_fn(|channel| {
                            ((u16::from(image.get_pixel(x0, y0)[channel])
                                + u16::from(image.get_pixel(x1, y0)[channel])
                                + u16::from(image.get_pixel(x0, y1)[channel])
                                + u16::from(image.get_pixel(x1, y1)[channel])
                                + 2)
                                / 4) as u8
                        }))
                    },
                );
                assert_eq!(
                    downsample_for_finders(&image),
                    expected,
                    "{width}x{height}, offset{offset}"
                );
                for extra in 1..=3 {
                    let mut storage = image.as_raw().clone();
                    storage.extend_from_slice(&[0xa5; 3][..extra]);
                    let with_trailing = RgbImage::from_raw(width, height, storage).unwrap();
                    assert_eq!(
                        downsample_for_finders(&with_trailing),
                        expected,
                        "{width}x{height}, offset{offset}, trailing{extra}"
                    );
                }
            }
        }
    }
}
