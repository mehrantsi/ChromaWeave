use super::*;

#[test]
fn syndrome_prefilter_preserves_bounded_native_corrections() {
    use rand::{Rng, SeedableRng, seq::SliceRandom};
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(0x6d64);
    for parity in [48, 96] {
        let encoder = Encoder::new(parity);
        let decoder = Decoder::new(parity);
        for errors in [0, 1, parity / 4, parity / 2, parity / 2 + 1, parity, 120] {
            let data: Vec<u8> = (0..255 - parity).map(|_| rng.random()).collect();
            let word = encoder.encode(&data).to_vec();
            assert!(
                GmdSyndromes::new(&word, parity)
                    .values
                    .iter()
                    .all(|&value| value == 0)
            );
            let mut raw = word;
            let mut positions: Vec<u8> = (0..255).map(|position| position as u8).collect();
            positions.shuffle(&mut rng);
            for &position in &positions[..errors] {
                raw[usize::from(position)] ^= rng.random_range(1..=255);
            }
            positions.shuffle(&mut rng);
            let mut syndromes = GmdSyndromes::new(&raw, parity);
            for erased in 0..=parity {
                if erased > 0 {
                    syndromes.erase(positions[erased - 1]);
                }
                if let Ok(corrected) = decoder.correct(&raw, Some(&positions[..erased])) {
                    let errors = corrected
                        .iter()
                        .zip(&raw)
                        .enumerate()
                        .filter(|&(position, (a, b))| {
                            a != b && !positions[..erased].contains(&(position as u8))
                        })
                        .count();
                    if 2 * errors + erased <= parity {
                        assert!(
                            syndromes.can_correct(),
                            "parity {parity}, erasures {erased}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn unsupported_signal_cannot_create_certificates() {
    let layout =
        codec::layout_for_grid_with_modulation(40, 2, 0, Modulation::Luma10LowEcc).unwrap();
    let decisions = vec![(0, [0.0; 10]); 1600];
    let mut known = (0..layout.rs_blocks).map(|_| None).collect::<Vec<_>>();
    certify(&decisions, &layout, &data_mask(40), &mut known).unwrap();
    assert!(known.iter().all(Option::is_none));
}

fn decisions_for_words(words: &[Vec<u8>], layout: &Layout) -> Vec<Decision> {
    let mut decisions = vec![(0, [1.0; 10]); (layout.grid_cells * layout.grid_cells) as usize];
    for (index, cell) in data_mask(layout.grid_cells)
        .into_iter()
        .enumerate()
        .filter_map(|(cell, valid)| valid.then_some(cell))
        .enumerate()
    {
        for bit in 0..10 {
            let position = index * 10 + bit;
            if position >= layout.coded_bytes * 8 {
                break;
            }
            let byte = position / 8;
            decisions[cell].0 |= u16::from(
                (words[byte % layout.rs_blocks][byte / layout.rs_blocks] >> (7 - position % 8)) & 1,
            ) << (9 - bit);
        }
    }
    decisions
}

#[test]
fn retained_certificates_correct_errors_and_reject_conflicting_words() {
    let layout =
        codec::layout_for_grid_with_modulation(40, 2, 0, Modulation::Luma10LowEcc).unwrap();
    let encoder = Encoder::new(layout.rs_parity_bytes);
    let words: Vec<_> = (0..layout.rs_blocks)
        .map(|block| {
            let data: Vec<_> = (0..layout.rs_data_bytes)
                .map(|position| (position + block * 17) as u8)
                .collect();
            encoder.encode(&data).to_vec()
        })
        .collect();
    let mut damaged = words.clone();
    for word in &mut damaged {
        for (index, value) in word.iter_mut().take(layout.rs_parity_bytes / 2).enumerate() {
            *value ^= (index + 1) as u8;
        }
    }
    let mut known = (0..layout.rs_blocks).map(|_| None).collect::<Vec<_>>();
    let mask = data_mask(layout.grid_cells);
    certify(
        &decisions_for_words(&damaged, &layout),
        &layout,
        &mask,
        &mut known,
    )
    .unwrap();
    for (certificate, word) in known.iter().zip(&words) {
        assert_eq!(&certificate.as_ref().unwrap().word, word);
    }
    certify(
        &decisions_for_words(&words, &layout),
        &layout,
        &mask,
        &mut known,
    )
    .unwrap();
    let conflicting =
        vec![encoder.encode(&vec![42; layout.rs_data_bytes]).to_vec(); layout.rs_blocks];
    assert!(
        certify(
            &decisions_for_words(&conflicting, &layout),
            &layout,
            &mask,
            &mut known,
        )
        .is_err()
    );
}

#[test]
fn gmd_rejects_distinct_hard_and_soft_candidates() {
    let layout =
        codec::layout_for_grid_with_modulation(40, 2, 0, Modulation::Luma10LowEcc).unwrap();
    let encoder = Encoder::new(layout.rs_parity_bytes);
    let a = encoder.encode(&vec![0; layout.rs_data_bytes]).to_vec();
    let mut data = vec![0; layout.rs_data_bytes];
    data[0] = 1;
    let b = encoder.encode(&data).to_vec();
    let difference: Vec<_> = (0..255).filter(|&i| a[i] != b[i]).collect();
    assert_eq!(difference.len(), layout.rs_parity_bytes + 1);
    let mut raw = a.clone();
    for &position in &difference[..layout.rs_parity_bytes / 2] {
        raw[position] = b[position];
    }
    let mut decisions = decisions_for_words(&vec![raw; layout.rs_blocks], &layout);
    let mask = data_mask(layout.grid_cells);
    for (index, cell) in mask
        .iter()
        .enumerate()
        .filter_map(|(cell, &valid)| valid.then_some(cell))
        .enumerate()
    {
        for bit in 0..10 {
            let byte = (index * 10 + bit) / 8;
            if byte >= layout.coded_bytes {
                break;
            }
            if difference[layout.rs_parity_bytes / 2..].contains(&(byte / layout.rs_blocks)) {
                decisions[cell].1[9 - bit] = 0.9;
            }
        }
    }
    let mut known = (0..layout.rs_blocks).map(|_| None).collect::<Vec<_>>();
    certify(&decisions, &layout, &mask, &mut known).unwrap();
    assert!(known.iter().all(Option::is_none));
}

#[test]
fn mismatched_and_duplicate_observations_leave_session_unchanged() {
    let raw: Vec<Feature> = (0..1600)
        .map(|i| std::array::from_fn(|axis| ((i * 31 + axis * 17) % 251) as f32))
        .collect();
    let mut session = PhotoRecovery {
        frames: vec![Frame {
            observation: FrameObservation {
                grid: 40,
                modulation: Modulation::Luma10LowEcc,
                raw: raw.clone(),
                mixed: raw.clone(),
            },
            initial: vec![(0, [0.0; 10]); 1600],
        }],
        known: Vec::new(),
        last_code_detected: true,
    };
    for candidate in [
        raw.clone(),
        raw.iter()
            .map(|row| row.map(|value| 255.0 - value))
            .collect(),
    ] {
        let outcome = session.push_observation(FrameObservation {
            grid: 40,
            modulation: Modulation::Luma10LowEcc,
            mixed: candidate.clone(),
            raw: candidate,
        });
        assert!(outcome.is_err());
        assert_eq!(session.frame_count(), 1);
    }
}

#[test]
#[ignore = "profiling prepared observation against independently extracted observations"]
fn prepared_observation_matches_extraction() {
    let manifest = std::fs::read(std::env::var("CW_RECOVERY_FIXTURES").unwrap()).unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
    let image = image::open(manifest["frames"][0].as_str().unwrap())
        .unwrap()
        .to_rgb8();
    let (_, prepared) = crate::scanner::scan_with_observation(&image);
    let prepared = prepared.unwrap();
    let separate = crate::scanner::observe_luma(&image).unwrap();
    for (name, a, b) in [
        ("raw", prepared.raw, separate.raw),
        ("mixed", prepared.mixed, separate.mixed),
    ] {
        for axis in 0..6 {
            let differences = a
                .iter()
                .zip(&b)
                .map(|(a, b)| f64::from(a[axis] - b[axis]).powi(2))
                .sum::<f64>();
            eprintln!(
                "{name} {axis}: RMS {:.6}",
                (differences / a.len() as f64).sqrt()
            );
        }
    }
}

#[test]
#[ignore = "requires original camera photos listed in CW_RECOVERY_FIXTURES"]
fn recovers_unchanged_gif_from_real_photos() {
    let manifest =
        std::fs::read(std::env::var("CW_RECOVERY_FIXTURES").expect("CW_RECOVERY_FIXTURES"))
            .unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
    let mut session = PhotoRecovery::new();
    let mut payload = None;
    let total = std::time::Instant::now();
    for path in manifest["frames"].as_array().unwrap() {
        let image = image::open(path.as_str().unwrap()).unwrap().to_rgb8();
        let started = std::time::Instant::now();
        let progress = session.push(&image).unwrap();
        eprintln!(
            "photo {}: {}/{} certified blocks in {:.3}s",
            progress.frames,
            progress.certified_blocks,
            progress.total_blocks,
            started.elapsed().as_secs_f64()
        );
        if let Some(decoded) = progress.decoded {
            payload = Some(decoded.payload);
            break;
        }
    }
    let payload = payload.expect("strict whole-packet camera recovery");
    assert_eq!(
        payload,
        std::fs::read(manifest["expected_payload"].as_str().unwrap()).unwrap()
    );
    if let Some(output) = manifest["recovered_output"].as_str() {
        std::fs::write(output, &payload).unwrap();
    }
    eprintln!(
        "exact {} bytes from real photos in {:.3}s",
        payload.len(),
        total.elapsed().as_secs_f64()
    );
}

#[test]
#[ignore = "requires original camera photos listed in CW_RECOVERY_FIXTURES"]
fn recovers_real_photo_geometric_variants() {
    use image::imageops::{FilterType, flip_horizontal, resize, rotate90};

    let manifest = std::fs::read(std::env::var("CW_RECOVERY_FIXTURES").unwrap()).unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
    let images: Vec<_> = manifest["frames"]
        .as_array()
        .unwrap()
        .iter()
        .map(|path| image::open(path.as_str().unwrap()).unwrap().to_rgb8())
        .collect();
    for variant in ["rotated", "mirrored", "scaled_0.85", "scaled_1.15"] {
        let mut session = PhotoRecovery::new();
        let mut payload = None;
        let mut elapsed = std::time::Duration::ZERO;
        let mut certificates = Vec::new();
        for image in &images {
            let image = match variant {
                "rotated" => rotate90(image),
                "mirrored" => flip_horizontal(image),
                _ => {
                    let scale = if variant == "scaled_0.85" { 0.85 } else { 1.15 };
                    resize(
                        image,
                        (f64::from(image.width()) * scale).round() as u32,
                        (f64::from(image.height()) * scale).round() as u32,
                        FilterType::Triangle,
                    )
                }
            };
            let started = std::time::Instant::now();
            let progress = session.push(&image).unwrap();
            elapsed += started.elapsed();
            certificates.push(progress.certified_blocks);
            if let Some(decoded) = progress.decoded {
                payload = Some(decoded.payload);
                break;
            }
        }
        let payload = payload.unwrap_or_else(|| panic!("{variant}: {certificates:?}"));
        assert_eq!(
            payload,
            std::fs::read(manifest["expected_payload"].as_str().unwrap()).unwrap(),
            "{variant}"
        );
        eprintln!(
            "{variant}: exact {} bytes, {certificates:?}, {:.3}ms processing",
            payload.len(),
            elapsed.as_secs_f64() * 1000.0
        );
    }
}

#[test]
fn parallel_certification_preserves_order_and_rejects_contradictions_atomically() {
    for grid in [96, 107, 148, 212] {
        let layout =
            codec::layout_for_grid_with_modulation(grid, 2, 0, Modulation::Luma10LowEcc).unwrap();
        let mask = data_mask(layout.grid_cells);
        let encoder = Encoder::new(layout.rs_parity_bytes);
        let words: Vec<_> = (0..layout.rs_blocks)
            .map(|block| {
                let data: Vec<_> = (0..layout.rs_data_bytes)
                    .map(|position| (position + block * 17) as u8)
                    .collect();
                encoder.encode(&data).to_vec()
            })
            .collect();
        let mut damaged = words.clone();
        for word in &mut damaged {
            for byte in word.iter_mut().take(layout.rs_parity_bytes / 2) {
                *byte ^= 73;
            }
        }
        let decisions = decisions_for_words(&damaged, &layout);
        let (interleaved, reliability) = transport(&decisions, &layout, &mask);
        let mut serial = vec![None; layout.rs_blocks];
        certify_blocks(&interleaved, &reliability, &layout, 0, &mut serial).unwrap();
        let mut parallel = vec![None; layout.rs_blocks];
        certify(&decisions, &layout, &mask, &mut parallel).unwrap();
        let snapshot = |known: &[Option<Certificate>]| {
            known
                .iter()
                .map(|entry| {
                    entry.as_ref().map(|certificate| {
                        (
                            certificate.word.clone(),
                            certificate.corrected,
                            certificate.erased,
                        )
                    })
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(snapshot(&parallel), snapshot(&serial));
        assert!(parallel.iter().all(Option::is_some));
        parallel.fill(None);
        certify_with_workers(&decisions, &layout, &mask, &mut parallel, 4).unwrap();
        assert_eq!(snapshot(&parallel), snapshot(&serial));
        let conflict_block = layout.rs_blocks - 1;
        let mut conflict = words;
        conflict[conflict_block] = encoder.encode(&vec![42; layout.rs_data_bytes]).to_vec();
        parallel[..conflict_block].fill(None);
        let previous = snapshot(&parallel);
        assert!(
            certify_with_workers(
                &decisions_for_words(&conflict, &layout),
                &layout,
                &mask,
                &mut parallel,
                4,
            )
            .is_err()
        );
        assert_eq!(snapshot(&parallel), previous);
    }
}
