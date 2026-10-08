use super::*;

fn legacy_calibration_weights(
    calibration: &Calibration,
    modulation: Modulation,
    grid: u32,
    cell: usize,
) -> [f32; 4] {
    let (width, height) = modulation.patch_shape();
    let (left, top) = modulation.pilot_origin(grid, 0);
    let (right, bottom) = modulation.pilot_origin(grid, 3);
    let x = ((cell as u32 % grid) as f32 - left as f32 - (width - 1) as f32 / 2.0)
        / (right - left) as f32;
    let y = ((cell as u32 / grid) as f32 - top as f32 - (height - 1) as f32 / 2.0)
        / (bottom - top) as f32;
    let x = x.clamp(0.0, 1.0);
    let y = y.clamp(0.0, 1.0);
    let mut weights = [(1.0 - x) * (1.0 - y), x * (1.0 - y), (1.0 - x) * y, x * y];
    for (weight, &valid) in weights.iter_mut().zip(&calibration.valid) {
        if !valid {
            *weight = 0.0;
        }
    }
    let mut total: f32 = weights.iter().sum();
    if total <= f32::EPSILON {
        weights = calibration.valid.map(|valid| if valid { 1.0 } else { 0.0 });
        total = weights.iter().sum();
    }
    weights.map(|weight| weight / total)
}

fn legacy_calibration_coordinates(
    calibration: &Calibration,
    modulation: Modulation,
    grid: u32,
    cell: usize,
    observed: &Feature,
) -> ([f32; 10], [f32; 10]) {
    let mut center = [0.0; 10];
    let mut inverse = [[0.0; 10]; 10];
    let mut noise = [0.0; 10];
    for (patch, weight) in legacy_calibration_weights(calibration, modulation, grid, cell)
        .into_iter()
        .enumerate()
    {
        if weight == 0.0 {
            continue;
        }
        let model = calibration.demixers[patch].as_ref().unwrap();
        for row in 0..6 {
            center[row] += weight * model.center[row];
            noise[row] += weight * model.noise[row];
            for (entry, reference) in inverse[row][..6].iter_mut().zip(model.inverse[row]) {
                *entry += weight * reference;
            }
        }
    }
    let centered: [f32; 10] = std::array::from_fn(|i| observed[i] - center[i]);
    let mut latent = [0.0; 10];
    for row in 0..6 {
        latent[row] = (0..6)
            .map(|column| inverse[row][column] * centered[column])
            .sum::<f32>();
    }
    (latent, noise)
}

