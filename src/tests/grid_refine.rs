use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn pattern(p: [f64; 2]) -> f64 {
    let x = (p[0] * 2.0).floor() as i64;
    let y = (p[1] * 2.0).floor() as i64;
    let mut v =
        (x as u64).wrapping_mul(0x9e3779b97f4a7c15) ^ (y as u64).wrapping_mul(0xbf58476d1ce4e5b9);
    v ^= v >> 30;
    v = v.wrapping_mul(0xbf58476d1ce4e5b9);
    v ^= v >> 27;
    40.0 + (v % 4) as f64 * 50.0
}

fn baseline_window_sums<const N: usize>(
    samples: &[[f64; N]],
    valid: &[bool],
    axes: [&AxisSamples; 2],
) -> ([[[f64; 4]; N]; 2], f64) {
    let weights: [f64; SAMPLE_SIDE] = std::array::from_fn(|i| {
        let d = (i as i32 - WINDOW_RADIUS - 1) as f64;
        (-d * d / (2.0 * 12.0 * 12.0)).exp()
    });
    let mut sums = [[[0.0; 4]; N]; 2];
    let mut possible_weight = 0.0;
    for y in 1..SAMPLE_SIDE - 1 {
        for x in 1..SAMPLE_SIDE - 1 {
            let i = y * SAMPLE_SIDE + x;
            let weight = weights[x] * weights[y];
            possible_weight += weight;
            if !valid[i] {
                continue;
            }
            for axis in 0..2 {
                let stride = if axis == 0 { 1 } else { SAMPLE_SIDE };
                if !valid[i - stride] || !valid[i + stride] {
                    continue;
                }
                let (sin, cos) = axes[axis].oscillations[if axis == 0 { x } else { y }];
                for channel in 0..N {
                    let gradient = (samples[i + stride][channel] - samples[i - stride][channel])
                        .abs()
                        * weight;
                    sums[axis][channel][0] += gradient * cos;
                    sums[axis][channel][1] += gradient * sin;
                    sums[axis][channel][2] += gradient;
                    sums[axis][channel][3] += weight;
                }
            }
        }
    }
    (sums, possible_weight)
}

fn assert_window_sums_match<const N: usize>() {
    let weights = WindowWeights::new();
    let axes = [AxisSamples::new(6.625), AxisSamples::new(53.0)];
    for constant in [false, true] {
        let samples: Vec<[f64; N]> = (0..SAMPLE_SIDE * SAMPLE_SIDE)
            .map(|i| {
                std::array::from_fn(|channel| {
                    if constant {
                        128.0 + channel as f64
                    } else {
                        pattern([
                            i as f64 / 8.0 - channel as f64 * 0.11,
                            (i / SAMPLE_SIDE) as f64 / 8.0 + channel as f64 * 0.19,
                        ])
                    }
                })
            })
            .collect();
        for mask in 0..3 {
            let valid: Vec<_> = (0..samples.len())
                .map(|i| match mask {
                    0 => true,
                    1 => i % 11 != 0 && !(18..26).contains(&(i % SAMPLE_SIDE)),
                    _ => false,
                })
                .collect();
            let (expected, possible_weight) =
                baseline_window_sums(&samples, &valid, [&axes[0], &axes[1]]);
            let actual = weights.accumulate(&samples, &valid, [&axes[0], &axes[1]]);
            assert_eq!(weights.possible_weight.to_bits(), possible_weight.to_bits());
            assert_eq!(
                actual.map(|axis| axis.map(|channel| channel.map(f64::to_bits))),
                expected.map(|axis| axis.map(|channel| channel.map(f64::to_bits))),
                "channels={N}, constant={constant}, mask={mask}"
            );
        }
    }
}

#[test]
fn cached_window_weights_and_shared_support_preserve_accumulation_bits() {
    assert_window_sums_match::<1>();
    assert_window_sums_match::<4>();
}

#[test]
fn shared_axis_tables_match_node_arithmetic_bits() {
    for grid in [31, 95, 96, 107, 149, 200, 201, 241] {
        let mesh_side = if grid <= 200 { 17 } else { 33 };
        let step = (f64::from(grid) - 1.0) / (mesh_side - 1) as f64;
        for index in 0..mesh_side {
            let center = 0.5 + index as f64 * step;
            let axis = AxisSamples::new(center);
            assert_eq!(axis.center.to_bits(), center.to_bits());
            for i in 0..SAMPLE_SIDE {
                let point = center + (i as i32 - WINDOW_RADIUS - 1) as f64 / 8.0;
                let (sin, cos) = (-4.0 * std::f64::consts::PI * point).sin_cos();
                assert_eq!(axis.points[i].to_bits(), point.to_bits());
                assert_eq!(axis.oscillations[i].0.to_bits(), sin.to_bits());
                assert_eq!(axis.oscillations[i].1.to_bits(), cos.to_bits());
            }
        }
    }
}

