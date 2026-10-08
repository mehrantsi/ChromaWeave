use anyhow::{Result, ensure};
use image::{Rgb, RgbImage};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

pub use crate::geometry::{Homography, Point, bilinear};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureConfig {
    pub rotation_degrees: f64,
    pub perspective: f64,
    pub frame_side: u32,
    pub code_fraction: f64,
    pub seed: u64,
    pub clutter: bool,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            rotation_degrees: 17.0,
            perspective: 0.12,
            frame_side: 1024,
            code_fraction: 0.7,
            seed: 1,
            clutter: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CaptureResult {
    pub image: RgbImage,
    pub source_to_frame: Homography,
    pub corners: [Point; 4],
}

impl CaptureResult {
    pub fn footprint_area_pixels(&self) -> f64 {
        let mut twice_area = 0.0;
        for index in 0..4 {
            let [x, y] = self.corners[index];
            let [next_x, next_y] = self.corners[(index + 1) % 4];
            twice_area += x * next_y - y * next_x;
        }
        twice_area.abs() * 0.5
    }
}

pub fn simulate_capture(source: &RgbImage, config: &CaptureConfig) -> Result<CaptureResult> {
    ensure!(
        (1..=8192).contains(&source.width()) && (1..=8192).contains(&source.height()),
        "capture source dimensions must be between 1 and 8192"
    );
    ensure!(
        (16..=8192).contains(&config.frame_side),
        "capture frame side must be between 16 and 8192"
    );
    ensure!(
        config.rotation_degrees.is_finite(),
        "rotation must be finite"
    );
    ensure!(
        config.perspective.is_finite() && (0.0..=0.3).contains(&config.perspective),
        "perspective must be between 0 and 0.3"
    );
    ensure!(
        config.code_fraction.is_finite() && (0.05..=0.9).contains(&config.code_fraction),
        "code fraction must be between 0.05 and 0.9"
    );
    let mut rng = ChaCha8Rng::seed_from_u64(config.seed);
    let source_corners = [
        [-0.5, -0.5],
        [source.width() as f64 - 0.5, -0.5],
        [source.width() as f64 - 0.5, source.height() as f64 - 0.5],
        [-0.5, source.height() as f64 - 0.5],
    ];
    let max_source_side = source.width().max(source.height()) as f64;
    let half_width = source.width() as f64 / max_source_side * 0.5;
    let half_height = source.height() as f64 / max_source_side * 0.5;
    let mut corners = [
        [-half_width, -half_height],
        [half_width, -half_height],
        [half_width, half_height],
        [-half_width, half_height],
    ];
    let (sin, cos) = config.rotation_degrees.to_radians().sin_cos();
    for point in &mut corners {
        point[0] += rng.random_range(-1.0..1.0) * config.perspective * half_width;
        point[1] += rng.random_range(-1.0..1.0) * config.perspective * half_height;
        *point = [
            point[0] * cos - point[1] * sin,
            point[0] * sin + point[1] * cos,
        ];
    }
    let minimum: Point = std::array::from_fn(|axis| {
        corners
            .iter()
            .map(|p| p[axis])
            .fold(f64::INFINITY, f64::min)
    });
    let maximum: Point = std::array::from_fn(|axis| {
        corners
            .iter()
            .map(|p| p[axis])
            .fold(f64::NEG_INFINITY, f64::max)
    });
    let extent = [maximum[0] - minimum[0], maximum[1] - minimum[1]];
    let scale = config.frame_side as f64 * config.code_fraction / extent[0].max(extent[1]);
    let center: Point = std::array::from_fn(|axis| {
        (config.frame_side as f64 - 1.0) * 0.5
            + rng.random_range(-0.4..0.4) * (config.frame_side as f64 - extent[axis] * scale)
    });
    for point in &mut corners {
        for axis in 0..2 {
            point[axis] =
                (point[axis] - (minimum[axis] + maximum[axis]) * 0.5) * scale + center[axis];
        }
    }
    let source_to_frame = Homography::from_quad(source_corners, corners)?;
    let frame_to_source = source_to_frame.inverse()?;
    let mut frame = background(config.frame_side, config.clutter, &mut rng);
    for (x, y, pixel) in frame.enumerate_pixels_mut() {
        let Some([sx, sy]) = frame_to_source.transform([x as f64, y as f64]) else {
            continue;
        };
        if sx >= -0.5
            && sy >= -0.5
            && sx < source.width() as f64 - 0.5
            && sy < source.height() as f64 - 0.5
        {
            *pixel = bilinear(source, sx, sy);
        }
    }
    Ok(CaptureResult {
        image: frame,
        source_to_frame,
        corners,
    })
}

pub fn subsample_chroma420(image: &RgbImage) -> Result<RgbImage> {
    let (width, height) = image.dimensions();
    ensure!(
        (1..=8192).contains(&width) && (1..=8192).contains(&height),
        "chroma image dimensions must be between 1 and 8192"
    );
    let chroma_width = width.div_ceil(2);
    let chroma_height = height.div_ceil(2);
    let luminance = |[r, g, b]: [f32; 3]| 0.299 * r + 0.587 * g + 0.114 * b;
    let mut chroma = vec![[0.0_f32; 2]; (chroma_width * chroma_height) as usize];
    for cy in 0..chroma_height {
        for cx in 0..chroma_width {
            let value = &mut chroma[(cy * chroma_width + cx) as usize];
            for dy in 0..2 {
                for dx in 0..2 {
                    let rgb = image
                        .get_pixel((cx * 2 + dx).min(width - 1), (cy * 2 + dy).min(height - 1))
                        .0
                        .map(f32::from);
                    let y = luminance(rgb);
                    value[0] += (rgb[2] - y) / (4.0 * 1.772);
                    value[1] += (rgb[0] - y) / (4.0 * 1.402);
                }
            }
        }
    }
    Ok(RgbImage::from_fn(width, height, |x, y| {
        let sx = (x as f32 * 0.5 - 0.25).clamp(0.0, (chroma_width - 1) as f32);
        let sy = (y as f32 * 0.5 - 0.25).clamp(0.0, (chroma_height - 1) as f32);
        let x0 = sx.floor() as u32;
        let y0 = sy.floor() as u32;
        let x1 = (x0 + 1).min(chroma_width - 1);
        let y1 = (y0 + 1).min(chroma_height - 1);
        let tx = sx - x0 as f32;
        let ty = sy - y0 as f32;
        let [cb, cr] = std::array::from_fn(|channel| {
            let a = chroma[(y0 * chroma_width + x0) as usize][channel];
            let b = chroma[(y0 * chroma_width + x1) as usize][channel];
            let c = chroma[(y1 * chroma_width + x0) as usize][channel];
            let d = chroma[(y1 * chroma_width + x1) as usize][channel];
            (a * (1.0 - tx) + b * tx) * (1.0 - ty) + (c * (1.0 - tx) + d * tx) * ty
        });
        let luma = luminance(image.get_pixel(x, y).0.map(f32::from));
        Rgb([
            luma + 1.402 * cr,
            luma - (0.299 * 1.402 * cr + 0.114 * 1.772 * cb) / 0.587,
            luma + 1.772 * cb,
        ]
        .map(|v| v.round().clamp(0.0, 255.0) as u8))
    }))
}

pub fn jpeg_roundtrip(image: &RgbImage, quality: u8) -> Result<RgbImage> {
    ensure!(
        (1..=100).contains(&quality),
        "JPEG quality must be between 1 and 100"
    );
    ensure!(
        (1..=8192).contains(&image.width()) && (1..=8192).contains(&image.height()),
        "JPEG image dimensions must be between 1 and 8192"
    );
    let mut encoded = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, quality).encode(
        image.as_raw(),
        image.width(),
        image.height(),
        image::ExtendedColorType::Rgb8,
    )?;
    Ok(image::load_from_memory_with_format(&encoded, image::ImageFormat::Jpeg)?.to_rgb8())
}

