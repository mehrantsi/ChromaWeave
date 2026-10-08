use super::*;
use crate::codec;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn validation_errors_are_machine_readable() {
    let truncated = RepairSymbol::parse(&[]).unwrap_err();
    assert_eq!(
        truncated.downcast_ref::<Error>(),
        Some(&Error::TruncatedRepairHeader)
    );
    let invalid = match RepairTransfer::with_id(&[1], 10240, 21, [0; 16]) {
        Ok(_) => panic!("invalid repair group accepted"),
        Err(error) => error,
    };
    assert_eq!(
        invalid.downcast_ref::<Error>(),
        Some(&Error::InvalidRepairGroupSize)
    );
}

#[test]
fn cwt_header_padding_and_group_offsets_match_protocol() {
    let bytes: Vec<_> = (0..20483).map(|i| (i % 251) as u8).collect();
    let id = std::array::from_fn(|i| i as u8);
    let transfer = RepairTransfer::with_id(&bytes, 10240, 20, id).unwrap();
    let symbols = transfer.group_symbols(0).unwrap();
    assert_eq!(symbols.len(), 7);
    assert_eq!(transfer.source_chunk_count(), 3);
    assert_eq!(transfer.symbol_count(), 7);
    assert_eq!(
        hex(&symbols[0].envelope().unwrap()[..REPAIR_HEADER_BYTES]),
        "43575400000102030405060708090a0b0c0d0e0f000003000028035000005c273141ce94288fe8be6feeee7741db232a12e1b9409dc967127b0161ef13201404"
    );
    for (index, symbol) in symbols.iter().enumerate() {
        assert_eq!(usize::from(symbol.header.symbol_index), index);
        assert_eq!(symbol.data.len(), 10240);
        let envelope = symbol.envelope().unwrap();
        let parsed = RepairSymbol::parse(&envelope).unwrap();
        assert_eq!(parsed.header, symbol.header);
        assert_eq!(parsed.data, symbol.data);
    }
    assert_eq!(&symbols[2].data[..3], &bytes[20480..]);
    assert!(symbols[2].data[3..].iter().all(|byte| *byte == 0));

    for (group_data, source_count, checks) in [
        (
            20,
            21,
            vec![
                (19, Some(19), None),
                (20, None, Some(0)),
                (23, None, Some(3)),
                (24, Some(20), None),
                (28, None, Some(3)),
            ],
        ),
        (
            60,
            61,
            vec![
                (59, Some(59), None),
                (60, None, Some(0)),
                (63, None, Some(3)),
                (64, Some(60), None),
                (68, None, Some(3)),
            ],
        ),
    ] {
        let header = |symbol_index| RepairHeader {
            transfer_id: [0; 16],
            symbol_index,
            source_chunk_count: source_count,
            chunk_bytes: 10240,
            original_bytes: (usize::from(source_count) - 1) as u32 * 10240 + 1,
            file_sha256: [0; 32],
            group_data,
            group_parity: 4,
        };
        for (symbol_index, source_index, repair_index) in checks {
            let position = header(symbol_index).position().unwrap();
            assert_eq!(position.source_index, source_index);
            assert_eq!(position.repair_index, repair_index);
        }
    }
}

#[test]
fn cwt_rejects_noncanonical_headers_and_source_padding() {
    let bytes: Vec<_> = (0..20483).map(|i| (i % 251) as u8).collect();
    let symbol = RepairTransfer::with_id(&bytes, 10240, 20, [5; 16])
        .unwrap()
        .group_symbols(0)
        .unwrap()
        .remove(2)
        .envelope()
        .unwrap();
    for length in 0..REPAIR_HEADER_BYTES {
        assert!(RepairSymbol::parse(&symbol[..length]).is_err());
    }
    for (start, end, replacement) in [
        (0, 4, b"XXXX".as_slice()),
        (20, 22, &7u16.to_le_bytes()),
        (22, 24, &2u16.to_le_bytes()),
        (24, 26, &1u16.to_le_bytes()),
        (26, 30, &0u32.to_le_bytes()),
        (62, 63, &[19]),
        (63, 64, &[3]),
    ] {
        let mut damaged = symbol.clone();
        damaged[start..end].copy_from_slice(replacement);
        assert!(RepairSymbol::parse(&damaged).is_err());
    }
    let mut damaged_padding = symbol.clone();
    *damaged_padding.last_mut().unwrap() = 1;
    assert!(RepairSymbol::parse(&damaged_padding).is_err());
    let mut truncated = symbol;
    truncated.pop();
    assert!(RepairSymbol::parse(&truncated).is_err());
}

