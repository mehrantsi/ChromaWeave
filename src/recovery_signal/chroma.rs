use anyhow::{Result, ensure};

use super::{Decision, field_workers};

const BASIS_COUNT: usize = 8;
const OFFSET: usize = 0;
const OWN_SIGN: usize = 1;
const INTERFERENCE_START: usize = 2;

#[derive(Clone, Copy)]
struct Model {
    coefficients: [f64; BASIS_COUNT],
    variance: f64,
}

pub(super) fn refine(
    grid: usize,
    features: &[[f32; 12]],
    initial: &[Decision],
    mask: &[bool],
) -> Result<Vec<Decision>> {
    refine_with_workers(grid, features, initial, mask, field_workers(grid))
}

fn refine_with_workers(
    grid: usize,
    features: &[[f32; 12]],
    initial: &[Decision],
    mask: &[bool],
    workers: usize,
) -> Result<Vec<Decision>> {
    let mut output = initial.to_vec();
    let mut valid = vec![false; mask.len()];
    for y in 1..grid - 1 {
        for x in 1..grid - 1 {
            let cell = y * grid + x;
            valid[cell] = [cell, cell - grid, cell + grid, cell - 1, cell + 1]
                .into_iter()
                .all(|i| mask[i]);
        }
    }
    let luma: Vec<[f64; 2]> = initial
        .iter()
        .map(|&(symbol, _)| {
            let ranks: [f64; 4] = std::array::from_fn(|q| {
                let gray = (symbol >> (2 * q)) & 3;
                f64::from(gray ^ (gray >> 1))
            });
            [
                ranks.iter().sum::<f64>() / 4.0 - 1.5,
                ranks.iter().map(|r| r * r).sum::<f64>() / 4.0 - 3.5,
            ]
        })
        .collect();
    let mut positions: Vec<_> = (0..grid - 1).step_by(16).collect();
    positions.push(grid - 1);
    let intervals: Vec<_> = (0..grid)
        .map(|value| {
            let lower = positions
                .partition_point(|&p| p <= value)
                .saturating_sub(1)
                .min(positions.len() - 2);
            let fraction = (value - positions[lower]) as f64
                / (positions[lower + 1] - positions[lower]) as f64;
            (lower, (fraction * 32.0).round() / 32.0)
        })
        .collect();

    let supports: Vec<Vec<usize>> = positions
        .iter()
        .flat_map(|&y| positions.iter().map(move |&x| (x, y)))
        .map(|(x, y)| {
            let mut support = Vec::new();
            for yy in y.saturating_sub(24)..(y + 25).min(grid) {
                for xx in x.saturating_sub(24)..(x + 25).min(grid) {
                    let cell = yy * grid + xx;
                    if valid[cell] && xx.is_multiple_of(2) && yy.is_multiple_of(2) {
                        support.push(cell);
                    }
                }
            }
            support
        })
        .collect();
    let nodes_per_worker = positions.len() * positions.len().div_ceil(workers);
    for axis in [4, 5] {
        let bit = axis + 4;
        let mut labels: Vec<_> = output
            .iter()
            .map(|&(s, _)| 2.0 * f64::from((s >> bit) & 1) - 1.0)
            .collect();
        for _ in 0..5 {
            let previous_labels = labels.clone();
            let basis: Vec<[f64; BASIS_COUNT]> = (0..mask.len())
                .map(|cell| {
                    let x = cell % grid;
                    let y = cell / grid;
                    [
                        1.0,
                        labels[cell],
                        labels[(y + grid - 1) % grid * grid + x],
                        labels[(y + 1) % grid * grid + x],
                        labels[y * grid + (x + grid - 1) % grid],
                        labels[y * grid + (x + 1) % grid],
                        luma[cell][0],
                        luma[cell][1],
                    ]
                })
                .collect();
            let mut models = vec![None; supports.len()];
            std::thread::scope(|scope| {
                for (chunk, models) in models.chunks_mut(nodes_per_worker).enumerate() {
                    let supports = &supports[chunk * nodes_per_worker..][..models.len()];
                    let basis = &basis;
                    let mut fit_models = move || {
                        let mut rows = Vec::new();
                        let mut values = Vec::new();
                        for (model, support) in models.iter_mut().zip(supports) {
                            rows.clear();
                            values.clear();
                            for &cell in support {
                                rows.push(basis[cell]);
                                values.push(f64::from(features[cell][axis]));
                            }
                            *model = fit(&rows, &values);
                        }
                    };
                    if workers == 1 {
                        fit_models();
                    } else {
                        scope.spawn(fit_models);
                    }
                }
            });
            ensure!(
                models.iter().any(Option::is_some),
                "no fitted chroma channel"
            );
            let rows_per_worker = grid.div_ceil(workers);
            std::thread::scope(|scope| {
                for (chunk, (decisions, labels)) in output
                    .chunks_mut(rows_per_worker * grid)
                    .zip(labels.chunks_mut(rows_per_worker * grid))
                    .enumerate()
                {
                    let start_cell = chunk * rows_per_worker * grid;
                    let models = &models;
                    let positions = &positions;
                    let intervals = &intervals;
                    let basis = &basis;
                    let classify = move || {
                        for (offset, (decision, label)) in
                            decisions.iter_mut().zip(labels).enumerate()
                        {
                            let cell = start_cell + offset;
                            let Some(model) = interpolate(
                                intervals,
                                positions.len(),
                                models,
                                cell % grid,
                                cell / grid,
                            ) else {
                                decision.1[bit] = 0.0;
                                continue;
                            };
                            let interference = model.coefficients[OFFSET]
                                + (INTERFERENCE_START..BASIS_COUNT)
                                    .map(|i| model.coefficients[i] * basis[cell][i])
                                    .sum::<f64>();
                            let signal = f64::from(features[cell][axis]) - interference;
                            *label = if signal > 0.0 { 1.0 } else { -1.0 };
                            let positive = (signal - model.coefficients[OWN_SIGN]).powi(2);
                            let negative = (signal + model.coefficients[OWN_SIGN]).powi(2);
                            decision.0 =
                                (decision.0 & !(1 << bit)) | (u16::from(signal > 0.0) << bit);
                            decision.1[bit] = ((positive - negative).abs()
                                / (positive + negative + 2.0 * model.variance))
                                as f32;
                        }
                    };
                    if workers == 1 {
                        classify();
                    } else {
                        scope.spawn(classify);
                    }
                }
            });
            if labels == previous_labels {
                break;
            }
        }
    }
    Ok(output)
}