#[test]
fn cached_calibration_coordinates_preserve_baseline_bits_and_read_order() {
    let references: [[Feature; 32]; 4] = std::array::from_fn(|patch| {
        std::array::from_fn(|index| {
            let symbol = crate::luma::pilot_symbol(index);
            std::array::from_fn(|axis| {
                if axis >= 6 {
                    return 0.0;
                }
                90.0 + axis as f32 * 5.125
                    + patch as f32 * 3.25
                    + (if axis < 4 { 28.0 } else { 20.0 }) * crate::luma::latent(symbol, axis)
                    + patch as f32 * 0.125 * crate::luma::latent(symbol, (axis + 1) % 6)
                    + (index % 3) as f32 * 0.015625
            })
        })
    });
    let finite: Feature = std::array::from_fn(|axis| 71.25 + axis as f32 * 17.125);
    let mut unused_nonfinite = finite;
    unused_nonfinite[6..].fill(f32::NAN);
    let observations = [
        finite,
        std::array::from_fn(|axis| (axis as f32 - 5.0) * 100_000.0),
        [-0.0; 12],
        [f32::INFINITY; 12],
        [f32::NEG_INFINITY; 12],
        [f32::NAN; 12],
        std::array::from_fn(|axis| match axis % 3 {
            0 => f32::from_bits(0xffc01234),
            1 => f32::INFINITY,
            _ => f32::NEG_INFINITY,
        }),
        unused_nonfinite,
    ];
    for modulation in [Modulation::Luma10, Modulation::Luma10LowEcc] {
        for grid in [40, 53, 107, 148, 255, 510] {
            let expected_cells: Vec<_> = (0..4)
                .flat_map(|patch| {
                    (0..32).map(move |symbol| calibration_cell_for(grid, patch, symbol, modulation))
                })
                .collect();
            let mut cells: Vec<_> = (0..grid)
                .flat_map(|position| {
                    [
                        position,
                        (grid - 1) * grid + position,
                        position * grid,
                        position * grid + grid - 1,
                        position * grid + position,
                    ]
                })
                .map(|cell| cell as usize)
                .collect();
            if grid <= 53 {
                cells.extend(0..(grid * grid) as usize);
            }
            cells.sort_unstable();
            cells.dedup();
            for mask in 0_u8..16 {
                let mut calls = Vec::new();
                let calibration = Calibration::read(grid, modulation, &mut |cell| {
                    calls.push(cell);
                    let index = expected_cells
                        .iter()
                        .position(|&expected| expected == cell)
                        .unwrap();
                    Ok(if mask & (1 << (index / 32)) != 0 {
                        references[index / 32][index % 32]
                    } else {
                        [128.0; 12]
                    })
                });
                assert_eq!(calls, expected_cells);
                let Ok(calibration) = calibration else {
                    assert_eq!(mask, 0);
                    continue;
                };
                assert_eq!(
                    calibration.valid,
                    std::array::from_fn(|patch| mask & (1 << patch) != 0)
                );
                if mask == 1 {
                    assert_eq!(
                        calibration.weights(grid, (grid * grid - 1) as usize),
                        [1.0, 0.0, 0.0, 0.0]
                    );
                }
                for &cell in &cells {
                    assert_eq!(
                        calibration.weights(grid, cell).map(f32::to_bits),
                        legacy_calibration_weights(&calibration, modulation, grid, cell)
                            .map(f32::to_bits),
                        "{modulation:?}, grid {grid}, mask {mask}, cell {cell}"
                    );
                    for observed in &observations {
                        let (latent, noise) = calibration.coordinates(grid, cell, observed);
                        let (expected_latent, expected_noise) = legacy_calibration_coordinates(
                            &calibration,
                            modulation,
                            grid,
                            cell,
                            observed,
                        );
                        assert_eq!(
                            latent.map(f32::to_bits),
                            expected_latent.map(f32::to_bits),
                            "{modulation:?}, grid {grid}, mask {mask}, cell {cell}, observed {observed:?}"
                        );
                        assert_eq!(noise.map(f32::to_bits), expected_noise.map(f32::to_bits));
                    }
                }
            }
        }
    }
}

#[test]
fn borrowed_features_preserve_decode_results_and_validation_failures() {
    let payload: Vec<u8> = (0..256).map(|i| (i * 131 + 47) as u8).collect();
    for mode in [Modulation::Luma10, Modulation::Luma10LowEcc] {
        let encoded = encode_with_modulation(&payload, 4, mode).unwrap();
        let grid = encoded.layout.grid_cells;
        let pristine: Vec<_> = (0..(grid * grid) as usize)
            .map(|cell| feature(&encoded.image, &encoded.layout, cell))
            .collect();
        for scenario in 0..4 {
            let mut observed = pristine.clone();
            match scenario {
                1 => {
                    let cell = data_cells_for(grid, mode).next().unwrap();
                    observed[cell][0] = 256.0 - observed[cell][0];
                }
                2 => {
                    for patch in 0..4 {
                        for symbol in 0..mode.calibration_symbols() {
                            observed[calibration_cell_for(grid, patch, symbol, mode)] = [128.0; 12];
                        }
                    }
                }
                3 => {
                    for cell in data_cells_for(grid, mode) {
                        observed[cell] = [128.0; 12];
                    }
                }
                _ => {}
            }
            let copied =
                decode_from_features_with_modulation(grid, 4, mode, |cell| Ok(observed[cell]));
            let borrowed = decode_from_feature_slice_with_modulation(grid, 4, mode, &observed);
            assert_eq!(
                borrowed.is_ok(),
                scenario < 2,
                "{mode:?}, scenario {scenario}"
            );
            match (copied, borrowed) {
                (Ok(copied), Ok(borrowed)) => {
                    assert_eq!(borrowed.payload, payload);
                    assert_eq!(borrowed.payload, copied.payload);
                    assert_eq!(
                        serde_json::to_value(&borrowed.layout).unwrap(),
                        serde_json::to_value(&copied.layout).unwrap()
                    );
                    assert_eq!(
                        serde_json::to_value(&borrowed.stats).unwrap(),
                        serde_json::to_value(&copied.stats).unwrap()
                    );
                }
                (Err(copied), Err(borrowed)) => {
                    assert_eq!(borrowed.to_string(), copied.to_string())
                }
                _ => {
                    panic!("borrowed and copied outcomes differ: {mode:?}, scenario {scenario}")
                }
            }
        }
    }
}

