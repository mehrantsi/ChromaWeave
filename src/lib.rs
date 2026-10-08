//! CW Luma10 encoding, automatic image scanning, photo recovery and transfer packets.
//!
//! The root API encodes framed symbols and automatically detects their geometry when
//! decoding. [`codec`] provides aligned-grid operations for known cell sizes;
//! [`transfer`] and [`transfer_repair`] implement CWT packet and erasure coding.
//! Images are in-memory [`image::RgbImage`] buffers. File formats, synthetic captures,
//! QR comparisons and the command-line executable require the `tools` feature.
//!
//! ```
//! let encoded = chromaweave::encode(b"hello", 4)?;
//! let decoded = chromaweave::decode(&encoded.image)?;
//! assert_eq!(decoded.payload, b"hello");
//! # Ok::<(), anyhow::Error>(())
//! ```
//!
//! Encode/decode errors use `anyhow::Error`; transfer validation exposes typed
//! [`transfer::Error`] values through the error chain.

#[cfg(any(test, feature = "tools"))]
pub mod capture;
#[cfg(any(test, feature = "tools"))]
pub mod channel;
pub mod codec;
#[cfg(feature = "tools")]
pub mod comparison;
pub mod compression;
#[cfg(feature = "tools")]
pub mod experiment;
mod finder_refine;
mod geometry;
mod grid_refine;
mod integrated_layout;
mod luma;
mod luma_channel;
mod luma_levels;
pub mod recovery;
mod recovery_fit;
mod recovery_signal;
pub mod scanner;
mod spatial_equalizer;
pub mod transfer;
pub mod transfer_repair;

/// Framed encoding, automatic scanning and session recovery.
pub use codec::{Compression, DecodeStats, Decoded, EncodeOptions, Encoded, Layout, Modulation};
pub use recovery::{PhotoRecovery, RecoveryProgress};
pub use scanner::{
    ScanAttempt, TransferScanner, decode, encode, encode_with_modulation, encode_with_options,
    maximum_payload_bytes, maximum_payload_bytes_with_modulation, scan,
};
