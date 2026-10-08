use super::super::{Model, inference::interpolate, refine};
use super::*;
#[test]
fn serial_and_parallel_passes_preserve_every_decision() {
    let mut state = 0x6377706172616c6c_u64;
    for grid in [64_usize, 96, 107, 108, 148, 212, 214] {
        let truth: Vec<u16> = (0..grid * grid)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state & 1023) as u16
            })
            .collect();
        let initial: Vec<_> = truth
            .iter()
            .enumerate()
            .map(|(cell, &symbol)| (symbol ^ if cell % 11 == 0 { 1 } else { 0 }, [0.5; 10]))
            .collect();
        let mut features: Vec<_> = truth
            .iter()
            .enumerate()
            .map(|(cell, &symbol)| {
                let mut features = [0.0; 12];
                for (q, value) in features[..4].iter_mut().enumerate() {
                    let gray = (symbol >> (q * 2)) & 3;
                    *value = 60.0 + 40.0 * f32::from(gray ^ (gray >> 1)) + (cell % 7) as f32 * 0.1;
                }
                features
            })
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
        for cell in (0..features.len()).step_by(403) {
            features[cell][cell % 4] = if cell % 2 == 0 {
                f32::NAN
            } else {
                f32::INFINITY
            };
        }
        let serial = refine_with_workers(grid, &features, &initial, &mask, 1);
        let parallel = refine_with_workers(grid, &features, &initial, &mask, 4);
        for cell in 0..features.len() {
            if !mask[cell] {
                assert_eq!(parallel[cell], initial[cell]);
            }
            for (quadrant, value) in features[cell][..4].iter().enumerate() {
                if !value.is_finite() {
                    assert_eq!(
                        (parallel[cell].0 >> (2 * quadrant)) & 3,
                        (initial[cell].0 >> (2 * quadrant)) & 3
                    );
                    assert_eq!(
                        parallel[cell].1[2 * quadrant..2 * quadrant + 2],
                        initial[cell].1[2 * quadrant..2 * quadrant + 2]
                    );
                }
            }
        }
        let bits = |values: Vec<(u16, [f32; 10])>| {
            values
                .into_iter()
                .map(|(symbol, confidence)| (symbol, confidence.map(f32::to_bits)))
                .collect::<Vec<_>>()
        };
        if grid <= 200 {
            assert_eq!(
                bits(refine(grid, &features, &initial, &mask)),
                bits(serial.clone())
            );
        }
        assert!(parallel.iter().zip(&initial).any(|(a, b)| a.0 != b.0));
        assert_eq!(bits(serial), bits(parallel), "grid {grid}");
        let absent = vec![false; grid * grid];
        assert_eq!(
            bits(refine_with_workers(grid, &features, &initial, &absent, 4)),
            bits(initial)
        );
    }
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
        (
            lower,
            (value - positions[lower]) as f64 / (positions[lower + 1] - positions[lower]) as f64,
        )
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
        coefficients: [0.0; 18],
        variance: 0.0,
    };
    for (index, weight) in weights {
        if weight <= f64::EPSILON {
            continue;
        }
        let model = models[index]?;
        for (out, value) in output.coefficients.iter_mut().zip(model.coefficients) {
            *out += weight * value;
        }
        output.variance += weight * model.variance;
    }
    Some(output)
}

#[test]
fn cached_intervals_preserve_missing_nodes_and_short_last_interval() {
    for positions in [vec![0, 8, 16], vec![0, 16, 32, 33], vec![0, 23, 46, 69, 86]] {
        let intervals: Vec<_> = (0..=*positions.last().unwrap())
            .map(|value| {
                let lower = positions
                    .partition_point(|&p| p <= value)
                    .saturating_sub(1)
                    .min(positions.len() - 2);
                (
                    lower,
                    (value - positions[lower]) as f64
                        / (positions[lower + 1] - positions[lower]) as f64,
                )
            })
            .collect();
        for missing in 0..4 {
            let models: Vec<_> = (0..positions.len().pow(2))
                .map(|node| {
                    if missing > 0 && node % (missing + 1) == 0 {
                        None
                    } else {
                        Some(Model {
                            coefficients: std::array::from_fn(|coefficient| {
                                (node * 37 + coefficient * 11) as f64 / 13.0 - 19.0
                            }),
                            variance: node as f64 / 7.0 + 1.0,
                        })
                    }
                })
                .collect();
            let bits = |model: Option<Model>| {
                model.map(|model| {
                    (
                        model.coefficients.map(f64::to_bits),
                        model.variance.to_bits(),
                    )
                })
            };
            for y in 0..intervals.len() {
                for x in 0..intervals.len() {
                    assert_eq!(
                        bits(interpolate(&intervals, positions.len(), &models, x, y)),
                        bits(legacy_interpolate(&positions, &models, x, y)),
                        "at {x},{y} missing {missing}"
                    );
                }
            }
        }
    }
}
