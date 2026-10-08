use super::*;

fn reference(grid: usize, observed: impl Fn(usize) -> Option<[f32; 4]>) -> LevelField {
    let step = (grid - 1).div_ceil(32).max(8);
    let mut positions: Vec<_> = (0..grid - 1).step_by(step).collect();
    positions.push(grid - 1);
    let radius = step * 3 / 2;
    let mut levels = Vec::new();
    let mut samples = Vec::new();
    for &y in &positions {
        for &x in &positions {
            samples.clear();
            for yy in y.saturating_sub(radius)..(y + radius + 1).min(grid) {
                for xx in x.saturating_sub(radius)..(x + radius + 1).min(grid) {
                    if xx.abs_diff(x).pow(2) + yy.abs_diff(y).pow(2) >= radius.pow(2) {
                        continue;
                    }
                    if let Some(values) = observed(yy * grid + xx) {
                        samples.extend(values.into_iter().filter(|value| value.is_finite()));
                    }
                }
            }
            levels.push(fit(&mut samples).unwrap_or(NOMINAL));
        }
    }
    let intervals = axis_intervals(&positions, grid);
    LevelField {
        positions,
        intervals,
        levels,
    }
}

fn reference_at(field: &LevelField, x: usize, y: usize) -> [f32; 4] {
    let interval = |value| {
        let lower = field
            .positions
            .partition_point(|&p| p <= value)
            .saturating_sub(1)
            .min(field.positions.len() - 2);
        let fraction = (value - field.positions[lower]) as f32
            / (field.positions[lower + 1] - field.positions[lower]) as f32;
        (lower, fraction)
    };
    let (x, u) = interval(x);
    let (y, v) = interval(y);
    let side = field.positions.len();
    std::array::from_fn(|rank| {
        (1.0 - v)
            * ((1.0 - u) * field.levels[y * side + x][rank]
                + u * field.levels[y * side + x + 1][rank])
            + v * ((1.0 - u) * field.levels[(y + 1) * side + x][rank]
                + u * field.levels[(y + 1) * side + x + 1][rank])
    })
}

#[test]
fn cached_intervals_preserve_boundary_and_nonfinite_lookup_bits() {
    for grid in [2usize, 8, 9, 10, 95, 96, 97, 256, 257, 258, 354] {
        let step = (grid - 1).div_ceil(32).max(8);
        let mut positions: Vec<_> = (0..grid - 1).step_by(step).collect();
        positions.push(grid - 1);
        for nonfinite in [false, true] {
            let levels = (0..positions.len().pow(2))
                .map(|node| {
                    std::array::from_fn(|rank| match (nonfinite, node % 11, rank) {
                        (true, 0, 0) => f32::NAN,
                        (true, 1, 1) => f32::INFINITY,
                        (true, 2, 2) => f32::NEG_INFINITY,
                        _ => ((node * 37 + rank * 13) % 97) as f32 / 11.0 - 4.0,
                    })
                })
                .collect();
            let field = LevelField {
                intervals: axis_intervals(&positions, grid),
                positions: positions.clone(),
                levels,
            };
            for y in 0..grid {
                for x in 0..grid {
                    assert_eq!(
                        field.at(x, y).map(f32::to_bits),
                        reference_at(&field, x, y).map(f32::to_bits),
                        "grid={grid}, x={x}, y={y}, nonfinite={nonfinite}"
                    );
                }
            }
        }
    }
}

#[test]
fn preserves_serial_fields_and_lookups_with_missing_or_invalid_observations() {
    for grid in [2, 40, 56, 96, 144, 354] {
        for missing in [false, true] {
            let observed = |cell: usize| {
                if missing || cell.is_multiple_of(23) {
                    return None;
                }
                let (x, y) = (cell % grid, cell / grid);
                if x > grid * 3 / 4 && y > grid * 3 / 4 {
                    return Some([2.0; 4]);
                }
                let mut values = [-3.2, -0.9, 1.4, 3.6].map(|value| {
                    value + x as f32 / grid as f32 * 0.3 + ((cell * 37) % 13) as f32 * 0.005
                });
                if cell.is_multiple_of(47) {
                    values[2] = f32::NAN;
                }
                if cell.is_multiple_of(89) {
                    values[3] = f32::INFINITY;
                }
                Some(values)
            };
            let expected = reference(grid, observed);
            let calls = std::cell::Cell::new(0);
            let actual = LevelField::estimate(grid, |cell| {
                calls.set(calls.get() + 1);
                observed(cell)
            });
            assert_eq!(calls.get(), grid * grid);
            assert_eq!(actual.positions, expected.positions);
            assert_eq!(
                actual
                    .levels
                    .iter()
                    .map(|value| value.map(f32::to_bits))
                    .collect::<Vec<_>>(),
                expected
                    .levels
                    .iter()
                    .map(|value| value.map(f32::to_bits))
                    .collect::<Vec<_>>()
            );
            for y in 0..grid {
                for x in 0..grid {
                    assert_eq!(
                        actual.at(x, y).map(f32::to_bits),
                        reference_at(&expected, x, y).map(f32::to_bits),
                        "grid={grid}, x={x}, y={y}"
                    );
                }
            }
        }
    }
}