fn fit(rows: &[[f64; BASIS_COUNT]], values: &[f64]) -> Option<Model> {
    if rows.len() < BASIS_COUNT {
        return None;
    }
    let coefficients = solve(rows, values, None)?;
    let residuals: Vec<_> = rows
        .iter()
        .zip(values)
        .map(|(row, value)| {
            value
                - row
                    .iter()
                    .zip(coefficients)
                    .map(|(a, b)| a * b)
                    .sum::<f64>()
        })
        .collect();
    let center = median(&residuals);
    let scale = (1.4826
        * median(
            &residuals
                .iter()
                .map(|r| (r - center).abs())
                .collect::<Vec<_>>(),
        ))
    .max(0.5);
    let weights: Vec<_> = residuals
        .iter()
        .map(|r| (2.5 * scale / r.abs().max(1e-6)).min(1.0))
        .collect();
    let coefficients = solve(rows, values, Some(&weights))?;
    let variance = rows
        .iter()
        .zip(values)
        .zip(&weights)
        .map(|((row, value), weight)| {
            weight
                * (value
                    - row
                        .iter()
                        .zip(coefficients)
                        .map(|(a, b)| a * b)
                        .sum::<f64>())
                .powi(2)
        })
        .sum::<f64>()
        / weights.iter().sum::<f64>();
    variance.is_finite().then_some(Model {
        coefficients,
        variance,
    })
}

fn median(values: &[f64]) -> f64 {
    let mut values = values.to_vec();
    let odd = !values.len().is_multiple_of(2);
    let middle = values.len() / 2;
    let (lower, upper, _) = values.select_nth_unstable_by(middle, f64::total_cmp);
    let lower = if odd {
        *upper
    } else {
        lower.iter().copied().max_by(f64::total_cmp).unwrap()
    };
    (lower + *upper) * 0.5
}

fn solve(
    rows: &[[f64; BASIS_COUNT]],
    values: &[f64],
    weights: Option<&[f64]>,
) -> Option<[f64; BASIS_COUNT]> {
    let mut system = [[0.0; BASIS_COUNT + 1]; BASIS_COUNT];
    for (index, (row, value)) in rows.iter().zip(values).enumerate() {
        let weight = weights.map_or(1.0, |w| w[index]);
        for i in 0..BASIS_COUNT {
            let weighted = weight * row[i];
            system[i][BASIS_COUNT] += weighted * value;
            for j in 0..=i {
                system[i][j] += weighted * row[j];
            }
        }
    }
    for i in 0..BASIS_COUNT {
        let (above, remaining) = system.split_at_mut(i);
        for (j, row) in above.iter_mut().enumerate() {
            row[i] = remaining[0][j];
        }
    }
    let scales: [f64; BASIS_COUNT] = std::array::from_fn(|i| system[i][i].sqrt());
    if scales.iter().any(|s| !s.is_finite() || *s <= f64::EPSILON) {
        return None;
    }
    for i in 0..BASIS_COUNT {
        for j in 0..BASIS_COUNT {
            system[i][j] /= scales[i] * scales[j];
        }
        system[i][BASIS_COUNT] /= scales[i];
    }
    for column in 0..BASIS_COUNT {
        let pivot = (column..BASIS_COUNT)
            .max_by(|&a, &b| system[a][column].abs().total_cmp(&system[b][column].abs()))?;
        if system[pivot][column].abs() < 1e-12 {
            return None;
        }
        system.swap(column, pivot);
        let divisor = system[column][column];
        for value in &mut system[column][column..] {
            *value /= divisor;
        }
        let row = system[column];
        for (index, other) in system.iter_mut().enumerate() {
            if index == column {
                continue;
            }
            let factor = other[column];
            for (value, pivot) in other[column..].iter_mut().zip(&row[column..]) {
                *value -= factor * pivot;
            }
        }
    }
    let coefficients = std::array::from_fn(|i| system[i][BASIS_COUNT] / scales[i]);
    coefficients
        .iter()
        .all(|v| v.is_finite())
        .then_some(coefficients)
}

fn interpolate(
    intervals: &[(usize, f64)],
    side: usize,
    models: &[Option<Model>],
    x: usize,
    y: usize,
) -> Option<Model> {
    let (x, u) = intervals[x];
    let (y, v) = intervals[y];
    let weights = [
        (y * side + x, (1.0 - u) * (1.0 - v)),
        (y * side + x + 1, u * (1.0 - v)),
        ((y + 1) * side + x, (1.0 - u) * v),
        ((y + 1) * side + x + 1, u * v),
    ];
    let mut output = Model {
        coefficients: [0.0; BASIS_COUNT],
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

#[cfg(test)]
#[path = "../tests/recovery_signal_chroma.rs"]
mod output_equivalence_tests;
