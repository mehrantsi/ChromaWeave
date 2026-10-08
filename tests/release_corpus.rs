#[path = "support/release_cases.rs"]
mod cases;

use chromaweave::{codec, recovery::PhotoRecovery, scanner, transfer, transfer_repair};
use image::{Rgb, RgbImage, imageops};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/release")
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn manifest() -> Value {
    serde_json::from_slice(&fs::read(root().join("manifest.json")).unwrap()).unwrap()
}

fn transport(payload: &[u8], compressed: bool) -> Vec<u8> {
    if !compressed {
        return payload.to_vec();
    }
    let mut bytes = (payload.len() as u32).to_le_bytes().to_vec();
    bytes.extend(zstd::bulk::compress(payload, 3).unwrap());
    bytes
}

fn wire(payload: &[u8], layout: &codec::Layout) -> Vec<u8> {
    let payload = transport(payload, layout.compression == "zstd");
    let magic = match (layout.modulation, layout.compression) {
        (codec::Modulation::Luma10, "none") => b"CWI4",
        (codec::Modulation::Luma10, _) => b"CWI5",
        (codec::Modulation::Luma10LowEcc, "none") => b"CWE4",
        (codec::Modulation::Luma10LowEcc, _) => b"CWE5",
    };
    let mut packet = vec![0; layout.rs_blocks * layout.rs_data_bytes];
    packet[..4].copy_from_slice(magic);
    packet[4..8].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    let mut crc = crc32fast::Hasher::new();
    crc.update(&packet[..8]);
    crc.update(&payload);
    packet[8..12].copy_from_slice(&crc.finalize().to_le_bytes());
    packet[12..12 + payload.len()].copy_from_slice(&payload);
    let encoder = reed_solomon::Encoder::new(layout.rs_parity_bytes);
    packet
        .chunks_exact(layout.rs_data_bytes)
        .flat_map(|data| encoder.encode(data).to_vec())
        .collect()
}

#[test]
fn checkpoint_packets_rasters_and_decoders_remain_compatible() {
    let manifest = manifest();
    assert_eq!(manifest["checkpoint"], cases::CHECKPOINT);
    for case in cases::optical_cases() {
        let old = &manifest["optical"][&case.name];
        assert_eq!(
            hash(&case.payload),
            old["payload_sha256"],
            "{} payload",
            case.name
        );
        let bare =
            codec::encode_with_options(&case.payload, 4, case.modulation, case.options).unwrap();
        let framed =
            scanner::encode_with_options(&case.payload, 4, case.modulation, case.options).unwrap();
        let expected_wire = fs::read(root().join(format!("{}.wire", case.name))).unwrap();
        assert_eq!(
            wire(&case.payload, &bare.layout),
            expected_wire,
            "{} wire",
            case.name
        );
        for (suffix, encoded, is_framed) in [("bare", bare, false), ("framed", framed, true)] {
            let golden = image::open(root().join(format!("{}.{suffix}.png", case.name)))
                .unwrap()
                .to_rgb8();
            assert_eq!(encoded.image, golden, "{} {suffix} pixels", case.name);
            for key in [
                "grid_cells",
                "cell_pixels",
                "image_side_pixels",
                "rs_blocks",
                "rs_data_bytes",
                "rs_parity_bytes",
                "coded_bytes",
                "payload_capacity_bytes",
                "payload_bytes",
                "encoded_payload_bytes",
                "compression",
                "net_bits_per_pixel",
            ] {
                assert_eq!(
                    serde_json::to_value(&encoded.layout).unwrap()[key],
                    old[suffix][key],
                    "{} {suffix} {key}",
                    case.name
                );
            }
            let decoded = if is_framed {
                scanner::decode(&golden)
            } else {
                codec::decode_with_modulation(&golden, 4, case.modulation)
            }
            .unwrap();
            assert_eq!(
                decoded.payload, case.payload,
                "{} {suffix} checkpoint decode",
                case.name
            );
            assert_eq!(decoded.layout.modulation, case.modulation);
            assert_eq!(decoded.layout.compression, encoded.layout.compression);
            assert_eq!(decoded.stats.corrected_bytes, 0);
            assert_eq!(decoded.stats.erased_bytes, 0);
        }
    }
}

