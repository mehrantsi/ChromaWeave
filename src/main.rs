use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use chromaweave::{
    capture::{CaptureConfig, illuminate, jpeg_roundtrip, simulate_capture, subsample_chroma420},
    channel::{ChannelConfig, Medium, simulate},
    codec,
    experiment::{ComparisonConfig, OcclusionPlacement, compare},
    scanner, transfer,
};
use clap::{Parser, Subcommand, ValueEnum};
use image::{ImageReader, Limits, RgbImage};
use serde::Serialize;

#[derive(Parser)]
#[command(
    name = "chromaweave",
    version,
    about = "CW Luma 10 encoder, automatic scanner and QR comparison tools"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Generate cycling CW Luma 10 transfer PNGs and a native-pixel player")]
    Transfer {
        input: PathBuf,
        output_dir: PathBuf,
        #[arg(long, default_value_t = 20480, value_parser = parse_chunk_bytes)]
        chunk_bytes: usize,
        #[arg(
            long,
            default_value_t = 20,
            value_parser = parse_repair_group_data,
            help = "Add four repair symbols after each group of 20 or 60 source chunks"
        )]
        repair_group: usize,
        #[arg(long, default_value_t = 12.0)]
        fps: f64,
        #[arg(long, value_enum, default_value_t = EccChoice::Standard)]
        ecc: EccChoice,
        #[arg(long, default_value_t = 8)]
        cell_pixels: u32,
    },
    #[command(about = "Encode a binary file as a self-locating CW Luma 10 PNG")]
    Encode {
        input: PathBuf,
        output: PathBuf,
        #[arg(long, default_value_t = 8)]
        cell_pixels: u32,
        #[arg(long, value_enum, default_value_t = EccChoice::Standard)]
        ecc: EccChoice,
        #[arg(long, value_enum, default_value_t = CompressionChoice::None)]
        compression: CompressionChoice,
    },
    #[command(about = "Locate and recover a CW Code from a PNG or JPEG image")]
    Decode { input: PathBuf, output: PathBuf },
    #[command(
        about = "Apply seeded synthetic print or screen degradation without changing geometry"
    )]
    Simulate {
        input: PathBuf,
        output: PathBuf,
        #[arg(long, value_enum)]
        medium: CaptureMedium,
        #[arg(long, default_value_t = 0.3)]
        severity: f32,
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
    #[command(
        about = "Create a synthetic camera frame with rotation, perspective and background clutter"
    )]
    Capture {
        input: PathBuf,
        output: PathBuf,
        #[arg(long, value_enum, default_value_t = CaptureMedium::Screen)]
        medium: CaptureMedium,
        #[arg(long, default_value_t = 0.3)]
        severity: f32,
        #[arg(long, default_value_t = 17.0)]
        rotation: f64,
        #[arg(long, default_value_t = 0.08)]
        perspective: f64,
        #[arg(long, default_value_t = 1024)]
        frame_side: u32,
        #[arg(long, default_value_t = 0.7)]
        code_fraction: f64,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        #[arg(long)]
        no_clutter: bool,
        #[arg(long, default_value_t = 0.0)]
        illumination: f64,
        #[arg(long)]
        jpeg_quality: Option<u8>,
        #[arg(
            long,
            help = "Apply a 2x2 chroma-subsampling model before JPEG quantization"
        )]
        chroma420: bool,
    },
    #[command(
        about = "Compare CW, QR-M and QR-H automatic decoding at identical capture footprints"
    )]
    Bench {
        #[arg(long, default_value_t = 512)]
        cw_bytes: usize,
        #[arg(long, default_value_t = 8)]
        cw_cell_pixels: u32,
        #[arg(long, value_enum, default_value_t = EccChoice::Standard)]
        ecc: EccChoice,
        #[arg(long, default_value_t = 0.0)]
        illumination: f64,
        #[arg(long)]
        jpeg_quality: Option<u8>,
        #[arg(
            long,
            help = "Apply a 2x2 chroma-subsampling model before JPEG quantization"
        )]
        chroma420: bool,
        #[arg(long, default_value_t = 512)]
        qr_bytes: usize,
        #[arg(long, default_value_t = 10)]
        trials: usize,
        #[arg(long, value_delimiter = ',', default_value = "384,512,640")]
        code_sides: Vec<u32>,
        #[arg(long, default_value_t = 1024)]
        frame_side: u32,
        #[arg(long, value_delimiter = ',', default_value = "0,0.4")]
        severities: Vec<f32>,
        #[arg(long, default_value_t = 0.0)]
        occlusion: f32,
        #[arg(long, value_enum, default_value = "center")]
        occlusion_placement: OcclusionPlacement,
        #[arg(long, default_value_t = 0.08)]
        perspective: f64,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum EccChoice {
    Standard,
    Low,
}

