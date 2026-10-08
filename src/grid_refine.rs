use std::collections::BinaryHeap;

const WINDOW_RADIUS: i32 = 24;
const SAMPLE_SIDE: usize = (WINDOW_RADIUS as usize + 1) * 2 + 1;

pub(crate) struct GridRefinement {
    step: f64,
    mesh_side: usize,
    shifts: Vec<[f64; 2]>,
    #[cfg(test)]
    confidence: Vec<[f32; 2]>,
}

impl GridRefinement {
    #[cfg(test)]
    pub(crate) fn estimate(
        grid: u32,
        integrated: bool,
        sample: impl Fn([f64; 2]) -> Option<f64> + Sync,
    ) -> Option<Self> {
        Self::estimate_channels(grid, integrated, |point| sample(point).map(|value| [value]))
            .into_iter()
            .next()
            .flatten()
    }

    pub(crate) fn estimate_channels<const N: usize>(
        grid: u32,
        integrated: bool,
        sample: impl Fn([f64; 2]) -> Option<[f64; N]> + Sync,
    ) -> [Option<Self>; N] {
        let workers = if grid >= 96 {
            std::thread::available_parallelism().map_or(1, |value| value.get().min(4))
        } else {
            1
        };
        Self::estimate_channels_with_workers(grid, integrated, sample, workers)
    }

    fn estimate_channels_with_workers<const N: usize>(
        grid: u32,
        integrated: bool,
        sample: impl Fn([f64; 2]) -> Option<[f64; N]> + Sync,
        workers: usize,
    ) -> [Option<Self>; N] {
        let mesh_side = if grid <= 200 { 17 } else { 33 };
        let step = (f64::from(grid) - 1.0) / (mesh_side - 1) as f64;
        let mut phases = vec![[[0.0; 2]; N]; mesh_side * mesh_side].into_boxed_slice();
        let mut confidence = vec![[[0.0f32; 2]; N]; phases.len()];
        let mut fixed = vec![false; phases.len()];
        let weights = WindowWeights::new();
        let axis_samples: Vec<_> = (0..mesh_side)
            .map(|index| AxisSamples::new(0.5 + index as f64 * step))
            .collect();
        let workers = workers.max(1).min(phases.len());
        if workers == 1 {
            let mut estimator =
                NodeEstimator::<N, _>::new(grid, integrated, &axis_samples, &weights, &sample);
            estimator.estimate_range(0, &mut phases, &mut confidence, &mut fixed);
        } else {
            let chunk_size = phases.len().div_ceil(workers);
            std::thread::scope(|scope| {
                for (chunk_index, ((phase_chunk, confidence_chunk), fixed_chunk)) in phases
                    .chunks_mut(chunk_size)
                    .zip(confidence.chunks_mut(chunk_size))
                    .zip(fixed.chunks_mut(chunk_size))
                    .enumerate()
                {
                    let start_index = chunk_index * chunk_size;
                    let sample = &sample;
                    let weights = &weights;
                    let axis_samples = &axis_samples;
                    scope.spawn(move || {
                        let mut estimator = NodeEstimator::<N, _>::new(
                            grid,
                            integrated,
                            axis_samples,
                            weights,
                            sample,
                        );
                        estimator.estimate_range(
                            start_index,
                            phase_chunk,
                            confidence_chunk,
                            fixed_chunk,
                        );
                    });
                }
            });
        }
        std::array::from_fn(|channel| {
            let channel_phases: Vec<_> = phases.iter().map(|value| value[channel]).collect();
            let channel_confidence: Vec<_> =
                confidence.iter().map(|value| value[channel]).collect();
            unwrap(
                integrated,
                step,
                mesh_side,
                &fixed,
                &channel_phases,
                &channel_confidence,
            )
            .map(|shifts| Self {
                step,
                mesh_side,
                shifts,
                #[cfg(test)]
                confidence: channel_confidence,
            })
        })
    }

