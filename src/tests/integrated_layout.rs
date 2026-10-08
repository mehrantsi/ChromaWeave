use super::*;
use std::collections::HashSet;

#[test]
fn finder_metadata_and_pilot_reservations_are_disjoint_and_complete() {
    for grid in [40, 59, 510] {
        let mut cells = HashSet::new();
        for patch in 0..4 {
            let left = (patch % 2) as u32 * (grid - 8);
            let top = (patch / 2) as u32 * (grid - 8);
            for y in top..top + 8 {
                for x in left..left + 8 {
                    assert!(cells.insert((x, y)));
                }
            }
            let (left, top) = pilot_origin(grid, patch);
            for y in top..top + 4 {
                for x in left..left + 8 {
                    assert!(cells.insert((x, y)));
                }
            }
        }
        for y in (1..6).chain(grid - 6..grid - 1) {
            for x in 8..grid - 8 {
                assert!(cells.insert((x, y)));
            }
        }
        assert_eq!(cells.len(), (10 * grid + 224) as usize);
        for y in 0..grid {
            for x in 0..grid {
                assert_eq!(reserved(grid, x, y), cells.contains(&(x, y)));
            }
        }
        assert_eq!(data_cell_count(grid), (grid * grid) as usize - cells.len());
    }
}
