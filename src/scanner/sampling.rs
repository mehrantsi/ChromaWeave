use anyhow::{Result, anyhow, ensure};
use image::RgbImage;

use super::{FINDER_CENTER, FrameMetadata, Homography, codec, finder_span, sample};

pub(super) fn sample_lattice(
    image: &RgbImage,
    [x, y]: [f64; 2],
    is_padding: bool,
) -> Option<[f64; 3]> {
    let max_x = f64::from(image.width()) - 0.5;
    let max_y = f64::from(image.height()) - 0.5;
    if !x.is_finite()
        || !y.is_finite()
        || (!is_padding && (x < -0.5 || y < -0.5 || x > max_x || y > max_y))
    {
        return None;
    }
    sample(
        image,
        [x.clamp(0.0, max_x - 0.5), y.clamp(0.0, max_y - 0.5)],
    )
}

pub(super) fn sample_channel_lattice(
    image: &RgbImage,
    [x, y]: [f64; 2],
    padding: bool,
    channel: usize,
) -> Option<f64> {
    let max_x = f64::from(image.width()) - 0.5;
    let max_y = f64::from(image.height()) - 0.5;
    if !x.is_finite()
        || !y.is_finite()
        || (!padding && (x < -0.5 || y < -0.5 || x > max_x || y > max_y))
    {
        return None;
    }
    let x = x.clamp(0.0, max_x - 0.5);
    let y = y.clamp(0.0, max_y - 0.5);
    let x0 = x as u32;
    let y0 = y as u32;
    let x1 = (x0 + 1).min(image.width() - 1);
    let y1 = (y0 + 1).min(image.height() - 1);
    let tx = x - f64::from(x0);
    let ty = y - f64::from(y0);
    Some(
        (1.0 - ty)
            * ((1.0 - tx) * f64::from(image.get_pixel(x0, y0)[channel])
                + tx * f64::from(image.get_pixel(x1, y0)[channel]))
            + ty * ((1.0 - tx) * f64::from(image.get_pixel(x0, y1)[channel])
                + tx * f64::from(image.get_pixel(x1, y1)[channel])),
    )
}

pub(super) fn color_registration(
    image: &RgbImage,
    transform: &Homography,
    metadata: FrameMetadata,
) -> [Option<crate::grid_refine::GridRefinement>; 4] {
    let origin = 2.0 - FINDER_CENTER;
    let span = finder_span(metadata.grid);
    crate::grid_refine::GridRefinement::estimate_channels(metadata.grid, true, |point| {
        let position =
            transform.transform([(origin + point[0]) / span, (origin + point[1]) / span])?;
        let [r, g, b] = sample_lattice(image, position, false)?;
        Some([r, g, b, 0.299 * r + 0.587 * g + 0.114 * b])
    })
}

fn registered_channel_point(color: Option<[f64; 2]>, luma: [f64; 2]) -> [f64; 2] {
    color.map_or(luma, |color| {
        std::array::from_fn(|axis| {
            let difference = color[axis] - luma[axis];
            luma[axis] + difference - (difference / 0.5).round() * 0.5
        })
    })
}

fn project_rgb([r, g, b]: [f32; 3]) -> [f32; 3] {
    let y = 0.299 * r + 0.587 * g + 0.114 * b;
    [y, (b - y) / 1.772, (r - y) / 1.402]
}

struct Registration {
    fields: [Option<crate::grid_refine::GridRefinement>; 4],
    attempted: bool,
}

fn register_grid(
    image: &RgbImage,
    transform: &Homography,
    metadata: FrameMetadata,
    require_alignment: bool,
) -> Registration {
    let span = finder_span(metadata.grid);
    let corners =
        [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]].map(|point| transform.transform(point));
    let shortest_pitch = (0..4)
        .filter_map(|i| Some((corners[i]?, corners[(i + 1) % 4]?)))
        .map(|(a, b)| (a[0] - b[0]).hypot(a[1] - b[1]) / span)
        .fold(f64::INFINITY, f64::min);
    let fields = if require_alignment || shortest_pitch >= 6.0 {
        color_registration(image, transform, metadata)
    } else {
        std::array::from_fn(|_| None)
    };
    Registration {
        fields,
        attempted: require_alignment || shortest_pitch >= 6.0,
    }
}

