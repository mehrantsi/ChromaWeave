#[derive(Clone, Copy)]
struct Model<const N: usize> {
    coefficients: [f64; N],
    variance: f64,
}

#[derive(Clone, Copy)]
struct Sample<const N: usize> {
    basis: [f64; N],
    value: f64,
    class: usize,
}

#[derive(Clone, Copy)]
struct Interval {
    lower: usize,
    fraction: f32,
    quantized_lower: usize,
    quantized_fraction: f64,
}

struct Field<const N: usize> {
    intervals: Vec<Interval>,
    node_side: usize,
    models: Vec<Option<Model<N>>>,
}

impl<const N: usize> Field<N> {
    fn estimate(
        side: usize,
        step: usize,
        radius: usize,
        luma: bool,
        sample: impl Fn(usize) -> Option<Sample<N>>,
    ) -> Self {
        let mut positions: Vec<_> = (0..side - 1).step_by(step).collect();
        positions.push(side - 1);
        let intervals = (0..side)
            .map(|value| {
                let lower = positions
                    .partition_point(|&p| p <= value)
                    .saturating_sub(1)
                    .min(positions.len() - 2);
                let coordinate = (lower as f64
                    + (value - positions[lower]) as f64
                        / (positions[lower + 1] - positions[lower]) as f64)
                    as f32;
                let lower = coordinate.floor() as usize;
                let quantized = (coordinate * 32.0).round_ties_even() as usize;
                Interval {
                    lower,
                    fraction: coordinate - lower as f32,
                    quantized_lower: quantized / 32,
                    quantized_fraction: (quantized % 32) as f64 / 32.0,
                }
            })
            .collect();
        let mut sample_indices = vec![usize::MAX; side * side];
        let mut prepared = Vec::new();
        for y in 0..side {
            for x in 0..side {
                let selected = if luma {
                    x / 2 % 2 == 0 && y / 2 % 2 == 0
                } else {
                    x % 2 == 0 && y % 2 == 0
                };
                let index = y * side + x;
                if selected && let Some(sample) = sample(index) {
                    sample_indices[index] = prepared.len();
                    prepared.push(sample);
                }
            }
        }
        let mut models = vec![None; positions.len().pow(2)];
        let workers = if side >= if luma { 192 } else { 96 } {
            std::thread::available_parallelism().map_or(1, |value| value.get().min(4))
        } else {
            1
        };
        let nodes_per_worker = positions.len() * positions.len().div_ceil(workers);
        std::thread::scope(|scope| {
            for (chunk, models) in models.chunks_mut(nodes_per_worker).enumerate() {
                let positions = &positions;
                let prepared = &prepared;
                let sample_indices = &sample_indices;
                let mut fit_models = move || {
                    let mut samples = Vec::new();
                    for (node, model) in models.iter_mut().enumerate() {
                        let node = chunk * nodes_per_worker + node;
                        let (x, y) = (
                            positions[node % positions.len()],
                            positions[node / positions.len()],
                        );
                        samples.clear();
                        for yy in y.saturating_sub(radius)..(y + radius + 1).min(side) {
                            for xx in x.saturating_sub(radius)..(x + radius + 1).min(side) {
                                if xx.abs_diff(x).pow(2) + yy.abs_diff(y).pow(2) >= radius.pow(2) {
                                    continue;
                                }
                                let sample = sample_indices[yy * side + xx];
                                if sample != usize::MAX {
                                    samples.push(prepared[sample]);
                                }
                            }
                        }
                        *model = fit(&samples, luma);
                    }
                };
                if workers == 1 {
                    fit_models();
                } else {
                    scope.spawn(fit_models);
                }
            }
        });
        Self {
            intervals,
            node_side: positions.len(),
            models,
        }
    }

