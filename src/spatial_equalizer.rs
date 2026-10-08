use serde::Serialize;

pub const MAX_LATTICE_SIDE: usize = 1024;

#[derive(Clone, Copy)]
pub struct Pilot {
    pub index: usize,
    pub value: [f32; 3],
    pub patch: usize,
}

#[derive(Clone, Copy, Serialize)]
pub struct ChannelModel {
    pub horizontal: f64,
    pub vertical: f64,
    pub unexplained_fraction: f64,
    pub regularization: f64,
}

#[derive(Default)]
struct Moments {
    count: usize,
    sum: [f64; 4],
    observed_sum: f64,
    cross: [[f64; 4]; 4],
    observed_cross: [f64; 4],
    observed_square: f64,
}

impl Moments {
    fn push(&mut self, basis: [f64; 4], observed: f64) {
        self.count += 1;
        self.observed_sum += observed;
        self.observed_square += observed * observed;
        for i in 0..4 {
            self.sum[i] += basis[i];
            self.observed_cross[i] += basis[i] * observed;
            for j in 0..4 {
                self.cross[i][j] += basis[i] * basis[j];
            }
        }
    }

    fn residual(&self, a: f64, b: f64) -> Option<(f64, f64)> {
        if self.count < 16 {
            return None;
        }
        let weights = [1.0, a, b, a * b];
        let mean = dot(&weights, &self.sum) / self.count as f64;
        let observed_mean = self.observed_sum / self.count as f64;
        let variance = (0..4)
            .map(|i| weights[i] * dot(&weights, &self.cross[i]))
            .sum::<f64>()
            - mean * mean * self.count as f64;
        let covariance = dot(&weights, &self.observed_cross) - mean * self.observed_sum;
        let observed_variance = self.observed_square - observed_mean * self.observed_sum;
        if variance <= 1e-6 || observed_variance <= 1e-6 || covariance <= 0.0 {
            return None;
        }
        Some((
            (observed_variance - covariance * covariance / variance).max(0.0),
            observed_variance,
        ))
    }
}

fn dot<const N: usize>(a: &[f64; N], b: &[f64; N]) -> f64 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}

pub fn equalize(
    observed: &[[f32; 3]],
    side: usize,
    pilots: &[Pilot],
) -> Option<(Vec<[f32; 3]>, [ChannelModel; 3])> {
    equalize_channels::<3>(observed, side, pilots)
}

pub(crate) fn equalize_luma(
    observed: &[[f32; 3]],
    side: usize,
    pilots: &[Pilot],
) -> Option<(Vec<[f32; 3]>, [ChannelModel; 3])> {
    equalize_channels::<1>(observed, side, pilots)
}

fn equalize_channels<const CHANNELS: usize>(
    observed: &[[f32; 3]],
    side: usize,
    pilots: &[Pilot],
) -> Option<(Vec<[f32; 3]>, [ChannelModel; 3])> {
    if !(3..=MAX_LATTICE_SIDE).contains(&side)
        || observed.len() != side * side
        || pilots
            .iter()
            .any(|p| p.index >= observed.len() || p.patch >= 4)
    {
        return None;
    }
    let mut known = vec![None; observed.len()];
    for pilot in pilots {
        known[pilot.index] = Some(pilot.value);
    }
    let mut moments: [[Moments; 4]; 3] =
        std::array::from_fn(|_| std::array::from_fn(|_| Moments::default()));
    for pilot in pilots {
        let index = pilot.index;
        if index % side == 0
            || index % side == side - 1
            || index < side
            || index + side >= observed.len()
        {
            continue;
        }
        let neighbors = [
            index - side - 1,
            index - side,
            index - side + 1,
            index - 1,
            index,
            index + 1,
            index + side - 1,
            index + side,
            index + side + 1,
        ];
        let values = neighbors.map(|i| known[i]);
        if values.iter().any(Option::is_none) {
            continue;
        }
        let values = values.map(Option::unwrap);
        for channel in 0..CHANNELS {
            let c = f64::from(values[4][channel]);
            let horizontal = f64::from(values[3][channel] + values[5][channel]);
            let vertical = f64::from(values[1][channel] + values[7][channel]);
            let diagonal = f64::from(
                values[0][channel] + values[2][channel] + values[6][channel] + values[8][channel],
            );
            moments[channel][pilot.patch].push(
                [
                    c,
                    horizontal - 2.0 * c,
                    vertical - 2.0 * c,
                    diagonal - 2.0 * horizontal - 2.0 * vertical + 4.0 * c,
                ],
                f64::from(observed[index][channel]),
            );
        }
    }
    let mut models = [ChannelModel {
        horizontal: 0.0,
        vertical: 0.0,
        unexplained_fraction: 0.0,
        regularization: 0.0,
    }; 3];
    let mut output = observed.to_vec();
    for channel in 0..CHANNELS {
        let informative: Vec<_> = moments[channel]
            .iter()
            .filter(|patch| patch.residual(0.0, 0.0).is_some())
            .collect();
        let score = |a: f64, b: f64| -> Option<f64> {
            let mut residual = 0.0;
            let mut variance = 0.0;
            for patch in &informative {
                let (r, v) = patch.residual(a, b)?;
                residual += r;
                variance += v;
            }
            (variance > 0.0).then_some(residual / variance)
        };
        let mut best = (0.0, 0.0, score(0.0, 0.0)?);
        for y in 0..=12 {
            for x in 0..=12 {
                let a = x as f64 * 0.02;
                let b = y as f64 * 0.02;
                if let Some(cost) = score(a, b)
                    && cost < best.2
                {
                    best = (a, b, cost);
                }
            }
        }
        for step in [0.01, 0.005, 0.0025] {
            let previous = best;
            for y in -1..=1 {
                for x in -1..=1 {
                    let a = (previous.0 + x as f64 * step).clamp(0.0, 0.24);
                    let b = (previous.1 + y as f64 * step).clamp(0.0, 0.24);
                    if let Some(cost) = score(a, b)
                        && cost < best.2
                    {
                        best = (a, b, cost);
                    }
                }
            }
        }
        let regularization = (best.2 / (1.0 - best.2).max(1e-6)).max(1e-4);
        models[channel] = ChannelModel {
            horizontal: best.0,
            vertical: best.1,
            unexplained_fraction: best.2,
            regularization,
        };
        if best.0 == 0.0 && best.1 == 0.0 {
            continue;
        }
        let recovered = invert(observed, channel, side, best.0, best.1, regularization);
        for (pixel, value) in output.iter_mut().zip(recovered) {
            pixel[channel] = value as f32;
        }
    }
    Some((output, models))
}

