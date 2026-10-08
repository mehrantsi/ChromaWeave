use super::*;

fn symbol(ranks: [u8; 4]) -> u16 {
    ranks.into_iter().enumerate().fold(768, |word, (q, rank)| {
        word | (u16::from(rank ^ (rank >> 1)) << (2 * q))
    })
}

#[test]
fn parallel_fits_preserve_exact_decisions_and_confidence() {
    let bits = |decisions: Vec<(u16, [f32; 10])>| {
        decisions
            .into_iter()
            .map(|(s, c)| (s, c.map(f32::to_bits)))
            .collect::<Vec<_>>()
    };
    for grid in [64, 107, 148, 212, 214] {
        let side = grid * 2;
        let ranks: Vec<_> = (0..side * side)
            .map(|i| {
                let mut value = (i as u64).wrapping_mul(0x9e3779b97f4a7c15);
                value ^= value >> 30;
                value = value.wrapping_mul(0xbf58476d1ce4e5b9);
                (value >> 62) as u8
            })
            .collect();
        let levels = [40.0f32, 92.0, 158.0, 218.0];
        let mut features = vec![[0.0; 12]; grid * grid];
        let mut initial = vec![(0, [0.8; 10]); grid * grid];
        let mask: Vec<_> = (0..grid * grid)
            .map(|cell| {
                !(crate::integrated_layout::reserved(
                    grid as u32,
                    (cell % grid) as u32,
                    (cell / grid) as u32,
                ) || (grid / 3..grid / 2).contains(&(cell % grid))
                    && (grid / 3..grid / 2).contains(&(cell / grid)))
            })
            .collect();
        for cell in 0..grid * grid {
            let mut selected = [0; 4];
            for (q, rank) in selected.iter_mut().enumerate() {
                let x = cell % grid * 2 + q % 2;
                let y = cell / grid * 2 + q / 2;
                let index = y * side + x;
                let adjacent = [
                    y * side + (x + 1).min(side - 1),
                    y * side + x.saturating_sub(1),
                    (y + 1).min(side - 1) * side + x,
                    y.saturating_sub(1) * side + x,
                ];
                let mut value = levels[usize::from(ranks[index])]
                    + adjacent
                        .into_iter()
                        .zip([-1.5, 2.0, 0.0, 6.0])
                        .map(|(i, c)| (f32::from(ranks[i]) * 2.0 - 3.0) * c)
                        .sum::<f32>();
                if index % 997 == 0 {
                    value = (levels[0] + levels[1]) * 0.5;
                }
                features[cell][q] = value;
                *rank = (0..4)
                    .min_by(|&a, &b| {
                        (value - levels[a])
                            .abs()
                            .total_cmp(&(value - levels[b]).abs())
                    })
                    .unwrap() as u8;
            }
            initial[cell].0 = symbol(selected);
        }
        for cell in (0..features.len()).step_by(403) {
            features[cell][cell % 4] = if cell % 2 == 0 {
                f32::NAN
            } else {
                f32::INFINITY
            };
        }
        let serial = refine_with_workers(grid, &features, &initial, &mask, 1);
        let parallel = refine_with_workers(grid, &features, &initial, &mask, 4);
        assert_ne!(parallel, initial);
        assert_eq!(bits(serial), bits(parallel));
    }
}

#[test]
fn reused_residuals_match_baseline_fit_bits() {
    for outliers in [false, true] {
        let samples: Vec<_> = (0..256)
            .map(|index| {
                let mut state = (index as u64 + 1).wrapping_mul(0x9e3779b97f4a7c15);
                let neighbors = std::array::from_fn(|_| {
                    state ^= state >> 30;
                    state = state.wrapping_mul(0xbf58476d1ce4e5b9);
                    (state >> 62) as f64 * 2.0 - 3.0
                });
                let rank = index % 4;
                let value = [40.25, 92.5, 158.75, 218.0][rank]
                    + neighbors
                        .iter()
                        .zip([-1.5, 2.0, 0.0, 6.0])
                        .map(|(neighbor, coefficient)| neighbor * coefficient)
                        .sum::<f64>()
                    + (index % 5) as f64 * 0.125
                    + if outliers && index % 23 == 0 {
                        70.0
                    } else {
                        0.0
                    };
                Sample {
                    rank,
                    neighbors,
                    value,
                }
            })
            .collect();
        let initial = solve(&samples, None).unwrap();
        let mut residuals: Vec<_> = samples
            .iter()
            .map(|sample| residual(sample, &initial))
            .collect();
        let center = median(&mut residuals);
        for value in &mut residuals {
            *value = (*value - center).abs();
        }
        let scale = (1.4826 * median(&mut residuals)).max(1.0);
        let weights: Vec<_> = samples
            .iter()
            .map(|sample| (2.5 * scale / residual(sample, &initial).abs().max(1e-6)).min(1.0))
            .collect();
        let coefficients = solve(&samples, Some(&weights)).unwrap();
        let variance = samples
            .iter()
            .zip(&weights)
            .map(|(sample, weight)| weight * residual(sample, &coefficients).powi(2))
            .sum::<f64>()
            / weights.iter().sum::<f64>();
        let actual = fit(&samples, &mut Vec::new(), &mut Vec::new()).unwrap();
        assert_eq!(
            actual.coefficients.map(f64::to_bits),
            coefficients.map(f64::to_bits)
        );
        assert_eq!(actual.variance.to_bits(), variance.to_bits());
    }
}

