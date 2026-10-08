#[derive(Clone, Copy)]
struct Model {
    coefficients: [f64; 8],
    variance: f64,
}

struct Sample {
    rank: usize,
    neighbors: [f64; 4],
    value: f64,
}

pub(crate) fn refine(
    grid: usize,
    features: &[[f32; 12]],
    initial: &[(u16, [f32; 10])],
    data_mask: &[bool],
) -> Vec<(u16, [f32; 10])> {
    let workers = if grid >= 96 {
        std::thread::available_parallelism().map_or(1, |value| value.get().min(4))
    } else {
        1
    };
    refine_with_workers(grid, features, initial, data_mask, workers)
}

fn refine_with_workers(
    grid: usize,
    features: &[[f32; 12]],
    initial: &[(u16, [f32; 10])],
    data_mask: &[bool],
    workers: usize,
) -> Vec<(u16, [f32; 10])> {
    let side = grid * 2;
    let mut values = vec![0.0; side * side];
    let mut ranks = vec![0u8; values.len()];
    let mut valid = vec![false; values.len()];
    for cell in 0..grid * grid {
        for (quadrant, value) in features[cell].iter().enumerate().take(4) {
            let index = (cell / grid * 2 + quadrant / 2) * side + cell % grid * 2 + quadrant % 2;
            values[index] = f64::from(*value);
            let gray = ((initial[cell].0 >> (quadrant * 2)) & 3) as u8;
            ranks[index] = gray ^ (gray >> 1);
            valid[index] = data_mask[cell] && values[index].is_finite();
        }
    }
    let neighbors = |index: usize| [index + 1, index - 1, index + side, index - side];
    let fitting: Vec<_> = (0..values.len())
        .map(|index| {
            index >= side
                && index + side < values.len()
                && index % side > 0
                && index % side + 1 < side
                && valid[index]
                && neighbors(index).into_iter().all(|other| valid[other])
        })
        .collect();
    let step = (side - 1).div_ceil(32).max(16);
    let mut positions: Vec<_> = (0..side - 1).step_by(step).collect();
    positions.push(side - 1);
    let intervals: Vec<_> = (0..side)
        .map(|value| {
            let lower = positions
                .partition_point(|&p| p <= value)
                .saturating_sub(1)
                .min(positions.len() - 2);
            let fraction = (value - positions[lower]) as f64
                / (positions[lower + 1] - positions[lower]) as f64;
            (lower, fraction)
        })
        .collect();
    let radius = step * 3 / 2;
    let mut output = initial.to_vec();
    let support: Vec<Vec<usize>> = positions
        .iter()
        .flat_map(|&y| positions.iter().map(move |&x| (x, y)))
        .map(|(x, y)| {
            let mut indices = Vec::new();
            let y_start = y.saturating_sub(radius);
            let y_end = (y + radius + 1).min(side);
            let x_start = x.saturating_sub(radius);
            let x_end = (x + radius + 1).min(side);
            let first_cell_y = (y_start / 2).div_ceil(2) * 2;
            let first_cell_x = (x_start / 2).div_ceil(2) * 2;
            for cell_y in (first_cell_y..y_end.div_ceil(2)).step_by(2) {
                for quadrant_y in 0..2 {
                    let yy = cell_y * 2 + quadrant_y;
                    if yy < y_start || yy >= y_end {
                        continue;
                    }
                    for cell_x in (first_cell_x..x_end.div_ceil(2)).step_by(2) {
                        for quadrant_x in 0..2 {
                            let xx = cell_x * 2 + quadrant_x;
                            if xx < x_start
                                || xx >= x_end
                                || xx.abs_diff(x).pow(2) + yy.abs_diff(y).pow(2) >= radius.pow(2)
                            {
                                continue;
                            }
                            let index = yy * side + xx;
                            if fitting[index] {
                                indices.push(index);
                            }
                        }
                    }
                }
            }
            indices
        })
        .collect();
    let mut neighbor_context = vec![[0.0; 4]; values.len()];
    let mut next = ranks.clone();
    let mut models = vec![None; support.len()];
    for _ in 0..5 {
        for (index, context) in neighbor_context.iter_mut().enumerate() {
            if fitting[index] {
                *context = neighbors(index).map(|i| f64::from(ranks[i]) * 2.0 - 3.0);
            }
        }
        let fit_nodes = |first: usize, nodes: &mut [Option<Model>]| {
            let mut samples = Vec::new();
            let mut residuals = Vec::new();
            let mut weights = Vec::new();
            for (offset, model) in nodes.iter_mut().enumerate() {
                samples.clear();
                samples.extend(support[first + offset].iter().map(|&index| Sample {
                    rank: usize::from(ranks[index]),
                    neighbors: neighbor_context[index],
                    value: values[index],
                }));
                *model = fit(&samples, &mut residuals, &mut weights);
            }
        };
        if workers == 1 {
            fit_nodes(0, &mut models);
        } else {
            let nodes_per_worker = models.len().div_ceil(workers);
            std::thread::scope(|scope| {
                for (index, nodes) in models.chunks_mut(nodes_per_worker).enumerate() {
                    let fit_nodes = &fit_nodes;
                    scope.spawn(move || fit_nodes(index * nodes_per_worker, nodes));
                }
            });
        }
        let rows_per_worker = grid.div_ceil(workers);
        let changed = std::thread::scope(|scope| {
            let mut jobs = Vec::new();
            for (chunk, (decisions, next)) in output
                .chunks_mut(rows_per_worker * grid)
                .zip(next.chunks_mut(rows_per_worker * grid * 4))
                .enumerate()
            {
                let start_cell = chunk * rows_per_worker * grid;
                let ranks = &ranks;
                let models = &models;
                let values = &values;
                let fitting = &fitting;
                let neighbor_context = &neighbor_context;
                let positions = &positions;
                let intervals = &intervals;
                let mut classify = move || {
                    let mut changed = false;
                    for (offset, decision) in decisions.iter_mut().enumerate() {
                        let cell = start_cell + offset;
                        if !data_mask[cell] {
                            continue;
                        }
                        for quadrant in 0..4 {
                            let x = cell % grid * 2 + quadrant % 2;
                            let y = cell / grid * 2 + quadrant / 2;
                            let index = y * side + x;
                            let model = fitting[index]
                                .then(|| interpolate(intervals, positions.len(), models, x, y))
                                .flatten();
                            let Some(model) = model else {
                                decision.0 = (decision.0 & !(3 << (2 * quadrant)))
                                    | (initial[cell].0 & (3 << (2 * quadrant)));
                                decision.1[2 * quadrant..2 * quadrant + 2].copy_from_slice(
                                    &initial[cell].1[2 * quadrant..2 * quadrant + 2],
                                );
                                let gray = ((initial[cell].0 >> (2 * quadrant)) & 3) as u8;
                                next[index - start_cell * 4] = gray ^ (gray >> 1);
                                changed |= next[index - start_cell * 4] != ranks[index];
                                continue;
                            };
                            let interference = neighbor_context[index]
                                .into_iter()
                                .enumerate()
                                .map(|(i, neighbor)| model.coefficients[4 + i] * neighbor)
                                .sum::<f64>();
                            let signal = values[index] - interference;
                            let distances: [f64; 4] = std::array::from_fn(|rank| {
                                (signal - model.coefficients[rank]).powi(2)
                            });
                            let rank = (0..4)
                                .min_by(|&a, &b| distances[a].total_cmp(&distances[b]))
                                .unwrap();
                            next[index - start_cell * 4] = rank as u8;
                            changed |= next[index - start_cell * 4] != ranks[index];
                            let gray = rank ^ (rank >> 1);
                            decision.0 = (decision.0 & !(3 << (2 * quadrant)))
                                | ((gray as u16) << (2 * quadrant));
                            for bit in 0..2 {
                                let mut distance = [f64::INFINITY; 2];
                                for (rank, &squared_error) in distances.iter().enumerate() {
                                    let value = ((rank ^ (rank >> 1)) >> bit) & 1;
                                    distance[value] = distance[value].min(squared_error);
                                }
                                decision.1[2 * quadrant + bit] = ((distance[0] - distance[1]).abs()
                                    / (distance[0] + distance[1] + 2.0 * model.variance.max(1.0)))
                                    as f32;
                            }
                        }
                    }
                    changed
                };
                if workers == 1 {
                    return classify();
                }
                jobs.push(scope.spawn(classify));
            }
            let mut changed = false;
            for job in jobs {
                changed |= job.join().expect("luma classification worker panicked");
            }
            changed
        });
        std::mem::swap(&mut ranks, &mut next);
        if !changed {
            break;
        }
    }
    output
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

fn fit(samples: &[Sample], residuals: &mut Vec<f64>, weights: &mut Vec<f64>) -> Option<Model> {
    let mut counts = [0; 4];
    for sample in samples {
        counts[sample.rank] += 1;
    }
    if counts.into_iter().any(|count| count < 8) {
        return None;
    }
    let initial = solve(samples, None)?;
    weights.clear();
    weights.extend(samples.iter().map(|sample| residual(sample, &initial)));
    residuals.clear();
    residuals.extend_from_slice(weights);
    let residual_center = median(residuals);
    for value in residuals.iter_mut() {
        *value = (*value - residual_center).abs();
    }
    let scale = (1.4826 * median(residuals)).max(1.0);
    for weight in weights.iter_mut() {
        *weight = (2.5 * scale / weight.abs().max(1e-6)).min(1.0);
    }
    let coefficients = solve(samples, Some(weights))?;
    let gap = (0..3)
        .map(|rank| coefficients[rank + 1] - coefficients[rank])
        .fold(f64::INFINITY, f64::min);
    if gap < 8.0 {
        return None;
    }
    let variance = samples
        .iter()
        .zip(weights.iter())
        .map(|(sample, weight)| weight * residual(sample, &coefficients).powi(2))
        .sum::<f64>()
        / weights.iter().sum::<f64>();
    if !variance.is_finite() || variance.sqrt() / gap >= 0.4 {
        return None;
    }
    Some(Model {
        coefficients,
        variance,
    })
}

fn residual(sample: &Sample, coefficients: &[f64; 8]) -> f64 {
    sample.value
        - coefficients[sample.rank]
        - sample
            .neighbors
            .iter()
            .zip(&coefficients[4..])
            .map(|(value, coefficient)| value * coefficient)
            .sum::<f64>()
}

fn median(values: &mut [f64]) -> f64 {
    let middle = values.len() / 2;
    let even = values.len().is_multiple_of(2);
    let (lower, upper, _) = values.select_nth_unstable_by(middle, f64::total_cmp);
    if even {
        let lower = lower.iter().max_by(|a, b| a.total_cmp(b)).copied().unwrap();
        (lower + *upper) * 0.5
    } else {
        *upper
    }
}

fn solve(samples: &[Sample], weights: Option<&[f64]>) -> Option<[f64; 8]> {
    let mut system = [[0.0; 9]; 8];
    for (index, sample) in samples.iter().enumerate() {
        let weight = weights.map_or(1.0, |weights| weights[index]);
        let rank = sample.rank;
        system[rank][rank] += weight;
        system[rank][8] += weight * sample.value;
        for i in 0..4 {
            let value = weight * sample.neighbors[i];
            system[rank][4 + i] += value;
            system[4 + i][rank] += value;
            system[4 + i][8] += value * sample.value;
            for j in 0..4 {
                system[4 + i][4 + j] += value * sample.neighbors[j];
            }
        }
    }
    let scales: [f64; 8] = std::array::from_fn(|i| system[i][i].sqrt());
    if scales
        .iter()
        .any(|value| !value.is_finite() || *value <= f64::EPSILON)
    {
        return None;
    }
    for i in 0..8 {
        for j in 0..8 {
            system[i][j] /= scales[i] * scales[j];
        }
        system[i][8] /= scales[i];
    }
    for column in 0..8 {
        let pivot = (column..8)
            .max_by(|&a, &b| system[a][column].abs().total_cmp(&system[b][column].abs()))?;
        if system[pivot][column].abs() < 1e-4 {
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
    let coefficients = std::array::from_fn(|i| system[i][8] / scales[i]);
    coefficients
        .iter()
        .all(|value| value.is_finite())
        .then_some(coefficients)
}

#[cfg(test)]
#[path = "tests/luma_channel.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/luma_channel_blind_output_equivalence.rs"]
mod blind_output_equivalence_tests;