    fn at(&self, x: usize, y: usize) -> Option<Model<N>> {
        let x_interval = self.intervals[x];
        let y_interval = self.intervals[y];
        let side = self.node_side;
        let (x, u) = (x_interval.lower, x_interval.fraction);
        let (y, v) = (y_interval.lower, y_interval.fraction);
        let available = |x: usize, y: usize| {
            if x < side && y < side && self.models[y * side + x].is_some() {
                1.0_f32
            } else {
                0.0
            }
        };
        let support = (1.0 - v) * ((1.0 - u) * available(x, y) + u * available(x + 1, y))
            + v * ((1.0 - u) * available(x, y + 1) + u * available(x + 1, y + 1));
        if support < 0.999 {
            return None;
        }
        let (x, u) = (x_interval.quantized_lower, x_interval.quantized_fraction);
        let (y, v) = (y_interval.quantized_lower, y_interval.quantized_fraction);
        let weights = [
            (x, y, (1.0 - u) * (1.0 - v)),
            (x + 1, y, u * (1.0 - v)),
            (x, y + 1, (1.0 - u) * v),
            (x + 1, y + 1, u * v),
        ];
        let mut model = Model {
            coefficients: [0.0; N],
            variance: 0.0,
        };
        for (x, y, weight) in weights {
            if weight == 0.0 || x >= side || y >= side {
                continue;
            }
            if let Some(node) = self.models[y * side + x] {
                for (out, value) in model.coefficients.iter_mut().zip(node.coefficients) {
                    *out += weight * value;
                }
                model.variance += weight * node.variance;
            }
        }
        Some(model)
    }
}

fn dot<const N: usize>(a: &[f64; N], b: &[f64; N]) -> f64 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
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

fn solve<const N: usize>(
    samples: &[Sample<N>],
    weights: Option<&[f64]>,
    luma: bool,
) -> Option<[f64; N]> {
    let mut matrix = [[0.0; N]; N];
    let mut rhs = [0.0; N];
    for (index, sample) in samples.iter().enumerate() {
        let weight = weights.map_or(1.0, |weights| weights[index]);
        if luma {
            let rank = sample.class;
            let weighted = weight * sample.basis[rank];
            rhs[rank] += weighted * sample.value;
            matrix[rank][rank] += weighted * sample.basis[rank];
            for i in 4..N {
                let weighted = weight * sample.basis[i];
                if weighted == 0.0 {
                    continue;
                }
                rhs[i] += weighted * sample.value;
                matrix[i][rank] += weighted * sample.basis[rank];
                for (entry, &value) in matrix[i][4..=i].iter_mut().zip(&sample.basis[4..]) {
                    *entry += weighted * value;
                }
            }
        } else {
            for i in 0..N {
                let weighted = weight * sample.basis[i];
                if weighted == 0.0 {
                    continue;
                }
                rhs[i] += weighted * sample.value;
                for (entry, &value) in matrix[i][..=i].iter_mut().zip(&sample.basis) {
                    *entry += weighted * value;
                }
            }
        }
    }
    #[allow(clippy::needless_range_loop)]
    for i in 0..N {
        for j in 0..i {
            matrix[j][i] = matrix[i][j];
        }
    }
    let scales: [f64; N] = std::array::from_fn(|i| matrix[i][i].sqrt());
    if scales.iter().any(|v| !v.is_finite() || *v <= f64::EPSILON) {
        return None;
    }
    for i in 0..N {
        for j in 0..N {
            matrix[i][j] /= scales[i] * scales[j];
        }
        rhs[i] /= scales[i];
    }
    for column in 0..N {
        let pivot = (column..N)
            .max_by(|&a, &b| matrix[a][column].abs().total_cmp(&matrix[b][column].abs()))?;
        if matrix[pivot][column].abs() < 1e-10 {
            return None;
        }
        matrix.swap(column, pivot);
        rhs.swap(column, pivot);
        let divisor = matrix[column][column];
        for value in &mut matrix[column][column..] {
            *value /= divisor;
        }
        rhs[column] /= divisor;
        let row = matrix[column];
        let value = rhs[column];
        for i in 0..N {
            if i == column {
                continue;
            }
            let factor = matrix[i][column];
            for (out, pivot) in matrix[i][column..].iter_mut().zip(&row[column..]) {
                *out -= factor * pivot;
            }
            rhs[i] -= factor * value;
        }
    }
    let coefficients = std::array::from_fn(|i| rhs[i] / scales[i]);
    coefficients
        .iter()
        .all(|v| v.is_finite())
        .then_some(coefficients)
}

