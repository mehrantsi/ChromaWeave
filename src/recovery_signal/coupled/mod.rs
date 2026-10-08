mod field;
mod inference;
mod model;

const LEVEL_COUNT: usize = 4;
const NEIGHBOR_COUNT: usize = 4;
const NEIGHBOR_LEVEL_COUNT: usize = LEVEL_COUNT - 1;
const CHROMA_COUNT: usize = 2;
const CHROMA_START: usize = LEVEL_COUNT + NEIGHBOR_COUNT * NEIGHBOR_LEVEL_COUNT;
const COEFFICIENT_COUNT: usize = CHROMA_START + CHROMA_COUNT;
const MAX_ACTIVE_TERMS: usize = NEIGHBOR_COUNT + CHROMA_COUNT;

type Coefficients = [f64; COEFFICIENT_COUNT];

const fn neighbor_coefficient(direction: usize, rank: usize) -> usize {
    LEVEL_COUNT + direction * NEIGHBOR_LEVEL_COUNT + rank - 1
}

const fn chroma_coefficient(axis: usize) -> usize {
    CHROMA_START + axis
}

#[derive(Clone, Copy)]
struct Model {
    coefficients: Coefficients,
    variance: f64,
}

#[derive(Clone, Copy)]
struct Sample {
    rank: usize,
    active_coefficients: [u8; MAX_ACTIVE_TERMS],
    negative_terms: u8,
    active_count: u8,
    value: f64,
}

pub(super) fn refine(
    grid: usize,
    features: &[[f32; 12]],
    initial: &[(u16, [f32; 10])],
    data_mask: &[bool],
) -> Vec<(u16, [f32; 10])> {
    field::refine_with_workers(
        grid,
        features,
        initial,
        data_mask,
        super::field_workers(grid),
    )
}
