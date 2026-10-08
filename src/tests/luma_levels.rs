use super::*;

#[test]
fn sparse_or_missing_levels_do_not_create_an_alphabet() {
    for alphabet in [&[-3.0][..], &[-3.0, 3.0], &[-3.0, -1.0, 1.0]] {
        let mut samples: Vec<_> = (0..512)
            .map(|i| alphabet[i % alphabet.len()] + (i % 7) as f32 * 0.01)
            .collect();
        assert!(fit(&mut samples).is_none());
    }
}

#[test]
fn learns_unevenly_populated_nonlinear_levels() {
    let levels = [-3.3, -0.8, 1.8, 3.7];
    let mut samples: Vec<_> = [30, 90, 500, 50]
        .into_iter()
        .enumerate()
        .flat_map(|(rank, count)| (0..count).map(move |i| levels[rank] + (i % 5) as f32 * 0.01))
        .collect();
    let fitted = fit(&mut samples).unwrap();
    for (a, b) in fitted.into_iter().zip(levels) {
        assert!((a - b).abs() < 0.05);
    }
}

#[test]
fn retains_rare_shifted_levels_at_minimum_cluster_size() {
    let levels = [-2.0, 0.1, 3.5, 6.8];
    let mut samples: Vec<_> = [8, 8, 8, 1000]
        .into_iter()
        .enumerate()
        .flat_map(|(rank, count)| std::iter::repeat_n(levels[rank], count))
        .collect();
    let fitted = fit(&mut samples).unwrap();
    for (observed, expected) in fitted.into_iter().zip(levels) {
        assert!((observed - expected).abs() < 0.05);
    }
}

#[test]
fn learns_shifted_scaled_and_nonlinear_levels() {
    for levels in [
        [0.0, 4.0, 8.0, 12.0],
        [-20.0, -12.0, -4.0, 4.0],
        [-2.0, 0.1, 3.5, 6.8],
    ] {
        let mut samples: Vec<_> = (0..1024)
            .map(|i| levels[i % 4] + ((i / 4 % 7) as f32 - 3.0) * 0.03)
            .collect();
        let fitted = fit(&mut samples).unwrap();
        for (observed, expected) in fitted.into_iter().zip(levels) {
            assert!((observed - expected).abs() < 0.05);
        }
    }
}

#[test]
fn noisy_missing_levels_do_not_create_an_alphabet() {
    for alphabet in [&[2.0][..], &[2.0, 9.0], &[2.0, 5.0, 9.0]] {
        let mut samples: Vec<_> = (0..1024)
            .map(|i| alphabet[i % alphabet.len()] + ((i / alphabet.len() % 9) as f32 - 4.0) * 0.08)
            .collect();
        assert!(fit(&mut samples).is_none());
    }
}