    pub(crate) fn adjust(&self, point: [f64; 2]) -> [f64; 2] {
        self.adjust_with_stencil(point, self.stencil(point))
    }

    pub(crate) fn adjust_channels<const N: usize>(
        fields: &[Option<Self>; N],
        point: [f64; 2],
    ) -> [Option<[f64; 2]>; N] {
        let Some(reference) = fields.iter().flatten().next() else {
            return [None; N];
        };
        let stencil = reference.stencil(point);
        std::array::from_fn(|channel| {
            fields[channel].as_ref().map(|field| {
                debug_assert_eq!(field.mesh_side, reference.mesh_side);
                debug_assert_eq!(field.step, reference.step);
                field.adjust_with_stencil(point, stencil)
            })
        })
    }

    fn stencil(&self, point: [f64; 2]) -> ([usize; 4], [f64; 2]) {
        let mesh_side = self.mesh_side;
        let x = ((point[0] - 0.5) / self.step).clamp(0.0, (mesh_side - 1) as f64);
        let y = ((point[1] - 0.5) / self.step).clamp(0.0, (mesh_side - 1) as f64);
        let ix = x.floor() as usize;
        let iy = y.floor() as usize;
        let jx = (ix + 1).min(mesh_side - 1);
        let jy = (iy + 1).min(mesh_side - 1);
        let fx = x - ix as f64;
        let fy = y - iy as f64;
        (
            [
                iy * mesh_side + ix,
                iy * mesh_side + jx,
                jy * mesh_side + ix,
                jy * mesh_side + jx,
            ],
            [fx, fy],
        )
    }

    fn adjust_with_stencil(
        &self,
        point: [f64; 2],
        (indices, [fx, fy]): ([usize; 4], [f64; 2]),
    ) -> [f64; 2] {
        std::array::from_fn(|a| {
            point[a]
                + (self.shifts[indices[0]][a] * (1.0 - fx) + self.shifts[indices[1]][a] * fx)
                    * (1.0 - fy)
                + (self.shifts[indices[2]][a] * (1.0 - fx) + self.shifts[indices[3]][a] * fx) * fy
        })
    }
}

struct AxisSamples {
    center: f64,
    points: [f64; SAMPLE_SIDE],
    oscillations: [(f64, f64); SAMPLE_SIDE],
}

impl AxisSamples {
    fn new(center: f64) -> Self {
        let points = std::array::from_fn(|i| center + (i as i32 - WINDOW_RADIUS - 1) as f64 / 8.0);
        let oscillations = points.map(|point| (-4.0 * std::f64::consts::PI * point).sin_cos());
        Self {
            center,
            points,
            oscillations,
        }
    }
}

struct WindowWeights {
    values: [f64; SAMPLE_SIDE * SAMPLE_SIDE],
    possible_weight: f64,
}

impl WindowWeights {
    fn new() -> Self {
        let axis: [f64; SAMPLE_SIDE] = std::array::from_fn(|i| {
            let d = (i as i32 - WINDOW_RADIUS - 1) as f64;
            (-d * d / (2.0 * 12.0 * 12.0)).exp()
        });
        let values = std::array::from_fn(|i| axis[i % SAMPLE_SIDE] * axis[i / SAMPLE_SIDE]);
        let mut possible_weight = 0.0;
        for y in 1..SAMPLE_SIDE - 1 {
            for x in 1..SAMPLE_SIDE - 1 {
                possible_weight += values[y * SAMPLE_SIDE + x];
            }
        }
        Self {
            values,
            possible_weight,
        }
    }

