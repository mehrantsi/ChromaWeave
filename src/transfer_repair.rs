//! CWT Reed–Solomon erasure coding across equally sized source shards.

use anyhow::{Result, bail, ensure};
use reed_solomon::Encoder;
use std::sync::OnceLock;

pub const PARITY_SHARDS: usize = 4;
pub const MAX_DATA_SHARDS: usize = 60;

const fn multiply_table() -> [[u8; 256]; 256] {
    let mut table = [[0; 256]; 256];
    let mut a = 0;
    while a < 256 {
        let mut b = 1;
        while b < 256 {
            let previous = table[a][b >> 1];
            table[a][b] = (previous << 1)
                ^ if previous & 128 != 0 { 0x1d } else { 0 }
                ^ if b & 1 != 0 { a as u8 } else { 0 };
            b += 1;
        }
        a += 1;
    }
    table
}

static MULTIPLY: [[u8; 256]; 256] = multiply_table();
static GENERATORS: [OnceLock<[[u8; MAX_DATA_SHARDS]; PARITY_SHARDS]>; MAX_DATA_SHARDS] =
    [const { OnceLock::new() }; MAX_DATA_SHARDS];

fn coefficients(data_shards: usize) -> &'static [[u8; MAX_DATA_SHARDS]; PARITY_SHARDS] {
    GENERATORS[data_shards - 1].get_or_init(|| {
        let encoder = Encoder::new(PARITY_SHARDS);
        let mut matrix = [[0; MAX_DATA_SHARDS]; PARITY_SHARDS];
        let mut basis = [0; MAX_DATA_SHARDS];
        for column in 0..data_shards {
            basis[column] = 1;
            let word = encoder.encode(&basis[..data_shards]);
            for (row, &value) in matrix.iter_mut().zip(word.ecc()) {
                row[column] = value;
            }
            basis[column] = 0;
        }
        matrix
    })
}

pub fn encode_parity(data: &[Vec<u8>]) -> Result<Vec<Vec<u8>>> {
    validate_data_shards(data.len())?;
    validate_shard_bytes(data[0].len())?;
    encode_inner(data)
}

pub fn recover(data_shards: usize, shards: &[Option<Vec<u8>>]) -> Result<Vec<(usize, Vec<u8>)>> {
    validate_data_shards(data_shards)?;
    let length = shards.iter().flatten().next().map_or(0, Vec::len);
    validate_shard_bytes(length)?;
    recover_inner(data_shards, shards)
}

fn validate_data_shards(data_shards: usize) -> Result<()> {
    ensure!(
        (1..=MAX_DATA_SHARDS).contains(&data_shards),
        "Invalid data shard count."
    );
    Ok(())
}

fn validate_shard_bytes(length: usize) -> Result<()> {
    ensure!(
        matches!(length, 10_240 | 20_480),
        "Invalid shard byte length."
    );
    Ok(())
}

fn encode_inner(data: &[Vec<u8>]) -> Result<Vec<Vec<u8>>> {
    validate_data_shards(data.len())?;
    let length = data[0].len();
    ensure!(
        length > 0 && data.iter().all(|shard| shard.len() == length),
        "Inconsistent shard byte lengths."
    );
    let inputs: Vec<_> = data.iter().map(Vec::as_slice).collect();
    Ok(coefficients(data.len())
        .iter()
        .map(|row| apply(&row[..data.len()], &inputs))
        .collect())
}