#[test]
fn compression_roundtrips_both_ecc_modes_and_reports_transport_size() {
    let payload: Vec<u8> = (0..=255).cycle().take(8192).collect();
    for modulation in [Modulation::Luma10, Modulation::Luma10LowEcc] {
        let encoded = encode_with_options(
            &payload,
            8,
            modulation,
            EncodeOptions {
                compression: Compression::Auto,
            },
        )
        .unwrap();
        assert_eq!(encoded.layout.compression, "zstd");
        assert_eq!(encoded.layout.payload_bytes, payload.len());
        assert!(encoded.layout.encoded_payload_bytes < payload.len());
        assert!(encoded.layout.encoded_payload_bytes <= encoded.layout.payload_capacity_bytes);
        let decoded = decode_with_modulation(&encoded.image, 8, modulation).unwrap();
        assert_eq!(decoded.payload, payload);
        assert_eq!(
            decoded.layout.encoded_payload_bytes,
            encoded.layout.encoded_payload_bytes
        );
        assert_eq!(decoded.layout.compression, "zstd");
        assert_eq!(decoded.layout.format, encoded.layout.format);
        let framed = crate::scanner::encode_with_options(
            &payload,
            8,
            modulation,
            EncodeOptions {
                compression: Compression::Auto,
            },
        )
        .unwrap();
        let scanned = crate::scanner::decode(&framed.image).unwrap();
        assert_eq!(scanned.payload, payload);
        assert_eq!(scanned.layout.compression, "zstd");
        assert_eq!(
            scanned.layout.encoded_payload_bytes,
            framed.layout.encoded_payload_bytes
        );
        assert_eq!(scanned.layout.format, framed.layout.format);
    }
}

#[test]
fn compression_preserves_raw_packets_when_no_rs_block_is_saved() {
    let payload = b"CWI5\x28\xb5\x2f\xfd accidental compressed format prefix";
    let uncompressed = encode(payload, 8).unwrap();
    let automatic = encode_with_options(
        payload,
        8,
        Modulation::Luma10,
        EncodeOptions {
            compression: Compression::Auto,
        },
    )
    .unwrap();
    assert_eq!(uncompressed.image, automatic.image);
    assert_eq!(automatic.layout.compression, "none");
    assert_eq!(automatic.layout.encoded_payload_bytes, payload.len());
    assert_eq!(decode(&automatic.image, 8).unwrap().payload, payload);
}

#[test]
fn compression_keeps_incompressible_data_raw_and_bounds_original_input() {
    use rand::{RngCore, SeedableRng};
    let options = EncodeOptions {
        compression: Compression::Auto,
    };
    for (seed, length) in [(20261004, 2048), (20261005, 8192), (20261006, 20480)] {
        let mut payload = vec![0; length];
        rand_chacha::ChaCha8Rng::seed_from_u64(seed).fill_bytes(&mut payload);
        let uncompressed =
            crate::scanner::encode_with_modulation(&payload, 4, Modulation::Luma10).unwrap();
        let automatic =
            crate::scanner::encode_with_options(&payload, 4, Modulation::Luma10, options).unwrap();
        assert_eq!(uncompressed.image, automatic.image);
        assert_eq!(automatic.layout.compression, "none");
        assert_eq!(
            crate::scanner::decode(&automatic.image).unwrap().payload,
            payload
        );
    }
    assert!(
        encode_with_options(
            &vec![0; crate::compression::MAX_DECOMPRESSED_BYTES + 1],
            4,
            Modulation::Luma10,
            options,
        )
        .err()
        .unwrap()
        .to_string()
        .contains("compression limit")
    );
}

#[test]
fn compression_rejects_malformed_envelopes_even_with_valid_packet_crc() {
    let encoded_payload = crate::compression::compress(&vec![0x53; 8192]).unwrap();
    let mut layout = choose_layout(encoded_payload.len(), 8, Modulation::Luma10).unwrap();
    layout.compression = "zstd";
    for damage in 0..3 {
        let mut bad_payload = encoded_payload.clone();
        match damage {
            0 => bad_payload[..4].copy_from_slice(&u32::MAX.to_le_bytes()),
            1 => bad_payload.push(0),
            _ => bad_payload[4] ^= 1,
        }
        let packet: Vec<u8> = codewords(&bad_payload, &layout)
            .iter()
            .flat_map(|block| block[..layout.rs_data_bytes].iter().copied())
            .collect();
        assert!(packet_payload(&packet, &layout).is_ok());
        assert!(decode_packet(&packet, &mut layout).is_err());
    }
}

