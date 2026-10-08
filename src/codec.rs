//! Aligned-grid encoding and decoding when the cell size is known.
//!
//! Use the crate root API for framed images with automatic geometry detection.

use anyhow::{Result, anyhow, bail, ensure};
use image::{Rgb, RgbImage};
use reed_solomon::{Decoder, Encoder};
use serde::Serialize;

pub const DEFAULT_CELL_PIXELS: u32 = 16;
pub const MAX_IMAGE_SIDE: u32 = 8192;
const MARGIN_CELLS: u32 = 2;
const DATA_BYTES: usize = 159;
const PARITY_BYTES: usize = 96;
const BLOCK_BYTES: usize = DATA_BYTES + PARITY_BYTES;
const HEADER_BYTES: usize = 12;
pub(crate) const SAMPLE_POSITIONS: [[f64; 2]; 4] =
    [[0.25, 0.25], [0.75, 0.25], [0.25, 0.75], [0.75, 0.75]];

pub(crate) type Feature = [f32; 12];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Modulation {
    Luma10,
    Luma10LowEcc,
}

impl Modulation {
    fn rs_data_bytes(self) -> usize {
        match self {
            Self::Luma10 => DATA_BYTES,
            Self::Luma10LowEcc => 207,
        }
    }
    fn rs_parity_bytes(self) -> usize {
        match self {
            Self::Luma10 => PARITY_BYTES,
            Self::Luma10LowEcc => 48,
        }
    }
    pub(crate) fn minimum_grid_cells(self) -> u32 {
        crate::integrated_layout::MIN_GRID_CELLS
    }
    pub(crate) fn pilot_origin(self, grid: u32, patch: usize) -> (u32, u32) {
        crate::integrated_layout::pilot_origin(grid, patch)
    }
    pub(crate) fn maximum_grid_cells(self) -> u32 {
        crate::luma::MAX_GRID_CELLS
    }
    pub(crate) fn minimum_cell_pixels(self) -> u32 {
        2
    }
    pub fn bits_per_cell(self) -> usize {
        10
    }
    pub fn symbol_count(self) -> usize {
        1 << self.bits_per_cell()
    }
    pub fn metadata_magic(self) -> u8 {
        match self {
            Self::Luma10 => 0xcd,
            Self::Luma10LowEcc => 0xce,
        }
    }
    fn magic(self) -> &'static [u8; 4] {
        match self {
            Self::Luma10 => b"CWI4",
            Self::Luma10LowEcc => b"CWE4",
        }
    }
    fn compressed_magic(self) -> [u8; 4] {
        let mut magic = *self.magic();
        magic[3] += 1;
        magic
    }
    fn patch_shape(self) -> (u32, u32) {
        (8, 4)
    }
    pub fn calibration_symbols(self) -> usize {
        crate::luma::PILOTS
    }
    fn pilot_symbol(self, index: usize) -> u16 {
        crate::luma::pilot_symbol(index)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Compression {
    #[default]
    None,
    Auto,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EncodeOptions {
    pub compression: Compression,
}

#[derive(Debug, Clone, Serialize)]
pub struct Layout {
    pub format: &'static str,
    pub modulation: Modulation,
    pub grid_cells: u32,
    pub cell_pixels: u32,
    pub image_side_pixels: u32,
    pub rs_blocks: usize,
    pub rs_data_bytes: usize,
    pub rs_parity_bytes: usize,
    pub coded_bytes: usize,
    pub payload_capacity_bytes: usize,
    pub payload_bytes: usize,
    pub encoded_payload_bytes: usize,
    pub compression: &'static str,
    pub net_bits_per_pixel: f64,
}

pub struct Encoded {
    pub image: RgbImage,
    pub layout: Layout,
}

#[derive(Debug, Clone, Serialize)]
pub struct DecodeStats {
    pub corrected_bytes: usize,
    pub erased_bytes: usize,
    pub mean_symbol_confidence: f32,
    pub minimum_symbol_confidence: f32,
}

#[derive(Debug)]
pub struct Decoded {
    pub payload: Vec<u8>,
    pub layout: Layout,
    pub stats: DecodeStats,
}

pub(crate) fn validate_cell_pixels(cell_pixels: u32, modulation: Modulation) -> Result<()> {
    let minimum = modulation.minimum_cell_pixels();
    ensure!(
        (minimum..=64).contains(&cell_pixels) && cell_pixels.is_multiple_of(2),
        "cell size for {modulation:?} must be an even number from {minimum} to 64 pixels"
    );
    Ok(())
}

pub(crate) fn layout_for_grid(grid: u32, cell_pixels: u32, payload_bytes: usize) -> Result<Layout> {
    layout_for_grid_with_modulation(grid, cell_pixels, payload_bytes, Modulation::Luma10)
}

pub(crate) fn layout_for_grid_with_modulation(
    grid: u32,
    cell_pixels: u32,
    payload_bytes: usize,
    modulation: Modulation,
) -> Result<Layout> {
    validate_cell_pixels(cell_pixels, modulation)?;
    let maximum_grid =
        (MAX_IMAGE_SIDE / cell_pixels - 2 * MARGIN_CELLS).min(modulation.maximum_grid_cells());
    ensure!(
        (modulation.minimum_grid_cells()..=maximum_grid).contains(&grid),
        "grid dimensions exceed prototype image bounds"
    );
    let data_cells = crate::integrated_layout::data_cell_count(grid);
    let blocks = data_cells * modulation.bits_per_cell() / (8 * BLOCK_BYTES);
    ensure!(
        blocks > 0,
        "grid is too small for one error-corrected packet"
    );
    let image_side_pixels = (grid + 2 * MARGIN_CELLS) * cell_pixels;
    Ok(Layout {
        format: match modulation {
            Modulation::Luma10 => "CWI4-luma10-experimental",
            Modulation::Luma10LowEcc => "CWE4-luma10-low-ecc-experimental",
        },
        modulation,
        grid_cells: grid,
        cell_pixels,
        image_side_pixels,
        rs_blocks: blocks,
        rs_data_bytes: modulation.rs_data_bytes(),
        rs_parity_bytes: modulation.rs_parity_bytes(),
        coded_bytes: blocks * BLOCK_BYTES,
        payload_capacity_bytes: blocks * modulation.rs_data_bytes() - HEADER_BYTES,
        payload_bytes,
        encoded_payload_bytes: payload_bytes,
        compression: "none",
        net_bits_per_pixel: payload_bytes as f64 * 8.0 / f64::from(image_side_pixels).powi(2),
    })
}

pub fn maximum_payload_bytes(cell_pixels: u32) -> Result<usize> {
    validate_cell_pixels(cell_pixels, Modulation::Luma10)?;
    let grid = (MAX_IMAGE_SIDE / cell_pixels - 2 * MARGIN_CELLS)
        .min(Modulation::Luma10.maximum_grid_cells());
    Ok(layout_for_grid(grid, cell_pixels, 0)?.payload_capacity_bytes)
}

fn choose_layout(payload_bytes: usize, cell_pixels: u32, modulation: Modulation) -> Result<Layout> {
    validate_cell_pixels(cell_pixels, modulation)?;
    ensure!(payload_bytes <= u32::MAX as usize, "payload is too large");
    let required_blocks = (payload_bytes + HEADER_BYTES).div_ceil(modulation.rs_data_bytes());
    let required_cells = 4 * modulation.calibration_symbols()
        + (required_blocks * BLOCK_BYTES * 8).div_ceil(modulation.bits_per_cell());
    let mut grid =
        ((required_cells as f64).sqrt().ceil() as u32).max(modulation.minimum_grid_cells());
    let required_data = (required_blocks * BLOCK_BYTES * 8).div_ceil(modulation.bits_per_cell());
    ensure!(
        grid <= modulation.maximum_grid_cells(),
        "payload exceeds profile grid bounds"
    );
    while crate::integrated_layout::data_cell_count(grid) < required_data {
        grid += 1;
        ensure!(
            grid <= modulation.maximum_grid_cells(),
            "payload exceeds profile grid bounds"
        );
    }
    let layout = layout_for_grid_with_modulation(grid, cell_pixels, payload_bytes, modulation)?;
    ensure!(
        payload_bytes <= layout.payload_capacity_bytes,
        "payload does not fit"
    );
    Ok(layout)
}

fn packet_checksum(header: &[u8], payload: &[u8]) -> u32 {
    let mut hash = crc32fast::Hasher::new();
    hash.update(header);
    hash.update(payload);
    hash.finalize()
}

fn codewords(payload: &[u8], layout: &Layout) -> Vec<Vec<u8>> {
    let mut packet = vec![0; layout.rs_blocks * layout.rs_data_bytes];
    packet[..4].copy_from_slice(&if layout.compression == "zstd" {
        layout.modulation.compressed_magic()
    } else {
        *layout.modulation.magic()
    });
    packet[4..8].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    let checksum = packet_checksum(&packet[..8], payload);
    packet[8..12].copy_from_slice(&checksum.to_le_bytes());
    packet[HEADER_BYTES..HEADER_BYTES + payload.len()].copy_from_slice(payload);
    let encoder = Encoder::new(layout.rs_parity_bytes);
    packet
        .chunks_exact(layout.rs_data_bytes)
        .map(|block| encoder.encode(block).to_vec())
        .collect()
}

pub fn encode(payload: &[u8], cell_pixels: u32) -> Result<Encoded> {
    encode_with_modulation(payload, cell_pixels, Modulation::Luma10)
}

pub fn encode_with_modulation(
    payload: &[u8],
    cell_pixels: u32,
    modulation: Modulation,
) -> Result<Encoded> {
    encode_with_options(payload, cell_pixels, modulation, EncodeOptions::default())
}

pub fn encode_with_options(
    payload: &[u8],
    cell_pixels: u32,
    modulation: Modulation,
    options: EncodeOptions,
) -> Result<Encoded> {
    validate_cell_pixels(cell_pixels, modulation)?;
    let compressed = if options.compression == Compression::Auto {
        let encoded = crate::compression::compress(payload)?;
        ((encoded.len() + HEADER_BYTES).div_ceil(modulation.rs_data_bytes())
            < (payload.len() + HEADER_BYTES).div_ceil(modulation.rs_data_bytes()))
        .then_some(encoded)
    } else {
        None
    };
    let encoded_payload = compressed.as_deref().unwrap_or(payload);
    let mut layout = choose_layout(encoded_payload.len(), cell_pixels, modulation)?;
    layout.payload_bytes = payload.len();
    layout.net_bits_per_pixel =
        payload.len() as f64 * 8.0 / f64::from(layout.image_side_pixels).powi(2);
    if compressed.is_some() {
        layout.compression = "zstd";
        layout.format = compressed_format(modulation);
    }
    let blocks = codewords(encoded_payload, &layout);
    let mut image = RgbImage::from_pixel(
        layout.image_side_pixels,
        layout.image_side_pixels,
        Rgb([255; 3]),
    );
    for patch in 0..4 {
        for symbol in 0..modulation.calibration_symbols() {
            draw_symbol(
                &mut image,
                &layout,
                calibration_cell_for(layout.grid_cells, patch, symbol, modulation),
                modulation.pilot_symbol(symbol),
            );
        }
    }
    let mut interleaved = Vec::with_capacity(layout.coded_bytes);
    for position in 0..BLOCK_BYTES {
        for block in &blocks {
            interleaved.push(block[position]);
        }
    }
    let bits = modulation.bits_per_cell();
    for (index, cell) in data_cells_for(layout.grid_cells, modulation).enumerate() {
        let mut symbol = 0;
        for bit in 0..bits {
            let position = index * bits + bit;
            let value = if position < interleaved.len() * 8 {
                (interleaved[position / 8] >> (7 - position % 8)) & 1
            } else {
                0
            };
            symbol = (symbol << 1) | u16::from(value);
        }
        draw_symbol(&mut image, &layout, cell, symbol);
    }
    Ok(Encoded { image, layout })
}

#[cfg(test)]
fn calibration_cell(grid: u32, patch: usize, symbol: usize) -> usize {
    calibration_cell_for(grid, patch, symbol, Modulation::Luma10)
}

fn calibration_cell_for(grid: u32, patch: usize, symbol: usize, modulation: Modulation) -> usize {
    let (width, _) = modulation.patch_shape();
    let (left, top) = modulation.pilot_origin(grid, patch);
    let x = left + symbol as u32 % width;
    let y = top + symbol as u32 / width;
    (y * grid + x) as usize
}

fn data_cells_for(grid: u32, _modulation: Modulation) -> impl Iterator<Item = usize> {
    (0..grid * grid)
        .filter(move |cell| !crate::integrated_layout::reserved(grid, cell % grid, cell / grid))
        .map(|cell| cell as usize)
}

#[derive(Clone, Copy)]
struct Demixer {
    center: [f32; 6],
    inverse: [[f32; 6]; 6],
    noise: [f32; 6],
}

impl Demixer {
    fn fit(references: &[Feature]) -> Option<Self> {
        let dimensions = 6;
        let mut center = [0.0; 6];
        let mut forward = [[0.0_f64; 6]; 6];
        let count = references.len() as f64;
        for (index, reference) in references.iter().enumerate() {
            let symbol = crate::luma::pilot_symbol(index);
            for row in 0..dimensions {
                center[row] += reference[row] / count as f32;
                for (column, entry) in forward[row][..dimensions].iter_mut().enumerate() {
                    let sign = f64::from(crate::luma::latent(symbol, column))
                        / if column < 4 { 5.0 } else { 1.0 };
                    *entry += f64::from(reference[row]) * sign / count;
                }
            }
        }
        let mut augmented = [[0.0; 12]; 6];
        for row in 0..dimensions {
            augmented[row][..dimensions].copy_from_slice(&forward[row][..dimensions]);
            augmented[row][dimensions + row] = 1.0;
        }
        for column in 0..dimensions {
            let pivot = (column..dimensions).max_by(|&a, &b| {
                augmented[a][column]
                    .abs()
                    .total_cmp(&augmented[b][column].abs())
            })?;
            let divisor = augmented[pivot][column];
            if !divisor.is_finite() || divisor.abs() < 1e-6 {
                return None;
            }
            augmented.swap(column, pivot);
            for entry in &mut augmented[column][..dimensions * 2] {
                *entry /= divisor;
            }
            let pivot_row = augmented[column];
            for (row, entries) in augmented[..dimensions].iter_mut().enumerate() {
                if row == column {
                    continue;
                }
                let factor = entries[column];
                for (entry, pivot) in entries[..dimensions * 2].iter_mut().zip(pivot_row) {
                    *entry -= factor * pivot;
                }
            }
        }
        let forward_norm = forward[..dimensions]
            .iter()
            .map(|row| row[..dimensions].iter().map(|v| v.abs()).sum::<f64>())
            .fold(0.0_f64, f64::max);
        let inverse_norm = augmented[..dimensions]
            .iter()
            .map(|row| {
                row[dimensions..dimensions * 2]
                    .iter()
                    .map(|v| v.abs())
                    .sum::<f64>()
            })
            .fold(0.0_f64, f64::max);
        if !inverse_norm.is_finite() || forward_norm * inverse_norm > 100.0 || inverse_norm > 1.0 {
            return None;
        }
        let mut inverse = [[0.0; 6]; 6];
        for row in 0..dimensions {
            for column in 0..dimensions {
                inverse[row][column] = augmented[row][dimensions + column] as f32;
            }
        }
        let mut model = Self {
            center,
            inverse,
            noise: [0.0; 6],
        };
        let mut residual = 0.0;
        let mut maximum = 0.0_f32;
        for (index, reference) in references.iter().enumerate() {
            let symbol = crate::luma::pilot_symbol(index);
            for row in 0..dimensions {
                let latent = (0..dimensions)
                    .map(|column| model.inverse[row][column] * (reference[column] - center[column]))
                    .sum::<f32>();
                let target = crate::luma::latent(symbol, row);
                let error = latent - target;
                residual += error * error;
                model.noise[row] += error * error / count as f32;
                maximum = maximum.max(error.abs());
            }
        }
        if residual / (references.len() * dimensions) as f32 > 0.1225 || maximum > 1.0 {
            return None;
        }
        Some(model)
    }
}

struct Calibration {
    valid: [bool; 4],
    demixers: [Option<Demixer>; 4],
    axis_coordinates: [Vec<f32>; 2],
}

impl Calibration {
    fn read(
        grid: u32,
        modulation: Modulation,
        feature: &mut impl FnMut(usize) -> Result<Feature>,
    ) -> Result<Self> {
        let mut references = [[[0.0; 12]; 32]; 4];
        let count = modulation.calibration_symbols();
        for (patch, dictionary) in references.iter_mut().enumerate() {
            for (symbol, reference) in dictionary.iter_mut().take(count).enumerate() {
                *reference = feature(calibration_cell_for(grid, patch, symbol, modulation))?;
            }
        }
        let demixers = std::array::from_fn(|patch| Demixer::fit(&references[patch][..count]));
        let valid = demixers.map(|model| model.is_some());
        ensure!(
            valid.iter().any(|&valid| valid),
            "all calibration patches are indistinguishable or have invalid mixing models"
        );
        Ok(Self {
            valid,
            demixers,
            axis_coordinates: Self::axis_coordinates(grid, modulation),
        })
    }

    fn axis_coordinates(grid: u32, modulation: Modulation) -> [Vec<f32>; 2] {
        let (width, height) = modulation.patch_shape();
        let (left, top) = modulation.pilot_origin(grid, 0);
        let (right, bottom) = modulation.pilot_origin(grid, 3);
        [(width, left, right), (height, top, bottom)].map(|(size, near, far)| {
            (0..grid)
                .map(|index| {
                    let coordinate = (index as f32 - near as f32 - (size - 1) as f32 / 2.0)
                        / (far - near) as f32;
                    coordinate.clamp(0.0, 1.0)
                })
                .collect()
        })
    }

    fn weights(&self, grid: u32, cell: usize) -> [f32; 4] {
        let x = self.axis_coordinates[0][(cell as u32 % grid) as usize];
        let y = self.axis_coordinates[1][(cell as u32 / grid) as usize];
        let mut weights = [(1.0 - x) * (1.0 - y), x * (1.0 - y), (1.0 - x) * y, x * y];
        for (weight, &valid) in weights.iter_mut().zip(&self.valid) {
            if !valid {
                *weight = 0.0;
            }
        }
        let mut total: f32 = weights.iter().sum();
        if total <= f32::EPSILON {
            weights = self.valid.map(|valid| if valid { 1.0 } else { 0.0 });
            total = weights.iter().sum();
        }
        weights.map(|weight| weight / total)
    }

    fn coordinates(&self, grid: u32, cell: usize, observed: &Feature) -> ([f32; 10], [f32; 10]) {
        let mut center = [0.0; 6];
        let mut inverse = [[0.0; 6]; 6];
        let mut noise = [0.0; 10];
        for (patch, weight) in self.weights(grid, cell).into_iter().enumerate() {
            if weight == 0.0 {
                continue;
            }
            let model = self.demixers[patch].as_ref().unwrap();
            for row in 0..6 {
                center[row] += weight * model.center[row];
                noise[row] += weight * model.noise[row];
                for (entry, reference) in inverse[row][..6].iter_mut().zip(model.inverse[row]) {
                    *entry += weight * reference;
                }
            }
        }
        let centered: [f32; 6] = std::array::from_fn(|i| observed[i] - center[i]);
        let mut latent = [0.0; 10];
        for row in 0..6 {
            latent[row] = (0..6)
                .map(|column| inverse[row][column] * centered[column])
                .sum::<f32>();
        }
        (latent, noise)
    }
}

fn cell_origin(layout: &Layout, cell: usize) -> (u32, u32) {
    let x = cell as u32 % layout.grid_cells;
    let y = cell as u32 / layout.grid_cells;
    (
        (x + MARGIN_CELLS) * layout.cell_pixels,
        (y + MARGIN_CELLS) * layout.cell_pixels,
    )
}

fn draw_symbol(image: &mut RgbImage, layout: &Layout, cell: usize, symbol: u16) {
    let (x0, y0) = cell_origin(layout, cell);
    let colors = SAMPLE_POSITIONS.map(|[u, v]| crate::luma::rgb(symbol, u as f32, v as f32));
    let half = layout.cell_pixels / 2;
    for y in 0..layout.cell_pixels {
        let row = usize::from(y >= half) * 2;
        for x in 0..layout.cell_pixels {
            image.put_pixel(x0 + x, y0 + y, colors[row + usize::from(x >= half)]);
        }
    }
}

pub(crate) fn feature(image: &RgbImage, layout: &Layout, cell: usize) -> Feature {
    let (x0, y0) = cell_origin(layout, cell);
    let samples = SAMPLE_POSITIONS.map(|[u, v]| {
        crate::geometry::bilinear(
            image,
            f64::from(x0) + u * f64::from(layout.cell_pixels) - 0.5,
            f64::from(y0) + v * f64::from(layout.cell_pixels) - 0.5,
        )
        .0
        .map(f32::from)
    });
    crate::luma::feature(samples)
}

pub fn decode(image: &RgbImage, cell_pixels: u32) -> Result<Decoded> {
    decode_with_modulation(image, cell_pixels, Modulation::Luma10)
}

pub fn decode_with_modulation(
    image: &RgbImage,
    cell_pixels: u32,
    modulation: Modulation,
) -> Result<Decoded> {
    validate_cell_pixels(cell_pixels, modulation)?;
    ensure!(
        image.width() == image.height(),
        "expected a square registered code image"
    );
    ensure!(
        image.width() <= MAX_IMAGE_SIDE,
        "image exceeds prototype bounds"
    );
    ensure!(
        image.width().is_multiple_of(cell_pixels),
        "image width is not a multiple of the cell size"
    );
    let grid = (image.width() / cell_pixels)
        .checked_sub(2 * MARGIN_CELLS)
        .ok_or_else(|| anyhow!("image is too small"))?;
    let layout = layout_for_grid_with_modulation(grid, cell_pixels, 0, modulation)?;
    decode_from_features_with_modulation(grid, cell_pixels, modulation, |cell| {
        Ok(feature(image, &layout, cell))
    })
}

fn compressed_format(modulation: Modulation) -> &'static str {
    match modulation {
        Modulation::Luma10 => "CWI5-luma10-experimental",
        Modulation::Luma10LowEcc => "CWE5-luma10-low-ecc-experimental",
    }
}

fn packet_payload<'a>(packet: &'a [u8], layout: &Layout) -> Result<&'a [u8]> {
    ensure!(packet.len() >= HEADER_BYTES, "truncated CW Code packet");
    ensure!(
        &packet[..4] == layout.modulation.magic()
            || packet[..4] == layout.modulation.compressed_magic(),
        "unrecognized or corrupted CW Code format"
    );
    let length = u32::from_le_bytes(packet[4..8].try_into()?) as usize;
    ensure!(
        length <= layout.payload_capacity_bytes && length <= packet.len() - HEADER_BYTES,
        "invalid payload length"
    );
    let payload = &packet[HEADER_BYTES..HEADER_BYTES + length];
    let checksum = u32::from_le_bytes(packet[8..12].try_into()?);
    ensure!(
        checksum == packet_checksum(&packet[..8], payload),
        "payload checksum mismatch"
    );
    if packet[HEADER_BYTES + length..]
        .iter()
        .any(|&byte| byte != 0)
    {
        bail!("noncanonical packet padding");
    }
    Ok(payload)
}

