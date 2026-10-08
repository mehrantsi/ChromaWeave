use super::*;
use std::{
    io::Write,
    process::{Command, Stdio},
};

fn camera_bgra(image: &RgbImage) -> (Vec<u8>, usize) {
    let stride = image.width() as usize * 4 + 12;
    let mut pixels = vec![0; stride * image.height() as usize];
    for (x, y, pixel) in image.enumerate_pixels() {
        let index = y as usize * stride + x as usize * 4;
        pixels[index..index + 4].copy_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
    }
    (pixels, stride)
}

unsafe fn take(value: *mut c_char) -> Value {
    let result =
        serde_json::from_str(unsafe { std::ffi::CStr::from_ptr(value) }.to_str().unwrap()).unwrap();
    unsafe {
        cw_string_free(value);
    }
    result
}

#[test]
fn transfer_repair_returns_only_missing_sources_from_unordered_inputs() {
    for length in [10_240, 20_480] {
        let data: Vec<Vec<u8>> = (0..3)
            .map(|index| {
                (0..length)
                    .map(|byte| (byte * 73 + index * 41) as u8)
                    .collect()
            })
            .collect();
        let parity = chromaweave::transfer_repair::encode_parity(&data).unwrap();
        let indices = [4, 1, 3];
        let packed: Vec<_> = parity[1]
            .iter()
            .chain(&data[1])
            .chain(&parity[0])
            .copied()
            .collect();
        let result = unsafe {
            take(cw_transfer_repair(
                3,
                length as u32,
                indices.as_ptr(),
                indices.len(),
                packed.as_ptr(),
                packed.len(),
            ))
        };
        assert_eq!(result["ok"], true);
        assert_eq!(
            result["recovered"],
            json!([
                {"index": 0, "data_hex": transfer_hex(&data[0])},
                {"index": 2, "data_hex": transfer_hex(&data[2])}
            ])
        );
        let all: Vec<_> = data.iter().chain(&parity).flatten().copied().collect();
        let indices: Vec<_> = (0..7).collect();
        let result = unsafe {
            take(cw_transfer_repair(
                3,
                length as u32,
                indices.as_ptr(),
                indices.len(),
                all.as_ptr(),
                all.len(),
            ))
        };
        assert_eq!(result, json!({"ok": true, "recovered": []}));
    }
}

#[test]
fn transfer_repair_rejects_invalid_bounds_indices_nulls_and_parity() {
    let indices = [0, 1, 2, 3, 4];
    let packed = vec![0; 5 * 10_240];
    for (data_shards, shard_bytes, count, length) in [
        (0, 10_240, 1, 10_240),
        (61, 10_240, 1, 10_240),
        (1, 0, 1, 0),
        (1, 1, 1, 1),
        (1, u32::MAX, 1, u32::MAX as usize),
        (1, 10_240, 0, 0),
        (2, 10_240, 1, 10_240),
        (1, 10_240, 6, 6 * 10_240),
        (1, 10_240, usize::MAX, usize::MAX),
        (1, 10_240, 1, 10_239),
        (1, 10_240, 1, 10_241),
    ] {
        let result = unsafe {
            take(cw_transfer_repair(
                data_shards,
                shard_bytes,
                indices.as_ptr(),
                count,
                packed.as_ptr(),
                length,
            ))
        };
        assert_eq!(result["status"], "error");
    }
    for (input_indices, input_packed) in [
        (std::ptr::null(), packed.as_ptr()),
        (indices.as_ptr(), std::ptr::null()),
    ] {
        let result = unsafe {
            take(cw_transfer_repair(
                1,
                10_240,
                input_indices,
                1,
                input_packed,
                10_240,
            ))
        };
        assert_eq!(result["status"], "error");
    }
    for indices in [[0, 0], [0, 5], [255, 0]] {
        let result = unsafe {
            take(cw_transfer_repair(
                1,
                10_240,
                indices.as_ptr(),
                indices.len(),
                packed.as_ptr(),
                2 * 10_240,
            ))
        };
        assert_eq!(result["status"], "error");
    }
    let data = vec![vec![71; 10_240]; 1];
    let parity = chromaweave::transfer_repair::encode_parity(&data).unwrap();
    let mut full: Vec<_> = data.iter().chain(&parity).flatten().copied().collect();
    full[4 * 10_240] ^= 1;
    let result = unsafe {
        take(cw_transfer_repair(
            1,
            10_240,
            indices.as_ptr(),
            indices.len(),
            full.as_ptr(),
            full.len(),
        ))
    };
    assert_eq!(result["status"], "error");
    assert_eq!(result["message"], "Inconsistent validated parity.");
    let data = vec![vec![31; 20_480]; 60];
    let parity = chromaweave::transfer_repair::encode_parity(&data).unwrap();
    let full: Vec<_> = data.iter().chain(&parity).flatten().copied().collect();
    let indices: Vec<_> = (0..64).collect();
    let result = unsafe {
        take(cw_transfer_repair(
            60,
            20_480,
            indices.as_ptr(),
            indices.len(),
            full.as_ptr(),
            full.len(),
        ))
    };
    assert_eq!(result, json!({"ok": true, "recovered": []}));
}