#[test]
fn checkpoint_transfer_envelopes_cover_chunk_and_group_boundaries() {
    let manifest = manifest();
    for (name, chunk, group, payload) in cases::transfer_cases() {
        let old = &manifest["transfer"][&name];
        assert_eq!(hash(&payload), old["payload_sha256"], "{name} payload");
        let repair =
            transfer::RepairTransfer::with_id(&payload, chunk, group, cases::TRANSFER_ID).unwrap();
        for group_index in 0..repair.group_count() {
            let symbols = repair.group_symbols(group_index).unwrap();
            for symbol in &symbols {
                let envelope = symbol.envelope().unwrap();
                assert_eq!(
                    hash(&envelope),
                    old["cwt"][symbol.header.symbol_index.to_string()],
                    "{name} CWT {}",
                    symbol.header.symbol_index
                );
                let parsed = transfer::RepairSymbol::parse(&envelope).unwrap();
                assert_eq!(parsed.header, symbol.header);
                assert_eq!(parsed.data, symbol.data);
            }
            let sources = symbols.len() - transfer_repair::PARITY_SHARDS;
            let retained_group =
                payload.len() < 3 * chunk || group_index == repair.group_count() - 1;
            let mut shards = Vec::new();
            for symbol in &symbols {
                let data = if retained_group {
                    let golden = fs::read(
                        root().join(format!("{name}.symbol-{}.bin", symbol.header.symbol_index)),
                    )
                    .unwrap();
                    transfer::RepairSymbol::parse(&golden)
                        .unwrap()
                        .data
                        .to_vec()
                } else {
                    symbol.data.clone()
                };
                shards.push(Some(data));
            }
            for shard in shards.iter_mut().take(sources.min(4)) {
                *shard = None;
            }
            let recovered = transfer_repair::recover(sources, &shards).unwrap();
            assert_eq!(recovered.len(), sources.min(4));
            for (index, bytes) in recovered {
                assert_eq!(
                    bytes, symbols[index].data,
                    "{name} recovered source {index}"
                );
            }
            if sources > 4 {
                shards[4] = None;
                assert!(
                    transfer_repair::recover(sources, &shards).is_err(),
                    "{name} five source erasures"
                );
            }
        }
    }
}

#[test]
fn malformed_checkpoint_transfer_envelopes_are_rejected() {
    let source = fs::read(root().join("cwt-10240-20-short.symbol-2.bin")).unwrap();
    for length in [0, 3, 63, source.len() - 1] {
        assert!(transfer::RepairSymbol::parse(&source[..length]).is_err());
    }
    let mut trailing = source.clone();
    trailing.push(0);
    assert!(transfer::RepairSymbol::parse(&trailing).is_err());
    for (offset, value) in [(0, b'X'), (22, 4), (25, 0), (62, 21), (63, 3), (67, 1)] {
        let mut damaged = source.clone();
        damaged[offset] = value;
        assert!(
            transfer::RepairSymbol::parse(&damaged).is_err(),
            "offset {offset}"
        );
    }
}

#[test]
fn distributable_synthetic_capture_exercises_scanner_geometry() {
    let golden = image::open(root().join("synthetic-projective.png"))
        .unwrap()
        .to_rgb8();
    let expected = cases::binary(1007);
    for (name, frame) in [
        ("original", golden.clone()),
        ("rotated", imageops::rotate90(&golden)),
        ("mirrored", imageops::flip_horizontal(&golden)),
    ] {
        assert_eq!(
            scanner::decode(&frame)
                .unwrap_or_else(|error| panic!("{name}: {error:#}"))
                .payload,
            expected
        );
    }
    assert!(scanner::decode(&RgbImage::from_pixel(300, 220, Rgb([230; 3]))).is_err());
    let mut missing_finder = image::open(root().join("standard-binary.framed.png"))
        .unwrap()
        .to_rgb8();
    for y in 0..40 {
        for x in 0..40 {
            missing_finder.put_pixel(x, y, Rgb([255; 3]));
        }
    }
    assert!(scanner::decode(&missing_finder).is_err());
}

