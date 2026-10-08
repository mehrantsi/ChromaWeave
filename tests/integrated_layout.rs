use chromaweave::{
    codec::{self, Modulation},
    scanner,
};
use image::imageops;
use rand::{RngCore, SeedableRng};
use rand_chacha::ChaCha8Rng;

fn payload(seed: u64, length: usize) -> Vec<u8> {
    let mut bytes = vec![0; length];
    ChaCha8Rng::seed_from_u64(seed).fill_bytes(&mut bytes);
    bytes
}

#[test]
fn integrated_native_frames_preserve_payload_in_all_orientations() {
    for seed in 610001..610006 {
        let payload = payload(seed, 2048);
        let encoded = scanner::encode_with_modulation(&payload, 2, Modulation::Luma10).unwrap();
        assert_eq!(encoded.image.dimensions(), (126, 126));
        assert_eq!(encoded.layout.grid_cells, 59);
        assert_eq!(encoded.layout.rs_blocks, 13);
        assert_eq!(encoded.layout.payload_capacity_bytes, 2055);
        let mut image = encoded.image;
        for _ in 0..4 {
            for reflected in [false, true] {
                let image = if reflected {
                    imageops::flip_horizontal(&image)
                } else {
                    image.clone()
                };
                let decoded = scanner::decode(&image).unwrap();
                assert_eq!(
                    decoded.payload, payload,
                    "seed {seed} reflected {reflected}"
                );
                assert_eq!(decoded.layout.modulation, Modulation::Luma10);
            }
            image = imageops::rotate90(&image);
        }
    }
}

#[test]
fn integrated_capacities_and_short_payloads_respect_reserved_cells() {
    let mode = Modulation::Luma10;
    for length in [0, 1, 147, 148, 2048, 2055, 2056, 4096] {
        let data = payload(610011, length);
        let bare = codec::encode_with_modulation(&data, 2, mode).unwrap();
        assert!(bare.layout.grid_cells >= 40);
        assert_eq!(
            codec::decode_with_modulation(&bare.image, 2, mode)
                .unwrap()
                .payload,
            data
        );
        let framed = scanner::encode_with_modulation(&data, 2, mode).unwrap();
        assert_eq!(scanner::decode(&framed.image).unwrap().payload, data);
    }
    let maximum = scanner::maximum_payload_bytes_with_modulation(2, mode).unwrap();
    assert_eq!(maximum, 198420);
    assert!(scanner::encode_with_modulation(&vec![0; maximum + 1], 2, mode).is_err());
    let at_limit = scanner::encode_with_modulation(&vec![0; maximum], 2, mode).unwrap();
    assert_eq!(at_limit.layout.grid_cells, 510);
}

#[test]
fn low_ecc_native_frames_roundtrip_and_preserve_profile_identity() {
    let mode = Modulation::Luma10LowEcc;
    let payload = payload(610021, 2048);
    let encoded = scanner::encode_with_modulation(&payload, 2, mode).unwrap();
    assert_eq!(encoded.layout.rs_data_bytes, 207);
    assert_eq!(encoded.layout.rs_parity_bytes, 48);
    assert_eq!(encoded.layout.grid_cells, 53);
    let mut image = encoded.image;
    for _ in 0..4 {
        for reflected in [false, true] {
            let oriented = if reflected {
                imageops::flip_horizontal(&image)
            } else {
                image.clone()
            };
            let decoded = scanner::decode(&oriented).unwrap();
            assert_eq!(decoded.payload, payload);
            assert_eq!(decoded.layout.modulation, mode);
            assert_eq!(decoded.layout.rs_data_bytes, 207);
            assert_eq!(decoded.layout.rs_parity_bytes, 48);
        }
        image = imageops::rotate90(&image);
    }
    let bare = codec::encode_with_modulation(&payload, 2, mode).unwrap();
    assert!(codec::decode_with_modulation(&bare.image, 2, Modulation::Luma10).is_err());
    assert_eq!(
        codec::decode_with_modulation(&bare.image, 2, mode)
            .unwrap()
            .payload,
        payload
    );
}

#[test]
fn low_ecc_capacity_boundaries_use_207_data_bytes_per_block() {
    let mode = Modulation::Luma10LowEcc;
    for (length, blocks) in [(0, 4), (816, 4), (817, 5), (1023, 5), (1024, 6)] {
        let data = payload(610031, length);
        let encoded = scanner::encode_with_modulation(&data, 2, mode).unwrap();
        assert_eq!(encoded.layout.rs_blocks, blocks);
        assert_eq!(scanner::decode(&encoded.image).unwrap().payload, data);
    }
    let maximum = scanner::maximum_payload_bytes_with_modulation(2, mode).unwrap();
    assert_eq!(maximum, 258324);
    assert!(scanner::encode_with_modulation(&vec![0; maximum + 1], 2, mode).is_err());
}
