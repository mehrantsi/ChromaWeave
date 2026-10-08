//! Combines observations of the same code from up to four photos.

use anyhow::{Result, ensure};
use image::RgbImage;
use reed_solomon::{Decoder, Encoder};

use crate::codec::{self, DecodeStats, Decoded, Feature, Layout, Modulation};

pub(crate) type Decision = (u16, [f32; 10]);

pub(crate) struct FrameObservation {
    pub grid: u32,
    pub modulation: Modulation,
    pub raw: Vec<Feature>,
    pub mixed: Vec<Feature>,
}

struct Frame {
    observation: FrameObservation,
    initial: Vec<Decision>,
}

#[derive(Clone)]
struct Certificate {
    word: Vec<u8>,
    corrected: usize,
    erased: usize,
}

/// Whole-packet recovery and the number of independently certified RS blocks.
pub struct RecoveryProgress {
    pub frames: usize,
    pub certified_blocks: usize,
    pub total_blocks: usize,
    pub decoded: Option<Decoded>,
}

/// A bounded session for complementary photos of one payload.
/// Create a new session before scanning a different code.
#[derive(Default)]
pub struct PhotoRecovery {
    frames: Vec<Frame>,
    known: Vec<Option<Certificate>>,
    last_code_detected: bool,
}

impl PhotoRecovery {
    pub const MAX_FRAMES: usize = 4;

