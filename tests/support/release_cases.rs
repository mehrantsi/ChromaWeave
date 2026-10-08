use chromaweave::codec::{Compression, EncodeOptions, Modulation};

pub const CHECKPOINT: &str = "b73e3d7c98884191a58b0bd1123009040c0d13a0";
pub const TRANSFER_ID: [u8; 16] = *b"CW release tests";

pub struct OpticalCase {
    pub name: String,
    pub payload: Vec<u8>,
    pub modulation: Modulation,
    pub options: EncodeOptions,
}

pub fn binary(length: usize) -> Vec<u8> {
    let mut state = 0xc0de_2026_1007_0001_u64;
    let mut bytes = Vec::with_capacity(length);
    while bytes.len() < length {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        bytes.extend_from_slice(&(value ^ (value >> 31)).to_le_bytes());
    }
    bytes.truncate(length);
    bytes
}

pub fn optical_cases() -> Vec<OpticalCase> {
    let mut cases = Vec::new();
    for (profile, modulation, block_tail, minimum_tail) in [
        ("standard", Modulation::Luma10, 147, 624),
        ("low", Modulation::Luma10LowEcc, 195, 816),
    ] {
        let inputs = [
            ("empty", Vec::new(), Compression::None),
            ("one", vec![0xa5], Compression::None),
            ("block-tail", binary(block_tail), Compression::None),
            ("block-next", binary(block_tail + 1), Compression::None),
            ("minimum-tail", binary(minimum_tail), Compression::None),
            ("minimum-next", binary(minimum_tail + 1), Compression::None),
            ("binary", binary(1007), Compression::None),
            (
                "compressed",
                (0..=255).cycle().take(8192).collect(),
                Compression::Auto,
            ),
            ("auto-small", vec![0; 100], Compression::Auto),
            ("auto-binary", binary(1007), Compression::Auto),
        ];
        for (name, payload, compression) in inputs {
            cases.push(OpticalCase {
                name: format!("{profile}-{name}"),
                payload,
                modulation,
                options: EncodeOptions { compression },
            });
        }
    }
    cases
}

pub fn transfer_cases() -> Vec<(String, usize, usize, Vec<u8>)> {
    let mut cases = Vec::new();
    for chunk in [10_240, 20_480] {
        for group in [20, 60] {
            for (name, length) in [("short", 2 * chunk + 3), ("partial", group * chunk + 3)] {
                cases.push((
                    format!("cwt-{chunk}-{group}-{name}"),
                    chunk,
                    group,
                    binary(length),
                ));
            }
        }
    }
    cases
}
