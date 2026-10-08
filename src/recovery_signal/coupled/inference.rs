use super::{COEFFICIENT_COUNT, Model, chroma_coefficient, neighbor_coefficient};
use crate::recovery_signal::Decision;

pub(super) struct InferenceField<'a> {
    pub(super) grid: usize,
    pub(super) side: usize,
    pub(super) model_side: usize,
    pub(super) data_mask: &'a [bool],
    pub(super) initial: &'a [Decision],
    pub(super) ranks: &'a [u8],
    pub(super) values: &'a [f64],
    pub(super) fitting: &'a [bool],
    pub(super) intervals: &'a [(usize, f64)],
    pub(super) models: &'a [Option<Model>],
}

impl InferenceField<'_> {
    pub(super) fn classify(
        &self,
        start_cell: usize,
        output: &mut [Decision],
        next: &mut [u8],
    ) -> bool {
        let mut changed = false;
        for (offset, decision) in output.iter_mut().enumerate() {
            let cell = start_cell + offset;
            if !self.data_mask[cell] {
                continue;
            }
            for quadrant in 0..4 {
                let x = cell % self.grid * 2 + quadrant % 2;
                let y = cell / self.grid * 2 + quadrant / 2;
                let index = y * self.side + x;
                let model = self.fitting[index]
                    .then(|| interpolate(self.intervals, self.model_side, self.models, x, y))
                    .flatten();
                let Some(model) = model else {
                    decision.0 = (decision.0 & !(3 << (2 * quadrant)))
                        | (self.initial[cell].0 & (3 << (2 * quadrant)));
                    decision.1[2 * quadrant..2 * quadrant + 2]
                        .copy_from_slice(&self.initial[cell].1[2 * quadrant..2 * quadrant + 2]);
                    let gray = ((self.initial[cell].0 >> (2 * quadrant)) & 3) as u8;
                    next[index - start_cell * 4] = gray ^ (gray >> 1);
                    changed |= next[index - start_cell * 4] != self.ranks[index];
                    continue;
                };
                let neighbor_indices = [
                    y * self.side + (x + 1).min(self.side - 1),
                    y * self.side + x.saturating_sub(1),
                    (y + 1).min(self.side - 1) * self.side + x,
                    y.saturating_sub(1) * self.side + x,
                ];
                let interference = neighbor_indices
                    .into_iter()
                    .enumerate()
                    .map(|(i, n)| {
                        let r = usize::from(self.ranks[n]);
                        if r == 0 {
                            0.0
                        } else {
                            model.coefficients[neighbor_coefficient(i, r)]
                        }
                    })
                    .sum::<f64>()
                    + model.coefficients[chroma_coefficient(0)]
                        * (f64::from((self.initial[cell].0 >> 8) & 1) * 2.0 - 1.0)
                    + model.coefficients[chroma_coefficient(1)]
                        * (f64::from((self.initial[cell].0 >> 9) & 1) * 2.0 - 1.0);
                let signal = self.values[index] - interference;
                let distances: [f64; 4] =
                    std::array::from_fn(|rank| (signal - model.coefficients[rank]).powi(2));
                let rank = (0..4)
                    .min_by(|&a, &b| distances[a].total_cmp(&distances[b]))
                    .unwrap();
                next[index - start_cell * 4] = rank as u8;
                changed |= next[index - start_cell * 4] != self.ranks[index];
                let gray = rank ^ (rank >> 1);
                decision.0 =
                    (decision.0 & !(3 << (2 * quadrant))) | ((gray as u16) << (2 * quadrant));
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
    }
}

pub(super) fn interpolate(
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
        coefficients: [0.0; COEFFICIENT_COUNT],
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
