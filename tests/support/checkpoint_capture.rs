use chromaweave::{
    capture::{CaptureConfig, simulate_capture},
    codec::Modulation,
    scanner,
};
#[allow(dead_code)]
mod cases {
    include!(env!("CW_RELEASE_CASES"));
}

#[test]
fn generate_synthetic_projective_capture() {
    let encoded =
        scanner::encode_with_modulation(&cases::binary(1007), 8, Modulation::Luma10).unwrap();
    let capture = simulate_capture(
        &encoded.image,
        &CaptureConfig {
            rotation_degrees: 17.0,
            perspective: 0.12,
            frame_side: 640,
            code_fraction: 0.75,
            seed: 41,
            clutter: true,
        },
    )
    .unwrap();
    assert_eq!(
        scanner::decode(&capture.image).unwrap().payload,
        cases::binary(1007)
    );
    capture
        .image
        .save(
            std::path::Path::new(&std::env::var("CW_CORPUS_OUTPUT").unwrap())
                .join("synthetic-projective.png"),
        )
        .unwrap();
}
