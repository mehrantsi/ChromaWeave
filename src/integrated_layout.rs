pub(crate) const MIN_GRID_CELLS: u32 = 40;
pub(crate) const FINDER_RESERVATION: u32 = 8;
pub(crate) const PILOT_WIDTH: u32 = 8;
pub(crate) const PILOT_HEIGHT: u32 = 4;

pub(crate) fn pilot_origin(grid: u32, patch: usize) -> (u32, u32) {
    (
        (patch % 2) as u32 * (grid - PILOT_WIDTH),
        if patch < 2 {
            FINDER_RESERVATION
        } else {
            grid - FINDER_RESERVATION - PILOT_HEIGHT
        },
    )
}

pub(crate) fn reserved(grid: u32, x: u32, y: u32) -> bool {
    let side = x < FINDER_RESERVATION || x >= grid - FINDER_RESERVATION;
    if side {
        y < FINDER_RESERVATION + PILOT_HEIGHT || y >= grid - FINDER_RESERVATION - PILOT_HEIGHT
    } else {
        (1..6).contains(&y) || (grid - 6..grid - 1).contains(&y)
    }
}

pub(crate) fn data_cell_count(grid: u32) -> usize {
    (0..grid * grid)
        .filter(|cell| !reserved(grid, cell % grid, cell / grid))
        .count()
}

#[cfg(test)]
#[path = "tests/integrated_layout.rs"]
mod tests;
