use super::*;

fn pilots() -> (Vec<[f32; 3]>, Vec<Pilot>) {
    let side = 32;
    let values: Vec<_> = (0..side * side)
        .map(|index| {
            std::array::from_fn(|c| ((index * 73 + index * index * 17 + c * 47) % 251) as f32)
        })
        .collect();
    let pilots = values
        .iter()
        .enumerate()
        .map(|(index, &value)| Pilot {
            index,
            value,
            patch: (index % side / 16) + 2 * (index / side / 16),
        })
        .collect();
    (values, pilots)
}

#[test]
fn identity_is_exact_with_only_one_informative_pilot_patch() {
    let (mut observed, pilots) = pilots();
    for pilot in &pilots {
        if pilot.patch != 3 {
            observed[pilot.index] = [245.0, 0.0, 0.0];
        }
    }
    let (output, models) = equalize(&observed, 32, &pilots).unwrap();
    assert_eq!(output, observed);
    assert!(
        models
            .iter()
            .all(|model| model.horizontal == 0.0 && model.vertical == 0.0)
    );
    assert!(equalize(&vec![[245.0; 3]; 32 * 32], 32, &pilots).is_none());
}

#[test]
fn row_filters_preserve_the_symmetric_separable_operator() {
    let input: Vec<_> = (0..64).map(|i| (i * i % 31) as f64).collect();
    let mut scratch = vec![0.0; 64];
    let mut actual = vec![0.0; 64];
    filter(&input, 8, 0.13, 0.19, &mut scratch, &mut actual);
    for y in 0..8 {
        for x in 0..8 {
            let mut expected = 0.0;
            for (dy, wy) in [(-1, 0.19), (0, 0.62), (1, 0.19)] {
                for (dx, wx) in [(-1, 0.13), (0, 0.74), (1, 0.13)] {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if (0..8).contains(&nx) && (0..8).contains(&ny) {
                        expected += wx * wy * input[ny as usize * 8 + nx as usize];
                    }
                }
            }
            assert!((actual[y * 8 + x] - expected).abs() < 1e-12);
        }
    }
}

#[test]
fn reduction_preserves_short_vectors_and_partial_groups() {
    for length in 0..=257 {
        let a: Vec<_> = (0..length)
            .map(|i| ((i * 73 + 19) % 251) as f64 / 11.0 - 12.0)
            .collect();
        let b: Vec<_> = (0..length)
            .map(|i| ((i * 31 + 7) % 131) as f64 / 13.0 - 8.0)
            .collect();
        let reference = a.iter().zip(&b).map(|(a, b)| a * b).sum::<f64>();
        assert!((dot_values(&a, &b) - reference).abs() < 1e-10);
        let norm = a.iter().map(|v| v * v).sum::<f64>();
        assert!((dot_values(&a, &a) - norm).abs() < 1e-10);
    }
}

#[test]
fn invalid_lattice_or_pilot_indices_are_rejected_before_allocation() {
    assert!(equalize(&[], MAX_LATTICE_SIDE + 1, &[]).is_none());
    assert!(
        equalize(
            &[[0.0; 3]; 9],
            3,
            &[Pilot {
                index: 9,
                value: [0.0; 3],
                patch: 0
            }]
        )
        .is_none()
    );
}
