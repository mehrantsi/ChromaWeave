//! Framed encoding and scanning with automatic finder and orientation detection.

use anyhow::{Result, anyhow, bail, ensure};
use image::{Rgb, RgbImage};

use crate::{codec, geometry::Homography};

mod sampling;

use sampling::{PreparedLuma, prepare_luma};
#[cfg(test)]
use sampling::{color_registration, sample_channel_lattice, sample_lattice};

const FINDER_CENTER: f64 = 5.5;
const MAX_METADATA_GRID: u32 = 0x7ff;

fn finder_span(grid: u32) -> f64 {
    f64::from(grid + 4) - 2.0 * FINDER_CENTER
}

pub fn maximum_payload_bytes(cell_pixels: u32) -> Result<usize> {
    maximum_payload_bytes_with_modulation(cell_pixels, codec::Modulation::Luma10)
}

pub fn maximum_payload_bytes_with_modulation(
    cell_pixels: u32,
    modulation: codec::Modulation,
) -> Result<usize> {
    codec::validate_cell_pixels(cell_pixels, modulation)?;
    let grid = (codec::MAX_IMAGE_SIDE / cell_pixels - 4)
        .min(MAX_METADATA_GRID)
        .min(modulation.maximum_grid_cells());
    Ok(
        codec::layout_for_grid_with_modulation(grid, cell_pixels, 0, modulation)?
            .payload_capacity_bytes,
    )
}

/// Encodes a framed Luma10 symbol with standard error correction.
pub fn encode(payload: &[u8], cell_pixels: u32) -> Result<codec::Encoded> {
    encode_with_modulation(payload, cell_pixels, codec::Modulation::Luma10)
}

pub fn encode_with_modulation(
    payload: &[u8],
    cell_pixels: u32,
    modulation: codec::Modulation,
) -> Result<codec::Encoded> {
    encode_with_options(
        payload,
        cell_pixels,
        modulation,
        codec::EncodeOptions::default(),
    )
}

