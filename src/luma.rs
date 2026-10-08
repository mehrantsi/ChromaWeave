use image::Rgb;

pub const PILOTS: usize = 32;
pub const MAX_GRID_CELLS: u32 = (crate::spatial_equalizer::MAX_LATTICE_SIDE as u32 - 4) / 2;
const WALSH_COLUMNS: [usize; 10] = [1, 2, 4, 8, 16, 3, 5, 6, 9, 10];

pub fn pilot_symbol(index: usize) -> u16 {
    let bit = |axis: usize| ((index & WALSH_COLUMNS[axis]).count_ones() & 1) as u16;
    let mut symbol = 0;
    for quadrant in 0..4 {
        let rank = 2 * bit(2 * quadrant) + bit(2 * quadrant + 1);
        symbol |= (rank ^ (rank >> 1)) << (2 * quadrant);
    }
    symbol | (bit(8) << 8) | (bit(9) << 9)
}

pub fn latent(symbol: u16, axis: usize) -> f32 {
    if axis < 4 {
        let gray = (symbol >> (2 * axis)) & 3;
        let rank = gray ^ (gray >> 1);
        2.0 * f32::from(rank) - 3.0
    } else if symbol & (1 << (axis + 4)) == 0 {
        -1.0
    } else {
        1.0
    }
}

pub fn rgb(symbol: u16, u: f32, v: f32) -> Rgb<u8> {
    let quadrant = usize::from(u >= 0.5) + 2 * usize::from(v >= 0.5);
    let y = 128.0 + 28.0 * latent(symbol, quadrant);
    let cb = 20.0 * latent(symbol, 4);
    let cr = 20.0 * latent(symbol, 5);
    Rgb([
        y + 1.402 * cr,
        y - (0.299 * 1.402 * cr + 0.114 * 1.772 * cb) / 0.587,
        y + 1.772 * cb,
    ]
    .map(|value| value.round() as u8))
}

pub fn feature(samples: [[f32; 3]; 4]) -> [f32; 12] {
    let mut output = [0.0; 12];
    for (quadrant, [r, g, b]) in samples.into_iter().enumerate() {
        let y = 0.299 * r + 0.587 * g + 0.114 * b;
        output[quadrant] = y;
        output[4] += (b - y) / (4.0 * 1.772);
        output[5] += (r - y) / (4.0 * 1.402);
    }
    output
}

pub(crate) fn classify_levels(
    latent: [f32; 10],
    noise: [f32; 10],
    luminance_levels: [f32; 4],
) -> (u16, [f32; 10]) {
    let mut symbol = 0;
    let mut confidence = [0.0; 10];
    for axis in 0..6 {
        let (levels, bits) = if axis < 4 { (4, 2) } else { (2, 1) };
        let mut distances = [[f32::INFINITY; 2]; 2];
        for (rank, level) in luminance_levels.iter().enumerate().take(levels) {
            let target = if axis < 4 {
                *level
            } else {
                2.0 * rank as f32 - 1.0
            };
            let gray = rank ^ (rank >> 1);
            let error = (latent[axis] - target).powi(2);
            for (bit, distance) in distances.iter_mut().enumerate().take(bits) {
                let value = (gray >> bit) & 1;
                distance[value] = distance[value].min(error);
            }
        }
        for (bit, distance) in distances.iter().enumerate().take(bits) {
            let index = if axis < 4 { axis * 2 + bit } else { axis + 4 };
            if distance[1] < distance[0] {
                symbol |= 1 << index;
            }
            confidence[index] = (distance[0] - distance[1]).abs()
                / (distance[0] + distance[1] + 2.0 * noise[axis].max(0.01));
        }
    }
    (symbol, confidence)
}

#[cfg(test)]
#[path = "tests/luma.rs"]
mod tests;
