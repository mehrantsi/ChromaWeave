use std::time::Instant;

use anyhow::{Result, ensure};
use image::{
    Rgb, RgbImage,
    imageops::{FilterType, resize},
};
use rand::{Rng, RngCore, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::Serialize;

use crate::{
    capture::{CaptureConfig, illuminate, jpeg_roundtrip, simulate_capture, subsample_chroma420},
    channel::{ChannelConfig, Medium, simulate},
    comparison::{QrErrorCorrection, decode_qr_zxing, encode_qr, qr_minimum_side_pixels},
    scanner,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum OcclusionPlacement {
    Center,
    Uniform,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct OcclusionRectangle {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComparisonConfig {
    pub cw_payload_bytes: usize,
    pub cw_cell_pixels: u32,
    pub modulation: crate::codec::Modulation,
    pub qr_payload_bytes: usize,
    pub trials: usize,
    pub code_sides: Vec<u32>,
    pub frame_side: u32,
    pub severities: Vec<f32>,
    pub occlusion: f32,
    pub occlusion_placement: OcclusionPlacement,
    pub perspective: f64,
    pub seed: u64,
    pub jpeg_quality: Option<u8>,
    pub chroma420: bool,
    pub illumination: f64,
}

#[derive(Debug, Serialize)]
pub struct ComparisonRow {
    pub symbology: String,
    pub payload_bytes: usize,
    pub medium: Medium,
    pub severity: f32,
    pub code_side_pixels: u32,
    pub bits_per_bbox_pixel: f64,
    pub mean_projected_area_pixels: f64,
    pub bits_per_mean_projected_pixel: f64,
    pub trials: usize,
    pub successful_payloads: usize,
    pub rejected_payloads: usize,
    pub incorrect_accepted_payloads: usize,
    pub success_rate: f64,
    pub success_wilson_95: [f64; 2],
    pub decode_p50_ms: f64,
    pub decode_p95_ms: f64,
    pub successful_decode_p95_ms: Option<f64>,
    pub first_rejection: Option<String>,
    pub trial_outcomes: Vec<TrialOutcome>,
}

#[derive(Debug, Serialize)]
pub struct TrialOutcome {
    pub seed: u64,
    pub source_occlusion: Option<OcclusionRectangle>,
    pub recovered: bool,
    pub incorrectly_accepted: bool,
    pub rejection: Option<String>,
    pub decode_ms: f64,
    pub cw_corrected_bytes: Option<usize>,
    pub cw_erased_bytes: Option<usize>,
}

#[derive(Default)]
struct Measurements {
    times: Vec<f64>,
    projected_area: f64,
    successes_times: Vec<f64>,
    successful: usize,
    incorrect: usize,
    first_rejection: Option<String>,
    outcomes: Vec<TrialOutcome>,
}

#[derive(Debug, Serialize)]
pub struct ComparisonReport {
    pub package_version: &'static str,
    pub build_profile: &'static str,
    pub architecture: &'static str,
    pub os: &'static str,
    pub qr_reader: &'static str,
    pub timing_scope: &'static str,
    pub validation_scope: &'static str,
    pub config: ComparisonConfig,
    pub excluded_variants: Vec<String>,
    pub rows: Vec<ComparisonRow>,
}

fn percentile(values: &mut [f64], fraction: f64) -> f64 {
    values.sort_by(f64::total_cmp);
    values[((values.len() as f64 * fraction).ceil() as usize).saturating_sub(1)]
}

fn wilson(successes: usize, trials: usize) -> [f64; 2] {
    let n = trials as f64;
    let p = successes as f64 / n;
    let z2 = 1.959963984540054_f64.powi(2);
    let center = (p + z2 / (2.0 * n)) / (1.0 + z2 / n);
    let radius = (z2 * (p * (1.0 - p) / n + z2 / (4.0 * n * n))).sqrt() / (1.0 + z2 / n);
    [(center - radius).max(0.0), (center + radius).min(1.0)]
}

fn qr_source(payload: &[u8], ec: QrErrorCorrection) -> Result<RgbImage> {
    let side = qr_minimum_side_pixels(payload.len(), ec)? * 8;
    Ok(encode_qr(payload, side, ec)?.image)
}

fn occlusion_rectangle(
    image_side: u32,
    area_fraction: f32,
    placement: OcclusionPlacement,
    trial_seed: u64,
) -> Option<OcclusionRectangle> {
    let side = (image_side as f32 * area_fraction.sqrt()).round() as u32;
    if side == 0 {
        return None;
    }
    let limit = image_side - side;
    let (x, y) = match placement {
        OcclusionPlacement::Center => (limit / 2, limit / 2),
        OcclusionPlacement::Uniform => {
            let mut rng = ChaCha8Rng::seed_from_u64(trial_seed ^ 0x4f43434c55534905);
            (rng.random_range(0..=limit), rng.random_range(0..=limit))
        }
    };
    Some(OcclusionRectangle {
        x,
        y,
        width: side,
        height: side,
    })
}

fn occlude(image: &mut RgbImage, rectangle: Option<OcclusionRectangle>) {
    if let Some(rectangle) = rectangle {
        for y in rectangle.y..rectangle.y + rectangle.height {
            for x in rectangle.x..rectangle.x + rectangle.width {
                image.put_pixel(x, y, Rgb([245, 245, 245]));
            }
        }
    }
}

pub fn compare(config: ComparisonConfig) -> Result<ComparisonReport> {
    ensure!(
        (1..=1000).contains(&config.trials),
        "trials must be in 1..=1000"
    );
    ensure!(
        (128..=2048).contains(&config.frame_side),
        "frame side must be in 128..=2048"
    );
    ensure!(
        config.cw_payload_bytes <= 1_000_000 && config.qr_payload_bytes <= 2331,
        "CW payload limit is 1 MB; byte-mode QR-M limit is 2331 bytes (QR-H is excluded above 1273 bytes)"
    );
    let cw_capacity =
        scanner::maximum_payload_bytes_with_modulation(config.cw_cell_pixels, config.modulation)?;
    ensure!(
        config.cw_payload_bytes <= cw_capacity,
        "CW payload of {} bytes exceeds capacity of {cw_capacity} bytes at cell size {}",
        config.cw_payload_bytes,
        config.cw_cell_pixels
    );
    ensure!(
        !config.code_sides.is_empty()
            && config.code_sides.iter().all(|&s| s >= 64
                && f64::from(s) / f64::from(config.frame_side) >= 0.05
                && f64::from(s) / f64::from(config.frame_side) <= 0.65),
        "code sides must be at least 64 and at most 65% of frame side to keep rotated symbols in view"
    );
    ensure!(
        !config.severities.is_empty()
            && config
                .severities
                .iter()
                .all(|s| s.is_finite() && (0.0..=1.0).contains(s)),
        "severities must be in [0,1]"
    );
    ensure!(
        config.occlusion.is_finite() && (0.0..=0.25).contains(&config.occlusion),
        "occlusion must be in [0,0.25]"
    );
    ensure!(
        config.perspective.is_finite() && (0.0..=0.15).contains(&config.perspective),
        "perspective must be in [0,0.15]"
    );
    ensure!(
        config.illumination.is_finite() && (0.0..=1.0).contains(&config.illumination),
        "illumination must be in [0,1]"
    );
    ensure!(
        config.jpeg_quality.is_none_or(|q| (1..=100).contains(&q)),
        "JPEG quality must be in 1..=100"
    );
    let excluded_variants = if config.qr_payload_bytes > 1273 {
        vec![format!(
            "QR-H: {} bytes exceeds its 1273-byte single-symbol capacity",
            config.qr_payload_bytes
        )]
    } else {
        Vec::new()
    };
    let mut rows = Vec::new();
    for medium in [Medium::Screen, Medium::Print] {
        for &severity in &config.severities {
            for &side in &config.code_sides {
                let variant_count = if config.qr_payload_bytes > 1273 { 2 } else { 3 };
                let mut measurements: Vec<Measurements> = (0..variant_count)
                    .map(|_| Measurements::default())
                    .collect();
                for trial in 0..config.trials {
                    let trial_seed = config.seed.wrapping_add(trial as u64);
                    let source_occlusion = occlusion_rectangle(
                        side,
                        config.occlusion,
                        config.occlusion_placement,
                        trial_seed,
                    );
                    for offset in 0..variant_count {
                        let variant = (trial + offset) % variant_count;
                        let payload_bytes = if variant == 0 {
                            config.cw_payload_bytes
                        } else {
                            config.qr_payload_bytes
                        };
                        let measurement = &mut measurements[variant];
                        let mut payload_rng = ChaCha8Rng::seed_from_u64(
                            config.seed.wrapping_add(trial as u64) ^ 0x5041594c4f414403,
                        );
                        let mut payload = vec![0; payload_bytes];
                        payload_rng.fill_bytes(&mut payload);
                        let source = match variant {
                            0 => {
                                scanner::encode_with_modulation(
                                    &payload,
                                    config.cw_cell_pixels,
                                    config.modulation,
                                )?
                                .image
                            }
                            1 => qr_source(&payload, QrErrorCorrection::M)?,
                            _ => qr_source(&payload, QrErrorCorrection::H)?,
                        };
                        let mut raster = resize(&source, side, side, FilterType::Triangle);
                        occlude(&mut raster, source_occlusion);
                        let channel = simulate(
                            &raster,
                            &ChannelConfig {
                                medium,
                                severity,
                                seed: trial_seed ^ 0x4348414e4e454c01,
                            },
                        )?;
                        let mut pose_rng =
                            ChaCha8Rng::seed_from_u64(trial_seed ^ 0x504f534500000004);
                        let capture = simulate_capture(
                            &channel,
                            &CaptureConfig {
                                rotation_degrees: pose_rng.random_range(-180.0..180.0),
                                perspective: config.perspective,
                                frame_side: config.frame_side,
                                code_fraction: f64::from(side) / f64::from(config.frame_side),
                                seed: trial_seed ^ 0x4341505455524502,
                                clutter: true,
                            },
                        )?;
                        measurement.projected_area += capture.footprint_area_pixels();
                        let mut observed = illuminate(&capture.image, config.illumination)?;
                        if config.chroma420 {
                            observed = subsample_chroma420(&observed)?;
                        }
                        if let Some(quality) = config.jpeg_quality {
                            observed = jpeg_roundtrip(&observed, quality)?;
                        }
                        let start = Instant::now();
                        let (decoded, cw_stats) = if variant == 0 {
                            match scanner::decode(&observed) {
                                Ok(decoded) => (Ok(decoded.payload), Some(decoded.stats)),
                                Err(error) => (Err(error), None),
                            }
                        } else {
                            (decode_qr_zxing(&observed), None)
                        };
                        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                        measurement.times.push(elapsed);
                        let mut outcome = TrialOutcome {
                            seed: trial_seed,
                            source_occlusion,
                            recovered: false,
                            incorrectly_accepted: false,
                            rejection: None,
                            decode_ms: elapsed,
                            cw_corrected_bytes: cw_stats.as_ref().map(|s| s.corrected_bytes),
                            cw_erased_bytes: cw_stats.as_ref().map(|s| s.erased_bytes),
                        };
                        match decoded {
                            Ok(bytes) if bytes == payload => {
                                measurement.successful += 1;
                                measurement.successes_times.push(elapsed);
                                outcome.recovered = true;
                            }
                            Ok(_) => {
                                measurement.incorrect += 1;
                                outcome.incorrectly_accepted = true;
                            }
                            Err(error) => {
                                let message = error.to_string();
                                measurement
                                    .first_rejection
                                    .get_or_insert_with(|| message.clone());
                                outcome.rejection = Some(message);
                            }
                        }
                        measurement.outcomes.push(outcome);
                    }
                }
                for (variant, measurement) in measurements.into_iter().enumerate() {
                    let payload_bytes = if variant == 0 {
                        config.cw_payload_bytes
                    } else {
                        config.qr_payload_bytes
                    };
                    let name = match variant {
                        0 => format!("CW-{:?}", config.modulation),
                        1 => "QR-M-ZXing".to_owned(),
                        _ => "QR-H-ZXing".to_owned(),
                    };
                    let Measurements {
                        mut times,
                        projected_area,
                        mut successes_times,
                        successful,
                        incorrect,
                        first_rejection,
                        outcomes,
                    } = measurement;
                    let successful_decode_p95_ms = (!successes_times.is_empty())
                        .then(|| percentile(&mut successes_times, 0.95));
                    rows.push(ComparisonRow {
                        symbology: name.to_owned(),
                        payload_bytes,
                        medium,
                        severity,
                        code_side_pixels: side,
                        bits_per_bbox_pixel: payload_bytes as f64 * 8.0 / f64::from(side).powi(2),
                        mean_projected_area_pixels: projected_area / config.trials as f64,
                        bits_per_mean_projected_pixel: payload_bytes as f64
                            * 8.0
                            * config.trials as f64
                            / projected_area,
                        trials: config.trials,
                        successful_payloads: successful,
                        rejected_payloads: config.trials - successful - incorrect,
                        incorrect_accepted_payloads: incorrect,
                        success_rate: successful as f64 / config.trials as f64,
                        success_wilson_95: wilson(successful, config.trials),
                        decode_p50_ms: percentile(&mut times, 0.5),
                        decode_p95_ms: percentile(&mut times, 0.95),
                        successful_decode_p95_ms,
                        first_rejection,
                        trial_outcomes: outcomes,
                    });
                    eprintln!(
                        "{name} {medium:?} severity={severity} side={side}: {successful}/{} recovered",
                        config.trials
                    );
                }
            }
        }
    }
    Ok(ComparisonReport {
        package_version: env!("CARGO_PKG_VERSION"),
        build_profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        architecture: std::env::consts::ARCH,
        os: std::env::consts::OS,
        qr_reader: "rxing 0.9.3 ZXing-C++ QrReader; automatic localization; TryHarder=true; byte content",
        timing_scope: "full automatic localization, geometric recovery and payload decode; excludes image IO and simulation; symbology order rotates between trials to balance ordering effects; one timed decode per image",
        validation_scope: "synthetic print/screen approximations only; same full symbol footprint, source resampling, degradation seed and pose for all symbologies; random binary payloads; independent payload/channel/pose/occlusion random streams; source-stage RGB245 square masks share exact rectangles across variants; optional chroma420 is a 2x2 BT.601 chroma box filter with centered bilinear reconstruction before 4:4:4 JPEG, not a native 4:2:0 JPEG bitstream; no physical validation or iPhone timing",
        config,
        excluded_variants,
        rows,
    })
}

#[cfg(test)]
#[path = "tests/experiment.rs"]
mod tests;
