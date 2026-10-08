use chromaweave::{
    codec::{Compression, EncodeOptions, Modulation},
    scanner,
};
use image::{DynamicImage, ImageFormat, RgbImage};
use serde_json::{Value, json};
use std::{
    ffi::{CString, c_char, c_void},
    io::Cursor,
    panic::{AssertUnwindSafe, catch_unwind},
    time::Instant,
};

mod pixels;

fn response(operation: impl FnOnce() -> Value) -> *mut c_char {
    let value = catch_unwind(AssertUnwindSafe(operation)).unwrap_or_else(
        |_| json!({"status": "error", "message": "The codec could not process this image."}),
    );
    CString::new(value.to_string()).unwrap().into_raw()
}

unsafe fn bgra_image(
    pixels: *const u8,
    length: usize,
    width: u32,
    height: u32,
    stride: usize,
) -> Result<RgbImage, &'static str> {
    let row_bytes = width as usize * 4;
    if pixels.is_null()
        || length > isize::MAX as usize
        || width == 0
        || height == 0
        || u64::from(width) * u64::from(height) > 40_000_000
        || stride < row_bytes
        || stride
            .checked_mul(height as usize)
            .is_none_or(|needed| needed > length)
    {
        return Err("Invalid camera image buffer.");
    }
    let input = unsafe { std::slice::from_raw_parts(pixels, length) };
    let rgb = pixels::bgra_to_rgb(input, width, height, stride);
    Ok(RgbImage::from_raw(width, height, rgb).unwrap())
}

#[unsafe(no_mangle)]
/// # Safety
/// `indices` and `packed` must reference `shard_count` and `length` readable bytes respectively.
pub unsafe extern "C" fn cw_transfer_repair(
    data_shards: u8,
    shard_bytes: u32,
    indices: *const u8,
    shard_count: usize,
    packed: *const u8,
    length: usize,
) -> *mut c_char {
    response(|| {
        let data_shards = usize::from(data_shards);
        let shard_bytes = shard_bytes as usize;
        if !(1..=chromaweave::transfer_repair::MAX_DATA_SHARDS).contains(&data_shards)
            || !matches!(shard_bytes, 10_240 | 20_480)
            || !(data_shards..=data_shards + chromaweave::transfer_repair::PARITY_SHARDS)
                .contains(&shard_count)
            || shard_count.checked_mul(shard_bytes) != Some(length)
            || indices.is_null()
            || packed.is_null()
        {
            return json!({"status": "error", "message": "Invalid transfer repair buffers."});
        }
        let total = data_shards + chromaweave::transfer_repair::PARITY_SHARDS;
        let mut seen = [false; 64];
        let mut order = [0; 64];
        for (position, index) in order.iter_mut().take(shard_count).enumerate() {
            let value = usize::from(unsafe { *indices.add(position) });
            if value >= total || seen[value] {
                return json!({"status": "error", "message": "Invalid or duplicate transfer shard index."});
            }
            seen[value] = true;
            *index = value;
        }
        let input = unsafe { std::slice::from_raw_parts(packed, length) };
        let mut shards = vec![None; total];
        for (&index, bytes) in order
            .iter()
            .take(shard_count)
            .zip(input.chunks_exact(shard_bytes))
        {
            shards[index] = Some(bytes.to_vec());
        }
        match chromaweave::transfer_repair::recover(data_shards, &shards) {
            Ok(recovered) => {
                json!({"ok": true, "recovered": recovered.into_iter().map(|(index, bytes)| {
                json!({"index": index, "data_hex": transfer_hex(&bytes)})
            }).collect::<Vec<_>>()})
            }
            Err(error) => json!({"status": "error", "message": error.to_string()}),
        }
    })
}

fn transfer_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        result.push(HEX[usize::from(byte >> 4)] as char);
        result.push(HEX[usize::from(byte & 15)] as char);
    }
    result
}

#[unsafe(no_mangle)]
pub extern "C" fn cw_recovery_create() -> *mut c_void {
    catch_unwind(|| Box::into_raw(Box::new(chromaweave::recovery::PhotoRecovery::new())).cast())
        .unwrap_or(std::ptr::null_mut())
}