#[test]
#[ignore = "explicit fixture generation; run against the recorded checkpoint"]
fn generate_release_corpus() {
    let output = PathBuf::from(
        std::env::var("CW_CORPUS_OUTPUT")
            .expect("set CW_CORPUS_OUTPUT to an empty output directory"),
    );
    fs::create_dir_all(&output).unwrap();
    assert_eq!(
        fs::read_dir(&output).unwrap().count(),
        0,
        "fixture generation requires an empty directory"
    );
    let mut manifest = json!({"schema_version": 1, "checkpoint": cases::CHECKPOINT, "optical": {}, "transfer": {}});
    for case in cases::optical_cases() {
        let bare =
            codec::encode_with_options(&case.payload, 4, case.modulation, case.options).unwrap();
        let framed =
            scanner::encode_with_options(&case.payload, 4, case.modulation, case.options).unwrap();
        let bytes = wire(&case.payload, &bare.layout);
        fs::write(output.join(format!("{}.wire", case.name)), &bytes).unwrap();
        bare.image
            .save(output.join(format!("{}.bare.png", case.name)))
            .unwrap();
        framed
            .image
            .save(output.join(format!("{}.framed.png", case.name)))
            .unwrap();
        manifest["optical"][&case.name] = json!({"payload_sha256": hash(&case.payload), "wire_sha256": hash(&bytes), "bare_rgb_sha256": hash(bare.image.as_raw()), "framed_rgb_sha256": hash(framed.image.as_raw()), "bare": bare.layout, "framed": framed.layout});
    }
    for (name, chunk, group, payload) in cases::transfer_cases() {
        let repair =
            transfer::RepairTransfer::with_id(&payload, chunk, group, cases::TRANSFER_ID).unwrap();
        let mut symbols = json!({});
        for group_index in 0..repair.group_count() {
            for symbol in repair.group_symbols(group_index).unwrap() {
                let bytes = symbol.envelope().unwrap();
                symbols[symbol.header.symbol_index.to_string()] = json!(hash(&bytes));
                if payload.len() < 3 * chunk || group_index == repair.group_count() - 1 {
                    fs::write(
                        output.join(format!("{name}.symbol-{}.bin", symbol.header.symbol_index)),
                        &bytes,
                    )
                    .unwrap();
                }
            }
        }
        manifest["transfer"][name] = json!({"payload_sha256": hash(&payload), "cwt": symbols});
    }
    fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
}

#[test]
fn distributable_camera_crop_recovers_exact_payload_through_public_pipelines() {
    let bytes = include_bytes!("fixtures/camera-code-only.png");
    let provenance: Value =
        serde_json::from_slice(include_bytes!("fixtures/camera-provenance.json")).unwrap();
    assert_eq!(hash(bytes), provenance["output_sha256"]);
    let camera = image::load_from_memory(bytes).unwrap().to_rgb8();
    for (name, frame) in [
        ("original", camera.clone()),
        ("rotated", imageops::rotate90(&camera)),
        ("mirrored", imageops::flip_horizontal(&camera)),
    ] {
        let decoded = scanner::decode(&frame).unwrap_or_else(|error| panic!("{name}: {error:#}"));
        assert_eq!(decoded.payload.len(), 20480);
        assert_eq!(
            hash(&decoded.payload),
            provenance["payload_sha256"],
            "{name}"
        );
        assert_eq!(decoded.layout.modulation, codec::Modulation::Luma10LowEcc);
    }
    let mut scanner = scanner::TransferScanner::new();
    let decoded = scanner.scan(&camera).result.unwrap();
    assert_eq!(hash(&decoded.payload), provenance["payload_sha256"]);
    scanner.reset();
    let mut recovery = PhotoRecovery::new();
    let progress = recovery.push(&camera).unwrap();
    assert!(recovery.code_detected());
    assert_eq!(progress.certified_blocks, progress.total_blocks);
    assert_eq!(
        hash(&progress.decoded.unwrap().payload),
        provenance["payload_sha256"]
    );
}
