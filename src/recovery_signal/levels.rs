pub(super) struct LevelField {
    positions: Vec<usize>,
    levels: Vec<[f32; 4]>,
}

impl LevelField {
    pub(super) fn estimate(
        grid: usize,
        observed: impl Fn(usize) -> Option<[f32; 4]>,
    ) -> anyhow::Result<Self> {
        let step = (grid - 1).div_ceil(32).max(8);
        let mut positions: Vec<_> = (0..grid - 1).step_by(step).collect();
        positions.push(grid - 1);
        let radius = step * 3 / 2;
        let observed: Vec<_> = (0..grid * grid).map(observed).collect();
        let mut levels = vec![None; positions.len().pow(2)];
        let workers = super::field_workers(grid);
        let nodes_per_worker = positions.len() * positions.len().div_ceil(workers);
        std::thread::scope(|scope| {
            for (chunk, levels) in levels.chunks_mut(nodes_per_worker).enumerate() {
                let positions = &positions;
                let observed = &observed;
                let mut fit_levels = move || {
                    let mut samples = Vec::new();
                    for (node, level) in levels.iter_mut().enumerate() {
                        let node = chunk * nodes_per_worker + node;
                        let (x, y) = (
                            positions[node % positions.len()],
                            positions[node / positions.len()],
                        );
                        samples.clear();
                        for yy in y.saturating_sub(radius)..(y + radius + 1).min(grid) {
                            for xx in x.saturating_sub(radius)..(x + radius + 1).min(grid) {
                                if xx.abs_diff(x).pow(2) + yy.abs_diff(y).pow(2) >= radius.pow(2) {
                                    continue;
                                }
                                if let Some(values) = observed[yy * grid + xx] {
                                    samples.extend(values.into_iter().filter(|v| v.is_finite()));
                                }
                            }
                        }
                        *level = fit(&mut samples);
                    }
                };
                if workers == 1 {
                    fit_levels();
                } else {
                    scope.spawn(fit_levels);
                }
            }
        });
        let valid: Vec<_> = levels.iter().map(Option::is_some).collect();
        let count = valid.iter().filter(|&&v| v).count();
        anyhow::ensure!(count > 0, "no valid local luminance alphabet");
        let mean: [f32; 4] = std::array::from_fn(|rank| {
            levels.iter().flatten().map(|v| v[rank]).sum::<f32>() / count as f32
        });
        let mut levels: Vec<_> = levels.into_iter().map(|v| v.unwrap_or(mean)).collect();
        let side = positions.len();

        for _ in 0..512 {
            let previous = levels.clone();
            let mut change = 0.0f32;
            for index in 0..levels.len() {
                if valid[index] {
                    continue;
                }
                let x = index % side;
                let y = index / side;
                let neighbors = [
                    x.checked_sub(1).map(|xx| y * side + xx),
                    (x + 1 < side).then_some(index + 1),
                    y.checked_sub(1).map(|yy| yy * side + x),
                    (y + 1 < side).then_some(index + side),
                ];
                let count = neighbors.iter().flatten().count() as f32;
                for rank in 0..4 {
                    levels[index][rank] = neighbors
                        .iter()
                        .flatten()
                        .map(|&i| previous[i][rank])
                        .sum::<f32>()
                        / count;
                    change = change.max((levels[index][rank] - previous[index][rank]).abs());
                }
            }

            if change < 1e-4 {
                break;
            }
        }
        Ok(Self { positions, levels })
    }

    pub(super) fn at(&self, x: usize, y: usize) -> [f32; 4] {
        let interval = |value| {
            let lower = self
                .positions
                .partition_point(|&p| p <= value)
                .saturating_sub(1)
                .min(self.positions.len() - 2);
            let fraction = (value - self.positions[lower]) as f32
                / (self.positions[lower + 1] - self.positions[lower]) as f32;
            (lower, fraction)
        };
        let (x, u) = interval(x);
        let (y, v) = interval(y);
        let side = self.positions.len();
        std::array::from_fn(|rank| {
            (1.0 - v)
                * ((1.0 - u) * self.levels[y * side + x][rank]
                    + u * self.levels[y * side + x + 1][rank])
                + v * ((1.0 - u) * self.levels[(y + 1) * side + x][rank]
                    + u * self.levels[(y + 1) * side + x + 1][rank])
        })
    }
}

fn fit(samples: &mut [f32]) -> Option<[f32; 4]> {
    if samples.len() < 32 {
        return None;
    }
    samples.sort_unstable_by(f32::total_cmp);
    let trim = (samples.len() / 64).min(7);
    let low = samples[trim];
    let high = samples[samples.len() - 1 - trim];
    let mut centers = std::array::from_fn(|rank| low + (high - low) * rank as f32 / 3.0);
    let partition = |centers: [f32; 4]| {
        let mut boundaries = [0; 5];
        boundaries[4] = samples.len();
        for i in 0..3 {
            let midpoint = (centers[i] + centers[i + 1]) * 0.5;
            boundaries[i + 1] = samples.partition_point(|v| *v <= midpoint);
        }
        boundaries
    };
    for _ in 0..24 {
        let boundaries = partition(centers);
        let previous = centers;
        for rank in 0..4 {
            let cluster = &samples[boundaries[rank]..boundaries[rank + 1]];
            if !cluster.is_empty() {
                centers[rank] =
                    (cluster[(cluster.len() - 1) / 2] + cluster[cluster.len() / 2]) * 0.5;
            }
        }
        if centers
            .iter()
            .zip(previous)
            .all(|(a, b)| (a - b).abs() < 1e-5)
        {
            break;
        }
    }
    let boundaries = partition(centers);
    let gaps: [f32; 3] = std::array::from_fn(|i| centers[i + 1] - centers[i]);
    if gaps.iter().any(|gap| *gap <= 0.8) {
        return None;
    }
    for rank in 0..4 {
        let cluster = &samples[boundaries[rank]..boundaries[rank + 1]];
        if cluster.len() < 8 {
            return None;
        }
        let mean = cluster.iter().sum::<f32>() / cluster.len() as f32;
        let variance =
            cluster.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / cluster.len() as f32;
        if variance.sqrt() / gaps[rank.min(2)] >= 0.4 {
            return None;
        }
    }
    Some(centers)
}