#[test]
fn selected_median_matches_full_sort() {
    for mut values in [
        vec![9.0, 1.0, 3.0],
        vec![9.0, 1.0, 4.0, 2.0],
        vec![-1.0, 0.0, 0.0, 1.0],
        vec![f64::NEG_INFINITY, 8.0, -3.0, f64::INFINITY],
    ] {
        let mut sorted = values.clone();
        sorted.sort_unstable_by(f64::total_cmp);
        let expected = (sorted[(sorted.len() - 1) / 2] + sorted[sorted.len() / 2]) * 0.5;
        assert_eq!(median(&mut values), expected);
    }
}

#[test]
fn missing_levels_and_structured_patterns_preserve_initial_decisions() {
    let grid = 40;
    for alphabet in [1, 2, 3, 4] {
        let mut features = vec![[0.0; 12]; grid * grid];
        let mut initial = Vec::new();
        for (cell, feature) in features.iter_mut().enumerate() {
            let ranks = std::array::from_fn(|q| ((cell % grid * 2 + q % 2) % alphabet) as u8);
            for q in 0..4 {
                feature[q] = 44.0 + 56.0 * f32::from(ranks[q]);
            }
            initial.push((symbol(ranks), [0.73; 10]));
        }
        assert_eq!(
            refine(grid, &features, &initial, &vec![true; grid * grid]),
            initial
        );
    }
}

#[test]
fn native_full_alphabet_and_chroma_are_preserved() {
    let grid = 40;
    let mut features = vec![[0.0; 12]; grid * grid];
    let initial: Vec<_> = features
        .iter_mut()
        .enumerate()
        .map(|(cell, feature)| {
            let ranks = std::array::from_fn(|q| ((cell * 73 + q * 19 + cell / 7) % 4) as u8);
            for q in 0..4 {
                feature[q] = 44.0 + 56.0 * f32::from(ranks[q]);
            }
            (symbol(ranks), [0.81; 10])
        })
        .collect();
    let output = refine(grid, &features, &initial, &vec![true; grid * grid]);
    for (actual, expected) in output.iter().zip(&initial) {
        assert_eq!(actual.0, expected.0);
        assert_eq!(actual.1[8..], expected.1[8..]);
    }
}

#[test]
fn learns_asymmetric_neighbor_interference_from_decisions() {
    let grid = 53;
    let side = grid * 2;
    let expected: Vec<u8> = (0..side * side)
        .map(|i| {
            let mut v = (i as u64).wrapping_mul(0x9e3779b97f4a7c15);
            v ^= v >> 30;
            v = v.wrapping_mul(0xbf58476d1ce4e5b9);
            (v >> 62) as u8
        })
        .collect();
    let levels = [40.0f32, 92.0, 158.0, 218.0];
    let mut features = vec![[0.0; 12]; grid * grid];
    let mut initial = vec![(0, [0.8; 10]); grid * grid];
    for cell in 0..grid * grid {
        let mut ranks = [0u8; 4];
        for q in 0..4 {
            let x = cell % grid * 2 + q % 2;
            let y = cell / grid * 2 + q / 2;
            let index = y * side + x;
            let neighbors = [
                y * side + (x + 1).min(side - 1),
                y * side + x.saturating_sub(1),
                (y + 1).min(side - 1) * side + x,
                y.saturating_sub(1) * side + x,
            ];
            let interference = neighbors
                .into_iter()
                .zip([-1.5, 2.0, 0.0, 6.0])
                .map(|(i, c)| (f32::from(expected[i]) * 2.0 - 3.0) * c)
                .sum::<f32>();
            let value = levels[usize::from(expected[index])] + interference;
            features[cell][q] = value;
            ranks[q] = (0..4)
                .min_by(|&a, &b| {
                    (value - levels[a])
                        .abs()
                        .total_cmp(&(value - levels[b]).abs())
                })
                .unwrap() as u8;
        }
        initial[cell].0 = symbol(ranks);
    }
    let output = refine(grid, &features, &initial, &vec![true; grid * grid]);
    let errors = |decisions: &[(u16, [f32; 10])]| {
        (0..grid * grid)
            .flat_map(|cell| (0..4).map(move |q| (cell, q)))
            .filter(|&(cell, q)| {
                let x = cell % grid * 2 + q % 2;
                let y = cell / grid * 2 + q / 2;
                let gray = ((decisions[cell].0 >> (q * 2)) & 3) as u8;
                x > 2
                    && y > 2
                    && x + 3 < side
                    && y + 3 < side
                    && (gray ^ (gray >> 1)) != expected[y * side + x]
            })
            .count()
    };
    let before = errors(&initial);
    let after = errors(&output);
    assert!(before > 20, "{before}");
    assert!(after < before / 4, "{before} -> {after}");
    assert!(
        output
            .iter()
            .zip(initial)
            .all(|(after, before)| after.1[8..] == before.1[8..])
    );
}
