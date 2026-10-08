use super::*;

#[test]
fn sparse_certificates_preserve_unsupported_alphabet_decisions() {
    use rand::{Rng, SeedableRng};
    let grid = 64;
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(0x73656564);
    let features: Vec<_> = (0..grid * grid)
        .map(|_| {
            let mut feature = [0.0; 12];
            for value in &mut feature[..4] {
                *value = 44.0 + 56.0 * rng.random_range(0..4) as f32;
            }
            for value in &mut feature[4..6] {
                *value = if rng.random::<bool>() { 24.0 } else { -24.0 };
            }
            feature
        })
        .collect();
    let mask: Vec<_> = (0..grid * grid)
        .map(|cell| {
            !crate::integrated_layout::reserved(
                grid as u32,
                (cell % grid) as u32,
                (cell / grid) as u32,
            )
        })
        .collect();
    let seed = initial::classify(grid, &features, &mask).unwrap();
    let cell = 32 * grid + 32;
    let mut known = vec![(0, 0); grid * grid];
    known[cell] = (1023, seed[cell].0 ^ 1023);
    let decisions =
        initialize_from_certificates(grid, &features, &features, &mask, &known).unwrap();
    assert_eq!(decisions[cell], (known[cell].1, [1.0; 10]));
    for (index, (actual, expected)) in decisions.iter().zip(&seed).enumerate() {
        if index != cell {
            assert_eq!(actual, expected, "cell {index}");
        }
    }
}

#[test]
fn neutral_fitted_evidence_does_not_reuse_a_confident_seed() {
    let seed = (1023, [0.9; 10]);
    let (symbol, confidence) = decision_from_evidence([0.0; 10], 1 << 8, seed, (0, 0));
    assert_eq!(symbol, 1023 & !(1 << 8));
    assert_eq!(confidence[8], 0.0);
    assert_eq!(confidence[9], 0.9);
}

#[test]
fn absent_models_preserve_seed_and_certified_bits_override_both_sources() {
    let seed = (1 << 0, [0.7; 10]);
    let mut fitted = [-2.0; 10];
    fitted[1] = 2.0;
    let (symbol, confidence) = decision_from_evidence(
        fitted,
        (1 << 0) | (1 << 1),
        seed,
        ((1 << 0) | (1 << 1), 1 << 0),
    );
    assert_eq!(symbol, 1 << 0);
    assert_eq!(confidence[0], 1.0);
    assert_eq!(confidence[1], 1.0);
    assert!(confidence[2..].iter().all(|&value| value == 0.7));
    assert_eq!(decision_from_evidence(fitted, 0, seed, (0, 0)), seed);
}
