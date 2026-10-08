use super::{
    CHROMA_COUNT, MAX_ACTIVE_TERMS, Sample, chroma_coefficient, inference::InferenceField,
    model::fit, neighbor_coefficient,
};

pub(super) fn refine_with_workers(
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
            (
                lower,
                (value - positions[lower]) as f64
                    / (positions[lower + 1] - positions[lower]) as f64,
            )
        })
        .collect();
    let radius = step * 3 / 2;
    let mut output = initial.to_vec();
    let sample_positions: Vec<_> = (0..values.len())
        .filter(|&index| {
            fitting[index]
                && (index % side / 2).is_multiple_of(2)
                && (index / side / 2).is_multiple_of(2)
        })
        .collect();
    let mut sample_indices = vec![usize::MAX; values.len()];
    for (sample, &index) in sample_positions.iter().enumerate() {
        sample_indices[index] = sample;
    }
    let supports: Vec<Vec<usize>> = positions
        .iter()
        .flat_map(|&y| positions.iter().map(move |&x| (x, y)))
        .map(|(x, y)| {
            let mut support = Vec::new();
            for yy in y.saturating_sub(radius)..(y + radius + 1).min(side) {
                for xx in x.saturating_sub(radius)..(x + radius + 1).min(side) {
                    if xx.abs_diff(x).pow(2) + yy.abs_diff(y).pow(2) >= radius.pow(2) {
                        continue;
                    }
                    let sample = sample_indices[yy * side + xx];
                    if sample != usize::MAX {
                        support.push(sample);
                    }
                }
            }
            support
        })
        .collect();
    for _ in 0..5 {
        let prepared = prepare_samples(grid, side, &sample_positions, &ranks, &values, initial);
        let mut models = vec![None; supports.len()];
        let nodes_per_worker = positions.len() * positions.len().div_ceil(workers);
        std::thread::scope(|scope| {
            for (chunk, models) in models.chunks_mut(nodes_per_worker).enumerate() {
                let supports = &supports[chunk * nodes_per_worker..][..models.len()];
                let prepared = &prepared;
                let mut fit_models = move || {
                    let mut samples = Vec::new();
                    for (model, support) in models.iter_mut().zip(supports) {
                        samples.clear();
                        samples.extend(support.iter().map(|&index| prepared[index]));
                        *model = fit(&samples);
                    }
                };
                if workers == 1 {
                    fit_models();
                } else {
                    scope.spawn(fit_models);
                }
            }
        });
        let mut next = ranks.clone();
        let rows_per_worker = grid.div_ceil(workers);
        let inference = InferenceField {
            grid,
            side,
            model_side: positions.len(),
            data_mask,
            initial,
            ranks: &ranks,
            values: &values,
            fitting: &fitting,
            intervals: &intervals,
            models: &models,
        };
        let changed = std::thread::scope(|scope| {
            let mut jobs = Vec::new();
            for (chunk, (output, next)) in output
                .chunks_mut(rows_per_worker * grid)
                .zip(next.chunks_mut(rows_per_worker * grid * 4))
                .enumerate()
            {
                let start_cell = chunk * rows_per_worker * grid;
                let inference = &inference;
                let mut classify = move || inference.classify(start_cell, output, next);
                if workers == 1 {
                    return classify();
                }
                jobs.push(scope.spawn(classify));
            }
            let mut changed = false;
            for job in jobs {
                changed |= job.join().expect("coupled classification worker panicked");
            }
            changed
        });
        ranks = next;
        if !changed {
            break;
        }
    }
    output
}

#[cfg(test)]
#[path = "../../tests/recovery_signal_coupled.rs"]
mod output_equivalence_tests;

fn prepare_samples(
    grid: usize,
    side: usize,
    sample_positions: &[usize],
    ranks: &[u8],
    values: &[f64],
    initial: &[super::super::Decision],
) -> Vec<Sample> {
    sample_positions
        .iter()
        .map(|&index| {
            let mut active_coefficients = [0; MAX_ACTIVE_TERMS];
            let mut count = 0;
            for (i, neighbor) in [index + 1, index - 1, index + side, index - side]
                .into_iter()
                .enumerate()
            {
                let rank = usize::from(ranks[neighbor]);
                if rank > 0 {
                    active_coefficients[count] = neighbor_coefficient(i, rank) as u8;
                    count += 1;
                }
            }
            let cell = (index / side / 2) * grid + index % side / 2;
            let mut negative = 0;
            for axis in 0..CHROMA_COUNT {
                let bit = axis + 8;
                active_coefficients[count] = chroma_coefficient(axis) as u8;
                negative |= u8::from(initial[cell].0 & (1 << bit) == 0) << count;
                count += 1;
            }
            Sample {
                rank: usize::from(ranks[index]),
                active_coefficients,
                negative_terms: negative,
                active_count: count as u8,
                value: values[index],
            }
        })
        .collect()
}