fn sample_registered_lattice(
    image: &RgbImage,
    transform: &Homography,
    metadata: FrameMetadata,
    side: usize,
    registration: &Registration,
) -> Result<Vec<[f32; 3]>> {
    let grid = metadata.grid as usize;
    let padding = 2usize;
    let origin = 2.0 - FINDER_CENTER;
    let span = finder_span(metadata.grid);
    let separate_channels = registration.fields[..3].iter().any(Option::is_some);
    let mut lattice = vec![[0.0; 3]; side * side];
    let fill_rows = |start_row: usize, rows: &mut [[f32; 3]]| -> Result<()> {
        for (row_index, row) in rows.chunks_exact_mut(side).enumerate() {
            let y = start_row + row_index;
            for (x, target) in row.iter_mut().enumerate() {
                let cell_point = [
                    (x as f64 - padding as f64 + 0.5) * 0.5,
                    (y as f64 - padding as f64 + 0.5) * 0.5,
                ];
                let is_padding = cell_point
                    .iter()
                    .any(|&point| point < 0.0 || point >= grid as f64);
                let offsets: &[f64] = if metadata.grid > 200 && registration.attempted {
                    &[-0.08, 0.0, 0.08]
                } else {
                    &[0.0]
                };
                let mut rgb = [0.0_f32; 3];
                for &dy in offsets {
                    for &dx in offsets {
                        let offset = [cell_point[0] + dx, cell_point[1] + dy];
                        if !separate_channels {
                            let point = registration.fields[3]
                                .as_ref()
                                .map_or(offset, |field| field.adjust(offset));
                            let position = transform
                                .transform([(origin + point[0]) / span, (origin + point[1]) / span])
                                .ok_or_else(|| anyhow!("invalid pilot projection"))?;
                            let pixel = sample_lattice(image, position, is_padding)
                                .ok_or_else(|| anyhow!("pilot lattice extends outside image"))?;
                            for channel in 0..3 {
                                rgb[channel] += (pixel[channel]
                                    / (offsets.len() * offsets.len()) as f64)
                                    as f32;
                            }
                            continue;
                        }
                        let adjusted = crate::grid_refine::GridRefinement::adjust_channels(
                            &registration.fields,
                            offset,
                        );
                        let luma = adjusted[3].unwrap_or(offset);
                        for (channel, value) in rgb.iter_mut().enumerate() {
                            let point = registered_channel_point(adjusted[channel], luma);
                            let position = transform
                                .transform([(origin + point[0]) / span, (origin + point[1]) / span])
                                .ok_or_else(|| anyhow!("invalid pilot projection"))?;
                            let pixel =
                                sample_channel_lattice(image, position, is_padding, channel)
                                    .ok_or_else(|| {
                                        anyhow!("pilot lattice extends outside image")
                                    })?;
                            *value += (pixel / (offsets.len() * offsets.len()) as f64) as f32;
                        }
                    }
                }
                *target = project_rgb(rgb);
            }
        }
        Ok(())
    };
    if grid >= 96 {
        let workers = std::thread::available_parallelism().map_or(1, |value| value.get().min(4));
        let rows_per_worker = side.div_ceil(workers);
        std::thread::scope(|scope| -> Result<()> {
            let mut jobs = Vec::new();
            for (index, rows) in lattice.chunks_mut(rows_per_worker * side).enumerate() {
                let fill_rows = &fill_rows;
                jobs.push(scope.spawn(move || fill_rows(index * rows_per_worker, rows)));
            }
            for job in jobs {
                job.join()
                    .map_err(|_| anyhow!("camera sampling worker failed"))??;
            }
            Ok(())
        })?;
    } else {
        fill_rows(0, &mut lattice)?;
    }
    Ok(lattice)
}

fn lattice_pilots(metadata: FrameMetadata, side: usize) -> Vec<crate::spatial_equalizer::Pilot> {
    let padding = 2usize;
    let mut pilots = Vec::new();
    for patch in 0..4 {
        for index in 0..32 {
            let (left, top) = metadata.modulation.pilot_origin(metadata.grid, patch);
            let cell_x = left as usize + index % 8;
            let cell_y = top as usize + index / 8;
            for q in 0..4 {
                let value = crate::luma::rgb(
                    crate::luma::pilot_symbol(index),
                    if q % 2 == 0 { 0.25 } else { 0.75 },
                    if q / 2 == 0 { 0.25 } else { 0.75 },
                )
                .0
                .map(f32::from);
                pilots.push(crate::spatial_equalizer::Pilot {
                    index: (padding + cell_y * 2 + q / 2) * side + padding + cell_x * 2 + q % 2,
                    value: project_rgb(value),
                    patch,
                });
            }
        }
    }
    pilots
}