#[test]
fn transfer_scanner_session_reuses_geometry_without_reusing_packet_state() {
    let first_payload: Vec<_> = (0..1024).map(|index| (index * 73 + 11) as u8).collect();
    let second_payload: Vec<_> = (0..1024).map(|index| (index * 73 + 89) as u8).collect();
    let first_image = scanner::encode_with_modulation(&first_payload, 4, Modulation::Luma10LowEcc)
        .unwrap()
        .image;
    let second_image =
        scanner::encode_with_modulation(&second_payload, 4, Modulation::Luma10LowEcc)
            .unwrap()
            .image;
    assert_eq!(first_image.dimensions(), second_image.dimensions());
    let (first_pixels, first_stride) = camera_bgra(&first_image);
    let (second_pixels, second_stride) = camera_bgra(&second_image);
    let session = cw_transfer_scanner_create();
    assert!(!session.is_null());

    unsafe {
        cw_transfer_scanner_reset(std::ptr::null_mut());
        cw_transfer_scanner_free(std::ptr::null_mut());
    }
    let no_session = unsafe {
        take(cw_transfer_scanner_decode_bgra(
            std::ptr::null_mut(),
            first_pixels.as_ptr(),
            first_pixels.len(),
            first_image.width(),
            first_image.height(),
            first_stride,
        ))
    };
    assert_eq!(no_session["status"], "error");
    assert_eq!(no_session["code_detected"], false);

    let invalid_buffer = unsafe {
        take(cw_transfer_scanner_decode_bgra(
            session,
            std::ptr::null(),
            0,
            1,
            1,
            4,
        ))
    };
    assert_eq!(invalid_buffer["status"], "error");
    assert_eq!(invalid_buffer["code_detected"], false);

    for (pixels, image, stride, expected) in [
        (&first_pixels, &first_image, first_stride, &first_payload),
        (
            &second_pixels,
            &second_image,
            second_stride,
            &second_payload,
        ),
    ] {
        let decoded = unsafe {
            take(cw_transfer_scanner_decode_bgra(
                session,
                pixels.as_ptr(),
                pixels.len(),
                image.width(),
                image.height(),
                stride,
            ))
        };
        assert_eq!(decoded["status"], "decoded");
        assert_eq!(decoded["code_detected"], true);
        assert_eq!(decoded["payload"], json!(expected));
    }

    unsafe {
        cw_transfer_scanner_reset(session);
    }
    let after_reset = unsafe {
        take(cw_transfer_scanner_decode_bgra(
            session,
            first_pixels.as_ptr(),
            first_pixels.len(),
            first_image.width(),
            first_image.height(),
            first_stride,
        ))
    };
    assert_eq!(after_reset["status"], "decoded");
    assert_eq!(after_reset["payload"], json!(first_payload));

    let blank = RgbImage::from_pixel(160, 160, image::Rgb([127, 127, 127]));
    let (blank_pixels, blank_stride) = camera_bgra(&blank);
    let not_found = unsafe {
        take(cw_transfer_scanner_decode_bgra(
            session,
            blank_pixels.as_ptr(),
            blank_pixels.len(),
            blank.width(),
            blank.height(),
            blank_stride,
        ))
    };
    assert_eq!(not_found["status"], "not_found");
    assert_eq!(not_found["code_detected"], false);
    unsafe {
        cw_transfer_scanner_free(session);
    }
}

