use super::*;

#[test]
fn balanced_trials_preserve_matched_footprints_and_recover_each_payload() {
    let report = compare(ComparisonConfig {
        cw_payload_bytes: 64,
        cw_cell_pixels: 8,
        modulation: crate::codec::Modulation::Luma10,
        qr_payload_bytes: 64,
        trials: 3,
        code_sides: vec![320],
        frame_side: 512,
        severities: vec![0.0],
        occlusion: 0.0,
        occlusion_placement: OcclusionPlacement::Center,
        perspective: 0.04,
        seed: 81,
        jpeg_quality: None,
        chroma420: false,
        illumination: 0.0,
    })
    .unwrap();
    assert_eq!(report.rows.len(), 6);
    for group in report.rows.as_chunks::<3>().0 {
        for row in group {
            assert_eq!(
                row.successful_payloads, 3,
                "{} {:?}",
                row.symbology, row.medium
            );
            assert_eq!(row.incorrect_accepted_payloads, 0);
            assert_eq!(
                row.trial_outcomes
                    .iter()
                    .map(|trial| trial.seed)
                    .collect::<Vec<_>>(),
                [81, 82, 83]
            );
            assert!(
                row.trial_outcomes
                    .iter()
                    .all(|trial| trial.recovered && trial.rejection.is_none())
            );
            assert!(row.trial_outcomes.iter().all(|trial| {
                trial.cw_corrected_bytes.is_some() == row.symbology.starts_with("CW-")
                    && trial.cw_erased_bytes.is_some() == row.symbology.starts_with("CW-")
            }));
            assert!(
                (row.mean_projected_area_pixels - group[0].mean_projected_area_pixels).abs() < 1e-7
            );
        }
    }
}

#[test]
fn occlusion_placement_is_deterministic_and_domain_separated() {
    let seed = 81;
    let actual = occlusion_rectangle(320, 0.04, OcclusionPlacement::Uniform, seed).unwrap();
    assert_eq!(
        Some(actual),
        occlusion_rectangle(320, 0.04, OcclusionPlacement::Uniform, seed)
    );
    let mut mask_rng = ChaCha8Rng::seed_from_u64(seed ^ 0x4f43434c55534905);
    assert_eq!(
        (actual.x, actual.y),
        (
            mask_rng.random_range(0..=256),
            mask_rng.random_range(0..=256)
        )
    );
    for domain in [
        0x5041594c4f414403,
        0x4348414e4e454c01,
        0x504f534500000004,
        0x4341505455524502,
    ] {
        let mut other_rng = ChaCha8Rng::seed_from_u64(seed ^ domain);
        assert_ne!(
            (actual.x, actual.y),
            (
                other_rng.random_range(0..=256),
                other_rng.random_range(0..=256)
            )
        );
    }
}

#[test]
fn occlusion_rectangles_preserve_center_zero_area_and_bounds() {
    for placement in [OcclusionPlacement::Center, OcclusionPlacement::Uniform] {
        assert_eq!(occlusion_rectangle(65, 0.0, placement, 1), None);
        let mut image = RgbImage::from_pixel(65, 65, Rgb([17, 31, 47]));
        let untouched = image.clone();
        occlude(&mut image, None);
        assert_eq!(image, untouched);
        for seed in 1..=100 {
            let rectangle = occlusion_rectangle(65, 0.25, placement, seed).unwrap();
            assert_eq!((rectangle.width, rectangle.height), (33, 33));
            assert!(rectangle.x + rectangle.width <= 65);
            assert!(rectangle.y + rectangle.height <= 65);
            if placement == OcclusionPlacement::Center {
                assert_eq!((rectangle.x, rectangle.y), (16, 16));
            }
            let mut masked = untouched.clone();
            occlude(&mut masked, Some(rectangle));
            assert_eq!(
                masked
                    .pixels()
                    .filter(|pixel| **pixel == Rgb([245, 245, 245]))
                    .count(),
                33 * 33
            );
        }
    }
}

#[test]
fn uniform_masks_are_identical_across_matched_variants() {
    let report = compare(ComparisonConfig {
        cw_payload_bytes: 64,
        cw_cell_pixels: 8,
        modulation: crate::codec::Modulation::Luma10,
        qr_payload_bytes: 64,
        trials: 1,
        code_sides: vec![320],
        frame_side: 512,
        severities: vec![0.0],
        occlusion: 0.02,
        occlusion_placement: OcclusionPlacement::Uniform,
        perspective: 0.04,
        seed: 81,
        jpeg_quality: None,
        chroma420: false,
        illumination: 0.0,
    })
    .unwrap();
    let expected = occlusion_rectangle(320, 0.02, OcclusionPlacement::Uniform, 81);
    assert!(expected.is_some());
    for row in report.rows {
        assert_eq!(row.trial_outcomes[0].source_occlusion, expected);
    }
}

#[test]
fn confidence_bounds_do_not_turn_small_samples_into_certainty() {
    let range = wilson(10, 10);
    assert!(range[0] < 0.75);
    assert!((range[1] - 1.0).abs() < 1e-12);
    let range = wilson(0, 10);
    assert!(range[0] < 1e-12);
    assert!(range[1] > 0.25);
}