pub fn encode_with_options(
    payload: &[u8],
    cell_pixels: u32,
    modulation: codec::Modulation,
    options: codec::EncodeOptions,
) -> Result<codec::Encoded> {
    let capacity = maximum_payload_bytes_with_modulation(cell_pixels, modulation)?;
    if options.compression == codec::Compression::None {
        ensure!(
            payload.len() <= capacity,
            "payload exceeds framed image bounds"
        );
    }
    let mut encoded = codec::encode_with_options(payload, cell_pixels, modulation, options)?;
    let side = encoded.image.width();
    ensure!(
        side <= codec::MAX_IMAGE_SIDE,
        "framed image exceeds prototype bounds"
    );
    let mut image = encoded.image;
    let finder_pixels = cell_pixels;
    for (x, y) in [
        (2, 2),
        (side / finder_pixels - 9, 2),
        (2, side / finder_pixels - 9),
        (side / finder_pixels - 9, side / finder_pixels - 9),
    ] {
        for my in 0..7 {
            for mx in 0..7 {
                if mx == 0
                    || mx == 6
                    || my == 0
                    || my == 6
                    || ((2..=4).contains(&mx) && (2..=4).contains(&my))
                {
                    for py in 0..finder_pixels {
                        for px in 0..finder_pixels {
                            image.put_pixel(
                                (x + mx) * finder_pixels + px,
                                (y + my) * finder_pixels + py,
                                Rgb([0; 3]),
                            );
                        }
                    }
                }
            }
        }
    }
    let word = encode_metadata_with_modulation(encoded.layout.grid_cells, modulation);
    let center = FINDER_CENTER * f64::from(cell_pixels);
    let span = f64::from(side) - 2.0 * center;
    for x in 0..side {
        let t = ((f64::from(x) + 0.5 - center) / span - 0.15) / 0.7;
        if !(0.0..1.0).contains(&t) || word & (1 << (31 - (t * 32.0) as u32)) == 0 {
            continue;
        }
        for y in 4 * finder_pixels..7 * finder_pixels {
            image.put_pixel(x, y, Rgb([0; 3]));
            image.put_pixel(x, side - 1 - y, Rgb([0; 3]));
        }
    }
    encoded.image = image;
    framed_layout(&mut encoded.layout, cell_pixels);
    Ok(encoded)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrameMetadata {
    grid: u32,
    modulation: codec::Modulation,
}

fn metadata_checksum(grid: u32, modulation: codec::Modulation) -> u32 {
    crc32fast::hash(&[modulation.metadata_magic(), (grid >> 8) as u8, grid as u8]) & 0x7f
}

#[cfg(test)]
fn encode_metadata(grid: u32) -> u32 {
    encode_metadata_with_modulation(grid, codec::Modulation::Luma10)
}

fn encode_metadata_with_modulation(grid: u32, modulation: codec::Modulation) -> u32 {
    let data = (u32::from(modulation.metadata_magic()) << 18)
        | (grid << 7)
        | metadata_checksum(grid, modulation);
    let mut word = 0u32;
    let mut bit = 0;
    for position in 1u32..32 {
        if !position.is_power_of_two() {
            word |= ((data >> bit) & 1) << (position - 1);
            bit += 1;
        }
    }
    for parity in [1, 2, 4, 8, 16] {
        let value = (1..32)
            .filter(|position| position & parity != 0)
            .fold(0, |value, position| value ^ ((word >> (position - 1)) & 1));
        word |= value << (parity - 1);
    }
    word | ((word.count_ones() & 1) << 31)
}

#[cfg(test)]
fn decode_metadata(word: u32) -> Option<u32> {
    decode_metadata_with_modulation(word)
        .filter(|metadata| metadata.modulation == codec::Modulation::Luma10)
        .map(|metadata| metadata.grid)
}

fn decode_metadata_with_modulation(mut word: u32) -> Option<FrameMetadata> {
    let syndrome = (1..32)
        .filter(|position| word & (1 << (position - 1)) != 0)
        .fold(0, |syndrome, position| syndrome ^ position);
    let odd_parity = word.count_ones() & 1 != 0;
    if syndrome != 0 {
        if !odd_parity {
            return None;
        }
        word ^= 1 << (syndrome - 1);
    }
    let mut data = 0;
    let mut bit = 0;
    for position in 1u32..32 {
        if !position.is_power_of_two() {
            data |= ((word >> (position - 1)) & 1) << bit;
            bit += 1;
        }
    }
    let grid = (data >> 7) & MAX_METADATA_GRID;
    let modulation = match data >> 18 {
        0xcd => codec::Modulation::Luma10,
        0xce => codec::Modulation::Luma10LowEcc,
        _ => return None,
    };
    if data & 0x7f != metadata_checksum(grid, modulation) {
        return None;
    }
    (grid >= modulation.minimum_grid_cells()
        && grid <= modulation.maximum_grid_cells()
        && (grid + 4) * modulation.minimum_cell_pixels() <= codec::MAX_IMAGE_SIDE)
        .then_some(FrameMetadata { grid, modulation })
}

fn framed_layout(layout: &mut codec::Layout, cell_pixels: u32) {
    layout.format = if layout.compression == "zstd" {
        match layout.modulation {
            codec::Modulation::Luma10 => "CWI5-luma10-framed-experimental",
            codec::Modulation::Luma10LowEcc => "CWE5-luma10-low-ecc-framed-experimental",
        }
    } else {
        match layout.modulation {
            codec::Modulation::Luma10 => "CWI4-luma10-framed-experimental",
            codec::Modulation::Luma10LowEcc => "CWE4-luma10-low-ecc-framed-experimental",
        }
    };
    layout.image_side_pixels = (layout.grid_cells + 4) * cell_pixels;
    layout.net_bits_per_pixel =
        layout.payload_bytes as f64 * 8.0 / f64::from(layout.image_side_pixels).powi(2);
}

#[derive(Clone)]
struct Component {
    area: usize,
    min: [u32; 2],
    max: [u32; 2],
    extremes: [[u32; 2]; 16],
}

struct ComponentBuilder {
    component: Component,
    projections: [f64; 16],
}

impl ComponentBuilder {
    fn new() -> Self {
        Self {
            component: Component {
                area: 0,
                min: [u32::MAX; 2],
                max: [0; 2],
                extremes: [[0; 2]; 16],
            },
            projections: [f64::NEG_INFINITY; 16],
        }
    }

    fn add(&mut self, x: u32, y: u32, boundary: bool, directions: &[[f64; 2]; 16]) {
        let component = &mut self.component;
        component.area += 1;
        component.min[0] = component.min[0].min(x);
        component.min[1] = component.min[1].min(y);
        component.max[0] = component.max[0].max(x);
        component.max[1] = component.max[1].max(y);
        if !boundary {
            return;
        }
        for (i, direction) in directions.iter().enumerate() {
            let projection = f64::from(x) * direction[0] + f64::from(y) * direction[1];
            if projection > self.projections[i] {
                self.projections[i] = projection;
                component.extremes[i] = [x, y];
            }
        }
    }

    fn add_run(&mut self, left: u32, right: u32, y: u32, directions: &[[f64; 2]; 16]) {
        self.add(left, y, true, directions);
        if right > left {
            self.add(right, y, true, directions);
            self.component.area += (right - left - 1) as usize;
        }
    }
}

impl Component {
    fn quad(&self) -> Option<[[f64; 2]; 4]> {
        let mut points = self
            .extremes
            .map(|[x, y]| [f64::from(x), f64::from(y)])
            .to_vec();
        points.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
        points.dedup();
        let mut hull = Vec::new();
        for p in &points {
            while hull.len() >= 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], *p) <= 0.0 {
                hull.pop();
            }
            hull.push(*p);
        }
        let lower = hull.len();
        for p in points.iter().rev().skip(1) {
            while hull.len() > lower && cross(hull[hull.len() - 2], hull[hull.len() - 1], *p) <= 0.0
            {
                hull.pop();
            }
            hull.push(*p);
        }
        hull.pop();
        while hull.len() > 4 {
            let n = hull.len();
            let index = (0..n).min_by(|&a, &b| {
                cross(hull[(a + n - 1) % n], hull[a], hull[(a + 1) % n])
                    .abs()
                    .total_cmp(&cross(hull[(b + n - 1) % n], hull[b], hull[(b + 1) % n]).abs())
            })?;
            hull.remove(index);
        }
        if hull.len() != 4 {
            return None;
        }
        Some([hull[0], hull[1], hull[2], hull[3]])
    }

    fn center(&self) -> Option<[f64; 2]> {
        Homography::from_quad(
            [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            self.quad()?,
        )
        .ok()?
        .transform([0.5, 0.5])
    }
}

