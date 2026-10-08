use super::{COEFFICIENT_COUNT, Coefficients, LEVEL_COUNT, Model, Sample};

pub(super) fn fit(samples: &[Sample]) -> Option<Model> {
    let mut counts = [0; LEVEL_COUNT];
    for sample in samples {
        counts[sample.rank] += 1;
    }
    if counts.into_iter().any(|count| count < 8) {
        return None;
    }
    let initial = solve(samples, None)?;
    let mut residuals: Vec<_> = samples
        .iter()
        .map(|sample| residual(sample, &initial))
        .collect();
    let residual_center = median(&mut residuals);
    for value in &mut residuals {
        *value = (*value - residual_center).abs();
    }
    let scale = (1.4826 * median(&mut residuals)).max(1.0);
    let weights: Vec<_> = samples
        .iter()
        .map(|sample| (2.5 * scale / residual(sample, &initial).abs().max(1e-6)).min(1.0))
        .collect();
    let coefficients = solve(samples, Some(&weights))?;
    let gap = (0..LEVEL_COUNT - 1)
        .map(|rank| coefficients[rank + 1] - coefficients[rank])
        .fold(f64::INFINITY, f64::min);
    if gap < 8.0 {
        return None;
    }
    let variance = samples
        .iter()
        .zip(&weights)
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

fn residual(sample: &Sample, coefficients: &Coefficients) -> f64 {
    sample.value
        - coefficients[sample.rank]
        - sample.active_coefficients[..usize::from(sample.active_count)]
            .iter()
            .enumerate()
            .map(|(slot, &index)| {
                let value = coefficients[usize::from(index)];
                if sample.negative_terms & (1 << slot) == 0 {
                    value
                } else {
                    -value
                }
            })
            .sum::<f64>()
}

fn median(values: &mut [f64]) -> f64 {
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

fn solve(samples: &[Sample], weights: Option<&[f64]>) -> Option<Coefficients> {
    let mut system = [[0.0; COEFFICIENT_COUNT + 1]; COEFFICIENT_COUNT];
    for (index, sample) in samples.iter().enumerate() {
        let weight = weights.map_or(1.0, |weights| weights[index]);
        let rank = sample.rank;
        system[rank][rank] += weight;
        system[rank][COEFFICIENT_COUNT] += weight * sample.value;
        let active = &sample.active_coefficients[..usize::from(sample.active_count)];
        for (slot, &i) in active.iter().enumerate() {
            let i = usize::from(i);
            let value = if sample.negative_terms & (1 << slot) == 0 {
                weight
            } else {
                -weight
            };
            system[rank][i] += value;
            system[i][rank] += value;
            system[i][COEFFICIENT_COUNT] += value * sample.value;
            for (other_slot, &j) in active[..=slot].iter().enumerate() {
                let product = if sample.negative_terms & (1 << other_slot) == 0 {
                    value
                } else {
                    -value
                };
                system[i][usize::from(j)] += product;
            }
        }
    }
    for i in LEVEL_COUNT..COEFFICIENT_COUNT {
        let (above, remaining) = system.split_at_mut(i);
        for (j, row) in above.iter_mut().enumerate().skip(LEVEL_COUNT) {
            row[i] = remaining[0][j];
        }
    }
    let scales: Coefficients = std::array::from_fn(|i| system[i][i].sqrt());
    if scales
        .iter()
        .any(|value| !value.is_finite() || *value <= f64::EPSILON)
    {
        return None;
    }
    for i in 0..COEFFICIENT_COUNT {
        for j in 0..COEFFICIENT_COUNT {
            system[i][j] /= scales[i] * scales[j];
        }
        system[i][COEFFICIENT_COUNT] /= scales[i];
    }
    for column in 0..COEFFICIENT_COUNT {
        let pivot = (column..COEFFICIENT_COUNT)
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
    let coefficients = std::array::from_fn(|i| system[i][COEFFICIENT_COUNT] / scales[i]);
    coefficients
        .iter()
        .all(|value| value.is_finite())
        .then_some(coefficients)
}