    pub fn new() -> Self {
        Self::default()
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    pub fn code_detected(&self) -> bool {
        self.last_code_detected
    }

    pub fn push(&mut self, image: &RgbImage) -> Result<RecoveryProgress> {
        ensure!(
            self.frames.len() < Self::MAX_FRAMES,
            "Four photos have been combined. Start over to scan another set."
        );
        #[cfg(test)]
        let started = std::time::Instant::now();
        if self.frames.is_empty() {
            let (attempt, observation) = crate::scanner::scan_with_observation(image);
            self.last_code_detected = attempt.code_detected;
            if let Ok(decoded) = attempt.result {
                return Ok(RecoveryProgress {
                    frames: 1,
                    certified_blocks: decoded.layout.rs_blocks,
                    total_blocks: decoded.layout.rs_blocks,
                    decoded: Some(decoded),
                });
            }
            #[cfg(test)]
            eprintln!("prepare: {:.3}s", started.elapsed().as_secs_f64());
            return self.push_observation(observation?);
        }
        self.last_code_detected = true;
        #[cfg(test)]
        eprintln!("normal: {:.3}s", started.elapsed().as_secs_f64());
        #[cfg(test)]
        let started = std::time::Instant::now();
        let observation = crate::scanner::observe_luma(image)?;
        #[cfg(test)]
        eprintln!("observe: {:.3}s", started.elapsed().as_secs_f64());
        self.push_observation(observation)
    }

    pub(crate) fn push_observation(
        &mut self,
        observation: FrameObservation,
    ) -> Result<RecoveryProgress> {
        ensure!(
            self.frames.len() < Self::MAX_FRAMES,
            "Four photos have been combined. Start over to scan another set."
        );
        let layout =
            codec::layout_for_grid_with_modulation(observation.grid, 2, 0, observation.modulation)?;
        self.last_code_detected = true;
        let cells = (observation.grid * observation.grid) as usize;
        ensure!(
            observation.raw.len() == cells
                && observation.mixed.len() == cells
                && observation
                    .raw
                    .iter()
                    .chain(&observation.mixed)
                    .flatten()
                    .all(|v| v.is_finite()),
            "Invalid camera observations."
        );
        let mask = data_mask(observation.grid);
        if let Some(first) = self.frames.first() {
            ensure!(
                first.observation.grid == observation.grid
                    && first.observation.modulation == observation.modulation,
                "This photo contains a different code layout. Start over to scan a new code."
            );
            ensure!(
                frame_correlation(&first.observation.raw, &observation.raw, &mask) >= 0.8,
                "This photo does not match the current code clearly enough. Keep the same code in view."
            );
            ensure!(
                !self
                    .frames
                    .iter()
                    .any(|frame| frame.observation.raw == observation.raw),
                "This photo is already in the set. Take another photo to add detail."
            );
        }
        #[cfg(test)]
        let started = std::time::Instant::now();
        let initial = if self.known.iter().any(Option::is_some) {
            crate::recovery_signal::initialize_from_certificates(
                observation.grid as usize,
                &observation.raw,
                &observation.mixed,
                &mask,
                &known_symbols(&layout, &mask, &self.known),
            )?
        } else {
            crate::recovery_signal::initialize(
                observation.grid as usize,
                &observation.raw,
                &observation.mixed,
                &mask,
            )?
        };
        ensure!(
            initial.len() == cells
                && initial.iter().all(|(symbol, confidence)| *symbol < 1024
                    && confidence
                        .iter()
                        .all(|v| v.is_finite() && (0.0..=1.0).contains(v))),
            "Invalid camera symbol estimates."
        );
        #[cfg(test)]
        eprintln!("initialize: {:.3}s", started.elapsed().as_secs_f64());
        self.frames.push(Frame {
            observation,
            initial,
        });
        let previous = self.known.clone();
        if self.known.is_empty() {
            self.known = (0..layout.rs_blocks).map(|_| None).collect();
        }
        let outcome = decode_frames(&self.frames, layout, &mask, &mut self.known);
        if outcome.is_err() {
            self.frames.pop();
            self.known = previous;
        }
        outcome
    }
}

fn data_mask(grid: u32) -> Vec<bool> {
    (0..grid * grid)
        .map(|cell| !crate::integrated_layout::reserved(grid, cell % grid, cell / grid))
        .collect()
}

fn frame_correlation(a: &[Feature], b: &[Feature], mask: &[bool]) -> f64 {
    let mut sums = [0.0; 6];
    for ((a, b), &valid) in a.iter().zip(b).zip(mask) {
        if !valid {
            continue;
        }
        for axis in 0..4 {
            let (x, y) = (f64::from(a[axis]), f64::from(b[axis]));
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
    if denominator > 0.0 {
        (n * xy - x * y) / denominator
    } else {
        0.0
    }
}

fn transport(decisions: &[Decision], layout: &Layout, mask: &[bool]) -> (Vec<u8>, Vec<f32>) {
    let mut bytes = vec![0; layout.coded_bytes];
    let mut confidence = vec![1.0f32; bytes.len()];
    for (index, cell) in mask
        .iter()
        .enumerate()
        .filter_map(|(cell, &valid)| valid.then_some(cell))
        .take((bytes.len() * 8).div_ceil(10))
        .enumerate()
    {
        let (symbol, reliability) = decisions[cell];
        for bit in 0..10 {
            let position = index * 10 + bit;
            if position >= bytes.len() * 8 {
                break;
            }
            bytes[position / 8] |= (((symbol >> (9 - bit)) & 1) as u8) << (7 - position % 8);
            confidence[position / 8] = confidence[position / 8].min(reliability[9 - bit]);
        }
    }
    (bytes, confidence)
}

static GF_MULTIPLY: [[u8; 256]; 256] = {
    let mut table = [[0; 256]; 256];
    let mut a = 0;
    while a < 256 {
        let mut b = 0;
        while b < 256 {
            let mut value = a as u8;
            let mut multiplier = b as u8;
            let mut product = 0;
            let mut bit = 0;
            while bit < 8 {
                if multiplier & 1 != 0 {
                    product ^= value;
                }
                value = (value << 1) ^ if value & 128 != 0 { 0x1d } else { 0 };
                multiplier >>= 1;
                bit += 1;
            }
            table[a][b] = product;
            b += 1;
        }
        a += 1;
    }
    table
};

const GF_POWERS: [u8; 255] = {
    let mut powers = [0; 255];
    powers[0] = 1;
    let mut index = 1;
    while index < 255 {
        let previous = powers[index - 1];
        powers[index] = (previous << 1) ^ if previous & 128 != 0 { 0x1d } else { 0 };
        index += 1;
    }
    powers
};

const GF_INVERSE: [u8; 256] = {
    let mut inverse = [0; 256];
    let mut index = 0;
    while index < 255 {
        inverse[GF_POWERS[index] as usize] = GF_POWERS[(255 - index) % 255];
        index += 1;
    }
    inverse
};

struct GmdSyndromes {
    values: Vec<u8>,
}

impl GmdSyndromes {
    fn new(word: &[u8], parity: usize) -> Self {
        Self {
            values: GF_POWERS[..parity]
                .iter()
                .map(|&point| {
                    word.iter().fold(0, |value, &byte| {
                        GF_MULTIPLY[point as usize][value as usize] ^ byte
                    })
                })
                .collect(),
        }
    }

    fn erase(&mut self, position: u8) {
        let point = GF_POWERS[254 - usize::from(position)];
        for index in 0..self.values.len() - 1 {
            self.values[index] =
                GF_MULTIPLY[point as usize][self.values[index] as usize] ^ self.values[index + 1];
        }
        self.values.pop();
    }

    fn can_correct(&self) -> bool {
        let mut locator = [0u8; 256];
        let mut previous = [0u8; 256];
        locator[0] = 1;
        previous[0] = 1;
        let mut degree = 0;
        let mut offset = 1;
        let mut last_discrepancy = 1;
        for index in 0..self.values.len() {
            let mut discrepancy = self.values[index];
            for term in 1..=degree {
                discrepancy ^=
                    GF_MULTIPLY[locator[term] as usize][self.values[index - term] as usize];
            }
            if discrepancy == 0 {
                offset += 1;
                continue;
            }
            let inverse = GF_INVERSE[last_discrepancy as usize];
            let factor = GF_MULTIPLY[discrepancy as usize][inverse as usize];
            let saved = locator;
            for term in 0..=index {
                if term + offset > self.values.len() {
                    break;
                }
                locator[term + offset] ^= GF_MULTIPLY[factor as usize][previous[term] as usize];
            }
            if 2 * degree <= index {
                degree = index + 1 - degree;
                previous = saved;
                last_discrepancy = discrepancy;
                offset = 1;
            } else {
                offset += 1;
            }
        }
        if 2 * degree > self.values.len() {
            return false;
        }
        if degree == 0 {
            return true;
        }
        let mut roots = 0;
        for &point in &GF_POWERS {
            let value = locator[..=degree]
                .iter()
                .rev()
                .fold(0, |value, &coefficient| {
                    GF_MULTIPLY[point as usize][value as usize] ^ coefficient
                });
            roots += usize::from(value == 0);
        }
        roots == degree
    }
}

fn certify(
    decisions: &[Decision],
    layout: &Layout,
    mask: &[bool],
    known: &mut [Option<Certificate>],
) -> Result<()> {
    let workers = if layout.grid_cells >= 96 {
        std::thread::available_parallelism().map_or(1, |value| value.get().min(4))
    } else {
        1
    };
    certify_with_workers(decisions, layout, mask, known, workers)
}

fn certify_with_workers(
    decisions: &[Decision],
    layout: &Layout,
    mask: &[bool],
    known: &mut [Option<Certificate>],
    workers: usize,
) -> Result<()> {
    let (interleaved, reliability) = transport(decisions, layout, mask);
    if workers == 1 {
        return certify_blocks(&interleaved, &reliability, layout, 0, known);
    }
    let mut next = known.to_vec();
    let blocks_per_worker = next.len().div_ceil(workers);
    std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for (chunk, known) in next.chunks_mut(blocks_per_worker).enumerate() {
            let interleaved = &interleaved;
            let reliability = &reliability;
            workers.push(scope.spawn(move || {
                certify_blocks(
                    interleaved,
                    reliability,
                    layout,
                    chunk * blocks_per_worker,
                    known,
                )
            }));
        }
        for worker in workers {
            worker.join().expect("RS certification worker panicked")?;
        }
        Ok::<_, anyhow::Error>(())
    })?;
    known.clone_from_slice(&next);
    Ok(())
}

fn certify_blocks(
    interleaved: &[u8],
    reliability: &[f32],
    layout: &Layout,
    first: usize,
    known: &mut [Option<Certificate>],
) -> Result<()> {
    let parity = layout.rs_parity_bytes;
    let decoder = Decoder::new(parity);
    let encoder = Encoder::new(parity);
    for (offset, previous_certificate) in known.iter_mut().enumerate() {
        let block = first + offset;
        let raw: Vec<_> = (0..255)
            .map(|position| interleaved[position * layout.rs_blocks + block])
            .collect();
        if previous_certificate
            .as_ref()
            .is_some_and(|certificate| certificate.word == raw)
        {
            continue;
        }
        let confidence: Vec<_> = (0..255)
            .map(|position| reliability[position * layout.rs_blocks + block])
            .collect();
        let mut rank: Vec<_> = (0..255usize).collect();
        rank.sort_by(|&a, &b| confidence[a].total_cmp(&confidence[b]).then(a.cmp(&b)));
        let rank: Vec<_> = rank.into_iter().map(|v| v as u8).collect();
        let required_erasures = confidence.iter().filter(|&&value| value == 0.0).count();
        if required_erasures > parity {
            continue;
        }
        let mut syndromes = GmdSyndromes::new(&raw, parity);
        for &position in &rank[..required_erasures] {
            syndromes.erase(position);
        }
        let mut accepted: Option<Certificate> = None;
        let mut conflicting = false;
        for erased in required_erasures..=parity {
            if erased > required_erasures {
                syndromes.erase(rank[erased - 1]);
            }
            // With odd unused capacity, the next prefix covers every bounded candidate here.
            if !(parity - erased).is_multiple_of(2) {
                continue;
            }
            if let Some(previous) = &accepted {
                let errors = previous.corrected
                    - rank[..erased]
                        .iter()
                        .filter(|&&position| {
                            previous.word[usize::from(position)] != raw[usize::from(position)]
                        })
                        .count();
                if 2 * errors + erased <= parity {
                    continue;
                }
            }
            if !syndromes.can_correct() {
                continue;
            }
            let Ok(corrected) = decoder.correct(&raw, Some(&rank[..erased])) else {
                continue;
            };
            let word = corrected.to_vec();
            let changed = word.iter().zip(&raw).filter(|(a, b)| a != b).count();
            let errors = changed
                - rank[..erased]
                    .iter()
                    .filter(|&&position| word[usize::from(position)] != raw[usize::from(position)])
                    .count();
            if 2 * errors + erased > parity {
                continue;
            }
            let distance: f64 = word
                .iter()
                .zip(&raw)
                .zip(&confidence)
                .map(|((a, b), &r)| 1.0 + if a == b { -f64::from(r) } else { f64::from(r) })
                .sum();
            if changed > parity / 2 && distance >= (parity + 1) as f64 {
                continue;
            }
            if decoder.is_corrupted(&word) || encoder.encode(corrected.data()).to_vec() != word {
                continue;
            }
            if let Some(previous) = &accepted {
                if previous.word != word {
                    conflicting = true;
                    break;
                }
            } else {
                accepted = Some(Certificate {
                    word,
                    corrected: changed,
                    erased,
                });
            }
        }
        if conflicting {
            continue;
        }
        if let Some(certificate) = accepted {
            if let Some(previous) = previous_certificate.as_ref() {
                ensure!(
                    previous.word == certificate.word,
                    "Photos produced conflicting error-corrected data. Start a new set."
                );
            } else {
                *previous_certificate = Some(certificate);
            }
        }
    }
    Ok(())
}

fn known_symbols(layout: &Layout, mask: &[bool], known: &[Option<Certificate>]) -> Vec<(u16, u16)> {
    let mut output = vec![(0, 0); mask.len()];
    for (index, cell) in mask
        .iter()
        .enumerate()
        .filter_map(|(cell, &valid)| valid.then_some(cell))
        .enumerate()
    {
        for bit in 0..10 {
            let position = index * 10 + bit;
            if position >= layout.coded_bytes * 8 {
                break;
            }
            let byte = position / 8;
            if let Some(certificate) = &known[byte % layout.rs_blocks] {
                output[cell].0 |= 1 << (9 - bit);
                output[cell].1 |= u16::from(
                    (certificate.word[byte / layout.rs_blocks] >> (7 - position % 8)) & 1,
                ) << (9 - bit);
            }
        }
    }
    output
}

fn seed_decisions(frames: &[Frame]) -> Vec<Decision> {
    (0..frames[0].initial.len())
        .map(|cell| {
            let mut evidence = [0.0; 10];
            for frame in frames {
                let (symbol, confidence) = frame.initial[cell];
                for bit in 0..10 {
                    evidence[bit] += if symbol & (1 << bit) == 0 {
                        -f64::from(confidence[bit])
                    } else {
                        f64::from(confidence[bit])
                    };
                }
            }
            let symbol = evidence
                .iter()
                .enumerate()
                .fold(0, |symbol, (bit, &value)| {
                    symbol | (u16::from(value > 0.0) << bit)
                });
            (
                symbol,
                evidence.map(|value| (value.abs() / frames.len() as f64) as f32),
            )
        })
        .collect()
}

fn fused_decisions(frames: &[Frame], known: &[(u16, u16)]) -> Vec<Decision> {
    let seed = seed_decisions(frames);
    let mut evidence = vec![[0.0; 10]; known.len()];
    let mut supported = vec![0u16; known.len()];
    for frame in frames {
        let (fitted, support) = crate::recovery_fit::refine_with_support(
            frame.observation.grid as usize,
            &frame.observation.mixed,
            &frame.initial,
            known,
        );
        for ((sum, present), (next, valid)) in evidence
            .iter_mut()
            .zip(&mut supported)
            .zip(fitted.into_iter().zip(support))
        {
            *present |= valid;
            for (bit, (sum, value)) in sum.iter_mut().zip(next).enumerate() {
                if valid & (1 << bit) != 0 {
                    *sum += value;
                }
            }
        }
    }
    evidence
        .into_iter()
        .zip(supported)
        .zip(seed)
        .zip(known)
        .map(|(((values, support), seed), &known)| {
            crate::recovery_signal::decision_from_evidence(values, support, seed, known)
        })
        .collect()
}

fn finish(
    layout: &Layout,
    decisions: &[Decision],
    known: &[Option<Certificate>],
) -> Result<Option<Decoded>> {
    if known.iter().any(Option::is_none) {
        return Ok(None);
    }
    let mut layout = layout.clone();
    let mut packet = Vec::with_capacity(layout.rs_blocks * layout.rs_data_bytes);
    let mut stats = DecodeStats {
        corrected_bytes: 0,
        erased_bytes: 0,
        mean_symbol_confidence: 0.0,
        minimum_symbol_confidence: 1.0,
    };
    for certificate in known.iter().flatten() {
        packet.extend_from_slice(&certificate.word[..layout.rs_data_bytes]);
        stats.corrected_bytes += certificate.corrected;
        stats.erased_bytes += certificate.erased;
    }
    let symbols = (layout.coded_bytes * 8).div_ceil(10);
    let mask = data_mask(layout.grid_cells);
    let mut confidence_sum = 0.0f64;
    for cell in mask
        .iter()
        .enumerate()
        .filter_map(|(cell, &valid)| valid.then_some(cell))
        .take(symbols)
    {
        let confidence = &decisions[cell].1;
        let margin = confidence.iter().copied().fold(1.0f32, f32::min);
        confidence_sum += f64::from(margin);
        stats.minimum_symbol_confidence = stats.minimum_symbol_confidence.min(margin);
    }
    stats.mean_symbol_confidence = (confidence_sum / symbols as f64) as f32;
    let payload = codec::decode_packet(&packet, &mut layout)?;
    Ok(Some(Decoded {
        payload,
        layout,
        stats,
    }))
}

fn decode_frames(
    frames: &[Frame],
    layout: Layout,
    mask: &[bool],
    known: &mut [Option<Certificate>],
) -> Result<RecoveryProgress> {
    let mut decisions = seed_decisions(frames);
    #[cfg(test)]
    let started = std::time::Instant::now();
    certify(&decisions, &layout, mask, known)?;
    #[cfg(test)]
    eprintln!("seed ECC: {:.3}s", started.elapsed().as_secs_f64());
    let mut decoded = finish(&layout, &decisions, known)?;
    for _ in 0..2 {
        if decoded.is_some() || known.iter().all(Option::is_none) {
            break;
        }
        let certified_before = known.iter().filter(|v| v.is_some()).count();
        #[cfg(test)]
        let started = std::time::Instant::now();
        decisions = fused_decisions(frames, &known_symbols(&layout, mask, known));
        #[cfg(test)]
        eprintln!("fitting: {:.3}s", started.elapsed().as_secs_f64());
        #[cfg(test)]
        let started = std::time::Instant::now();
        certify(&decisions, &layout, mask, known)?;
        #[cfg(test)]
        eprintln!("fit ECC: {:.3}s", started.elapsed().as_secs_f64());
        decoded = finish(&layout, &decisions, known)?;
        if known.iter().filter(|v| v.is_some()).count() == certified_before {
            break;
        }
    }
    Ok(RecoveryProgress {
        frames: frames.len(),
        certified_blocks: known.iter().filter(|v| v.is_some()).count(),
        total_blocks: layout.rs_blocks,
        decoded,
    })
}

#[cfg(test)]
#[path = "tests/recovery.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/recovery_parity_prefix.rs"]
mod parity_prefix_tests;
