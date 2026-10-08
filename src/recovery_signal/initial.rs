use anyhow::{Result, ensure};

use super::{Decision, field_workers, levels};

pub(super) fn classify(
    grid: usize,
    features: &[[f32; 12]],
    mask: &[bool],
) -> Result<Vec<Decision>> {
    let levels = levels::LevelField::estimate(grid, |cell| {
        mask[cell].then(|| std::array::from_fn(|q| (features[cell][q] - 128.0) / 28.0))
    })?;
    let step = (grid - 1).div_ceil(32).max(8);
    let mut positions: Vec<_> = (0..grid - 1).step_by(step).collect();
    positions.push(grid - 1);
    let radius = step * 3 / 2;
    let mut models = vec![[None; 2]; positions.len().pow(2)];
    let workers = field_workers(grid);
    let nodes_per_worker = positions.len() * positions.len().div_ceil(workers);
    std::thread::scope(|scope| {
        for (chunk, models) in models.chunks_mut(nodes_per_worker).enumerate() {
            let positions = &positions;
            let mut fit_models = move || {
                let mut samples = [Vec::new(), Vec::new()];
                for (node, model) in models.iter_mut().enumerate() {
                    let node = chunk * nodes_per_worker + node;
                    let (x, y) = (
                        positions[node % positions.len()],
                        positions[node / positions.len()],
                    );
                    samples.iter_mut().for_each(Vec::clear);
                    for yy in y.saturating_sub(radius)..(y + radius + 1).min(grid) {
                        for xx in x.saturating_sub(radius)..(x + radius + 1).min(grid) {
                            if xx.abs_diff(x).pow(2) + yy.abs_diff(y).pow(2) >= radius.pow(2) {
                                continue;
                            }
                            let cell = yy * grid + xx;
                            if mask[cell] {
                                for axis in 0..2 {
                                    samples[axis].push(features[cell][4 + axis]);
                                }
                            }
                        }
                    }
                    *model = std::array::from_fn(|axis| two_levels(&mut samples[axis]));
                }
            };
            if workers == 1 {
                fit_models();
            } else {
                scope.spawn(fit_models);
            }
        }
    });
    for axis in 0..2 {
        ensure!(
            models.iter().any(|model| model[axis].is_some()),
            "no local chroma alphabet"
        );
    }
    Ok((0..features.len())
        .map(|cell| {
            let x = cell % grid;
            let y = cell / grid;
            let interval = |value| {
                let lower = positions
                    .partition_point(|&p| p <= value)
                    .saturating_sub(1)
                    .min(positions.len() - 2);
                (
                    lower,
                    (value - positions[lower]) as f32
                        / (positions[lower + 1] - positions[lower]) as f32,
                )
            };
            let (xx, u) = interval(x);
            let (yy, v) = interval(y);
            let side = positions.len();
            let weights = [
                (yy * side + xx, (1.0 - u) * (1.0 - v)),
                (yy * side + xx + 1, u * (1.0 - v)),
                ((yy + 1) * side + xx, (1.0 - u) * v),
                ((yy + 1) * side + xx + 1, u * v),
            ];
            let mut latent = [0.0; 10];
            let mut noise = [0.04; 10];
            for q in 0..4 {
                latent[q] = (features[cell][q] - 128.0) / 28.0;
            }
            for axis in 0..2 {
                let mut model = [0.0; 3];
                let mut available = true;
                for (index, weight) in weights {
                    if weight == 0.0 {
                        continue;
                    }
                    let Some(values) = models[index][axis] else {
                        available = false;
                        break;
                    };
                    for (output, value) in model.iter_mut().zip(values) {
                        *output += weight * value;
                    }
                }
                if available {
                    latent[4 + axis] = (features[cell][4 + axis] - model[0]) / model[1];
                    noise[4 + axis] = model[2] / model[1].powi(2);
                } else {
                    noise[4 + axis] = f32::INFINITY;
                }
            }
            crate::luma::classify_levels(latent, noise, levels.at(x, y))
        })
        .collect())
}

fn two_levels(samples: &mut [f32]) -> Option<[f32; 3]> {
    if samples.len() < 2 {
        return None;
    }
    samples.sort_unstable_by(f32::total_cmp);
    let mut centers = [samples[samples.len() / 4], samples[samples.len() * 3 / 4]];
    for _ in 0..16 {
        let boundary = samples.partition_point(|&value| value <= (centers[0] + centers[1]) * 0.5);
        if boundary == 0 || boundary == samples.len() {
            break;
        }
        let next = [
            samples[boundary / 2],
            samples[boundary + (samples.len() - boundary) / 2],
        ];
        if centers.iter().zip(next).all(|(a, b)| (*a - b).abs() < 1e-5) {
            break;
        }
        centers = next;
    }
    let center = (centers[0] + centers[1]) * 0.5;
    let gain = (centers[1] - centers[0]) * 0.5;
    let variance = samples
        .iter()
        .map(|value| (value - centers[usize::from(*value > center)]).powi(2))
        .sum::<f32>()
        / samples.len() as f32;
    (gain > 0.0 && gain.is_finite() && variance.is_finite()).then_some([center, gain, variance])
}
