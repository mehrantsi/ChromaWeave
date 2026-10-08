//! Pure CWT envelope validation, serialization and file-byte splitting.
//!
//! Construct transfers with an application-provided ID using `with_id`. Random-ID
//! constructors and PNG/HTML generation are available with the `tools` feature.
//! Validation errors can be matched with `error.downcast_ref::<Error>()`.

use anyhow::{Result, ensure};
use sha2::{Digest, Sha256};

pub const REPAIR_HEADER_BYTES: usize = 64;
pub const MAX_FILE_BYTES: usize = 32 * 1024 * 1024;
pub const CHUNK_SIZES: [usize; 2] = [10 * 1024, 20 * 1024];
const REPAIR_MAGIC: &[u8; 4] = b"CWT\0";
const REPAIR_GROUP_SIZES: [usize; 2] = [20, 60];

/// A CWT validation failure, available by downcasting returned errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    InvalidChunkSize,
    InvalidFileSize,
    InvalidRepairGroupSize,
    InvalidRepairParityCount,
    NoncanonicalSourceCount,
    TooManyRepairSymbols,
    SymbolIndexOutOfBounds,
    MissingSourceGroup,
    NoncanonicalGroupGap,
    TruncatedRepairHeader,
    UnrecognizedRepairSymbol,
    NoncanonicalSymbolLength,
    NonzeroSourcePadding,
    InvalidSymbolDataLength,
    GroupIndexOutOfBounds,
    InvalidEncodedParity,
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::InvalidChunkSize => {
                "transfer chunks must contain 10240 or 20480 application bytes"
            }
            Self::InvalidFileSize => "transfer file must contain 1 through 33554432 bytes",
            Self::InvalidRepairGroupSize => "repair group must contain 20 or 60 data symbols",
            Self::InvalidRepairParityCount => "CWT requires exactly four repair symbols per group",
            Self::NoncanonicalSourceCount => "noncanonical source chunk count",
            Self::TooManyRepairSymbols => "CWT symbol count exceeds its global index",
            Self::SymbolIndexOutOfBounds => "CWT symbol index is outside the transfer",
            Self::MissingSourceGroup => "CWT symbol has no source group",
            Self::NoncanonicalGroupGap => "CWT symbol index falls into a noncanonical group gap",
            Self::TruncatedRepairHeader => "truncated CWT header",
            Self::UnrecognizedRepairSymbol => "unrecognized CWT symbol",
            Self::NoncanonicalSymbolLength => "noncanonical CWT symbol length",
            Self::NonzeroSourcePadding => "nonzero CWT source padding",
            Self::InvalidSymbolDataLength => "CWT symbol data must be a full source chunk",
            Self::GroupIndexOutOfBounds => "repair group index is outside the transfer",
            Self::InvalidEncodedParity => "repair encoder returned an invalid parity group",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Debug, PartialEq, Eq)]
/// The canonical CWT header and group position metadata.
pub struct RepairHeader {
    pub transfer_id: [u8; 16],
    pub symbol_index: u16,
    pub source_chunk_count: u16,
    pub chunk_bytes: u16,
    pub original_bytes: u32,
    pub file_sha256: [u8; 32],
    pub group_data: u8,
    pub group_parity: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SymbolPosition {
    pub group_index: usize,
    pub source_index: Option<usize>,
    pub repair_index: Option<usize>,
}

impl RepairHeader {
    pub fn position(&self) -> Result<SymbolPosition> {
        let chunk_bytes = usize::from(self.chunk_bytes);
        let original_bytes = self.original_bytes as usize;
        let source_count = usize::from(self.source_chunk_count);
        let group_data = usize::from(self.group_data);
        ensure!(CHUNK_SIZES.contains(&chunk_bytes), Error::InvalidChunkSize);
        ensure!(
            (1..=MAX_FILE_BYTES).contains(&original_bytes),
            Error::InvalidFileSize
        );
        ensure!(
            REPAIR_GROUP_SIZES.contains(&group_data)
                && group_data <= crate::transfer_repair::MAX_DATA_SHARDS,
            Error::InvalidRepairGroupSize
        );
        ensure!(
            usize::from(self.group_parity) == crate::transfer_repair::PARITY_SHARDS,
            Error::InvalidRepairParityCount
        );
        let expected_sources = original_bytes.div_ceil(chunk_bytes);
        ensure!(
            source_count == expected_sources,
            Error::NoncanonicalSourceCount
        );
        let group_count = source_count.div_ceil(group_data);
        let symbol_count = source_count + group_count * usize::from(self.group_parity);
        ensure!(
            symbol_count <= usize::from(u16::MAX) + 1,
            Error::TooManyRepairSymbols
        );
        let symbol_index = usize::from(self.symbol_index);
        ensure!(symbol_index < symbol_count, Error::SymbolIndexOutOfBounds);

        let group_stride = group_data + usize::from(self.group_parity);
        let group_index = symbol_index / group_stride;
        let local_index = symbol_index % group_stride;
        let group_source_start = group_index * group_data;
        ensure!(group_source_start < source_count, Error::MissingSourceGroup);
        let group_sources = group_data.min(source_count - group_source_start);
        ensure!(
            local_index < group_sources + usize::from(self.group_parity),
            Error::NoncanonicalGroupGap
        );
        if local_index < group_sources {
            Ok(SymbolPosition {
                group_index,
                source_index: Some(group_source_start + local_index),
                repair_index: None,
            })
        } else {
            Ok(SymbolPosition {
                group_index,
                source_index: None,
                repair_index: Some(local_index - group_sources),
            })
        }
    }

