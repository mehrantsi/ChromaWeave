use super::*;

#[test]
fn compressed_roundtrips_preserve_arbitrary_bytes() {
    for payload in [
        Vec::new(),
        vec![0; 120_000],
        (0..=255).cycle().take(8192).collect(),
        b"CWV4\x28\xb5\x2f\xfd accidental format prefixes".to_vec(),
    ] {
        assert_eq!(decompress(&compress(&payload).unwrap()).unwrap(), payload);
    }
}

#[test]
fn rejects_corruption_lengths_truncation_and_trailing_frames() {
    let encoded = compress(&vec![0x53; 8192]).unwrap();
    for length in 0..encoded.len() {
        assert!(
            decompress(&encoded[..length]).is_err(),
            "truncated length {length}"
        );
    }
    let mut oversized = encoded.clone();
    oversized[..4].copy_from_slice(&((MAX_DECOMPRESSED_BYTES + 1) as u32).to_le_bytes());
    assert!(
        decompress(&oversized)
            .unwrap_err()
            .to_string()
            .contains("compression limit")
    );
    let mut wrong_length = encoded.clone();
    wrong_length[..4].copy_from_slice(&8191_u32.to_le_bytes());
    assert!(decompress(&wrong_length).is_err());
    let mut trailing = encoded.clone();
    trailing.push(0);
    assert!(decompress(&trailing).is_err());
    let mut concatenated = encoded.clone();
    concatenated.extend_from_slice(&encoded[4..]);
    assert!(decompress(&concatenated).is_err());
    let mut damaged = encoded;
    damaged[4] ^= 1;
    assert!(decompress(&damaged).is_err());
}
