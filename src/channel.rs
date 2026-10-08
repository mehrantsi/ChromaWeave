use anyhow::{Result, ensure};
use image::imageops::{FilterType, resize};
use image::{Rgb, RgbImage};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
pub enum Medium {
    Screen,
    Print,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelConfig {
    pub medium: Medium,
    pub severity: f32,
    pub seed: u64,
}

pub fn simulate(image: &RgbImage, config: &ChannelConfig) -> Result<RgbImage> {
    let severity = config.severity;
    ensure!(
        severity.is_finite() && (0.0..=1.0).contains(&severity),
        "severity must be finite and between 0 and 1"
    );

    if severity == 0.0 || image.width() == 0 || image.height() == 0 {
        return Ok(image.clone());
    }

    let mut rng = ChaCha8Rng::seed_from_u64(config.seed);
    let medium_image = match config.medium {
        Medium::Screen => screen_medium(image, severity, &mut rng),
        Medium::Print => print_medium(image, severity),
    };

    let mut camera = medium_image.clone();
    let blurred = gaussian_blur(&medium_image);
    blend(&mut camera, &blurred, severity * 0.22);

    let camera_sample = resample(&camera, 1.0 + severity * 1.8, FilterType::Triangle);
    blend(&mut camera, &camera_sample, severity * 0.55);

    let gains: [f32; 3] = std::array::from_fn(|_| 1.0 + signed(&mut rng) * severity * 0.08);
    let offsets: [f32; 3] = std::array::from_fn(|_| signed(&mut rng) * severity * 4.0);
    let cross_talk: [f32; 3] = std::array::from_fn(|_| signed(&mut rng) * severity * 0.025);

    for pixel in camera.pixels_mut() {
        let original = [pixel[0] as f32, pixel[1] as f32, pixel[2] as f32];
        let shifted = [
            original[0] * gains[0] + offsets[0] + cross_talk[0] * (original[2] - original[1]),
            original[1] * gains[1] + offsets[1] + cross_talk[1] * (original[0] - original[2]),
            original[2] * gains[2] + offsets[2] + cross_talk[2] * (original[1] - original[0]),
        ];

        for channel in 0..3 {
            pixel[channel] = clamp_channel(shifted[channel] + gaussian(&mut rng) * severity * 3.5);
        }
    }

    Ok(camera)
}

fn screen_medium(image: &RgbImage, severity: f32, rng: &mut ChaCha8Rng) -> RgbImage {
    let raster = resample(image, 1.0 + severity * 1.4, FilterType::Nearest);
    let mut output = image.clone();
    blend(&mut output, &raster, severity * 0.25);

    let drift: [f32; 3] = std::array::from_fn(|_| signed(rng) * severity * 7.0);
    for pixel in output.pixels_mut() {
        for channel in 0..3 {
            pixel[channel] = clamp_channel(pixel[channel] as f32 + drift[channel]);
        }
    }

    output
}

fn print_medium(image: &RgbImage, severity: f32) -> RgbImage {
    let mut dots = halftone_cmy(image);
    let expanded_ink = expand_ink(&dots);
    blend(&mut dots, &expanded_ink, severity * 0.24);

    let mut output = image.clone();
    blend(&mut output, &dots, severity * 0.45);
    output
}

fn signed(rng: &mut ChaCha8Rng) -> f32 {
    rng.random_range(-1.0..1.0)
}

fn gaussian(rng: &mut ChaCha8Rng) -> f32 {
    let u1 = rng.random::<f32>().max(f32::MIN_POSITIVE);
    let u2 = rng.random::<f32>();
    (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
}

fn clamp_channel(value: f32) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

fn blend(target: &mut RgbImage, layer: &RgbImage, amount: f32) {
    for (target, layer) in target.pixels_mut().zip(layer.pixels()) {
        for channel in 0..3 {
            target[channel] = clamp_channel(
                target[channel] as f32 * (1.0 - amount) + layer[channel] as f32 * amount,
            );
        }
    }
}

fn resample(image: &RgbImage, divisor: f32, filter: FilterType) -> RgbImage {
    let width = image.width();
    let height = image.height();
    let sampled_width = ((width as f32 / divisor).round() as u32).max(1);
    let sampled_height = ((height as f32 / divisor).round() as u32).max(1);
    let sampled = resize(image, sampled_width, sampled_height, filter);
    resize(&sampled, width, height, filter)
}

fn gaussian_blur(image: &RgbImage) -> RgbImage {
    let width = image.width();
    let height = image.height();
    let mut output = RgbImage::new(width, height);
    let weights = [[1_u32, 2, 1], [2, 4, 2], [1, 2, 1]];

    for y in 0..height {
        for x in 0..width {
            let mut sums = [0_u32; 3];
            for (dy_index, dy) in [-1_i32, 0, 1].iter().copied().enumerate() {
                for (dx_index, dx) in [-1_i32, 0, 1].iter().copied().enumerate() {
                    let sample_x = offset(x, dx, width - 1);
                    let sample_y = offset(y, dy, height - 1);
                    let sample = image.get_pixel(sample_x, sample_y);
                    let weight = weights[dy_index][dx_index];
                    for channel in 0..3 {
                        sums[channel] += sample[channel] as u32 * weight;
                    }
                }
            }
            output.put_pixel(
                x,
                y,
                Rgb(std::array::from_fn(|channel| {
                    ((sums[channel] + 8) / 16) as u8
                })),
            );
        }
    }

    output
}

fn offset(coordinate: u32, delta: i32, maximum: u32) -> u32 {
    if delta < 0 {
        coordinate.saturating_sub((-delta) as u32)
    } else {
        coordinate.saturating_add(delta as u32).min(maximum)
    }
}

fn halftone_cmy(image: &RgbImage) -> RgbImage {
    const BAYER: [[u8; 8]; 8] = [
        [0, 48, 12, 60, 3, 51, 15, 63],
        [32, 16, 44, 28, 35, 19, 47, 31],
        [8, 56, 4, 52, 11, 59, 7, 55],
        [40, 24, 36, 20, 43, 27, 39, 23],
        [2, 50, 14, 62, 1, 49, 13, 61],
        [34, 18, 46, 30, 33, 17, 45, 29],
        [10, 58, 6, 54, 9, 57, 5, 53],
        [42, 26, 38, 22, 41, 25, 37, 21],
    ];
    let mut output = RgbImage::new(image.width(), image.height());

    for y in 0..image.height() {
        for x in 0..image.width() {
            let pixel = image.get_pixel(x, y);
            let mut result = [0; 3];
            for channel in 0..3 {
                let (screen_x, screen_y) = match channel {
                    0 => (x, y),
                    1 => (y, 7 - (x & 7)),
                    _ => (7 - (x & 7), 7 - (y & 7)),
                };
                let threshold = BAYER[(screen_y & 7) as usize][(screen_x & 7) as usize] as f32;
                let ink_coverage = 1.0 - pixel[channel] as f32 / 255.0;
                result[channel] = if ink_coverage * 64.0 > threshold {
                    0
                } else {
                    255
                };
            }
            output.put_pixel(x, y, Rgb(result));
        }
    }

    output
}

fn expand_ink(image: &RgbImage) -> RgbImage {
    let width = image.width();
    let height = image.height();
    let mut output = RgbImage::new(width, height);

    for y in 0..height {
        for x in 0..width {
            let mut minimum = [u8::MAX; 3];
            for dy in [-1_i32, 0, 1].iter().copied() {
                for dx in [-1_i32, 0, 1].iter().copied() {
                    let sample_x = offset(x, dx, width - 1);
                    let sample_y = offset(y, dy, height - 1);
                    let sample = image.get_pixel(sample_x, sample_y);
                    for channel in 0..3 {
                        minimum[channel] = minimum[channel].min(sample[channel]);
                    }
                }
            }
            output.put_pixel(x, y, Rgb(minimum));
        }
    }

    output
}

#[cfg(test)]
#[path = "tests/channel.rs"]
mod tests;