pub fn illuminate(image: &RgbImage, strength: f64) -> Result<RgbImage> {
    ensure!(
        strength.is_finite() && (0.0..=1.0).contains(&strength),
        "illumination strength must be between 0 and 1"
    );
    ensure!(
        (1..=8192).contains(&image.width()) && (1..=8192).contains(&image.height()),
        "illumination image dimensions must be between 1 and 8192"
    );
    if strength == 0.0 {
        return Ok(image.clone());
    }
    Ok(RgbImage::from_fn(image.width(), image.height(), |x, y| {
        let nx = (f64::from(x) + 0.5) / f64::from(image.width()) * 2.0 - 1.0;
        let ny = (f64::from(y) + 0.5) / f64::from(image.height()) * 2.0 - 1.0;
        let exposure = 1.0 + strength * (0.32 * nx + 0.16 * ny - 0.12 * (nx * nx + ny * ny));
        let warmth = strength * 0.08 * nx;
        let gains = [exposure + warmth, exposure, exposure - warmth];
        let source = image.get_pixel(x, y);
        Rgb(std::array::from_fn(|channel| {
            (f64::from(source[channel]) * gains[channel])
                .round()
                .clamp(0.0, 255.0) as u8
        }))
    }))
}

fn background(side: u32, clutter: bool, rng: &mut ChaCha8Rng) -> RgbImage {
    let base = rng.random_range(218.0..240.0);
    let tint: [f64; 3] = std::array::from_fn(|_| rng.random_range(-5.0..5.0));
    let gradient: Point = std::array::from_fn(|_| rng.random_range(-8.0..8.0));
    let mut image = RgbImage::from_fn(side, side, |x, y| {
        let intensity = base
            + gradient[0] * x as f64 / side as f64
            + gradient[1] * y as f64 / side as f64
            + rng.random_range(-1.5..1.5);
        Rgb(std::array::from_fn(|channel| {
            (intensity + tint[channel]).round().clamp(0.0, 255.0) as u8
        }))
    });
    if clutter {
        for _ in 0..8 {
            let x0 = rng.random_range(0..side);
            let y0 = rng.random_range(0..side);
            let width = rng.random_range((side / 40).max(1)..=(side / 5).max(1));
            let height = rng.random_range((side / 80).max(1)..=(side / 8).max(1));
            let color = Rgb(std::array::from_fn(|_| rng.random_range(70..200)));
            for y in y0..y0.saturating_add(height).min(side) {
                for x in x0..x0.saturating_add(width).min(side) {
                    image.put_pixel(x, y, color);
                }
            }
        }
    }
    image
}

#[cfg(test)]
#[path = "tests/capture.rs"]
mod tests;
