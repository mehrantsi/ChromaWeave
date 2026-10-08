use super::*;
use reed_solomon::Decoder;

struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn sources(&mut self, count: usize, length: usize) -> Vec<Vec<u8>> {
        (0..count)
            .map(|_| (0..length).map(|_| self.next() as u8).collect())
            .collect()
    }
}

fn native_parity(sources: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let encoder = Encoder::new(PARITY_SHARDS);
    let mut output = vec![vec![0; sources[0].len()]; PARITY_SHARDS];
    let mut column = vec![0; sources.len()];
    for index in 0..sources[0].len() {
        for (byte, source) in column.iter_mut().zip(sources) {
            *byte = source[index];
        }
        let encoded = encoder.encode(&column);
        for (shard, &value) in output.iter_mut().zip(encoded.ecc()) {
            shard[index] = value;
        }
    }
    output
}

fn native_recover(data_shards: usize, shards: &[Option<Vec<u8>>]) -> Vec<(usize, Vec<u8>)> {
    let length = shards.iter().flatten().next().unwrap().len();
    let erasures: Vec<_> = shards
        .iter()
        .enumerate()
        .filter_map(|(i, shard)| shard.is_none().then_some(i as u8))
        .collect();
    let decoder = Decoder::new(PARITY_SHARDS);
    let mut word = vec![0; shards.len()];
    let mut result: Vec<_> = (0..data_shards)
        .filter(|&i| shards[i].is_none())
        .map(|i| (i, vec![0; length]))
        .collect();
    for column in 0..length {
        for (byte, shard) in word.iter_mut().zip(shards) {
            *byte = shard.as_ref().map_or(0, |shard| shard[column]);
        }
        let output = decoder.correct(&word, Some(&erasures)).unwrap();
        for (index, bytes) in &mut result {
            bytes[column] = output.data()[*index];
        }
    }
    result
}

#[test]
fn all_gf_products_match_independent_log_exp_and_have_inverses() {
    let mut exp = [0u8; 255];
    let mut log = [0usize; 256];
    let mut value = 1u16;
    for (i, entry) in exp.iter_mut().enumerate() {
        *entry = value as u8;
        log[value as usize] = i;
        value <<= 1;
        if value & 256 != 0 {
            value ^= 0x11d;
        }
    }
    for a in 0..256 {
        for b in 0..256 {
            assert_eq!(
                MULTIPLY[a][b],
                if a == 0 || b == 0 {
                    0
                } else {
                    exp[(log[a] + log[b]) % 255]
                }
            );
        }
    }
    for a in 1..=255 {
        assert_eq!(MULTIPLY[a][inverse(a as u8).unwrap() as usize], 1);
    }
}

#[test]
fn every_group_size_matches_native_systematic_encoder() {
    let mut random = Random(0x43575400);
    for data_shards in 1..=MAX_DATA_SHARDS {
        let source = random.sources(data_shards, 17);
        assert_eq!(encode_inner(&source).unwrap(), native_parity(&source));
    }
}

#[test]
fn random_mixed_erasures_match_native_column_decoder_without_mutating_known_shards() {
    let mut random = Random(0x43575400);
    for data_shards in [1, 2, 20, 60] {
        for length in [1, 17, 257] {
            let source = random.sources(data_shards, length);
            let parity = native_parity(&source);
            let full: Vec<_> = source.iter().chain(&parity).cloned().map(Some).collect();
            for erased in 0..=PARITY_SHARDS {
                for _ in 0..10 {
                    let mut shards = full.clone();
                    for _ in 0..erased {
                        loop {
                            let index = random.next() as usize % shards.len();
                            if shards[index].take().is_some() {
                                break;
                            }
                        }
                    }
                    let before = shards.clone();
                    let repaired = recover_inner(data_shards, &shards).unwrap();
                    assert_eq!(repaired, native_recover(data_shards, &shards));
                    assert_eq!(shards, before);
                    for (index, bytes) in &repaired {
                        assert_eq!(bytes, &source[*index]);
                        assert!(before[*index].is_none());
                        shards[*index] = Some(bytes.clone());
                    }
                    let restored: Vec<_> = shards[..data_shards]
                        .iter()
                        .map(|shard| shard.clone().unwrap())
                        .collect();
                    assert_eq!(native_parity(&restored), parity);
                }
            }
        }
    }
}

#[test]
fn production_shard_sizes_and_all_parity_loss_are_supported() {
    let mut random = Random(71);
    for length in [10_240, 20_480] {
        for data_shards in [1, 60] {
            let source = random.sources(data_shards, length);
            let parity = encode_parity(&source).unwrap();
            let mut shards: Vec<_> = source.iter().chain(&parity).cloned().map(Some).collect();
            for shard in shards.iter_mut().skip(data_shards) {
                *shard = None;
            }
            assert!(recover(data_shards, &shards).unwrap().is_empty());
            for (target, erased) in (0..data_shards.min(4)).enumerate() {
                shards[erased] = None;
                shards[data_shards + target] = Some(parity[target].clone());
            }
            for (index, bytes) in recover(data_shards, &shards).unwrap() {
                assert_eq!(bytes, source[index]);
            }
        }
    }
}

#[test]
fn malformed_shapes_insufficient_shards_and_conflicting_parity_reject() {
    assert!(encode_parity(&[]).is_err());
    assert!(encode_parity(&vec![vec![0; 10_240]; 61]).is_err());
    assert!(encode_parity(&[vec![0; 10_240], vec![0; 20_480]]).is_err());
    for length in [0, 1, 10_239, 20_481] {
        assert!(encode_parity(&[vec![0; length]]).is_err());
        assert!(recover(1, &vec![Some(vec![0; length]); 5]).is_err());
    }
    let source = Random(71).sources(20, 17);
    let parity = native_parity(&source);
    let full: Vec<_> = source.iter().chain(&parity).cloned().map(Some).collect();
    assert!(recover_inner(0, &full).is_err());
    assert!(recover_inner(61, &full).is_err());
    assert!(recover_inner(20, &full[..23]).is_err());
    assert!(recover_inner(20, &vec![None; 24]).is_err());
    assert!(invert(&[vec![1, 1], vec![1, 1]]).is_err());
    let mut shards = full.clone();
    shards[0] = Some(vec![0; 16]);
    assert!(recover_inner(20, &shards).is_err());
    let mut shards = full.clone();
    for shard in &mut shards[..5] {
        *shard = None;
    }
    assert!(recover_inner(20, &shards).is_err());
    for corrupt in [0, 20, 23] {
        let mut shards = full.clone();
        shards[corrupt].as_mut().unwrap()[0] ^= 1;
        assert!(recover_inner(20, &shards).is_err());
        shards[1] = None;
        let before = shards.clone();
        assert!(recover_inner(20, &shards).is_err());
        assert_eq!(shards, before);
    }
}

#[test]
fn erasure_equations_do_not_authenticate_received_shards() {
    let source = Random(71).sources(20, 17);
    let parity = native_parity(&source);
    let mut shards: Vec<_> = source.iter().chain(&parity).cloned().map(Some).collect();
    for shard in &mut shards[1..5] {
        *shard = None;
    }
    shards[0].as_mut().unwrap()[0] ^= 1;
    let before = shards.clone();
    let repaired = recover_inner(20, &shards).unwrap();
    assert!(repaired.iter().any(|(i, bytes)| bytes != &source[*i]));
    assert_eq!(shards, before);
}