#[test]
fn flat_field_has_no_registration_information() {
    assert!(GridRefinement::estimate(107, true, |_| Some(128.0)).is_none());
}
#[test]
fn follows_smooth_distortion_without_payload_values() {
    let delta = |p: [f64; 2]| {
        let x = std::f64::consts::PI * p[0] / 107.0;
        let y = std::f64::consts::PI * p[1] / 107.0;
        [0.38 * x.sin() * y.sin(), 0.2 * (2.0 * x).sin() * y.sin()]
    };
    let r = GridRefinement::estimate(107, true, |p| {
        let d = delta(p);
        Some(pattern([p[0] - d[0], p[1] - d[1]]))
    })
    .unwrap();
    let mut max = 0.0f64;
    for y in [15.0, 30.0, 50.0, 70.0, 90.0] {
        for x in [15.0, 30.0, 50.0, 70.0, 90.0] {
            let p = [x, y];
            let d = delta(p);
            let a = r.adjust(p);
            max = max.max((a[0] - x - d[0]).hypot(a[1] - y - d[1]));
        }
    }
    assert!(max < 0.09, "{max}");
}

#[test]
fn channel_estimates_match_separate_sampling() {
    let shifts = [[0.28, -0.16], [-0.12, 0.23], [0.17, 0.31], [-0.2, -0.27]];
    let shared_calls = AtomicUsize::new(0);
    let combined = GridRefinement::estimate_channels::<4>(107, true, |point| {
        shared_calls.fetch_add(1, Ordering::Relaxed);
        Some(std::array::from_fn(|channel| {
            pattern([point[0] - shifts[channel][0], point[1] - shifts[channel][1]])
        }))
    });
    for y in [-1.0, 0.5, 6.625, 53.0, 106.5, 109.0] {
        for x in [-1.0, 0.5, 6.625, 53.0, 106.5, 109.0] {
            let point = [x, y];
            let shared = GridRefinement::adjust_channels(&combined, point);
            for (channel, field) in combined.iter().enumerate() {
                assert_eq!(
                    shared[channel],
                    field.as_ref().map(|field| field.adjust(point))
                );
            }
        }
    }
    let mut separate_calls_total = 0;

    for channel in 0..4 {
        let separate_calls = AtomicUsize::new(0);
        let separate = GridRefinement::estimate(107, true, |point| {
            separate_calls.fetch_add(1, Ordering::Relaxed);
            Some(pattern([
                point[0] - shifts[channel][0],
                point[1] - shifts[channel][1],
            ]))
        })
        .unwrap();
        let combined = combined[channel].as_ref().unwrap();
        assert_eq!(combined.step, separate.step);
        assert_eq!(combined.shifts, separate.shifts);
        let shared = shared_calls.load(Ordering::Relaxed);
        let separate = separate_calls.load(Ordering::Relaxed);
        assert_eq!(separate, shared);
        separate_calls_total += separate;
    }
    assert_eq!(
        separate_calls_total,
        shared_calls.load(Ordering::Relaxed) * 4
    );
}

#[test]
fn parallel_medium_and_large_meshes_match_sequential_bits() {
    let shifts = [[0.28, -0.16], [-0.12, 0.23], [0.17, 0.31], [-0.2, -0.27]];
    for (grid, integrated, missing_samples) in [
        (107, true, false),
        (149, false, true),
        (200, true, true),
        (241, false, false),
        (241, true, true),
    ] {
        let sample = |point: [f64; 2]| {
            if missing_samples
                && ((point[0].floor() as i64 + point[1].floor() as i64 * 3).rem_euclid(11) == 0
                    || (point[0] > f64::from(grid) * 0.4 && point[0] < f64::from(grid) * 0.5))
            {
                return None;
            }
            Some(std::array::from_fn(|channel| {
                pattern([point[0] - shifts[channel][0], point[1] - shifts[channel][1]])
            }))
        };
        let sequential =
            GridRefinement::estimate_channels_with_workers::<4>(grid, integrated, sample, 1);
        let parallel =
            GridRefinement::estimate_channels_with_workers::<4>(grid, integrated, sample, 4);
        for channel in 0..4 {
            let sequential = sequential[channel].as_ref().unwrap();
            let parallel = parallel[channel].as_ref().unwrap();
            assert_eq!(parallel.step.to_bits(), sequential.step.to_bits());
            assert_eq!(parallel.mesh_side, sequential.mesh_side);
            assert_eq!(
                parallel
                    .shifts
                    .iter()
                    .flatten()
                    .map(|v| v.to_bits())
                    .collect::<Vec<_>>(),
                sequential
                    .shifts
                    .iter()
                    .flatten()
                    .map(|v| v.to_bits())
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                parallel
                    .confidence
                    .iter()
                    .flatten()
                    .map(|v| v.to_bits())
                    .collect::<Vec<_>>(),
                sequential
                    .confidence
                    .iter()
                    .flatten()
                    .map(|v| v.to_bits())
                    .collect::<Vec<_>>()
            );
        }
    }
}