fn cross(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn finder_dark_mask(image: &RgbImage) -> Vec<u8> {
    const MIN_LOCAL_CONTRAST: u32 = 8;
    let (w, h) = (image.width() as usize, image.height() as usize);
    let mut luminance: Vec<u8> = image
        .as_raw()
        .as_chunks::<3>()
        .0
        .iter()
        .map(|pixel| {
            ((77 * u32::from(pixel[0]) + 150 * u32::from(pixel[1]) + 29 * u32::from(pixel[2])) >> 8)
                as u8
        })
        .collect();
    let radius = (w.min(h) / 128).max(16);
    let window = 2 * radius + 1;
    let mut row_sums = vec![0_u32; window * w];
    let mut column_sums = vec![0_u32; w];
    let mut next_row = 0;
    for y in 0..h {
        if y > radius {
            let row = (y - radius - 1) % window * w;
            for x in 0..w {
                column_sums[x] -= row_sums[row + x];
            }
        }
        while next_row < (y + radius + 1).min(h) {
            let source = &luminance[next_row * w..(next_row + 1) * w];
            let row = next_row % window * w;
            let mut sum: u32 = source[..(radius + 1).min(w)]
                .iter()
                .map(|&v| u32::from(v))
                .sum();
            for x in 0..w {
                if x > 0 {
                    if x > radius {
                        sum -= u32::from(source[x - radius - 1]);
                    }
                    if x + radius < w {
                        sum += u32::from(source[x + radius]);
                    }
                }
                row_sums[row + x] = sum;
                column_sums[x] += sum;
            }
            next_row += 1;
        }
        let rows = (y + radius + 1).min(h) - y.saturating_sub(radius);
        for (x, &sum) in column_sums.iter().enumerate() {
            let columns = (x + radius + 1).min(w) - x.saturating_sub(radius);
            let index = y * w + x;
            luminance[index] = u8::from(
                (u32::from(luminance[index]) + MIN_LOCAL_CONTRAST) * (rows * columns) as u32 <= sum,
            );
        }
    }
    luminance
}

fn dark_components(image: &RgbImage, scale: u32) -> Vec<Component> {
    let (w, h) = image.dimensions();
    let mut dark = finder_dark_mask(image);
    let directions = std::array::from_fn(|i| {
        let angle = i as f64 * std::f64::consts::TAU / 16.0;
        [angle.cos(), angle.sin()]
    });
    let mut components = Vec::new();
    let mut stack = Vec::new();
    for seed in 0..dark.len() {
        if dark[seed] != 1 {
            continue;
        }
        stack.push(seed);
        let mut builder = ComponentBuilder::new();
        while let Some(index) = stack.pop() {
            if dark[index] != 1 {
                continue;
            }
            let x = index as u32 % w;
            let y = index as u32 / w;
            let row = (y * w) as usize;
            let mut left = x;
            let mut right = x;
            while left > 0 && dark[row + left as usize - 1] == 1 {
                left -= 1;
            }
            while right + 1 < w && dark[row + right as usize + 1] == 1 {
                right += 1;
            }
            dark[row + left as usize..=row + right as usize].fill(2);
            builder.add_run(left, right, y, &directions);
            for adjacent_y in [y.wrapping_sub(1), y + 1] {
                if adjacent_y >= h {
                    continue;
                }
                let adjacent = (adjacent_y * w) as usize;
                let mut in_run = false;
                for nx in left..=right {
                    let next = adjacent + nx as usize;
                    let foreground = dark[next] == 1;
                    if foreground && !in_run {
                        stack.push(next);
                    }
                    in_run = foreground;
                }
            }
        }
        let component = builder.component;
        if component.area >= (8 / scale.pow(2)).max(2) as usize
            && component.max[0] - component.min[0] >= (2 / scale).max(1)
            && component.max[1] - component.min[1] >= (2 / scale).max(1)
        {
            components.push(component);
        }
    }
    components.sort_by_key(|c| std::cmp::Reverse(c.area));
    components
}

#[cfg(test)]
fn finders(image: &RgbImage) -> Vec<[f64; 2]> {
    finders_at_scale(
        image,
        if image.width().max(image.height()) > 2048 {
            2
        } else {
            1
        },
    )
}

fn downsample_for_finders(image: &RgbImage) -> RgbImage {
    let (width, height) = image.dimensions();
    let raw = image.as_raw();
    let mut output = RgbImage::new(width.div_ceil(2), height.div_ceil(2));
    if output.is_empty() {
        return output;
    }
    let input_stride = width as usize * 3;
    let output_stride = output.width() as usize * 3;
    for (y, output_row) in output.as_mut().chunks_exact_mut(output_stride).enumerate() {
        let top_y = y * 2;
        let bottom_y = (top_y + 1).min(height as usize - 1);
        let top = &raw[top_y * input_stride..(top_y + 1) * input_stride];
        let bottom = &raw[bottom_y * input_stride..(bottom_y + 1) * input_stride];
        for ((pixel, a), b) in output_row
            .as_chunks_mut::<3>()
            .0
            .iter_mut()
            .zip(top.as_chunks::<6>().0)
            .zip(bottom.as_chunks::<6>().0)
        {
            for channel in 0..3 {
                pixel[channel] = ((u16::from(a[channel])
                    + u16::from(a[channel + 3])
                    + u16::from(b[channel])
                    + u16::from(b[channel + 3])
                    + 2)
                    / 4) as u8;
            }
        }
        if !width.is_multiple_of(2) {
            let pixel = &mut output_row[output_stride - 3..];
            for channel in 0..3 {
                pixel[channel] = ((2 * u16::from(top[input_stride - 3 + channel])
                    + 2 * u16::from(bottom[input_stride - 3 + channel])
                    + 2)
                    / 4) as u8;
            }
        }
    }
    output
}

fn finders_at_scale(image: &RgbImage, scale: u32) -> Vec<[f64; 2]> {
    const BUCKET_PIXELS: u32 = 64;
    let detector = (scale == 2).then(|| downsample_for_finders(image));
    let detector = detector.as_ref().unwrap_or(image);
    let components = dark_components(detector, scale);
    let columns = detector.width().div_ceil(BUCKET_PIXELS) as usize;
    let mut buckets =
        vec![Vec::new(); columns * detector.height().div_ceil(BUCKET_PIXELS) as usize];
    for (index, component) in components.iter().enumerate() {
        let x = (component.min[0] + component.max[0]) / 2 / BUCKET_PIXELS;
        let y = (component.min[1] + component.max[1]) / 2 / BUCKET_PIXELS;
        buckets[y as usize * columns + x as usize].push(index);
    }
    let mut found = Vec::new();
    'outer: for outer in &components {
        let width = outer.max[0] - outer.min[0] + 1;
        let height = outer.max[1] - outer.min[1] + 1;
        let density = outer.area as f64 / f64::from(width * height);
        if !(0.15..0.8).contains(&density) {
            continue;
        }
        let Some(center) = outer.center() else {
            continue;
        };
        'inner: for y in outer.min[1] / BUCKET_PIXELS..=outer.max[1] / BUCKET_PIXELS {
            for x in outer.min[0] / BUCKET_PIXELS..=outer.max[0] / BUCKET_PIXELS {
                for &index in &buckets[y as usize * columns + x as usize] {
                    let inner = &components[index];
                    let ratio = outer.area as f64 / inner.area as f64;
                    if !(1.4..4.5).contains(&ratio) {
                        continue;
                    }
                    if !(0..2).all(|i| inner.min[i] > outer.min[i] && inner.max[i] < outer.max[i]) {
                        continue;
                    }
                    let Some(core) = inner.center() else {
                        continue;
                    };
                    if (center[0] - core[0]).hypot(center[1] - core[1])
                        > f64::from(width.min(height)) * 0.13
                    {
                        continue;
                    }
                    let source_center = center.map(|v| (v + 0.5) * f64::from(scale) - 0.5);
                    if found.iter().any(|p: &[f64; 2]| {
                        (p[0] - source_center[0]).hypot(p[1] - source_center[1])
                            < f64::from(width.min(height) * scale) * 0.5
                    }) {
                        break 'inner;
                    }
                    let Some(refined) = outer
                        .quad()
                        .map(|quad| {
                            quad.map(|point| point.map(|v| (v + 0.5) * f64::from(scale) - 0.5))
                        })
                        .and_then(|quad| crate::finder_refine::refine_center(image, quad))
                    else {
                        continue 'outer;
                    };
                    found.push(refined);
                    break 'inner;
                }
            }
        }
    }
    found.truncate(20);
    found
}

