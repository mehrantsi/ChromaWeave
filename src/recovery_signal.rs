use anyhow::Result;

mod chroma;
mod coupled;
mod initial;
mod levels;

pub(crate) type Decision = (u16, [f32; 10]);

pub(crate) fn initialize(
    grid: usize,
    raw: &[[f32; 12]],
    mixed: &[[f32; 12]],
    mask: &[bool],
) -> Result<Vec<Decision>> {
    #[cfg(test)]
    let started = std::time::Instant::now();
    let initial = initial::classify(grid, raw, mask)?;
    #[cfg(test)]
    eprintln!("alphabet: {:.3}s", started.elapsed().as_secs_f64());
    #[cfg(test)]
    let started = std::time::Instant::now();
    let initial = crate::luma_channel::refine(grid, raw, &initial, mask);
    #[cfg(test)]
    eprintln!("blind luma: {:.3}s", started.elapsed().as_secs_f64());
    #[cfg(test)]
    let started = std::time::Instant::now();
    let chroma = chroma::refine(grid, raw, &initial, mask)?;
    #[cfg(test)]
    eprintln!("blind chroma: {:.3}s", started.elapsed().as_secs_f64());
    #[cfg(test)]
    let started = std::time::Instant::now();
    let output = coupled::refine(grid, mixed, &chroma, mask);
    #[cfg(test)]
    eprintln!("blind coupled: {:.3}s", started.elapsed().as_secs_f64());
    Ok(output)
}

pub(crate) fn initialize_from_certificates(
    grid: usize,
    raw: &[[f32; 12]],
    mixed: &[[f32; 12]],
    mask: &[bool],
    known: &[(u16, u16)],
) -> Result<Vec<Decision>> {
    let seed = initial::classify(grid, raw, mask)?;
    let (fitted, support) = crate::recovery_fit::refine_with_support(grid, mixed, &seed, known);
    Ok(fitted
        .into_iter()
        .zip(support)
        .zip(seed)
        .zip(known)
        .map(|(((values, support), seed), &known)| {
            decision_from_evidence(values, support, seed, known)
        })
        .collect())
}

pub(crate) fn decision_from_evidence(
    values: [f64; 10],
    support: u16,
    seed: Decision,
    known: (u16, u16),
) -> Decision {
    let (seed_symbol, seed_confidence) = seed;
    let (mask, target) = known;
    let fitted_symbol = values.iter().enumerate().fold(0, |symbol, (bit, &value)| {
        symbol | (u16::from(value > 0.0) << bit)
    });
    let symbol = (fitted_symbol & support) | (seed_symbol & !support);
    let confidence = std::array::from_fn(|bit| {
        if mask & (1 << bit) != 0 {
            1.0
        } else if support & (1 << bit) != 0 {
            (values[bit].abs() / 2.0).tanh() as f32
        } else {
            seed_confidence[bit]
        }
    });
    ((symbol & !mask) | target, confidence)
}

fn field_workers(grid: usize) -> usize {
    if grid >= 96 {
        std::thread::available_parallelism().map_or(1, |value| value.get().min(4))
    } else {
        1
    }
}

#[cfg(test)]
#[path = "tests/recovery_signal_support.rs"]
mod support_tests;