pub(crate) fn decode_packet(packet: &[u8], layout: &mut Layout) -> Result<Vec<u8>> {
    let encoded_payload = packet_payload(packet, layout)?;
    let compressed = packet[..4] == layout.modulation.compressed_magic();
    let payload = if compressed {
        crate::compression::decompress(encoded_payload)?
    } else {
        encoded_payload.to_vec()
    };
    layout.payload_bytes = payload.len();
    layout.encoded_payload_bytes = encoded_payload.len();
    layout.compression = if compressed { "zstd" } else { "none" };
    if compressed {
        layout.format = compressed_format(layout.modulation);
    }
    layout.net_bits_per_pixel =
        payload.len() as f64 * 8.0 / f64::from(layout.image_side_pixels).powi(2);
    Ok(payload)
}

pub(crate) fn decode_from_features_with_modulation(
    grid: u32,
    cell_pixels: u32,
    modulation: Modulation,
    mut feature: impl FnMut(usize) -> Result<Feature>,
) -> Result<Decoded> {
    let calibration = Calibration::read(grid, modulation, &mut feature)?;
    let observed = (0..(grid * grid) as usize)
        .map(&mut feature)
        .collect::<Result<Vec<_>>>()?;
    decode_observed(grid, cell_pixels, modulation, calibration, &observed)
}

