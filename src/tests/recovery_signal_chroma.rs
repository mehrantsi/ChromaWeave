use super::*;

fn bits(decisions: Vec<(u16, [f32; 10])>) -> Vec<(u16, [u32; 10])> {
    decisions
        .into_iter()
        .map(|(s, c)| (s, c.map(f32::to_bits)))
        .collect()
}

fn legacy_interpolate(
    positions: &[usize],
    models: &[Option<Model>],
    x: usize,
    y: usize,
) -> Option<Model> {
    let interval = |value| {
        let lower = positions
            .partition_point(|&p| p <= value)
            .saturating_sub(1)
            .min(positions.len() - 2);
        let fraction =
            (value - positions[lower]) as f64 / (positions[lower + 1] - positions[lower]) as f64;
        (lower, (fraction * 32.0).round() / 32.0)
    };
    let (x, u) = interval(x);
    let (y, v) = interval(y);
    let side = positions.len();
    let weights = [
        (y * side + x, (1.0 - u) * (1.0 - v)),
        (y * side + x + 1, u * (1.0 - v)),
        ((y + 1) * side + x, (1.0 - u) * v),
        ((y + 1) * side + x + 1, u * v),
    ];
    let mut output = Model {
        coefficients: [0.0; 8],
        variance: 0.0,
    };
    for (index, weight) in weights {
        if weight == 0.0 {
            continue;
        }
        let model = models[index]?;
        for (a, b) in output.coefficients.iter_mut().zip(model.coefficients) {
            *a += weight * b;
        }
        output.variance += weight * model.variance;
    }
    Some(output)
}

#[test]
fn cached_interpolation_preserves_boundaries_and_missing_support() {
    for positions in [vec![0, 8, 16], vec![0, 16, 32, 33], vec![0, 23, 46, 69, 86]] {
        let side = positions.len();
        let limit = *positions.last().unwrap();
        let intervals: Vec<_> = (0..=limit)
            .map(|value| {
                let lower = positions
                    .partition_point(|&p| p <= value)
                    .saturating_sub(1)
                    .min(side - 2);
                let fraction = (value - positions[lower]) as f64
                    / (positions[lower + 1] - positions[lower]) as f64;
                (lower, (fraction * 32.0).round() / 32.0)
            })
            .collect();
        let original: Vec<_> = (0..side * side)
            .map(|node| {
                Some(Model {
                    coefficients: std::array::from_fn(|i| (node as f64 - 7.1) * (i as f64 + 0.31)),
                    variance: node as f64 + 0.47,
                })
            })
            .collect();
        let model_bits = |model: Option<Model>| {
            model.map(|m| (m.coefficients.map(f64::to_bits), m.variance.to_bits()))
        };
        for missing in 0..=original.len() {
            let mut models = original.clone();
            if missing < models.len() {
                models[missing] = None;
            }
            for y in 0..=limit {
                for x in 0..=limit {
                    assert_eq!(
                        model_bits(interpolate(&intervals, side, &models, x, y)),
                        model_bits(legacy_interpolate(&positions, &models, x, y)),
                        "positions {positions:?}, missing {missing}, cell ({x}, {y})"
                    );
                }
            }
        }
    }
}

#[test]
fn serial_and_parallel_chroma_preserve_every_decision_and_tail() {
    let random = |cell: usize| {
        let mut value = (cell as u64).wrapping_mul(0x9e3779b97f4a7c15);
        value ^= value >> 30;
        value = value.wrapping_mul(0xbf58476d1ce4e5b9);
        value ^ (value >> 27)
    };
    for grid in [64_usize, 96, 107, 108, 148, 212, 214] {
        let initial: Vec<Decision> = (0..grid * grid)
            .map(|cell| ((random(cell) & 1023) as u16, [0.73; 10]))
            .collect();
        let mask: Vec<_> = (0..grid * grid)
            .map(|cell| {
                let reserved = crate::integrated_layout::reserved(
                    grid as u32,
                    (cell % grid) as u32,
                    (cell / grid) as u32,
                );
                let hole = (grid / 3..2 * grid / 3).contains(&(cell % grid))
                    && (grid.div_ceil(4) - 14..grid.div_ceil(4) + 14).contains(&(cell / grid));
                !(reserved || hole)
            })
            .collect();
        let mut features = vec![[0.0; 12]; grid * grid];
        for (cell, feature) in features.iter_mut().enumerate() {
            let x = cell % grid;
            let y = cell / grid;
            let neighbors = [
                (y + grid - 1) % grid * grid + x,
                (y + 1) % grid * grid + x,
                y * grid + (x + grid - 1) % grid,
                y * grid + (x + 1) % grid,
            ];
            for axis in 0..2 {
                let bit = axis + 8;
                let label = |c: usize| 2.0 * f64::from((initial[c].0 >> bit) & 1) - 1.0;
                let interference = neighbors
                    .into_iter()
                    .zip([0.7, -1.2, 0.3, 0.5])
                    .map(|(n, c)| c * label(n))
                    .sum::<f64>();
                let signal = if cell.is_multiple_of(17) {
                    -label(cell)
                } else {
                    label(cell)
                };
                feature[4 + axis] = (2.7 + 6.0 * signal + interference) as f32;
            }
        }
        let serial = refine_with_workers(grid, &features, &initial, &mask, 1).unwrap();
        let parallel = refine_with_workers(grid, &features, &initial, &mask, 4).unwrap();
        assert!(parallel.iter().zip(&initial).any(|(a, b)| a.0 != b.0));
        assert_eq!(bits(serial), bits(parallel), "grid {grid}");
        let mut missing_second_axis = features.clone();
        for feature in &mut missing_second_axis {
            feature[5] = f32::NAN;
        }
        let serial_error =
            refine_with_workers(grid, &missing_second_axis, &initial, &mask, 1).unwrap_err();
        let parallel_error =
            refine_with_workers(grid, &missing_second_axis, &initial, &mask, 4).unwrap_err();
        assert_eq!(serial_error.to_string(), "no fitted chroma channel");
        assert_eq!(serial_error.to_string(), parallel_error.to_string());
        let absent = vec![false; grid * grid];
        assert!(refine_with_workers(grid, &features, &initial, &absent, 1).is_err());
        assert!(refine_with_workers(grid, &features, &initial, &absent, 4).is_err());
    }
}
