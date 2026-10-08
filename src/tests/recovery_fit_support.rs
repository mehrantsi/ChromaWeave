use super::*;
use rand::{Rng, SeedableRng};

#[test]
fn valid_neutral_model_is_distinct_from_absent_model() {
    let grid = 64;
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(0x737570706f7274);
    let initial: Vec<_> = (0..grid * grid)
        .map(|_| (rng.random_range(0..1024), [0.5; 10]))
        .collect();
    let mut known: Vec<_> = initial.iter().map(|&(symbol, _)| (1023, symbol)).collect();
    let mut features = vec![[0.0; 12]; grid * grid];
    for (cell, &(symbol, _)) in initial.iter().enumerate() {
        for (q, value) in features[cell].iter_mut().enumerate().take(4) {
            let gray = (symbol >> (2 * q)) & 3;
            *value = 80.0 + 18.0 * f32::from(gray ^ (gray >> 1));
        }
        features[cell][4] = 20.0 * sign(symbol, 8) as f32;
    }
    let cell = 32 * grid + 32;
    assert!(!crate::integrated_layout::reserved(grid as u32, 32, 32));
    known[cell].0 &= !((1 << 8) | (1 << 9));
    features[cell][4] = 0.0;
    let (evidence, support) = refine_with_support(grid, &features, &initial, &known);
    assert_ne!(support[cell] & (1 << 8), 0);
    assert!(evidence[cell][8].abs() < 1e-6);
    assert_eq!(support[cell] & (1 << 9), 0);
    assert_eq!(evidence[cell][9], 0.0);
    let unsupported = vec![(0, 0); grid * grid];
    let (evidence, support) = refine_with_support(grid, &features, &initial, &unsupported);
    assert!(support.iter().all(|&mask| mask == 0));
    assert!(evidence.iter().flatten().all(|&value| value == 0.0));
}