pub(crate) fn decode_from_feature_slice_with_modulation(
    grid: u32,
    cell_pixels: u32,
    modulation: Modulation,
    observed: &[Feature],
) -> Result<Decoded> {
    let calibration = Calibration::read(grid, modulation, &mut |cell| Ok(observed[cell]))?;
    decode_observed(grid, cell_pixels, modulation, calibration, observed)
}

fn decode_observed(
    grid: u32,
    cell_pixels: u32,
    modulation: Modulation,
    calibration: Calibration,
    observed: &[Feature],
) -> Result<Decoded> {
    let mut data_mask = vec![false; observed.len()];
    for cell in data_cells_for(grid, modulation) {
        data_mask[cell] = true;
    }
    let coordinates: Vec<_> = observed
        .iter()
        .enumerate()
        .map(|(cell, value)| calibration.coordinates(grid, cell, value))
        .collect();
    let levels = crate::luma_levels::LevelField::estimate(grid as usize, |cell| {
        data_mask[cell].then(|| coordinates[cell].0[..4].try_into().unwrap())
    });
    let initial = coordinates
        .into_iter()
        .enumerate()
        .map(|(cell, (latent, noise))| {
            crate::luma::classify_levels(
                latent,
                noise,
                levels.at(cell % grid as usize, cell / grid as usize),
            )
        })
        .collect::<Vec<_>>();
    if let Ok(decoded) = decode_classified(grid, cell_pixels, modulation, &initial) {
        return Ok(decoded);
    }
    let decisions = crate::luma_channel::refine(grid as usize, observed, &initial, &data_mask);
    decode_classified(grid, cell_pixels, modulation, &decisions)
}