fn recover_inner(data_shards: usize, shards: &[Option<Vec<u8>>]) -> Result<Vec<(usize, Vec<u8>)>> {
    validate_data_shards(data_shards)?;
    ensure!(
        shards.len() == data_shards + PARITY_SHARDS,
        "Invalid total shard count."
    );
    let present = shards.iter().flatten().count();
    ensure!(
        present >= data_shards && shards.len() - present <= PARITY_SHARDS,
        "Insufficient validated shards."
    );
    let length = shards.iter().flatten().next().unwrap().len();
    ensure!(
        length > 0 && shards.iter().flatten().all(|shard| shard.len() == length),
        "Inconsistent shard byte lengths."
    );
    let parity = coefficients(data_shards);
    let missing: Vec<_> = (0..data_shards)
        .filter(|&index| shards[index].is_none())
        .collect();
    let rows: Vec<_> = (0..PARITY_SHARDS)
        .filter(|&row| shards[data_shards + row].is_some())
        .take(missing.len())
        .collect();
    let matrix: Vec<Vec<u8>> = rows
        .iter()
        .map(|&row| missing.iter().map(|&index| parity[row][index]).collect())
        .collect();
    let inverse = invert(&matrix)?;
    let available: Vec<_> = (0..data_shards)
        .filter(|&index| shards[index].is_some())
        .chain(rows.iter().map(|&row| data_shards + row))
        .collect();
    let inputs: Vec<_> = available
        .iter()
        .map(|&index| shards[index].as_deref().unwrap())
        .collect();
    let recovered: Vec<_> = missing
        .iter()
        .zip(&inverse)
        .map(|(&index, inverse_row)| {
            let row: Vec<_> = available
                .iter()
                .map(|&input| {
                    if input >= data_shards {
                        inverse_row[rows
                            .iter()
                            .position(|&row| row == input - data_shards)
                            .unwrap()]
                    } else {
                        inverse_row
                            .iter()
                            .zip(&rows)
                            .fold(0, |sum, (&coefficient, &row)| {
                                sum ^ MULTIPLY[coefficient as usize][parity[row][input] as usize]
                            })
                    }
                })
                .collect();
            (index, apply(&row, &inputs))
        })
        .collect();
    let sources: Vec<_> = (0..data_shards)
        .map(|index| {
            shards[index].as_deref().unwrap_or_else(|| {
                recovered
                    .iter()
                    .find(|(i, _)| *i == index)
                    .unwrap()
                    .1
                    .as_slice()
            })
        })
        .collect();
    let mut expected = vec![0; length];
    for (row, coefficients) in parity.iter().enumerate() {
        if let Some(received) = &shards[data_shards + row] {
            apply_into(&coefficients[..data_shards], &sources, &mut expected);
            ensure!(expected == *received, "Inconsistent validated parity.");
        }
    }
    Ok(recovered)
}

fn inverse(value: u8) -> Result<u8> {
    ensure!(value != 0, "Singular erasure matrix.");
    let mut output = 1;
    let mut power = value;
    let mut exponent = 254;
    while exponent != 0 {
        if exponent & 1 != 0 {
            output = MULTIPLY[output as usize][power as usize];
        }
        power = MULTIPLY[power as usize][power as usize];
        exponent >>= 1;
    }
    Ok(output)
}

fn invert(matrix: &[Vec<u8>]) -> Result<Vec<Vec<u8>>> {
    let side = matrix.len();
    ensure!(side <= PARITY_SHARDS, "Invalid erasure matrix size.");
    let mut rows = vec![vec![0; side * 2]; side];
    for (index, row) in rows.iter_mut().enumerate() {
        ensure!(matrix[index].len() == side, "Nonsquare erasure matrix.");
        row[..side].copy_from_slice(&matrix[index]);
        row[side + index] = 1;
    }
    for column in 0..side {
        let Some(pivot) = (column..side).find(|&row| rows[row][column] != 0) else {
            bail!("Singular erasure matrix.");
        };
        rows.swap(column, pivot);
        let multiplier = &MULTIPLY[inverse(rows[column][column])? as usize];
        for entry in &mut rows[column] {
            *entry = multiplier[*entry as usize];
        }
        let pivot_row = rows[column].clone();
        for (row, entries) in rows.iter_mut().enumerate() {
            if row == column {
                continue;
            }
            let multiplier = &MULTIPLY[entries[column] as usize];
            for (entry, &pivot) in entries.iter_mut().zip(&pivot_row) {
                *entry ^= multiplier[pivot as usize];
            }
        }
    }
    Ok(rows.into_iter().map(|row| row[side..].to_vec()).collect())
}

fn apply(coefficients: &[u8], inputs: &[&[u8]]) -> Vec<u8> {
    let mut result = vec![0; inputs[0].len()];
    apply_into(coefficients, inputs, &mut result);
    result
}

fn apply_into(coefficients: &[u8], inputs: &[&[u8]], result: &mut [u8]) {
    result.fill(0);
    for (&coefficient, input) in coefficients.iter().zip(inputs) {
        if coefficient == 0 {
            continue;
        }
        let row = &MULTIPLY[coefficient as usize];
        for (output, &byte) in result.iter_mut().zip(*input) {
            *output ^= row[byte as usize];
        }
    }
}

#[cfg(test)]
#[path = "tests/transfer_repair.rs"]
mod tests;