#[test]
fn cwt_parity_matches_native_encoder_golden_vectors() {
    let goldens = [
        (
            10240,
            [
                "e35d23f00223d0866534484261672efbeb7de9b9b7cb9c6f7285b25277db1711",
                "5c71abf62f7fed4a37407cc03f11f6a52b111223c140c274dbacfd4b552c41d2",
                "0aca47bed64240d55d9d788015575990fc18ed3d383cc41f234a56063a342845",
                "cd55dfaa4ec6a53dd42bca8cc7a83fb60c08b21eec9b48e1f74b91e5870f2d47",
            ],
        ),
        (
            20480,
            [
                "872a92664265444f0f04e3ce4169db8689100877f89441d3188dca450087785e",
                "f488fcf41f2fbcc2bca1bc18fafcecc26dfecb54bb32b69d4f1ca2441cced96e",
                "d80356506b879888f4e561a8c0456cb5ec38da5cdd5d4ad7d1aca9c22930dbda",
                "45bd8abb77cdf8b4a3cd20c10cd91ac3b1c5dee0fb412b93b405b7d63d922585",
            ],
        ),
    ];
    for (chunk_bytes, expected) in goldens {
        let data: Vec<Vec<u8>> = (0..3)
            .map(|shard| {
                (0..chunk_bytes)
                    .map(|byte| (byte * 73 + shard * 41) as u8)
                    .collect()
            })
            .collect();
        let parity = crate::transfer_repair::encode_parity(&data).unwrap();
        assert_eq!(parity.len(), 4);
        for (shard, expected_hash) in parity.iter().zip(expected) {
            assert_eq!(hex(&Sha256::digest(shard)), expected_hash);
        }
    }
}

#[test]
fn both_chunk_sizes_round_trip_and_hash_the_complete_file() {
    for size in CHUNK_SIZES {
        for group_data in REPAIR_GROUP_SIZES {
            for length in [1, size - 1, size, size + 1, 2 * size, 2 * size + 3] {
                let bytes: Vec<_> = (0..length).map(|i| (i * 73 + 19) as u8).collect();
                let transfer = RepairTransfer::with_id(&bytes, size, group_data, [7; 16]).unwrap();
                let mut joined = Vec::new();
                for group_index in 0..transfer.group_count() {
                    for symbol in transfer.group_symbols(group_index).unwrap() {
                        let envelope = symbol.envelope().unwrap();
                        let parsed = RepairSymbol::parse(&envelope).unwrap();
                        assert_eq!(
                            usize::from(parsed.header.source_chunk_count),
                            length.div_ceil(size)
                        );
                        assert_eq!(
                            parsed.header.file_sha256,
                            <[u8; 32]>::from(Sha256::digest(&bytes))
                        );
                        if let Some(index) = parsed.header.position().unwrap().source_index {
                            joined.extend_from_slice(
                                &parsed.data[..(length - index * size).min(size)],
                            );
                        }
                    }
                }
                assert_eq!(joined, bytes);
                assert!(transfer.group_symbols(transfer.group_count()).is_err());
            }
        }
    }
}

#[test]
fn transfer_bounds_are_rejected() {
    assert!(RepairTransfer::with_id(&[], 10240, 20, [0; 16]).is_err());
    assert!(RepairTransfer::with_id(&[1], 10239, 20, [0; 16]).is_err());
    let bytes = vec![0; MAX_FILE_BYTES + 1];
    assert!(RepairTransfer::with_id(&bytes, 20480, 20, [0; 16]).is_err());
    let maximum = RepairTransfer::with_id(&bytes[..MAX_FILE_BYTES], 10240, 20, [0; 16]).unwrap();
    assert_eq!(maximum.source_chunk_count(), 3277);
    let group = maximum.group_symbols(maximum.group_count() - 1).unwrap();
    let envelope = group[16].envelope().unwrap();
    let tail = RepairSymbol::parse(&envelope).unwrap();
    assert_eq!(tail.header.position().unwrap().source_index, Some(3276));
    assert_eq!(tail.data.len(), 10240);
    assert!(tail.data[8192..].iter().all(|byte| *byte == 0));
}

#[test]
fn corrupted_magic_is_rejected() {
    let mut envelope = RepairTransfer::with_id(&[9], 10240, 20, [0; 16])
        .unwrap()
        .group_symbols(0)
        .unwrap()[0]
        .envelope()
        .unwrap();
    envelope[..4].copy_from_slice(b"XXXX");
    assert_eq!(
        RepairSymbol::parse(&envelope)
            .unwrap_err()
            .downcast_ref::<Error>(),
        Some(&Error::UnrecognizedRepairSymbol)
    );
}

#[test]
fn cw_packets_preserve_envelopes_without_compression() {
    let bytes: Vec<_> = (0..20483).map(|i| (i % 251) as u8).collect();
    let transfer = RepairTransfer::with_id(&bytes, 20480, 20, [3; 16]).unwrap();
    let symbols = transfer.group_symbols(0).unwrap();
    for mode in [codec::Modulation::Luma10, codec::Modulation::Luma10LowEcc] {
        for symbol in &symbols {
            let envelope = symbol.envelope().unwrap();
            let encoded = codec::encode_with_options(
                &envelope,
                4,
                mode,
                codec::EncodeOptions {
                    compression: codec::Compression::None,
                },
            )
            .unwrap();
            assert_eq!(encoded.layout.compression, "none");
            let decoded = codec::decode_with_modulation(&encoded.image, 4, mode).unwrap();
            assert_eq!(decoded.payload, envelope);
            assert_eq!(
                RepairSymbol::parse(&decoded.payload).unwrap().data,
                symbol.data
            );
        }
    }
}
