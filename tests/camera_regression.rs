use chromaweave::{codec::Modulation, scanner};
use image::imageops;

#[test]
fn adaptive_luma_preserves_structured_payloads() {
    for mode in [Modulation::Luma10, Modulation::Luma10LowEcc] {
        for payload in [
            vec![0; 10_000],
            vec![255; 2048],
            b"camera regression: structured text\n".repeat(80),
        ] {
            for pitch in [2, 8] {
                let encoded = scanner::encode_with_modulation(&payload, pitch, mode).unwrap();
                let recovered = scanner::decode(&encoded.image).unwrap();
                assert_eq!(recovered.payload, payload, "{mode:?}, pitch {pitch}");
            }
        }
    }
}

#[test]
#[ignore = "requires CW_CAMERA_FIXTURE and CW_CAMERA_EXPECTED"]
fn recovers_external_camera_frame_and_geometric_variants() {
    let frame = image::open(std::env::var("CW_CAMERA_FIXTURE").unwrap())
        .unwrap()
        .to_rgb8();
    let expected = std::fs::read(std::env::var("CW_CAMERA_EXPECTED").unwrap()).unwrap();
    let variants = [
        ("original", frame.clone()),
        ("rotated", imageops::rotate90(&frame)),
        ("mirrored", imageops::flip_horizontal(&frame)),
        (
            "scaled",
            imageops::resize(
                &frame,
                frame.width() * 3 / 4,
                frame.height() * 3 / 4,
                imageops::FilterType::Lanczos3,
            ),
        ),
    ];
    for (name, image) in variants {
        let start = std::time::Instant::now();
        let decoded = scanner::decode(&image).unwrap_or_else(|error| panic!("{name}: {error:#}"));
        assert_eq!(decoded.payload, expected, "{name}");
        eprintln!(
            "{name}: {} bytes, {:?}, {} corrected, {} erased",
            expected.len(),
            start.elapsed(),
            decoded.stats.corrected_bytes,
            decoded.stats.erased_bytes
        );
    }
}