#[unsafe(no_mangle)]
/// # Safety
/// `session` must be null or an unfreed handle from `cw_recovery_create`, with no calls in flight.
pub unsafe extern "C" fn cw_recovery_free(session: *mut c_void) {
    if !session.is_null() {
        unsafe {
            drop(Box::from_raw(
                session.cast::<chromaweave::recovery::PhotoRecovery>(),
            ));
        }
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// `session` must be a live handle used serially. `pixels` must reference `length` readable bytes.
pub unsafe extern "C" fn cw_recovery_decode_bgra(
    session: *mut c_void,
    pixels: *const u8,
    length: usize,
    width: u32,
    height: u32,
    stride: usize,
) -> *mut c_char {
    response(|| {
        let start = Instant::now();
        if session.is_null() {
            return json!({"status": "error", "message": "No photo recovery session."});
        }
        let session = unsafe { &mut *session.cast::<chromaweave::recovery::PhotoRecovery>() };
        let image = match unsafe { bgra_image(pixels, length, width, height, stride) } {
            Ok(image) => image,
            Err(message) => {
                return json!({"status": "error", "message": message, "frames": session.frame_count(), "max_frames": 4});
            }
        };
        match session.push(&image) {
            Ok(progress) => {
                let mut result = json!({"status": "pending", "frames": progress.frames, "max_frames": 4,
                    "code_detected": true,
                    "certified_blocks": progress.certified_blocks, "total_blocks": progress.total_blocks,
                    "elapsed_ms": start.elapsed().as_secs_f64() * 1000.0});
                if let Some(decoded) = progress.decoded {
                    result["status"] = json!("decoded");
                    result["payload"] = json!(decoded.payload);
                    result["modulation"] = json!(decoded.layout.modulation);
                    result["corrected_bytes"] = json!(decoded.stats.corrected_bytes);
                    result["erased_bytes"] = json!(decoded.stats.erased_bytes);
                    result["confidence"] = json!(decoded.stats.mean_symbol_confidence);
                    result["rs_data_bytes"] = json!(decoded.layout.rs_data_bytes);
                    result["rs_parity_bytes"] = json!(decoded.layout.rs_parity_bytes);
                }
                result
            }
            Err(error) => {
                json!({"status": "error", "message": format!("{error:#}"), "frames": session.frame_count(),
                "code_detected": session.code_detected(),
                "max_frames": 4, "elapsed_ms": start.elapsed().as_secs_f64() * 1000.0})
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn cw_transfer_scanner_create() -> *mut c_void {
    catch_unwind(|| Box::into_raw(Box::new(scanner::TransferScanner::new())).cast())
        .unwrap_or(std::ptr::null_mut())
}

#[unsafe(no_mangle)]
/// # Safety
/// `session` must be null or a live handle used serially and returned by `cw_transfer_scanner_create`.
pub unsafe extern "C" fn cw_transfer_scanner_reset(session: *mut c_void) {
    if !session.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
            (&mut *session.cast::<scanner::TransferScanner>()).reset();
        }));
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// `session` must be null or an unfreed handle returned by `cw_transfer_scanner_create`.
pub unsafe extern "C" fn cw_transfer_scanner_free(session: *mut c_void) {
    if !session.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
            drop(Box::from_raw(session.cast::<scanner::TransferScanner>()));
        }));
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// `session` must be a live handle used serially. `pixels` must reference `length` readable bytes.
pub unsafe extern "C" fn cw_transfer_scanner_decode_bgra(
    session: *mut c_void,
    pixels: *const u8,
    length: usize,
    width: u32,
    height: u32,
    stride: usize,
) -> *mut c_char {
    response(|| {
        let start = Instant::now();
        if session.is_null() {
            return json!({
                "status": "error",
                "message": "No transfer scanner session.",
                "code_detected": false,
            });
        }
        let session = unsafe { &mut *session.cast::<scanner::TransferScanner>() };
        let image = match unsafe { bgra_image(pixels, length, width, height, stride) } {
            Ok(image) => image,
            Err(message) => {
                return json!({
                    "status": "error",
                    "message": message,
                    "code_detected": false,
                    "elapsed_ms": start.elapsed().as_secs_f64() * 1000.0,
                });
            }
        };
        scan_attempt_response(session.scan(&image), start.elapsed().as_secs_f64() * 1000.0)
    })
}

fn scan_attempt_response(attempt: scanner::ScanAttempt, elapsed_ms: f64) -> Value {
    let code_detected = attempt.code_detected;
    match attempt.result {
        Ok(decoded) => json!({"status": "decoded", "payload": decoded.payload,
            "modulation": decoded.layout.modulation, "corrected_bytes": decoded.stats.corrected_bytes,
            "rs_data_bytes": decoded.layout.rs_data_bytes, "rs_parity_bytes": decoded.layout.rs_parity_bytes,
            "erased_bytes": decoded.stats.erased_bytes, "confidence": decoded.stats.mean_symbol_confidence,
            "code_detected": code_detected, "elapsed_ms": elapsed_ms}),
        Err(error) => json!({"status": "not_found", "message": format!("{error:#}"),
            "code_detected": code_detected, "elapsed_ms": elapsed_ms}),
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// `value` must be null or an unfreed pointer returned by this bridge.
pub unsafe extern "C" fn cw_string_free(value: *mut c_char) {
    if !value.is_null() {
        unsafe {
            drop(CString::from_raw(value));
        }
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// A non-null `pixels` pointer must reference `length` readable bytes for this call.
pub unsafe extern "C" fn cw_decode_bgra(
    pixels: *const u8,
    length: usize,
    width: u32,
    height: u32,
    stride: usize,
) -> *mut c_char {
    response(|| {
        let start = Instant::now();
        let image = match unsafe { bgra_image(pixels, length, width, height, stride) } {
            Ok(image) => image,
            Err(message) => return json!({"status": "error", "message": message}),
        };
        let attempt = scanner::scan(&image);
        scan_attempt_response(attempt, start.elapsed().as_secs_f64() * 1000.0)
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// A non-null `payload` pointer must reference `length` readable bytes for this call.
pub unsafe extern "C" fn cw_encode_png(
    payload: *const u8,
    length: usize,
    profile: u32,
    cell_pixels: u32,
) -> *mut c_char {
    response(|| encode_png(payload, length, profile, cell_pixels, 0))
}

#[unsafe(no_mangle)]
/// # Safety
/// A non-null `payload` pointer must reference `length` readable bytes for this call.
pub unsafe extern "C" fn cw_encode_png_with_options(
    payload: *const u8,
    length: usize,
    profile: u32,
    cell_pixels: u32,
    compression: u32,
) -> *mut c_char {
    response(|| encode_png(payload, length, profile, cell_pixels, compression))
}

fn encode_png(
    payload: *const u8,
    length: usize,
    profile: u32,
    cell_pixels: u32,
    compression: u32,
) -> Value {
    let modulation = match profile {
        3 => Modulation::Luma10,
        4 => Modulation::Luma10LowEcc,
        _ => return json!({"status": "error", "message": "Unknown CW profile."}),
    };
    let Ok(capacity) = scanner::maximum_payload_bytes_with_modulation(cell_pixels, modulation)
    else {
        return json!({"status": "error", "message": "Invalid cell pitch."});
    };
    let compression = match compression {
        0 => Compression::None,
        1 => Compression::Auto,
        _ => return json!({"status": "error", "message": "Unknown compression option."}),
    };
    match compression {
        Compression::None if length > capacity || (length > 0 && payload.is_null()) => {
            return json!({"status": "error", "message": "Payload exceeds this profile's capacity."});
        }
        Compression::Auto if length > chromaweave::compression::MAX_DECOMPRESSED_BYTES => {
            return json!({"status": "error", "message": "Payload exceeds the compression input limit."});
        }
        Compression::Auto if length > 0 && payload.is_null() => {
            return json!({"status": "error", "message": "Invalid payload buffer."});
        }
        _ => {}
    }
    let input = if length == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(payload, length) }
    };
    match scanner::encode_with_options(
        input,
        cell_pixels,
        modulation,
        EncodeOptions { compression },
    ) {
        Ok(encoded) => {
            let side = encoded.image.width();
            let mut png = Cursor::new(Vec::new());
            if let Err(error) =
                DynamicImage::ImageRgb8(encoded.image).write_to(&mut png, ImageFormat::Png)
            {
                return json!({"status": "error", "message": error.to_string()});
            }
            json!({"status": "encoded", "png": png.into_inner(), "side_pixels": side,
                "rs_data_bytes": encoded.layout.rs_data_bytes, "rs_parity_bytes": encoded.layout.rs_parity_bytes,
                "payload_bytes": encoded.layout.payload_bytes,
                "encoded_payload_bytes": encoded.layout.encoded_payload_bytes,
                "compression": encoded.layout.compression, "modulation": modulation})
        }
        Err(error) => json!({"status": "error", "message": error.to_string()}),
    }
}

#[cfg(test)]
#[path = "tests/lib.rs"]
mod tests;