    #[inline]
    fn accumulate<const N: usize>(
        &self,
        samples: &[[f64; N]],
        valid: &[bool],
        axes: [&AxisSamples; 2],
    ) -> [[[f64; 4]; N]; 2] {
        let mut sums = [[[0.0; 4]; N]; 2];
        let mut support = [0.0; 2];
        for y in 1..SAMPLE_SIDE - 1 {
            for x in 1..SAMPLE_SIDE - 1 {
                let i = y * SAMPLE_SIDE + x;
                let weight = self.values[i];
                if !valid[i] {
                    continue;
                }
                for axis in 0..2 {
                    let stride = if axis == 0 { 1 } else { SAMPLE_SIDE };
                    if !valid[i - stride] || !valid[i + stride] {
                        continue;
                    }
                    support[axis] += weight;
                    let (sin, cos) = axes[axis].oscillations[if axis == 0 { x } else { y }];
                    for channel in 0..N {
                        let gradient =
                            (samples[i + stride][channel] - samples[i - stride][channel]).abs()
                                * weight;
                        sums[axis][channel][0] += gradient * cos;
                        sums[axis][channel][1] += gradient * sin;
                        sums[axis][channel][2] += gradient;
                    }
                }
            }
        }
        for axis in 0..2 {
            for sum in &mut sums[axis] {
                sum[3] = support[axis];
            }
        }
        sums
    }
}

struct NodeEstimator<'a, const N: usize, S: Fn([f64; 2]) -> Option<[f64; N]>> {
    grid: u32,
    integrated: bool,
    axis_samples: &'a [AxisSamples],
    weights: &'a WindowWeights,
    sample: &'a S,
    samples: Box<[[f64; N]]>,
    valid: Vec<bool>,
}

impl<'a, const N: usize, S: Fn([f64; 2]) -> Option<[f64; N]>> NodeEstimator<'a, N, S> {
    fn new(
        grid: u32,
        integrated: bool,
        axis_samples: &'a [AxisSamples],
        weights: &'a WindowWeights,
        sample: &'a S,
    ) -> Self {
        let samples = vec![[0.0; N]; SAMPLE_SIDE * SAMPLE_SIDE].into_boxed_slice();
        let valid = vec![false; samples.len()];
        Self {
            grid,
            integrated,
            axis_samples,
            weights,
            sample,
            samples,
            valid,
        }
    }

    fn estimate_range(
        &mut self,
        start_index: usize,
        phases: &mut [[[f64; 2]; N]],
        confidence: &mut [[[f32; 2]; N]],
        fixed: &mut [bool],
    ) {
        let grid = self.grid;
        let integrated = self.integrated;
        let axis_samples = self.axis_samples;
        let mesh_side = axis_samples.len();
        let weights = self.weights;
        let sample = self.sample;
        let samples = &mut self.samples;
        let valid = &mut self.valid;
        let masked = |[x, y]: [f64; 2]| {
            if x < 0.0 || y < 0.0 || x >= f64::from(grid) || y >= f64::from(grid) {
                return true;
            }
            if !integrated {
                return false;
            }
            let side = x < 8.0 || x >= f64::from(grid) - 8.0;
            if side {
                y < 8.0 || y >= f64::from(grid) - 8.0
            } else {
                (1.0..6.0).contains(&y)
                    || (f64::from(grid) - 6.0..f64::from(grid) - 1.0).contains(&y)
            }
        };
        for (local_index, ((phase, node_confidence), node_fixed)) in phases
            .iter_mut()
            .zip(confidence.iter_mut())
            .zip(fixed.iter_mut())
            .enumerate()
        {
            let index = start_index + local_index;
            let ny = index / mesh_side;
            let nx = index % mesh_side;
            let axes = [&axis_samples[nx], &axis_samples[ny]];
            let center = axes.map(|axis| axis.center);
            let corner = integrated
                && (center[0] < 8.0 || center[0] >= f64::from(grid) - 8.0)
                && (center[1] < 8.0 || center[1] >= f64::from(grid) - 8.0);
            if corner {
                *node_fixed = true;
                continue;
            }
            for y in 0..SAMPLE_SIDE {
                for x in 0..SAMPLE_SIDE {
                    let point = [axes[0].points[x], axes[1].points[y]];
                    let i = y * SAMPLE_SIDE + x;
                    valid[i] = !masked(point);
                    if valid[i] {
                        if let Some(values) = sample(point) {
                            samples[i] = values;
                        } else {
                            valid[i] = false;
                        }
                    }
                }
            }
            let sums = weights.accumulate(samples, valid, axes);
            for axis in 0..2 {
                for channel in 0..N {
                    let [real, imag, total, support] = sums[axis][channel];
                    if total > 1.0 && support >= weights.possible_weight * 0.3 {
                        phase[channel][axis] = -imag.atan2(real) / (4.0 * std::f64::consts::PI);
                        node_confidence[channel][axis] = (real.hypot(imag) / total) as f32;
                    }
                }
            }
        }
    }
}