fn parse_chunk_bytes(value: &str) -> std::result::Result<usize, String> {
    match value.parse::<usize>() {
        Ok(size) if transfer::CHUNK_SIZES.contains(&size) => Ok(size),
        _ => Err("chunk bytes must be 10240 or 20480".into()),
    }
}

fn parse_repair_group_data(value: &str) -> std::result::Result<usize, String> {
    match value.parse::<usize>() {
        Ok(size) if [20, 60].contains(&size) => Ok(size),
        _ => Err("repair group must contain 20 or 60 data symbols".into()),
    }
}

impl From<EccChoice> for codec::Modulation {
    fn from(value: EccChoice) -> Self {
        match value {
            EccChoice::Standard => Self::Luma10,
            EccChoice::Low => Self::Luma10LowEcc,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum CompressionChoice {
    None,
    Auto,
}

impl From<CompressionChoice> for codec::Compression {
    fn from(value: CompressionChoice) -> Self {
        match value {
            CompressionChoice::None => Self::None,
            CompressionChoice::Auto => Self::Auto,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum CaptureMedium {
    Screen,
    Print,
}

impl From<CaptureMedium> for Medium {
    fn from(value: CaptureMedium) -> Self {
        match value {
            CaptureMedium::Screen => Self::Screen,
            CaptureMedium::Print => Self::Print,
        }
    }
}

fn read_payload(
    path: &Path,
    cell_pixels: u32,
    modulation: codec::Modulation,
    compression: codec::Compression,
) -> Result<Vec<u8>> {
    let capacity = match compression {
        codec::Compression::None => {
            scanner::maximum_payload_bytes_with_modulation(cell_pixels, modulation)?
        }
        codec::Compression::Auto => chromaweave::compression::MAX_DECOMPRESSED_BYTES,
    };
    let mut payload = Vec::new();
    fs::File::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .take(capacity as u64 + 1)
        .read_to_end(&mut payload)
        .with_context(|| format!("reading {}", path.display()))?;
    ensure!(
        payload.len() <= capacity,
        "input exceeds capacity of {capacity} bytes at cell size {cell_pixels}"
    );
    Ok(payload)
}

fn load_image(path: &Path) -> Result<RgbImage> {
    let mut reader =
        ImageReader::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(codec::MAX_IMAGE_SIDE);
    limits.max_image_height = Some(codec::MAX_IMAGE_SIDE);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    Ok(reader
        .decode()
        .with_context(|| format!("decoding {}", path.display()))?
        .to_rgb8())
}

fn write_json<T: Serialize>(value: &T, output: Option<&Path>) -> Result<()> {
    let json = serde_json::to_string_pretty(value)?;
    if let Some(path) = output {
        fs::write(path, format!("{json}\n"))
            .with_context(|| format!("writing {}", path.display()))?;
    }
    println!("{json}");
    Ok(())
}

fn save_decoded(decoded: codec::Decoded, output: &Path) -> Result<()> {
    fs::write(output, &decoded.payload).with_context(|| format!("writing {}", output.display()))?;
    write_json(
        &serde_json::json!({"layout": decoded.layout, "decode": decoded.stats}),
        None,
    )
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Transfer {
            input,
            output_dir,
            chunk_bytes,
            repair_group,
            fps,
            ecc,
            cell_pixels,
        } => {
            let manifest = transfer::generate(
                &input,
                &output_dir,
                transfer::Options {
                    chunk_bytes,
                    fps,
                    modulation: ecc.into(),
                    cell_pixels,
                    repair_group_data: repair_group,
                },
            )?;
            write_json(
                &serde_json::json!({
                    "transfer_id": manifest.transfer_id_hex,
                    "file_sha256": manifest.file_sha256,
                    "original_bytes": manifest.original_bytes,
                    "chunk_bytes": manifest.chunk_bytes,
                    "source_chunk_count": manifest.source_chunk_count,
                    "symbol_count": manifest.symbol_count,
                    "repair_group_data": manifest.repair_group_data,
                    "repair_group_parity": manifest.repair_group_parity,
                    "fps": manifest.fps,
                    "full_chunk_offered_bits_per_second": manifest.full_chunk_offered_bits_per_second,
                    "cycle_average_offered_bits_per_second": manifest.cycle_average_offered_bits_per_second,
                    "useful_cycle_bitrate": manifest.useful_cycle_bitrate,
                    "player": output_dir.join("index.html"),
                    "manifest": output_dir.join("manifest.json"),
                }),
                None,
            )?;
        }
        Command::Encode {
            input,
            output,
            cell_pixels,
            ecc,
            compression,
        } => {
            let modulation = ecc.into();
            let compression = compression.into();
            let encoded = scanner::encode_with_options(
                &read_payload(&input, cell_pixels, modulation, compression)?,
                cell_pixels,
                modulation,
                codec::EncodeOptions { compression },
            )?;
            encoded
                .image
                .save(&output)
                .with_context(|| format!("writing {}", output.display()))?;
            write_json(&encoded.layout, None)?;
        }
        Command::Decode { input, output } => {
            save_decoded(scanner::decode(&load_image(&input)?)?, &output)?
        }
        Command::Simulate {
            input,
            output,
            medium,
            severity,
            seed,
        } => {
            let config = ChannelConfig {
                medium: medium.into(),
                severity,
                seed,
            };
            simulate(&load_image(&input)?, &config)?.save(&output)?;
            write_json(&config, None)?;
        }
        Command::Capture {
            input,
            output,
            medium,
            severity,
            rotation,
            perspective,
            frame_side,
            code_fraction,
            seed,
            no_clutter,
            illumination,
            jpeg_quality,
            chroma420,
        } => {
            let medium = ChannelConfig {
                medium: medium.into(),
                severity,
                seed,
            };
            let geometry = CaptureConfig {
                rotation_degrees: rotation,
                perspective,
                frame_side,
                code_fraction,
                seed,
                clutter: !no_clutter,
            };
            let capture = simulate_capture(&simulate(&load_image(&input)?, &medium)?, &geometry)?;
            let mut observed = illuminate(&capture.image, illumination)?;
            if chroma420 {
                observed = subsample_chroma420(&observed)?;
            }
            if let Some(quality) = jpeg_quality {
                observed = jpeg_roundtrip(&observed, quality)?;
            }
            observed.save(&output)?;
            write_json(
                &serde_json::json!({"medium": medium, "geometry": geometry, "corners": capture.corners, "illumination": illumination, "jpeg_quality": jpeg_quality, "chroma420": chroma420}),
                None,
            )?;
        }
        Command::Bench {
            cw_bytes,
            cw_cell_pixels,
            ecc,
            illumination,
            jpeg_quality,
            chroma420,
            qr_bytes,
            trials,
            code_sides,
            frame_side,
            severities,
            occlusion,
            occlusion_placement,
            perspective,
            seed,
            output,
        } => {
            let report = compare(ComparisonConfig {
                cw_payload_bytes: cw_bytes,
                cw_cell_pixels,
                modulation: ecc.into(),
                illumination,
                jpeg_quality,
                chroma420,
                qr_payload_bytes: qr_bytes,
                trials,
                code_sides,
                frame_side,
                severities,
                occlusion,
                occlusion_placement,
                perspective,
                seed,
            })?;
            write_json(&report, output.as_deref())?;
        }
    }
    Ok(())
}