fn sample(image: &RgbImage, p: [f64; 2]) -> Option<[f64; 3]> {
    let [x, y] = p;
    if !x.is_finite()
        || !y.is_finite()
        || x < 0.0
        || y < 0.0
        || x > f64::from(image.width() - 1)
        || y > f64::from(image.height() - 1)
    {
        return None;
    }
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(image.width() - 1);
    let y1 = (y0 + 1).min(image.height() - 1);
    let tx = x - f64::from(x0);
    let ty = y - f64::from(y0);
    Some(std::array::from_fn(|c| {
        (1.0 - ty)
            * ((1.0 - tx) * f64::from(image.get_pixel(x0, y0)[c])
                + tx * f64::from(image.get_pixel(x1, y0)[c]))
            + ty * ((1.0 - tx) * f64::from(image.get_pixel(x0, y1)[c])
                + tx * f64::from(image.get_pixel(x1, y1)[c]))
    }))
}

#[cfg(test)]
fn read_grid(image: &RgbImage, transform: &Homography, y: f64) -> Option<u32> {
    read_frame_metadata(image, transform, y).map(|metadata| metadata.grid)
}

fn read_frame_metadata(image: &RgbImage, transform: &Homography, y: f64) -> Option<FrameMetadata> {
    let mut values = [0.0_f64; 32];
    for (bit, value) in values.iter_mut().enumerate() {
        for offset in [-0.2, 0.0, 0.2] {
            let p = transform.transform([0.15 + 0.7 * (bit as f64 + 0.5 + offset) / 32.0, y])?;
            *value += sample(image, p)?.iter().sum::<f64>() / 9.0;
        }
    }
    let low = values.iter().copied().fold(f64::INFINITY, f64::min);
    let high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if high - low < 60.0 {
        return None;
    }
    let threshold = (low + high) * 0.5;
    let word = values.into_iter().fold(0u32, |word, value| {
        (word << 1) | u32::from(value < threshold)
    });
    decode_metadata_with_modulation(word)
}

