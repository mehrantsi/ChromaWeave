use anyhow::{Context, Result, ensure};

pub const MAX_DECOMPRESSED_BYTES: usize = 16 * 1024 * 1024;

pub(crate) fn compress(payload: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        payload.len() <= MAX_DECOMPRESSED_BYTES,
        "original payload exceeds compression limit of {MAX_DECOMPRESSED_BYTES} bytes"
    );
    let frame = zstd::bulk::compress(payload, 3).context("compressing Zstandard payload")?;
    let mut encoded = Vec::with_capacity(4 + frame.len());
    encoded.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    encoded.extend_from_slice(&frame);
    Ok(encoded)
}

pub(crate) fn decompress(encoded: &[u8]) -> Result<Vec<u8>> {
    ensure!(encoded.len() >= 8, "truncated compressed payload");
    let original_bytes = u32::from_le_bytes(encoded[..4].try_into()?) as usize;
    ensure!(
        original_bytes <= MAX_DECOMPRESSED_BYTES,
        "original payload exceeds compression limit of {MAX_DECOMPRESSED_BYTES} bytes"
    );
    let frame = &encoded[4..];
    ensure!(
        frame[..4] == [0x28, 0xb5, 0x2f, 0xfd],
        "invalid Zstandard frame"
    );
    let frame_bytes = zstd::zstd_safe::find_frame_compressed_size(frame).map_err(|error| {
        anyhow::anyhow!(
            "invalid Zstandard frame: {}",
            zstd::zstd_safe::get_error_name(error)
        )
    })?;
    ensure!(
        frame_bytes == frame.len(),
        "trailing data after Zstandard frame"
    );
    let content_bytes = zstd::zstd_safe::get_frame_content_size(frame)
        .map_err(|_| anyhow::anyhow!("invalid Zstandard frame header"))?;
    ensure!(
        content_bytes == Some(original_bytes as u64),
        "Zstandard frame original length mismatch"
    );
    let mut decoder = zstd::bulk::Decompressor::new()?;
    decoder.set_parameter(zstd::zstd_safe::DParameter::WindowLogMax(24))?;
    let payload = decoder
        .decompress(frame, original_bytes)
        .context("decompressing Zstandard payload")?;
    ensure!(
        payload.len() == original_bytes,
        "decompressed payload length mismatch"
    );
    Ok(payload)
}

#[cfg(test)]
#[path = "tests/compression.rs"]
mod tests;