fn packet_sha256(packet: &[u8]) -> String {
    let mut child = Command::new("shasum")
        .args(["-a", "256"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("shasum is required for the ignored external fixture test");
    child.stdin.take().unwrap().write_all(packet).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned()
}

fn assert_external_packet(result: &Value) {
    assert_eq!(result["status"], "decoded");
    assert_eq!(result["code_detected"], true);
    let packet: Vec<u8> = serde_json::from_value(result["payload"].clone()).unwrap();
    assert_eq!(packet.len(), 10_302);
    assert_eq!(
        packet_sha256(&packet),
        "1e9b68922d3806a0530cc17201561c2ac43b1f40834a6f392574af47677b44b6"
    );
}

#[test]
#[ignore = "set CW_TRANSFER_SCANNER_FIXTURE_DIR to the Build20 live-success fixture directory"]
fn transfer_scanner_decodes_build20_live_success_packet_83_after_stateless_failure() {
    let directory = std::env::var_os("CW_TRANSFER_SCANNER_FIXTURE_DIR")
        .map(std::path::PathBuf::from)
        .expect("CW_TRANSFER_SCANNER_FIXTURE_DIR is required");
    let diagnostics: Value =
        serde_json::from_slice(&std::fs::read(directory.join("diagnostics.json")).unwrap())
            .unwrap();
    let frame = &diagnostics["frames"][0];
    let width = frame["width"].as_u64().unwrap() as u32;
    let height = frame["height"].as_u64().unwrap() as u32;
    let stride = frame["stride"].as_u64().unwrap() as usize;
    let pixels = std::fs::read(directory.join("frame-1.bgra")).unwrap();
    assert_eq!(
        pixels.len(),
        frame["byte_length"].as_u64().unwrap() as usize
    );
    assert_eq!(pixels.len(), stride * height as usize);

    let stateless = unsafe {
        take(cw_decode_bgra(
            pixels.as_ptr(),
            pixels.len(),
            width,
            height,
            stride,
        ))
    };
    assert_eq!(stateless["status"], "not_found");

    let session = cw_transfer_scanner_create();
    assert!(!session.is_null());
    for _ in 0..2 {
        let decoded = unsafe {
            take(cw_transfer_scanner_decode_bgra(
                session,
                pixels.as_ptr(),
                pixels.len(),
                width,
                height,
                stride,
            ))
        };
        assert_external_packet(&decoded);
    }
    unsafe {
        cw_transfer_scanner_reset(session);
    }
    let decoded_after_reset = unsafe {
        take(cw_transfer_scanner_decode_bgra(
            session,
            pixels.as_ptr(),
            pixels.len(),
            width,
            height,
            stride,
        ))
    };
    assert_external_packet(&decoded_after_reset);
    unsafe {
        cw_transfer_scanner_free(session);
    }
}

#[test]
fn recovery_session_owns_no_borrowed_pixel_memory() {
    let payload: Vec<u8> = (0..2048).map(|i| (i * 73 + 41) as u8).collect();
    let encoded = scanner::encode_with_modulation(&payload, 4, Modulation::Luma10LowEcc).unwrap();
    let image = encoded.image;
    let stride = image.width() as usize * 4 + 12;
    let mut pixels = vec![0; stride * image.height() as usize];
    for (x, y, value) in image.enumerate_pixels() {
        let offset = y as usize * stride + x as usize * 4;
        pixels[offset..offset + 4].copy_from_slice(&[value[2], value[1], value[0], 255]);
    }
    let session = cw_recovery_create();
    assert!(!session.is_null());
    let invalid = unsafe {
        take(cw_recovery_decode_bgra(
            session,
            std::ptr::null(),
            0,
            1,
            1,
            4,
        ))
    };
    assert_eq!(invalid["status"], "error");
    assert_eq!(invalid["frames"], 0);
    let decoded = unsafe {
        take(cw_recovery_decode_bgra(
            session,
            pixels.as_ptr(),
            pixels.len(),
            image.width(),
            image.height(),
            stride,
        ))
    };
    pixels.fill(0);
    unsafe {
        cw_recovery_free(session);
    }
    assert_eq!(decoded["status"], "decoded");
    assert_eq!(decoded["frames"], 1);
    assert_eq!(decoded["payload"], json!(payload));
}

#[test]
fn ffi_preserves_both_luma10_profiles_through_padded_camera_rows() {
    for (profile, length, side, modulation, data_bytes, parity_bytes) in [
        (3, 2048, 126, "Luma10", 159, 96),
        (4, 2428, 122, "Luma10LowEcc", 207, 48),
    ] {
        let payload: Vec<u8> = (0..length).map(|i| (i * 131 + 47) as u8).collect();
        let encoded = unsafe { take(cw_encode_png(payload.as_ptr(), payload.len(), profile, 2)) };
        assert_eq!(encoded["status"], "encoded");
        assert_eq!(encoded["side_pixels"], side);
        assert_eq!(encoded["rs_data_bytes"], data_bytes);
        assert_eq!(encoded["rs_parity_bytes"], parity_bytes);
        let png: Vec<u8> = serde_json::from_value(encoded["png"].clone()).unwrap();
        let image = image::load_from_memory(&png).unwrap().to_rgb8();
        let stride = image.width() as usize * 4 + 16;
        let mut bgra = vec![0; stride * image.height() as usize];
        for (x, y, pixel) in image.enumerate_pixels() {
            let i = y as usize * stride + x as usize * 4;
            bgra[i..i + 4].copy_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
        }
        let decoded = unsafe {
            take(cw_decode_bgra(
                bgra.as_ptr(),
                bgra.len(),
                image.width(),
                image.height(),
                stride,
            ))
        };
        assert_eq!(decoded["status"], "decoded");
        assert_eq!(decoded["modulation"], modulation);
        assert_eq!(decoded["rs_data_bytes"], data_bytes);
        assert_eq!(decoded["rs_parity_bytes"], parity_bytes);
        assert_eq!(decoded["payload"], json!(payload));
    }
}

#[test]
fn ffi_rejects_invalid_buffers_and_compression_selector() {
    let result = unsafe { take(cw_decode_bgra(std::ptr::null(), 0, 158, 158, 632)) };
    assert_eq!(result["status"], "error");
    let pixel = [0u8; 4];
    let result = unsafe {
        take(cw_decode_bgra(
            pixel.as_ptr(),
            isize::MAX as usize + 1,
            1,
            1,
            4,
        ))
    };
    assert_eq!(result["status"], "error");
    let result = unsafe { take(cw_encode_png(std::ptr::null(), 2048, 0, 2)) };
    assert_eq!(result["status"], "error");
    let result = unsafe { take(cw_encode_png_with_options(std::ptr::null(), 2048, 3, 2, 1)) };
    assert_eq!(result["status"], "error");
    let payload = [0; 16];
    let result = unsafe {
        take(cw_encode_png_with_options(
            payload.as_ptr(),
            payload.len(),
            3,
            2,
            2,
        ))
    };
    assert_eq!(result["status"], "error");
}

#[test]
fn ffi_rejects_removed_profiles() {
    let payload = [0u8; 16];
    for profile in 0..=2 {
        let result = unsafe { take(cw_encode_png(payload.as_ptr(), payload.len(), profile, 2)) };
        assert_eq!(result["status"], "error");
        assert_eq!(result["message"], "Unknown CW profile.");
    }
}

#[test]
fn ffi_compressed_luma10_code_round_trips_original_payload() {
    let capacity = scanner::maximum_payload_bytes_with_modulation(2, Modulation::Luma10).unwrap();
    let payload = vec![b'a'; capacity + 1];
    let encoded = unsafe {
        take(cw_encode_png_with_options(
            payload.as_ptr(),
            payload.len(),
            3,
            2,
            1,
        ))
    };
    assert_eq!(encoded["status"], "encoded");
    assert_eq!(encoded["compression"], "zstd");
    assert_eq!(encoded["payload_bytes"], payload.len());
    assert!(encoded["encoded_payload_bytes"].as_u64().unwrap() < payload.len() as u64);
    let png: Vec<u8> = serde_json::from_value(encoded["png"].clone()).unwrap();
    let image = image::load_from_memory(&png).unwrap().to_rgb8();
    let stride = image.width() as usize * 4 + 16;
    let mut bgra = vec![0; stride * image.height() as usize];
    for (x, y, pixel) in image.enumerate_pixels() {
        let i = y as usize * stride + x as usize * 4;
        bgra[i..i + 4].copy_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
    }
    let decoded = unsafe {
        take(cw_decode_bgra(
            bgra.as_ptr(),
            bgra.len(),
            image.width(),
            image.height(),
            stride,
        ))
    };
    assert_eq!(decoded["status"], "decoded");
    assert_eq!(decoded["payload"], json!(payload));
}
