use super::*;
use std::{fs, io::Read, path::Path};

use anyhow::Context;
use serde::Serialize;

use crate::{codec, scanner};

pub struct Options {
    pub chunk_bytes: usize,
    pub fps: f64,
    pub modulation: codec::Modulation,
    pub cell_pixels: u32,
    pub repair_group_data: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            chunk_bytes: 20 * 1024,
            fps: 12.0,
            modulation: codec::Modulation::Luma10,
            cell_pixels: 8,
            repair_group_data: 20,
        }
    }
}

#[derive(Serialize)]
pub struct Frame {
    pub index: usize,
    pub filename: String,
    pub application_bytes: usize,
    pub envelope_bytes: usize,
    pub image_side_pixels: u32,
    pub grid_cells: u32,
    pub is_repair: bool,
    pub source_index: Option<usize>,
}

#[derive(Serialize)]
pub struct Manifest {
    pub schema_version: u32,
    pub envelope: &'static str,
    pub header_bytes: usize,
    pub source_name: String,
    pub transfer_id_hex: String,
    pub file_sha256: String,
    pub original_bytes: usize,
    pub chunk_bytes: usize,
    pub source_chunk_count: usize,
    pub symbol_count: usize,
    pub repair_group_data: usize,
    pub repair_group_parity: usize,
    pub fps: f64,
    pub ecc: &'static str,
    pub cell_pixels: u32,
    pub compression: &'static str,
    pub full_chunk_offered_bits_per_second: f64,
    pub cycle_average_offered_bits_per_second: f64,
    pub useful_cycle_bitrate: f64,
    pub frames: Vec<Frame>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn generate(input: &Path, output: &Path, options: Options) -> Result<Manifest> {
    ensure!(
        options.fps.is_finite() && options.fps > 0.0,
        "frame rate must be finite and positive"
    );
    let file = fs::File::open(input).with_context(|| format!("reading {}", input.display()))?;
    let mut data = Vec::new();
    file.take((MAX_FILE_BYTES + 1) as u64)
        .read_to_end(&mut data)?;
    ensure!(
        (1..=MAX_FILE_BYTES).contains(&data.len()),
        "transfer file must contain 1 through {MAX_FILE_BYTES} bytes"
    );
    ensure!(
        CHUNK_SIZES.contains(&options.chunk_bytes),
        "transfer chunks must contain 10240 or 20480 application bytes"
    );
    let group_data = options.repair_group_data;
    ensure!(
        REPAIR_GROUP_SIZES.contains(&group_data)
            && group_data <= crate::transfer_repair::MAX_DATA_SHARDS,
        "repair group must contain 20 or 60 data symbols"
    );
    let capacity =
        scanner::maximum_payload_bytes_with_modulation(options.cell_pixels, options.modulation)?;
    let symbol_bytes = REPAIR_HEADER_BYTES + options.chunk_bytes;
    ensure!(
        symbol_bytes <= capacity,
        "transfer chunk exceeds this cell size's CW payload capacity"
    );
    if output.exists() {
        ensure!(
            output.is_dir() && fs::read_dir(output)?.next().is_none(),
            "transfer output directory must be empty"
        );
    } else {
        fs::create_dir_all(output).with_context(|| format!("creating {}", output.display()))?;
    }
    let transfer = RepairTransfer::new(&data, options.chunk_bytes, group_data)?;
    let mut frames = Vec::with_capacity(transfer.symbol_count());
    for group_index in 0..transfer.group_count() {
        for symbol in transfer.group_symbols(group_index)? {
            let position = symbol.header.position()?;
            let source_index = position.source_index;
            let application_bytes = source_index.map_or(0, |index| {
                (data.len() - index * options.chunk_bytes).min(options.chunk_bytes)
            });
            let index = usize::from(symbol.header.symbol_index);
            frames.push(encode_frame(
                &symbol.envelope()?,
                index,
                application_bytes,
                position.repair_index.is_some(),
                source_index,
                output,
                &options,
            )?);
        }
    }
    let offered_bits_per_second = symbol_bytes as f64 * options.fps * 8.0;
    let manifest = Manifest {
        schema_version: 2,
        envelope: "CWT",
        header_bytes: REPAIR_HEADER_BYTES,
        source_name: input
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        transfer_id_hex: hex(&transfer.transfer_id),
        file_sha256: hex(&transfer.file_sha256),
        original_bytes: data.len(),
        chunk_bytes: options.chunk_bytes,
        source_chunk_count: transfer.source_chunk_count(),
        symbol_count: transfer.symbol_count(),
        repair_group_data: group_data,
        repair_group_parity: crate::transfer_repair::PARITY_SHARDS,
        fps: options.fps,
        ecc: match options.modulation {
            codec::Modulation::Luma10 => "standard",
            codec::Modulation::Luma10LowEcc => "low",
        },
        cell_pixels: options.cell_pixels,
        compression: "none",
        full_chunk_offered_bits_per_second: offered_bits_per_second,
        cycle_average_offered_bits_per_second: offered_bits_per_second,
        useful_cycle_bitrate: data.len() as f64 * options.fps * 8.0
            / transfer.symbol_count() as f64,
        frames,
    };
    fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    let embedded = serde_json::to_string(&manifest)?.replace('<', "\\u003c");
    fs::write(
        output.join("index.html"),
        include_str!("transfer_player.html").replace("__CW_TRANSFER_MANIFEST__", &embedded),
    )?;
    Ok(manifest)
}

fn encode_frame(
    bytes: &[u8],
    index: usize,
    application_bytes: usize,
    is_repair: bool,
    source_index: Option<usize>,
    output: &Path,
    options: &Options,
) -> Result<Frame> {
    let encoded = scanner::encode_with_options(
        bytes,
        options.cell_pixels,
        options.modulation,
        codec::EncodeOptions {
            compression: codec::Compression::None,
        },
    )?;
    let filename = format!("frame-{index:05}.png");
    encoded.image.save(output.join(&filename))?;
    Ok(Frame {
        index,
        filename,
        application_bytes,
        envelope_bytes: bytes.len(),
        image_side_pixels: encoded.image.width(),
        grid_cells: encoded.layout.grid_cells,
        is_repair,
        source_index,
    })
}