fn filter(input: &[f64], side: usize, a: f64, b: f64, horizontal: &mut [f64], output: &mut [f64]) {
    for (source, target) in input
        .chunks_exact(side)
        .zip(horizontal.chunks_exact_mut(side))
    {
        target[0] = (1.0 - 2.0 * a) * source[0] + a * source[1];
        for ((out, center), (left, right)) in target[1..side - 1]
            .iter_mut()
            .zip(&source[1..side - 1])
            .zip(source[..side - 2].iter().zip(&source[2..]))
        {
            *out = (1.0 - 2.0 * a) * center + a * left + a * right;
        }
        target[side - 1] = (1.0 - 2.0 * a) * source[side - 1] + a * source[side - 2];
    }
    let len = input.len();
    for ((out, center), below) in output[..side]
        .iter_mut()
        .zip(&horizontal[..side])
        .zip(&horizontal[side..side * 2])
    {
        *out = (1.0 - 2.0 * b) * center + b * below;
    }
    for ((out, center), (above, below)) in output[side..len - side]
        .iter_mut()
        .zip(&horizontal[side..len - side])
        .zip(
            horizontal[..len - side * 2]
                .iter()
                .zip(&horizontal[side * 2..]),
        )
    {
        *out = (1.0 - 2.0 * b) * center + b * above + b * below;
    }
    for ((out, center), above) in output[len - side..]
        .iter_mut()
        .zip(&horizontal[len - side..])
        .zip(&horizontal[len - side * 2..len - side])
    {
        *out = (1.0 - 2.0 * b) * center + b * above;
    }
}

fn dot_values(a: &[f64], b: &[f64]) -> f64 {
    let mut sums = [0.0; 4];
    let (a_chunks, a_remainder) = a.as_chunks::<4>();
    let (b_chunks, b_remainder) = b.as_chunks::<4>();
    for (a, b) in a_chunks.iter().zip(b_chunks) {
        for i in 0..4 {
            sums[i] += a[i] * b[i];
        }
    }
    sums.into_iter().sum::<f64>()
        + a_remainder
            .iter()
            .zip(b_remainder)
            .map(|(a, b)| a * b)
            .sum::<f64>()
}

fn invert(
    observed: &[[f32; 3]],
    channel: usize,
    side: usize,
    a: f64,
    b: f64,
    lambda: f64,
) -> Vec<f64> {
    let mean = observed.iter().map(|v| f64::from(v[channel])).sum::<f64>() / observed.len() as f64;
    let mut x: Vec<f64> = observed
        .iter()
        .map(|v| f64::from(v[channel]) - mean)
        .collect();
    let mut scratch = vec![0.0; x.len()];
    let mut intermediate = vec![0.0; x.len()];
    let mut applied = vec![0.0; x.len()];
    filter(&x, side, a, b, &mut scratch, &mut intermediate);
    filter(&intermediate, side, a, b, &mut scratch, &mut applied);
    let mut r: Vec<f64> = intermediate
        .iter()
        .zip(&applied)
        .zip(&x)
        .map(|((rhs, applied), x)| rhs - (applied + lambda * x))
        .collect();
    let mut p = r.clone();
    let mut norm = dot_values(&r, &r);
    for _ in 0..16 {
        if norm < 1e-12 {
            break;
        }
        filter(&p, side, a, b, &mut scratch, &mut intermediate);
        filter(&intermediate, side, a, b, &mut scratch, &mut applied);
        for (out, value) in applied.iter_mut().zip(&p) {
            *out += lambda * value;
        }
        let denominator = dot_values(&p, &applied);
        if denominator <= 0.0 {
            break;
        }
        let alpha = norm / denominator;
        for ((x, r), (p, applied)) in x.iter_mut().zip(&mut r).zip(p.iter().zip(&applied)) {
            *x += alpha * p;
            *r -= alpha * applied;
        }
        let next = dot_values(&r, &r);
        let beta = next / norm;
        for (p, r) in p.iter_mut().zip(&r) {
            *p = r + beta * (*p);
        }
        norm = next;
    }
    for value in &mut x {
        *value = mean + (1.0 + lambda) * (*value);
    }
    x
}

#[cfg(test)]
#[path = "tests/spatial_equalizer.rs"]
mod tests;
