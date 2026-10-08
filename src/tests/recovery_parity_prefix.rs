use super::*;
use std::collections::BTreeSet;

fn candidates(raw: &[u8], confidence: &[f32], parity: usize, reduced: bool) -> BTreeSet<Vec<u8>> {
    let decoder = Decoder::new(parity);
    let encoder = Encoder::new(parity);
    let mut rank: Vec<_> = (0..raw.len()).collect();
    rank.sort_by(|&a, &b| confidence[a].total_cmp(&confidence[b]).then(a.cmp(&b)));
    let rank: Vec<_> = rank.into_iter().map(|index| index as u8).collect();
    let required = confidence.iter().filter(|&&value| value == 0.0).count();
    let mut output = BTreeSet::new();
    if required > parity {
        return output;
    }
    let mut syndromes = GmdSyndromes::new(raw, parity);
    for &position in &rank[..required] {
        syndromes.erase(position);
    }
    for erased in required..=parity {
        if erased > required {
            syndromes.erase(rank[erased - 1]);
        }
        if reduced && (erased % 2 != parity % 2 || !syndromes.can_correct()) {
            continue;
        }
        let Ok(corrected) = decoder.correct(raw, Some(&rank[..erased])) else {
            continue;
        };
        let word = corrected.to_vec();
        if decoder.is_corrupted(&word) || encoder.encode(corrected.data()).to_vec() != word {
            continue;
        }
        let changed = word.iter().zip(raw).filter(|(a, b)| a != b).count();
        let errors = changed
            - rank[..erased]
                .iter()
                .filter(|&&index| word[usize::from(index)] != raw[usize::from(index)])
                .count();
        if 2 * errors + erased > parity {
            continue;
        }
        let distance: f64 = word
            .iter()
            .zip(raw)
            .zip(confidence)
            .map(|((a, b), &value)| {
                1.0 + if a == b {
                    -f64::from(value)
                } else {
                    f64::from(value)
                }
            })
            .sum();
        if changed <= parity / 2 || distance < (parity + 1) as f64 {
            output.insert(word);
        }
    }
    output
}

#[test]
fn parity_prefixes_preserve_native_candidate_sets() {
    let mut state = 0x1894_7781_3381_6b39u64;
    let mut random = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state as u8
    };
    for parity in [47, 48, 95, 96] {
        let encoder = Encoder::new(parity);
        for scenario in 0..12 {
            let data: Vec<_> = (0..255 - parity).map(|_| random()).collect();
            let word = encoder.encode(&data).to_vec();
            let mut raw = word;
            let mut confidence = vec![1.0; 255];
            let errors = [
                0,
                parity / 2 - 1,
                parity / 2,
                parity / 2 + 1,
                parity - 1,
                parity,
            ][scenario % 6];
            let mut positions: Vec<_> = (0..255).collect();
            for index in (1..255).rev() {
                let target = usize::from(random()) % (index + 1);
                positions.swap(index, target);
            }
            for (error, &position) in positions[..errors].iter().enumerate() {
                raw[position] ^= random().max(1);
                confidence[position] = if error < scenario {
                    0.0
                } else {
                    f32::from(random()) / 255.0
                };
            }
            if scenario >= 6 {
                for &position in &positions[errors..errors + scenario] {
                    confidence[position] = 0.0;
                }
            }
            assert_eq!(
                candidates(&raw, &confidence, parity, false),
                candidates(&raw, &confidence, parity, true),
                "parity={parity}, scenario={scenario}"
            );
        }
    }
}

#[test]
fn parity_prefixes_include_full_capacity_required_erasures() {
    for parity in [47, 48, 95, 96] {
        let word = Encoder::new(parity)
            .encode(&vec![23; 255 - parity])
            .to_vec();
        let mut raw = word.clone();
        let mut confidence = vec![1.0; 255];
        for position in 0..parity {
            raw[position] ^= 73;
            confidence[position] = 0.0;
        }
        let expected = BTreeSet::from([word]);
        assert_eq!(candidates(&raw, &confidence, parity, false), expected);
        assert_eq!(candidates(&raw, &confidence, parity, true), expected);
    }
}

#[test]
fn parity_prefixes_preserve_both_ambiguous_words() {
    for parity in [47, 48, 95, 96] {
        let encoder = Encoder::new(parity);
        let a = encoder.encode(&vec![0; 255 - parity]).to_vec();
        let mut data = vec![0; 255 - parity];
        data[0] = 1;
        let b = encoder.encode(&data).to_vec();
        let differences: Vec<_> = (0..255).filter(|&index| a[index] != b[index]).collect();
        assert_eq!(differences.len(), parity + 1);
        let mut raw = a.clone();
        let mut confidence = vec![1.0; 255];
        for &position in &differences[..parity / 2] {
            raw[position] = b[position];
        }
        for &position in &differences[parity / 2..] {
            confidence[position] = 0.9;
        }
        let expected = BTreeSet::from([a, b]);
        assert_eq!(candidates(&raw, &confidence, parity, false), expected);
        assert_eq!(candidates(&raw, &confidence, parity, true), expected);
        for &position in &differences[parity / 2..parity / 2 + 3] {
            confidence[position] = 0.0;
        }
        assert_eq!(
            candidates(&raw, &confidence, parity, false),
            candidates(&raw, &confidence, parity, true)
        );
    }
}
