use super::*;

#[test]
fn pam_pilots_span_six_balanced_orthogonal_axes() {
    for a in 0..6 {
        let mean: f32 = (0..PILOTS).map(|i| latent(pilot_symbol(i), a)).sum();
        assert_eq!(mean, 0.0);
        for b in 0..6 {
            let gram: f32 = (0..PILOTS)
                .map(|i| {
                    let symbol = pilot_symbol(i);
                    latent(symbol, a) * latent(symbol, b)
                })
                .sum();
            assert_eq!(
                gram,
                if a != b {
                    0.0
                } else if a < 4 {
                    160.0
                } else {
                    32.0
                }
            );
        }
    }
}