fn fit<const N: usize>(samples: &[Sample<N>], luma: bool) -> Option<Model<N>> {
    let classes = if luma { 4 } else { 2 };
    let mut counts = [0; 4];
    for sample in samples {
        counts[sample.class] += 1;
    }
    if samples.len() < 32 || counts[..classes].iter().any(|&count| count < 8) {
        return None;
    }
    let initial = solve(samples, None, luma)?;
    let mut residuals: Vec<_> = samples
        .iter()
        .map(|s| s.value - dot(&s.basis, &initial))
        .collect();
    let center = median(&mut residuals);
    for residual in &mut residuals {
        *residual = (*residual - center).abs();
    }
    let scale = (1.4826 * median(&mut residuals)).max(1.0);
    let weights: Vec<_> = samples
        .iter()
        .map(|s| (2.5 * scale / (s.value - dot(&s.basis, &initial)).abs().max(1e-6)).min(1.0))
        .collect();
    let coefficients = solve(samples, Some(&weights), luma)?;
    let gap = if luma {
        (0..3)
            .map(|r| coefficients[r + 1] - coefficients[r])
            .fold(f64::INFINITY, f64::min)
    } else {
        2.0 * coefficients[1]
    };
    let variance = samples
        .iter()
        .zip(&weights)
        .map(|(s, w)| w * (s.value - dot(&s.basis, &coefficients)).powi(2))
        .sum::<f64>()
        / weights.iter().sum::<f64>();
    if !variance.is_finite()
        || gap < if luma { 8.0 } else { 1.0 }
        || (luma && variance.sqrt() / gap >= 0.4)
    {
        return None;
    }
    Some(Model {
        coefficients,
        variance,
    })
}

fn interior(side: usize, valid: &[bool]) -> Vec<bool> {
    (0..side * side)
        .map(|i| {
            i >= side
                && i + side < valid.len()
                && i % side > 0
                && i % side + 1 < side
                && valid[i]
                && valid[i - side]
                && valid[i + side]
                && valid[i - 1]
                && valid[i + 1]
        })
        .collect()
}

fn sign(symbol: u16, bit: usize) -> f64 {
    2.0 * f64::from((symbol >> bit) & 1) - 1.0
}

fn log_add(a: f64, b: f64) -> f64 {
    let maximum = a.max(b);
    if maximum == f64::NEG_INFINITY {
        maximum
    } else {
        maximum + (a.min(b) - maximum).exp().ln_1p()
    }
}

pub(crate) fn refine_with_support(
    grid: usize,
    features: &[[f32; 12]],
    initial: &[(u16, [f32; 10])],
    known: &[(u16, u16)],
) -> (Vec<[f64; 10]>, Vec<u16>) {
    let mut support = vec![0; grid * grid];
    let evidence = refine_detail(grid, features, initial, known, Some(&mut support));
    (evidence, support)
}