    pub fn to_bytes(&self) -> Result<[u8; REPAIR_HEADER_BYTES]> {
        self.position()?;
        let mut bytes = [0; REPAIR_HEADER_BYTES];
        bytes[..4].copy_from_slice(REPAIR_MAGIC);
        bytes[4..20].copy_from_slice(&self.transfer_id);
        bytes[20..22].copy_from_slice(&self.symbol_index.to_le_bytes());
        bytes[22..24].copy_from_slice(&self.source_chunk_count.to_le_bytes());
        bytes[24..26].copy_from_slice(&self.chunk_bytes.to_le_bytes());
        bytes[26..30].copy_from_slice(&self.original_bytes.to_le_bytes());
        bytes[30..62].copy_from_slice(&self.file_sha256);
        bytes[62] = self.group_data;
        bytes[63] = self.group_parity;
        Ok(bytes)
    }
}

#[derive(Debug)]
/// A validated, padded CWT symbol borrowing its shard bytes.
pub struct RepairSymbol<'a> {
    pub header: RepairHeader,
    pub data: &'a [u8],
}

impl<'a> RepairSymbol<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        ensure!(
            bytes.len() >= REPAIR_HEADER_BYTES,
            Error::TruncatedRepairHeader
        );
        ensure!(&bytes[..4] == REPAIR_MAGIC, Error::UnrecognizedRepairSymbol);
        let header = RepairHeader {
            transfer_id: bytes[4..20].try_into()?,
            symbol_index: u16::from_le_bytes(bytes[20..22].try_into()?),
            source_chunk_count: u16::from_le_bytes(bytes[22..24].try_into()?),
            chunk_bytes: u16::from_le_bytes(bytes[24..26].try_into()?),
            original_bytes: u32::from_le_bytes(bytes[26..30].try_into()?),
            file_sha256: bytes[30..62].try_into()?,
            group_data: bytes[62],
            group_parity: bytes[63],
        };
        let position = header.position()?;
        let chunk_bytes = usize::from(header.chunk_bytes);
        ensure!(
            bytes.len() == REPAIR_HEADER_BYTES + chunk_bytes,
            Error::NoncanonicalSymbolLength
        );
        let data = &bytes[REPAIR_HEADER_BYTES..];
        if let Some(source_index) = position.source_index {
            let valid =
                (header.original_bytes as usize - source_index * chunk_bytes).min(chunk_bytes);
            ensure!(
                data[valid..].iter().all(|byte| *byte == 0),
                Error::NonzeroSourcePadding
            );
        }
        Ok(Self { header, data })
    }
}

/// Produces CWT source and parity symbols using a caller-provided ID.
pub struct RepairTransfer<'a> {
    data: &'a [u8],
    transfer_id: [u8; 16],
    file_sha256: [u8; 32],
    chunk_bytes: usize,
    group_data: usize,
    source_chunk_count: usize,
    group_count: usize,
    symbol_count: usize,
}

pub struct RepairSymbolData {
    pub header: RepairHeader,
    pub data: Vec<u8>,
}

impl RepairSymbolData {
    pub fn envelope(&self) -> Result<Vec<u8>> {
        ensure!(
            self.data.len() == usize::from(self.header.chunk_bytes),
            Error::InvalidSymbolDataLength
        );
        let mut bytes = Vec::with_capacity(REPAIR_HEADER_BYTES + self.data.len());
        bytes.extend_from_slice(&self.header.to_bytes()?);
        bytes.extend_from_slice(&self.data);
        Ok(bytes)
    }
}

