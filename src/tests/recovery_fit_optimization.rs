use super::*;
#[test]
fn class_context_covariance_preserves_solutions() {
    let mut state = 93_u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for case in 0..256 {
        let samples: Vec<Sample<10>> = (0..32 + (case % 19) * 13)
            .map(|i| {
                let class = if case % 31 == 0 {
                    0
                } else {
                    next() as usize % 4
                };
                let mut basis = [0.0; 10];
                basis[class] = 1.0;
                for value in &mut basis[4..8] {
                    *value = [-3.0, -1.0, 1.0, 3.0][next() as usize % 4];
                }
                for value in &mut basis[8..] {
                    *value = if next() & 1 == 0 { -1.0 } else { 1.0 };
                }
                Sample {
                    basis,
                    value: (next() % 40000) as f64 / 137.0 - i as f64,
                    class,
                }
            })
            .collect();
        let weights: Vec<_> = samples
            .iter()
            .map(|_| [0.0, 0.1, 0.3, 0.9, 1.0][next() as usize % 5])
            .collect();
        for weights in [None, Some(weights.as_slice())] {
            let expected = solve(&samples, weights, false);
            let actual = solve(&samples, weights, true);
            assert_eq!(
                actual.map(|values| values.map(f64::to_bits)),
                expected.map(|values| values.map(f64::to_bits)),
                "case {case}"
            );
        }
    }
}
#[test]
fn zero_weight_missing_nodes_do_not_hide_missing_interpolation_support() {
    let mut field = Field::<1>::estimate(17, 8, 1, false, |_| None);
    field.models[0] = Some(Model {
        coefficients: [42.0],
        variance: 3.0,
    });
    assert_eq!(field.at(0, 0).unwrap().coefficients, [42.0]);
    assert!(field.at(1, 0).is_none());
    assert!(field.at(0, 1).is_none());
    field.models[1] = Some(Model {
        coefficients: [50.0],
        variance: 3.0,
    });
    assert_eq!(field.at(4, 0).unwrap().coefficients, [46.0]);
    assert_eq!(field.at(8, 0).unwrap().coefficients, [50.0]);
    assert!(field.at(9, 0).is_none());
    field.models[8] = Some(Model {
        coefficients: [80.0],
        variance: 3.0,
    });
    assert_eq!(field.at(16, 16).unwrap().coefficients, [80.0]);
}