fn refine_detail(
    grid: usize,
    features: &[[f32; 12]],
    initial: &[(u16, [f32; 10])],
    known: &[(u16, u16)],
    mut support: Option<&mut [u16]>,
) -> Vec<[f64; 10]> {
    let side = grid * 2;
    let symbols: Vec<_> = initial
        .iter()
        .zip(known)
        .map(|((symbol, _), (mask, value))| (symbol & !mask) | (value & mask))
        .collect();
    let data: Vec<_> = (0..grid * grid)
        .map(|cell| {
            !crate::integrated_layout::reserved(
                grid as u32,
                (cell % grid) as u32,
                (cell / grid) as u32,
            )
        })
        .collect();
    let cell_quadrant = |index: usize| {
        let (x, y) = (index % side, index / side);
        ((y / 2) * grid + x / 2, (y % 2) * 2 + x % 2)
    };
    let ranks: Vec<_> = (0..side * side)
        .map(|i| {
            let (cell, q) = cell_quadrant(i);
            let gray = (symbols[cell] >> (q * 2)) & 3;
            usize::from(gray ^ (gray >> 1))
        })
        .collect();
    let quarter_data: Vec<_> = (0..side * side).map(|i| data[cell_quadrant(i).0]).collect();
    let quarter_interior = interior(side, &quarter_data);
    let luma_context = |index: usize| {
        let (cell, _) = cell_quadrant(index);
        let mut basis = [0.0; 10];
        basis[ranks[index]] = 1.0;
        for (i, neighbor) in [index + 1, index - 1, index + side, index - side]
            .into_iter()
            .enumerate()
        {
            basis[4 + i] = ranks[neighbor] as f64 * 2.0 - 3.0;
        }
        basis[8] = sign(symbols[cell], 8);
        basis[9] = sign(symbols[cell], 9);
        basis
    };
    let step = (side - 1).div_ceil(32);
    let luma = Field::estimate(side, step, step * 3 / 2, true, |index| {
        let (cell, q) = cell_quadrant(index);
        (quarter_interior[index]
            && (known[cell].0 >> (2 * q)) & 3 == 3
            && features[cell][q].is_finite())
        .then(|| Sample {
            basis: luma_context(index),
            value: f64::from(features[cell][q]),
            class: ranks[index],
        })
    });
    let mut output = vec![[0.0; 10]; grid * grid];
    for (index, &is_interior) in quarter_interior.iter().enumerate() {
        if !is_interior {
            continue;
        }
        let (cell, q) = cell_quadrant(index);
        if (known[cell].0 >> (2 * q)) & 3 == 3 {
            continue;
        }
        let Some(model) = luma.at(index % side, index / side) else {
            continue;
        };
        let basis = luma_context(index);
        let interference: f64 = basis[4..]
            .iter()
            .zip(&model.coefficients[4..])
            .map(|(a, b)| a * b)
            .sum();
        let signal = f64::from(features[cell][q]) - interference;
        let likelihood: [f64; 4] = std::array::from_fn(|rank| {
            let gray = rank ^ (rank >> 1);
            if ((((gray as u16) << (2 * q)) ^ known[cell].1) & known[cell].0 & (3 << (2 * q))) != 0
            {
                f64::NEG_INFINITY
            } else {
                -(signal - model.coefficients[rank]).powi(2) / (2.0 * model.variance.max(1e-12))
            }
        });
        for bit in 0..2 {
            if known[cell].0 & (1 << (q * 2 + bit)) != 0 {
                continue;
            }
            let mut sums = [f64::NEG_INFINITY; 2];
            for (rank, value) in likelihood.iter().enumerate() {
                let class = ((rank ^ (rank >> 1)) >> bit) & 1;
                sums[class] = log_add(sums[class], *value);
            }
            let llr = sums[1] - sums[0];
            if llr.is_finite() {
                output[cell][q * 2 + bit] = llr;
                if let Some(support) = support.as_deref_mut() {
                    support[cell] |= 1 << (q * 2 + bit);
                }
            }
        }
    }
    let cell_interior = interior(grid, &data);
    let moments: Vec<_> = symbols
        .iter()
        .map(|symbol| {
            let ranks: [f64; 4] = std::array::from_fn(|q| {
                let gray = (symbol >> (q * 2)) & 3;
                f64::from(gray ^ (gray >> 1))
            });
            [
                ranks.iter().sum::<f64>() / 4.0 - 1.5,
                ranks.iter().map(|r| r * r).sum::<f64>() / 4.0 - 3.5,
            ]
        })
        .collect();
    for axis in [4, 5] {
        let bit = axis + 4;
        let chroma_context = |cell: usize| {
            let mut basis = [
                1.0,
                sign(symbols[cell], bit),
                0.0,
                0.0,
                0.0,
                0.0,
                moments[cell][0],
                moments[cell][1],
            ];
            for (i, neighbor) in [cell - grid, cell + grid, cell - 1, cell + 1]
                .into_iter()
                .enumerate()
            {
                basis[2 + i] = sign(symbols[neighbor], bit);
            }
            basis
        };
        let field = Field::estimate(grid, 16, 24, false, |cell| {
            (cell_interior[cell]
                && known[cell].0 & (1 << bit) != 0
                && features[cell][axis].is_finite())
            .then(|| Sample {
                basis: chroma_context(cell),
                value: f64::from(features[cell][axis]),
                class: usize::from((symbols[cell] >> bit) & 1),
            })
        });
        for cell in 0..grid * grid {
            if !cell_interior[cell] || known[cell].0 & (1 << bit) != 0 {
                continue;
            }
            let Some(model) = field.at(cell % grid, cell / grid) else {
                continue;
            };
            let basis = chroma_context(cell);
            let interference = model.coefficients[0]
                + basis[2..]
                    .iter()
                    .zip(&model.coefficients[2..])
                    .map(|(a, b)| a * b)
                    .sum::<f64>();
            let signal = f64::from(features[cell][axis]) - interference;
            let positive = (signal - model.coefficients[1]).powi(2);
            let negative = (signal + model.coefficients[1]).powi(2);
            let llr = (negative - positive) / (2.0 * model.variance.max(1.0));
            if llr.is_finite() {
                output[cell][bit] = llr;
                if let Some(support) = support.as_deref_mut() {
                    support[cell] |= 1 << bit;
                }
            }
        }
    }
    output
}

#[cfg(test)]
#[path = "tests/recovery_fit_support.rs"]
mod support_tests;

#[cfg(test)]
#[path = "tests/recovery_fit_optimization.rs"]
mod optimization_tests;
