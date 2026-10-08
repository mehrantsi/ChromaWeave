use chromaweave::{
    channel::{ChannelConfig, Medium, simulate},
    codec::{decode, encode},
};

#[test]
fn recovers_binary_payloads_through_both_mild_synthetic_channels() {
    let payload: Vec<u8> = (0..1024).map(|i| (i * 131 + 47) as u8).collect();
    let encoded = encode(&payload, 8).unwrap();
    for medium in [Medium::Screen, Medium::Print] {
        for seed in [1, 41, 731] {
            let observed = simulate(
                &encoded.image,
                &ChannelConfig {
                    medium,
                    severity: 0.35,
                    seed,
                },
            )
            .unwrap();
            let recovered = decode(&observed, 8).unwrap();
            assert_eq!(recovered.payload, payload, "medium {medium:?}, seed {seed}");
        }
    }
}
