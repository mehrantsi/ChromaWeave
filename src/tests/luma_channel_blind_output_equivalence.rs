use super::*;
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
        (lower, fraction)
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
        if weight <= f64::EPSILON {
            continue;
        }
        let model = models[index]?;
        for (coefficient, value) in output.coefficients.iter_mut().zip(model.coefficients) {
            *coefficient += weight * value;
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
                (lower, fraction)
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