#[test]
fn raw_systematic_validator_rejects_bad_magic_length_crc_and_padding() {
    for modulation in [Modulation::Luma10, Modulation::Luma10LowEcc] {
        let payload = b"complete packet validation before repair";
        let layout = choose_layout(payload.len(), 8, modulation).unwrap();
        let packet: Vec<u8> = codewords(payload, &layout)
            .iter()
            .flat_map(|block| block[..layout.rs_data_bytes].iter().copied())
            .collect();
        assert_eq!(packet_payload(&packet, &layout).unwrap(), payload);
        let mut bad = packet.clone();
        bad[0] ^= 1;
        assert!(
            packet_payload(&bad, &layout)
                .unwrap_err()
                .to_string()
                .contains("format")
        );
        let mut bad = packet.clone();
        bad[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(
            packet_payload(&bad, &layout)
                .unwrap_err()
                .to_string()
                .contains("length")
        );
        let mut bad = packet.clone();
        bad[8] ^= 1;
        assert!(
            packet_payload(&bad, &layout)
                .unwrap_err()
                .to_string()
                .contains("checksum")
        );
        let mut bad = packet.clone();
        bad[HEADER_BYTES + payload.len()] = 1;
        assert!(
            packet_payload(&bad, &layout)
                .unwrap_err()
                .to_string()
                .contains("padding")
        );
    }
}

#[test]
fn raw_systematic_validation_keeps_observed_erasures_without_applying_parity_repairs() {
    let payload: Vec<u8> = (0..256).map(|i| (i * 131 + 47) as u8).collect();
    let mode = Modulation::Luma10;
    let encoded = encode_with_modulation(&payload, 8, mode).unwrap();
    let bits = mode.bits_per_cell();
    let blocks = encoded.layout.rs_blocks;
    let parity_start_bit = DATA_BYTES * blocks * 8;
    let index = parity_start_bit.div_ceil(bits);
    let target = data_cells_for(encoded.layout.grid_cells, mode)
        .nth(index)
        .unwrap();
    let decoded =
        decode_from_features_with_modulation(encoded.layout.grid_cells, 8, mode, |cell| {
            let mut observed = feature(&encoded.image, &encoded.layout, cell);
            if cell == target {
                observed[0] = 128.0;
            }
            Ok(observed)
        })
        .unwrap();
    assert_eq!(decoded.payload, payload);
    assert_eq!(decoded.stats.corrected_bytes, 0);
    assert!(decoded.stats.erased_bytes > 0);
}

#[test]
fn raw_packet_accepts_parity_damage_and_repairs_systematic_damage_within_budget() {
    let payload: Vec<u8> = (0..256).map(|i| (i * 131 + 47) as u8).collect();
    for mode in [Modulation::Luma10, Modulation::Luma10LowEcc] {
        for (start, errors) in [
            (mode.rs_data_bytes(), 1),
            (HEADER_BYTES, 1),
            (HEADER_BYTES, mode.rs_parity_bytes() / 2 + 1),
        ] {
            let mut encoded = encode_with_modulation(&payload, 8, mode).unwrap();
            let mut blocks = codewords(&payload, &encoded.layout);
            for byte in &mut blocks[0][start..start + errors] {
                *byte ^= 1;
            }
            let packet: Vec<u8> = blocks
                .iter()
                .flat_map(|block| block[..mode.rs_data_bytes()].iter().copied())
                .collect();
            assert_eq!(
                packet_payload(&packet, &encoded.layout).is_ok(),
                start == mode.rs_data_bytes()
            );
            let interleaved: Vec<u8> = (0..BLOCK_BYTES)
                .flat_map(|position| blocks.iter().map(move |block| block[position]))
                .collect();
            let bits = mode.bits_per_cell();
            let cells = data_cells_for(encoded.layout.grid_cells, mode)
                .take((interleaved.len() * 8).div_ceil(bits));
            for (index, cell) in cells.enumerate() {
                let mut symbol = 0;
                for bit in 0..bits {
                    let position = index * bits + bit;
                    let value = if position < interleaved.len() * 8 {
                        (interleaved[position / 8] >> (7 - position % 8)) & 1
                    } else {
                        0
                    };
                    symbol = (symbol << 1) | u16::from(value);
                }
                draw_symbol(&mut encoded.image, &encoded.layout, cell, symbol);
            }
            let result = decode_with_modulation(&encoded.image, 8, mode);
            if errors > mode.rs_parity_bytes() / 2 {
                assert!(
                    result.is_err(),
                    "{mode:?} accepted beyond-budget data damage"
                );
            } else {
                let decoded = result.unwrap();
                assert_eq!(decoded.payload, payload);
                assert_eq!(
                    decoded.stats.corrected_bytes,
                    usize::from(start != mode.rs_data_bytes())
                );
                assert_eq!(decoded.stats.erased_bytes, 0);
            }
        }
    }
}

#[test]
fn low_ecc_repairs_twenty_four_errors_and_rejects_twenty_five() {
    let mode = Modulation::Luma10LowEcc;
    let payload: Vec<u8> = (0..180).map(|i| (i * 67 + 11) as u8).collect();
    let encoded = encode_with_modulation(&payload, 2, mode).unwrap();
    assert_eq!(encoded.layout.rs_blocks, 4);
    assert_eq!(encoded.layout.rs_data_bytes, 207);
    assert_eq!(encoded.layout.rs_parity_bytes, 48);
    let original = codewords(&payload, &encoded.layout);
    for errors in [24, 25] {
        let mut damaged = encoded.image.clone();
        let mut blocks = original.clone();
        for byte in blocks[0][HEADER_BYTES..HEADER_BYTES + errors].iter_mut() {
            *byte ^= 0x55;
        }
        draw_codewords(&mut damaged, &encoded.layout, &blocks);
        let result = decode_with_modulation(&damaged, 2, mode);
        if errors == 24 {
            let decoded = result.unwrap();
            assert_eq!(decoded.payload, payload);
            assert_eq!(decoded.stats.corrected_bytes, 24);
        } else {
            assert!(result.is_err());
        }
    }
}

#[test]
fn low_ecc_checksum_rejects_a_valid_rs_packet_with_modified_payload() {
    let mode = Modulation::Luma10LowEcc;
    let payload = b"integrity";
    let mut encoded = encode_with_modulation(payload, 2, mode).unwrap();
    let mut packet =
        codewords(payload, &encoded.layout)[0][..encoded.layout.rs_data_bytes].to_vec();
    packet[HEADER_BYTES] ^= 1;
    let mut blocks = codewords(payload, &encoded.layout);
    blocks[0] = Encoder::new(encoded.layout.rs_parity_bytes)
        .encode(&packet)
        .to_vec();
    draw_codewords(&mut encoded.image, &encoded.layout, &blocks);
    assert!(
        decode_with_modulation(&encoded.image, 2, mode)
            .unwrap_err()
            .to_string()
            .contains("checksum")
    );
}

#[test]
fn low_ecc_raw_packet_accepts_parity_damage() {
    let mode = Modulation::Luma10LowEcc;
    let payload = b"systematic packet";
    let mut encoded = encode_with_modulation(payload, 2, mode).unwrap();
    let mut blocks = codewords(payload, &encoded.layout);
    blocks[0][encoded.layout.rs_data_bytes] ^= 0x55;
    draw_codewords(&mut encoded.image, &encoded.layout, &blocks);
    let decoded = decode_with_modulation(&encoded.image, 2, mode).unwrap();
    assert_eq!(decoded.payload, payload);
    assert_eq!(decoded.stats.corrected_bytes, 0);
}

fn draw_codewords(image: &mut RgbImage, layout: &Layout, blocks: &[Vec<u8>]) {
    let interleaved: Vec<u8> = (0..BLOCK_BYTES)
        .flat_map(|position| blocks.iter().map(move |block| block[position]))
        .collect();
    let bits = layout.modulation.bits_per_cell();
    for (index, cell) in data_cells_for(layout.grid_cells, layout.modulation)
        .take((interleaved.len() * 8).div_ceil(bits))
        .enumerate()
    {
        let symbol = (0..bits).fold(0, |value, bit| {
            let position = index * bits + bit;
            let next = if position < interleaved.len() * 8 {
                (interleaved[position / 8] >> (7 - position % 8)) & 1
            } else {
                0
            };
            (value << 1) | u16::from(next)
        });
        draw_symbol(image, layout, cell, symbol);
    }
}

#[test]
fn luma10_calibration_recovers_all_symbols_and_rejects_erased_pilots() {
    let modulation = Modulation::Luma10;
    let observe = |symbol| {
        let samples = SAMPLE_POSITIONS.map(|[u, v]| {
            crate::luma::rgb(symbol, u as f32, v as f32)
                .0
                .map(f32::from)
        });
        crate::luma::feature(samples)
    };
    let references: [Feature; crate::luma::PILOTS] =
        std::array::from_fn(|index| observe(modulation.pilot_symbol(index)));
    let model = Demixer::fit(&references).unwrap();
    eprintln!(
        "Luma10 quantization residual variance: {:?}",
        &model.noise[..6]
    );
    let calibration = Calibration {
        valid: [true; 4],
        demixers: [Some(model); 4],
        axis_coordinates: Calibration::axis_coordinates(53, modulation),
    };
    let mut minimum = u8::MAX;
    let mut maximum = u8::MIN;
    for symbol in 0..1024 {
        let (latent, noise) = calibration.coordinates(53, 1400, &observe(symbol));
        let (decoded, confidence) =
            crate::luma::classify_levels(latent, noise, [-3.0, -1.0, 1.0, 3.0]);
        assert_eq!(decoded, symbol);
        assert!(confidence.iter().all(|&value| value > 0.98));
        for [u, v] in SAMPLE_POSITIONS {
            for value in crate::luma::rgb(symbol, u as f32, v as f32).0 {
                minimum = minimum.min(value);
                maximum = maximum.max(value);
            }
        }
    }
    eprintln!("Luma10 RGB gamut: {minimum}..{maximum}");
    assert!(Demixer::fit(&[[128.0; 12]; 32]).is_none());
}

#[test]
fn binary_payloads_round_trip_across_block_and_grid_boundaries() {
    for length in [
        0, 1, 146, 147, 148, 158, 159, 160, 305, 306, 307, 1024, 4096,
    ] {
        let payload: Vec<u8> = (0..length).map(|i| (i * 73 + 191) as u8).collect();
        let encoded = encode(&payload, 4).unwrap();
        let decoded = decode(&encoded.image, 4).unwrap();
        assert_eq!(payload, decoded.payload, "payload length {length}");
        assert_eq!(decoded.stats.corrected_bytes, 0);
        assert!(decoded.stats.mean_symbol_confidence > 0.98);
        assert_eq!(encoded.layout.coded_bytes, decoded.layout.coded_bytes);
    }
}

fn erase_calibration_patch(encoded: &mut Encoded, patch: usize) {
    for symbol in 0..crate::luma::PILOTS {
        let cell = calibration_cell(encoded.layout.grid_cells, patch, symbol);
        let (x, y) = cell_origin(&encoded.layout, cell);
        for dy in 0..encoded.layout.cell_pixels {
            for dx in 0..encoded.layout.cell_pixels {
                encoded.image.put_pixel(x + dx, y + dy, Rgb([255; 3]));
            }
        }
    }
}

#[test]
fn survives_missing_calibration_patches_with_at_least_one_usable_dictionary() {
    let payload: Vec<u8> = (0..1024).map(|i| (i * 31 + 19) as u8).collect();
    for erased in [1, 2, 4, 8, 3, 7] {
        let mut encoded = encode(&payload, 8).unwrap();
        for patch in 0..4 {
            if erased & (1 << patch) != 0 {
                erase_calibration_patch(&mut encoded, patch);
            }
        }
        let decoded = decode(&encoded.image, 8).unwrap();
        assert_eq!(decoded.payload, payload);
        assert_eq!(decoded.stats.corrected_bytes, 0);
    }
}

#[test]
fn rejects_images_when_all_calibration_patches_are_erased() {
    let mut encoded = encode(b"lost calibration", 8).unwrap();
    for patch in 0..4 {
        erase_calibration_patch(&mut encoded, patch);
    }
    assert!(
        decode(&encoded.image, 8)
            .unwrap_err()
            .to_string()
            .contains("all calibration patches")
    );
}

#[test]
fn rejects_wrong_geometry_and_blank_images() {
    assert!(encode(b"test", 3).is_err());
    assert!(encode(b"test", 0).is_err());
    assert!(decode(&RgbImage::from_pixel(640, 640, Rgb([255; 3])), 16).is_err());
    assert!(decode(&RgbImage::new(641, 640), 16).is_err());
    assert!(decode(&RgbImage::new(1, 1), 16).is_err());
}