impl<'a> RepairTransfer<'a> {
    #[cfg(feature = "tools")]
    pub fn new(data: &'a [u8], chunk_bytes: usize, group_data: usize) -> Result<Self> {
        Self::with_id(data, chunk_bytes, group_data, rand::random())
    }

    pub fn with_id(
        data: &'a [u8],
        chunk_bytes: usize,
        group_data: usize,
        transfer_id: [u8; 16],
    ) -> Result<Self> {
        ensure!(
            (1..=MAX_FILE_BYTES).contains(&data.len()),
            Error::InvalidFileSize
        );
        ensure!(CHUNK_SIZES.contains(&chunk_bytes), Error::InvalidChunkSize);
        ensure!(
            REPAIR_GROUP_SIZES.contains(&group_data)
                && group_data <= crate::transfer_repair::MAX_DATA_SHARDS,
            Error::InvalidRepairGroupSize
        );
        let source_chunk_count = data.len().div_ceil(chunk_bytes);
        let group_count = source_chunk_count.div_ceil(group_data);
        let symbol_count = source_chunk_count + group_count * crate::transfer_repair::PARITY_SHARDS;
        ensure!(
            symbol_count <= usize::from(u16::MAX) + 1,
            Error::TooManyRepairSymbols
        );
        Ok(Self {
            data,
            transfer_id,
            file_sha256: Sha256::digest(data).into(),
            chunk_bytes,
            group_data,
            source_chunk_count,
            group_count,
            symbol_count,
        })
    }

    pub fn source_chunk_count(&self) -> usize {
        self.source_chunk_count
    }

    pub fn group_count(&self) -> usize {
        self.group_count
    }

    pub fn symbol_count(&self) -> usize {
        self.symbol_count
    }

    pub fn group_symbols(&self, group_index: usize) -> Result<Vec<RepairSymbolData>> {
        ensure!(group_index < self.group_count, Error::GroupIndexOutOfBounds);
        let source_start = group_index * self.group_data;
        let source_count = self.group_data.min(self.source_chunk_count - source_start);
        let mut data_shards = Vec::with_capacity(source_count);
        for source_index in source_start..source_start + source_count {
            let start = source_index * self.chunk_bytes;
            let end = (start + self.chunk_bytes).min(self.data.len());
            let mut shard = vec![0; self.chunk_bytes];
            shard[..end - start].copy_from_slice(&self.data[start..end]);
            data_shards.push(shard);
        }
        let parity_shards = crate::transfer_repair::encode_parity(&data_shards)?;
        ensure!(
            parity_shards.len() == crate::transfer_repair::PARITY_SHARDS
                && parity_shards
                    .iter()
                    .all(|shard| shard.len() == self.chunk_bytes),
            Error::InvalidEncodedParity
        );

        let group_stride = self.group_data + crate::transfer_repair::PARITY_SHARDS;
        let group_data = self.group_data as u8;
        let mut symbols = Vec::with_capacity(source_count + parity_shards.len());
        for (local_index, data) in data_shards.into_iter().enumerate() {
            let symbol_index = group_index * group_stride + local_index;
            symbols.push(RepairSymbolData {
                header: self.header(symbol_index, group_data)?,
                data,
            });
        }
        for (parity_index, data) in parity_shards.into_iter().enumerate() {
            let local_index = source_count + parity_index;
            let symbol_index = group_index * group_stride + local_index;
            symbols.push(RepairSymbolData {
                header: self.header(symbol_index, group_data)?,
                data,
            });
        }
        Ok(symbols)
    }

    fn header(&self, symbol_index: usize, group_data: u8) -> Result<RepairHeader> {
        let header = RepairHeader {
            transfer_id: self.transfer_id,
            symbol_index: symbol_index as u16,
            source_chunk_count: self.source_chunk_count as u16,
            chunk_bytes: self.chunk_bytes as u16,
            original_bytes: self.data.len() as u32,
            file_sha256: self.file_sha256,
            group_data,
            group_parity: crate::transfer_repair::PARITY_SHARDS as u8,
        };
        header.position()?;
        Ok(header)
    }
}

#[cfg(test)]
#[path = "tests/transfer.rs"]
mod tests;

#[cfg(feature = "tools")]
#[path = "transfer_generator.rs"]
mod generator;
#[cfg(feature = "tools")]
pub use generator::{Frame, Manifest, Options, generate};