fn decode_classified(
    grid: u32,
    cell_pixels: u32,
    modulation: Modulation,
    decisions: &[(u16, [f32; 10])],
) -> Result<Decoded> {
    let mut layout = layout_for_grid_with_modulation(grid, cell_pixels, 0, modulation)?;
    let mut interleaved = vec![0; layout.coded_bytes];
    let mut confidence = vec![1.0_f32; layout.coded_bytes];
    let mut stats = DecodeStats {
        corrected_bytes: 0,
        erased_bytes: 0,
        mean_symbol_confidence: 0.0,
        minimum_symbol_confidence: 1.0,
    };
    let bits = modulation.bits_per_cell();
    let symbols = (layout.coded_bytes * 8).div_ceil(bits);
    let mut confidence_sum = 0.0_f64;
    for (index, cell) in data_cells_for(grid, modulation).take(symbols).enumerate() {
        let (symbol, bit_confidence) = decisions[cell];
        let margin = bit_confidence[..bits]
            .iter()
            .copied()
            .fold(1.0_f32, f32::min);
        confidence_sum += f64::from(margin);
        stats.minimum_symbol_confidence = stats.minimum_symbol_confidence.min(margin);
        for bit in 0..bits {
            let position = index * bits + bit;
            if position >= layout.coded_bytes * 8 {
                break;
            }
            let byte = position / 8;
            confidence[byte] = confidence[byte].min(bit_confidence[bits - 1 - bit]);
            interleaved[byte] |= (((symbol >> (bits - 1 - bit)) & 1) as u8) << (7 - position % 8);
        }
    }
    stats.mean_symbol_confidence = (confidence_sum / symbols as f64) as f32;
    let mut packet = Vec::with_capacity(layout.rs_blocks * layout.rs_data_bytes);
    for index in 0..layout.rs_blocks {
        for position in 0..layout.rs_data_bytes {
            packet.push(interleaved[position * layout.rs_blocks + index]);
        }
    }
    if let Ok(payload) = decode_packet(&packet, &mut layout) {
        stats.erased_bytes = confidence.iter().filter(|&&value| value < 0.12).count();
        return Ok(Decoded {
            payload,
            layout,
            stats,
        });
    }
    packet.clear();
    let decoder = Decoder::new(layout.rs_parity_bytes);
    let mut block = [0; BLOCK_BYTES];
    let mut erasures = Vec::with_capacity(layout.rs_parity_bytes);
    for index in 0..layout.rs_blocks {
        erasures.clear();
        for (position, byte) in block.iter_mut().enumerate() {
            let interleaved_position = position * layout.rs_blocks + index;
            *byte = interleaved[interleaved_position];
            if confidence[interleaved_position] < 0.12 {
                erasures.push(position as u8);
            }
        }
        ensure!(
            erasures.len() <= layout.rs_parity_bytes,
            "Reed-Solomon block {index} has too many uncertain bytes"
        );
        stats.erased_bytes += erasures.len();
        let (corrected, count) = decoder
            .correct_err_count(&block, Some(&erasures))
            .map_err(|_| anyhow!("Reed-Solomon block {index} is uncorrectable"))?;
        stats.corrected_bytes += count;
        packet.extend_from_slice(corrected.data());
    }
    let payload = decode_packet(&packet, &mut layout)?;
    Ok(Decoded {
        payload,
        layout,
        stats,
    })
}

#[cfg(test)]
#[path = "tests/codec.rs"]
mod tests;