#[cfg(test)]
fn equalized_features(
    image: &RgbImage,
    transform: &Homography,
    metadata: FrameMetadata,
) -> Result<Vec<codec::Feature>> {
    Ok(prepare_luma(image, transform, metadata, false, false)?.features())
}

fn grid_is_resolved(quad: &[[f64; 2]; 4], grid: u32) -> bool {
    let perimeter: f64 = (0..4)
        .map(|i| {
            let next = (i + 1) % 4;
            (quad[i][0] - quad[next][0]).hypot(quad[i][1] - quad[next][1])
        })
        .sum();
    perimeter / (4.0 * finder_span(grid)) >= 1.5
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum FailureStage {
    Resolution,
    Equalizer,
    Payload,
}

struct ScanFailure {
    stage: FailureStage,
    area: f64,
    error: anyhow::Error,
}

fn retain_failure(
    failure: &mut Option<ScanFailure>,
    stage: FailureStage,
    area: f64,
    quad: &[[f64; 2]; 4],
    metadata: FrameMetadata,
    error: anyhow::Error,
) {
    if failure.as_ref().is_some_and(|previous| {
        previous.stage > stage || (previous.stage == stage && previous.area >= area)
    }) {
        return;
    }
    let edges: [f64; 4] = std::array::from_fn(|i| {
        let next = (i + 1) % 4;
        (quad[i][0] - quad[next][0]).hypot(quad[i][1] - quad[next][1])
    });
    let span = finder_span(metadata.grid);
    let shortest = edges.into_iter().fold(f64::INFINITY, f64::min) / span;
    let mean = edges.iter().sum::<f64>() / (4.0 * span);
    let stage_name = match stage {
        FailureStage::Resolution => "insufficient resolution",
        FailureStage::Equalizer => "spatial equalizer failed",
        FailureStage::Payload => "payload decode failed",
    };
    *failure = Some(ScanFailure {
        stage,
        area,
        error: error.context(format!(
            "{stage_name}: grid {}, profile {:?}, observed sampling {shortest:.2} px/cell shortest edge ({mean:.2} mean)",
            metadata.grid, metadata.modulation
        )),
    });
}

struct OrientedCandidate {
    destination: [[f64; 2]; 4],
    transform: Homography,
    metadata: FrameMetadata,
}

fn orient_candidate(
    image: &RgbImage,
    quad: &[[f64; 2]; 4],
    mirrored: bool,
    rotation: usize,
) -> Option<OrientedCandidate> {
    let destination =
        std::array::from_fn(|i| quad[(rotation + if mirrored { 4 - i } else { i }) % 4]);
    let transform = Homography::from_quad(
        [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        destination,
    )
    .ok()?;
    let metadata = read_frame_metadata(image, &transform, 0.0)
        .or_else(|| read_frame_metadata(image, &transform, 1.0))?;
    Some(OrientedCandidate {
        destination,
        transform,
        metadata,
    })
}

fn candidate_quads(image: &RgbImage) -> Result<Vec<(f64, [[f64; 2]; 4])>> {
    if image.width().max(image.height()) > 2048
        && let Ok(candidates) = candidate_quads_at_scale(image, 2)
        && candidates.iter().any(|(_, quad)| {
            [false, true].into_iter().any(|mirrored| {
                (0..4).any(|rotation| orient_candidate(image, quad, mirrored, rotation).is_some())
            })
        })
    {
        return Ok(candidates);
    }
    candidate_quads_at_scale(image, 1)
}

fn candidate_quads_at_scale(image: &RgbImage, scale: u32) -> Result<Vec<(f64, [[f64; 2]; 4])>> {
    ensure!(
        image.width() >= 32 && image.height() >= 32,
        "image is too small to locate code"
    );
    ensure!(
        u64::from(image.width()) * u64::from(image.height()) <= 40_000_000,
        "capture exceeds 40 megapixel scan limit"
    );
    let centers = finders_at_scale(image, scale);
    ensure!(
        centers.len() >= 4,
        "found {} of four required finder markers",
        centers.len()
    );
    let mut candidates = Vec::new();
    for a in 0..centers.len() {
        for b in a + 1..centers.len() {
            for c in b + 1..centers.len() {
                for d in c + 1..centers.len() {
                    let mut quad = [centers[a], centers[b], centers[c], centers[d]];
                    let center = [
                        quad.iter().map(|p| p[0]).sum::<f64>() / 4.0,
                        quad.iter().map(|p| p[1]).sum::<f64>() / 4.0,
                    ];
                    quad.sort_by(|a, b| {
                        (a[1] - center[1])
                            .atan2(a[0] - center[0])
                            .total_cmp(&(b[1] - center[1]).atan2(b[0] - center[0]))
                    });
                    if !(0..4).all(|i| cross(quad[i], quad[(i + 1) % 4], quad[(i + 2) % 4]) > 20.0)
                    {
                        continue;
                    }
                    let area = cross(quad[0], quad[1], quad[2]) + cross(quad[0], quad[2], quad[3]);
                    candidates.push((area, quad));
                }
            }
        }
    }
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
    Ok(candidates)
}

/// Detects a framed symbol and returns only a checksum-validated payload.
pub fn decode(image: &RgbImage) -> Result<codec::Decoded> {
    scan(image).result
}

/// A decoding result plus whether a recognizable symbol was detected.
pub struct ScanAttempt {
    pub result: Result<codec::Decoded>,
    pub code_detected: bool,
}

/// Scans once while retaining detection status when payload recovery fails.
pub fn scan(image: &RgbImage) -> ScanAttempt {
    let mut code_detected = false;
    let result = decode_detected(image, &mut code_detected, None);
    ScanAttempt {
        result,
        code_detected,
    }
}

type FrameOrigin = ([[f64; 2]; 4], FrameMetadata);

#[derive(Default)]
struct RecoveryCandidates {
    observation: Option<(crate::recovery::FrameObservation, FrameOrigin)>,
    selected: Option<(f64, Homography, FrameMetadata)>,
    selected_error: Option<anyhow::Error>,
    analysis_error: Option<String>,
}

impl RecoveryCandidates {
    fn select(
        &mut self,
        score: Option<f64>,
        transform: Homography,
        metadata: FrameMetadata,
    ) -> bool {
        let Some(score) = score else {
            return false;
        };
        if self
            .selected
            .as_ref()
            .is_none_or(|&(best, _, _)| score > best)
        {
            self.selected = Some((score, transform, metadata));
            self.selected_error = None;
            true
        } else {
            false
        }
    }

    fn retain_observation(&mut self, prepared: PreparedLuma, origin: FrameOrigin, selected: bool) {
        if self.observation.is_some() || !prepared.registration_attempted {
            return;
        }
        match prepared.observation() {
            Ok(observation) => self.observation = Some((observation, origin)),
            Err(error) if selected => self.selected_error = Some(error),
            Err(_) => {}
        }
    }

    fn origin(&self) -> Option<FrameOrigin> {
        if let Some((_, origin)) = &self.observation {
            return Some(*origin);
        }
        let (_, transform, metadata) = self.selected?;
        let corners = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
            .map(|point| transform.transform(point));
        Some((
            [corners[0]?, corners[1]?, corners[2]?, corners[3]?],
            metadata,
        ))
    }

    fn finish(self, image: &RgbImage) -> Result<crate::recovery::FrameObservation> {
        if let Some((observation, _)) = self.observation {
            return Ok(observation);
        }
        if let Some(error) = self.analysis_error {
            bail!(error);
        }
        let (score, transform, metadata) = self
            .selected
            .ok_or_else(|| anyhow!("No CW Luma 10 code found."))?;
        ensure!(
            score >= 0.5,
            "The code's orientation markers are not clear enough. Refocus with the whole border visible."
        );
        if let Some(error) = self.selected_error {
            return Err(error);
        }
        prepare_luma(image, &transform, metadata, true, true)?.observation()
    }
}

pub(crate) fn scan_with_observation(
    image: &RgbImage,
) -> (ScanAttempt, Result<crate::recovery::FrameObservation>) {
    let mut code_detected = false;
    let mut recovery = RecoveryCandidates::default();
    let result = decode_detected(image, &mut code_detected, Some(&mut recovery));
    let observation = if result.is_err() {
        recovery.finish(image)
    } else {
        recovery
            .observation
            .map(|(observation, _)| observation)
            .ok_or_else(|| anyhow!("No CW Luma 10 code found."))
    };
    (
        ScanAttempt {
            result,
            code_detected,
        },
        observation,
    )
}

fn decode_detected(
    image: &RgbImage,
    code_detected: &mut bool,
    observation: Option<&mut RecoveryCandidates>,
) -> Result<codec::Decoded> {
    let force_alignment = observation.is_some();
    decode_candidates(
        image,
        code_detected,
        observation,
        None,
        None,
        force_alignment,
    )
}

fn decode_candidates(
    image: &RgbImage,
    code_detected: &mut bool,
    mut observation: Option<&mut RecoveryCandidates>,
    candidates: Option<Vec<(f64, [[f64; 2]; 4])>>,
    mut accepted: Option<&mut Option<FrameOrigin>>,
    force_alignment: bool,
) -> Result<codec::Decoded> {
    let candidates = match candidates.map_or_else(|| candidate_quads(image), Ok) {
        Ok(candidates) => candidates,
        Err(error) => {
            if let Some(recovery) = observation.as_deref_mut() {
                recovery.analysis_error = Some(error.to_string());
            }
            return Err(error);
        }
    };
    let mut failure = None;
    for (area, quad) in candidates {
        for mirrored in [false, true] {
            for rotation in 0..4 {
                let Some(OrientedCandidate {
                    destination,
                    transform,
                    metadata,
                }) = orient_candidate(image, &quad, mirrored, rotation)
                else {
                    continue;
                };
                *code_detected = true;
                let score = pilot_luma_correlation(image, &transform, metadata);
                let selected = observation
                    .as_deref_mut()
                    .is_some_and(|recovery| recovery.select(score, transform, metadata));
                if score.is_some_and(|score| score < 0.5) {
                    continue;
                }
                if !grid_is_resolved(&destination, metadata.grid) {
                    retain_failure(
                        &mut failure,
                        FailureStage::Resolution,
                        area,
                        &destination,
                        metadata,
                        anyhow!("mean cell pitch is below the 1.50 px/cell decoding minimum"),
                    );
                    continue;
                }
                let prepared =
                    match prepare_luma(image, &transform, metadata, force_alignment, false) {
                        Ok(prepared) => prepared,
                        Err(error) => {
                            retain_failure(
                                &mut failure,
                                FailureStage::Equalizer,
                                area,
                                &destination,
                                metadata,
                                error,
                            );
                            continue;
                        }
                    };
                match decode_prepared(&prepared, metadata) {
                    Ok(decoded) => {
                        if let Some(accepted) = accepted.as_deref_mut() {
                            *accepted = Some((destination, metadata));
                        }
                        return Ok(decoded);
                    }
                    Err(error) => {
                        if let Some(recovery) = observation.as_deref_mut() {
                            recovery.retain_observation(
                                prepared,
                                (destination, metadata),
                                selected,
                            );
                        }
                        retain_failure(
                            &mut failure,
                            FailureStage::Payload,
                            area,
                            &destination,
                            metadata,
                            error,
                        );
                    }
                }
            }
        }
    }
    if let Some(failure) = failure {
        return Err(failure.error);
    }
    bail!("finder candidates did not contain valid frame metadata")
}

fn decode_prepared(prepared: &PreparedLuma, metadata: FrameMetadata) -> Result<codec::Decoded> {
    let FrameMetadata { grid, modulation } = metadata;
    let cell_pixels = if (grid + 4) * 8 <= codec::MAX_IMAGE_SIDE {
        8
    } else if (grid + 4) * 4 <= codec::MAX_IMAGE_SIDE {
        4
    } else {
        modulation.minimum_cell_pixels()
    };
    let features = prepared.features();
    let mut decoded =
        codec::decode_from_feature_slice_with_modulation(grid, cell_pixels, modulation, &features)?;
    framed_layout(&mut decoded.layout, cell_pixels);
    Ok(decoded)
}

pub(crate) fn observe_luma(image: &RgbImage) -> Result<crate::recovery::FrameObservation> {
    let mut selected = None;
    for (_, quad) in candidate_quads(image)? {
        for mirrored in [false, true] {
            for rotation in 0..4 {
                let Some(OrientedCandidate {
                    transform,
                    metadata,
                    ..
                }) = orient_candidate(image, &quad, mirrored, rotation)
                else {
                    continue;
                };
                let Some(score) = pilot_luma_correlation(image, &transform, metadata) else {
                    continue;
                };
                if selected.as_ref().is_none_or(|&(best, _, _)| score > best) {
                    selected = Some((score, transform, metadata));
                }
            }
        }
    }
    let (score, transform, metadata) =
        selected.ok_or_else(|| anyhow!("No CW Luma 10 code found."))?;
    ensure!(
        score >= 0.5,
        "The code's orientation markers are not clear enough. Refocus with the whole border visible."
    );
    prepare_luma(image, &transform, metadata, true, true)?.observation()
}

fn pilot_luma_correlation(
    image: &RgbImage,
    transform: &Homography,
    metadata: FrameMetadata,
) -> Option<f64> {
    let origin = 2.0 - FINDER_CENTER;
    let span = finder_span(metadata.grid);
    let mut correlation = 0.0;
    for patch in 0..4 {
        let (left, top) = metadata.modulation.pilot_origin(metadata.grid, patch);
        let mut sums = [0.0; 6];
        for index in 0..32 {
            for quadrant in 0..4 {
                let point = [
                    (origin
                        + f64::from(left)
                        + (index % 8) as f64
                        + 0.25
                        + (quadrant % 2) as f64 * 0.5)
                        / span,
                    (origin
                        + f64::from(top)
                        + (index / 8) as f64
                        + 0.25
                        + (quadrant / 2) as f64 * 0.5)
                        / span,
                ];
                let [r, g, b] = sample(image, transform.transform(point)?)?;
                let x = f64::from(crate::luma::latent(
                    crate::luma::pilot_symbol(index),
                    quadrant,
                ));
                let y = 0.299 * r + 0.587 * g + 0.114 * b;
                sums[0] += 1.0;
                sums[1] += x;
                sums[2] += y;
                sums[3] += x * x;
                sums[4] += y * y;
                sums[5] += x * y;
            }
        }
        let [n, x, y, xx, yy, xy] = sums;
        let denominator = ((n * xx - x * x).max(0.0) * (n * yy - y * y).max(0.0)).sqrt();
        if denominator <= 0.0 {
            return None;
        }
        correlation += (n * xy - x * y) / denominator;
    }
    Some(correlation / 4.0)
}

/// Tracks geometry across distinct transfer packets; reset when changing sources.
#[derive(Default)]
pub struct TransferScanner {
    geometry: Option<TrackedGeometry>,
}

struct TrackedGeometry {
    origin: FrameOrigin,
    dimensions: (u32, u32),
}

impl TrackedGeometry {
    fn candidate(&self, image: &RgbImage) -> Option<(f64, [[f64; 2]; 4])> {
        let (previous, metadata) = self.origin;
        let dimensions = self.dimensions;
        if image.dimensions() != dimensions {
            return None;
        }
        let pitch = (0..4)
            .map(|i| {
                (previous[i][0] - previous[(i + 1) % 4][0])
                    .hypot(previous[i][1] - previous[(i + 1) % 4][1])
                    / finder_span(metadata.grid)
            })
            .fold(0.0, f64::max);
        let radius = (8.0 * pitch).ceil().max(16.0) as u32;
        let scale = if image.width().max(image.height()) > 2048 {
            2
        } else {
            1
        };
        let mut quad = [[0.0; 2]; 4];
        for (index, [x, y]) in previous.into_iter().enumerate() {
            let left = (x.floor() as i64 - i64::from(radius)).max(0) as u32;
            let top = (y.floor() as i64 - i64::from(radius)).max(0) as u32;
            let right = (x.ceil() as u32 + radius + 1).min(image.width());
            let bottom = (y.ceil() as u32 + radius + 1).min(image.height());
            if left >= right || top >= bottom {
                return None;
            }
            let crop =
                image::imageops::crop_imm(image, left, top, right - left, bottom - top).to_image();
            let found = finders_at_scale(&crop, scale);
            if found.len() != 1 {
                return None;
            }
            quad[index] = [found[0][0] + f64::from(left), found[0][1] + f64::from(top)];
        }
        // An accepted mirrored orientation may order these corners clockwise.
        let orientation = cross(previous[0], previous[1], previous[2]).signum();
        if !(0..4)
            .all(|i| orientation * cross(quad[i], quad[(i + 1) % 4], quad[(i + 2) % 4]) > 20.0)
        {
            return None;
        }
        let transform =
            Homography::from_quad([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]], quad).ok()?;
        let current = read_frame_metadata(image, &transform, 0.0)
            .or_else(|| read_frame_metadata(image, &transform, 1.0))?;
        if current != metadata || pilot_luma_correlation(image, &transform, current)? < 0.5 {
            return None;
        }
        Some((
            (cross(quad[0], quad[1], quad[2]) + cross(quad[0], quad[2], quad[3])).abs(),
            quad,
        ))
    }
}

impl TransferScanner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.geometry = None;
    }

    pub fn scan(&mut self, image: &RgbImage) -> ScanAttempt {
        let candidate = self
            .geometry
            .as_ref()
            .and_then(|geometry| geometry.candidate(image));
        let mut code_detected = false;
        if let Some(candidate) = candidate {
            let (result, needs_reacquisition) =
                self.decode_frame(image, Some(vec![candidate]), &mut code_detected);
            if !needs_reacquisition {
                return ScanAttempt {
                    result,
                    code_detected,
                };
            }
        }
        let (result, _) = self.decode_frame(image, None, &mut code_detected);
        ScanAttempt {
            result,
            code_detected,
        }
    }

    fn decode_frame(
        &mut self,
        image: &RgbImage,
        candidates: Option<Vec<(f64, [[f64; 2]; 4])>>,
        code_detected: &mut bool,
    ) -> (Result<codec::Decoded>, bool) {
        self.reset();
        let mut accepted = None;
        let mut recovery = RecoveryCandidates::default();
        let normal = decode_candidates(
            image,
            code_detected,
            Some(&mut recovery),
            candidates,
            Some(&mut accepted),
            false,
        );
        if normal.is_ok() {
            self.geometry = accepted.map(|origin| TrackedGeometry {
                origin,
                dimensions: image.dimensions(),
            });
            return (normal, false);
        }
        let origin = recovery.origin();
        let observation = match recovery.finish(image) {
            Ok(observation) => observation,
            Err(error) => return (Err(error), true),
        };
        match crate::recovery::PhotoRecovery::new().push_observation(observation) {
            Ok(progress) => {
                if let Some(decoded) = progress.decoded {
                    self.geometry = origin.map(|origin| TrackedGeometry {
                        origin,
                        dimensions: image.dimensions(),
                    });
                    (Ok(decoded), false)
                } else {
                    (
                        Err(normal.unwrap_err().context(format!(
                            "Single frame validated {}/{} blocks",
                            progress.certified_blocks, progress.total_blocks
                        ))),
                        false,
                    )
                }
            }
            Err(error) => (Err(error), false),
        }
    }
}

#[cfg(test)]
#[path = "tests/scanner.rs"]
mod tests;