pub(super) fn prepare_luma(
    image: &RgbImage,
    transform: &Homography,
    metadata: FrameMetadata,
    require_alignment: bool,
    luma_only: bool,
) -> Result<PreparedLuma> {
    #[cfg(test)]
    let stage_started = std::time::Instant::now();
    let grid = metadata.grid as usize;
    let padding = 2usize;
    let side = grid * 2 + padding * 2;
    ensure!(
        side <= crate::spatial_equalizer::MAX_LATTICE_SIDE,
        "spatial pilot lattice exceeds experimental bounds"
    );
    let registration = register_grid(image, transform, metadata, require_alignment);
    #[cfg(test)]
    let registration_elapsed = stage_started.elapsed();
    let lattice = sample_registered_lattice(image, transform, metadata, side, &registration)?;
    #[cfg(test)]
    let sampling_elapsed = stage_started.elapsed();
    let pilots = lattice_pilots(metadata, side);
    let (equalized, _models) = if luma_only {
        crate::spatial_equalizer::equalize_luma(&lattice, side, &pilots)
    } else {
        crate::spatial_equalizer::equalize(&lattice, side, &pilots)
    }
    .ok_or_else(|| anyhow!("invalid spatial pilot model"))?;
    #[cfg(test)]
    eprintln!(
        "prepare stages: registration {:.3}s sampling {:.3}s equalizer {:.3}s",
        registration_elapsed.as_secs_f64(),
        (sampling_elapsed - registration_elapsed).as_secs_f64(),
        (stage_started.elapsed() - sampling_elapsed).as_secs_f64()
    );
    Ok(PreparedLuma {
        metadata,
        side,
        lattice,
        equalized,
        aligned: registration.fields.iter().all(Option::is_some),
        registration_attempted: registration.attempted,
    })
}

pub(super) struct PreparedLuma {
    metadata: FrameMetadata,
    side: usize,
    lattice: Vec<[f32; 3]>,
    equalized: Vec<[f32; 3]>,
    aligned: bool,
    pub(super) registration_attempted: bool,
}

impl PreparedLuma {
    pub(super) fn features(&self) -> Vec<codec::Feature> {
        let grid = self.metadata.grid as usize;
        (0..grid * grid)
            .map(|cell| {
                let samples = std::array::from_fn(|q| {
                    let p = self.equalized[(2 + (cell / grid) * 2 + q / 2) * self.side
                        + 2
                        + (cell % grid) * 2
                        + q % 2];
                    [
                        p[0] + 1.402 * p[2],
                        p[0] - (0.299 * 1.402 * p[2] + 0.114 * 1.772 * p[1]) / 0.587,
                        p[0] + 1.772 * p[1],
                    ]
                });
                crate::luma::feature(samples)
            })
            .collect()
    }

    pub(super) fn observation(self) -> Result<crate::recovery::FrameObservation> {
        ensure!(
            self.aligned,
            "Not enough sharp detail to align the color layers. Refocus or enlarge the displayed code."
        );
        let grid = self.metadata.grid as usize;
        let mut raw = vec![[0.0f32; 12]; grid * grid];
        let mut mixed = raw.clone();
        for cell in 0..grid * grid {
            for quadrant in 0..4 {
                let index = (2 + cell / grid * 2 + quadrant / 2) * self.side
                    + 2
                    + cell % grid * 2
                    + quadrant % 2;
                raw[cell][quadrant] = self.lattice[index][0];
                mixed[cell][quadrant] = self.equalized[index][0];
                for channel in 1..3 {
                    raw[cell][3 + channel] += self.lattice[index][channel] / 4.0;
                    mixed[cell][3 + channel] += self.lattice[index][channel] / 4.0;
                }
            }
        }
        Ok(crate::recovery::FrameObservation {
            grid: self.metadata.grid,
            modulation: self.metadata.modulation,
            raw,
            mixed,
        })
    }
}