fn unwrap(
    integrated: bool,
    step: f64,
    mesh_side: usize,
    fixed: &[bool],
    phases: &[[f64; 2]],
    confidence: &[[f32; 2]],
) -> Option<Vec<[f64; 2]>> {
    let mut shifts = vec![[0.0; 2]; phases.len()];
    for axis in 0..2 {
        if confidence.iter().filter(|v| v[axis] > 0.15).count() < phases.len() / 2 {
            return None;
        }
        let mut visited = fixed.to_vec();
        let mut queue = BinaryHeap::new();
        let neighbors = |i: usize| {
            [
                i.checked_sub(mesh_side),
                if i / mesh_side + 1 < mesh_side {
                    Some(i + mesh_side)
                } else {
                    None
                },
                if !i.is_multiple_of(mesh_side) {
                    Some(i - 1)
                } else {
                    None
                },
                if i % mesh_side + 1 < mesh_side {
                    Some(i + 1)
                } else {
                    None
                },
            ]
        };
        let anchor = if integrated { 8.5 } else { 0.5 };
        let seed = (0..phases.len())
            .filter(|&i| !fixed[i] && confidence[i][axis] > 0.15)
            .min_by(|&a, &b| {
                let distance = |i: usize| {
                    let x = 0.5 + (i % mesh_side) as f64 * step - anchor;
                    let y = 0.5 + (i / mesh_side) as f64 * step - anchor;
                    x * x + y * y
                };
                distance(a).total_cmp(&distance(b))
            })?;
        visited[seed] = true;
        shifts[seed][axis] = phases[seed][axis];
        for j in neighbors(seed).into_iter().flatten() {
            if !visited[j] {
                queue.push((confidence[j][axis].to_bits(), j, seed));
            }
        }
        while let Some((_, i, parent)) = queue.pop() {
            if visited[i] {
                continue;
            }
            let phase = phases[i][axis];
            shifts[i][axis] = if confidence[i][axis] > 0.15 {
                phase + ((shifts[parent][axis] - phase) / 0.5).round() * 0.5
            } else {
                shifts[parent][axis]
            };
            visited[i] = true;
            for j in neighbors(i).into_iter().flatten() {
                if !visited[j] {
                    queue.push((confidence[j][axis].to_bits(), j, i));
                }
            }
        }
    }
    for _ in 0..64 {
        for i in 0..shifts.len() {
            if fixed[i] {
                continue;
            }
            for axis in 0..2 {
                if confidence[i][axis] > 0.15 {
                    continue;
                }
                let mut sum = 0.0;
                let mut count = 0.0;
                if i >= mesh_side {
                    sum += shifts[i - mesh_side][axis];
                    count += 1.0;
                }
                if i + mesh_side < shifts.len() {
                    sum += shifts[i + mesh_side][axis];
                    count += 1.0;
                }
                if i % mesh_side > 0 {
                    sum += shifts[i - 1][axis];
                    count += 1.0;
                }
                if i % mesh_side + 1 < mesh_side {
                    sum += shifts[i + 1][axis];
                    count += 1.0;
                }
                shifts[i][axis] = sum / count;
            }
        }
    }
    Some(shifts)
}

#[cfg(test)]
#[path = "tests/grid_refine.rs"]
mod tests;
