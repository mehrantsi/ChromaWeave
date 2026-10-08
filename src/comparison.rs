use anyhow::{Result, anyhow, ensure};
use image::{Rgb, RgbImage, imageops};
use qrcode::bits::Bits;
use qrcode::{Color, EcLevel, QrCode, Version};
use serde::Serialize;

use crate::codec::MAX_IMAGE_SIDE;

const QUIET_ZONE_MODULES: u32 = 4;

#[derive(Debug, Clone, Copy, Serialize)]
pub enum QrErrorCorrection {
    M,
    H,
}

impl QrErrorCorrection {
    fn level(self) -> EcLevel {
        match self {
            Self::M => EcLevel::M,
            Self::H => EcLevel::H,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct QrMetadata {
    pub format: &'static str,
    pub ec_level: QrErrorCorrection,
    pub version: u8,
    pub modules: u32,
    pub quiet_zone_modules: u32,
    pub module_pitch_pixels: u32,
    pub raster_side_pixels: u32,
    pub occupied_side_pixels: u32,
    pub payload_bytes: usize,
    pub payload_capacity_bytes: usize,
    pub net_bits_per_pixel: f64,
}

pub struct QrEncoded {
    pub image: RgbImage,
    pub metadata: QrMetadata,
}

pub fn qr_byte_capacity(version: u8, ec_level: QrErrorCorrection) -> Result<usize> {
    ensure!(
        (1..=40).contains(&version),
        "QR version must be from 1 to 40"
    );
    let bits = Bits::new(Version::Normal(i16::from(version)));
    let count_bits = if version <= 9 { 8 } else { 16 };
    Ok((bits.max_len(ec_level.level())? - 4 - count_bits) / 8)
}

fn choose_version(payload_bytes: usize, ec_level: QrErrorCorrection) -> Result<u8> {
    for version in 1..=40 {
        if payload_bytes <= qr_byte_capacity(version, ec_level)? {
            return Ok(version);
        }
    }
    Err(anyhow!(
        "payload exceeds QR byte-mode capacity at {ec_level:?}"
    ))
}

pub fn qr_minimum_side_pixels(payload_bytes: usize, ec_level: QrErrorCorrection) -> Result<u32> {
    let version = choose_version(payload_bytes, ec_level)?;
    Ok(17 + 4 * u32::from(version) + 2 * QUIET_ZONE_MODULES)
}

pub fn encode_qr(
    payload: &[u8],
    side_pixels: u32,
    ec_level: QrErrorCorrection,
) -> Result<QrEncoded> {
    ensure!(
        (1..=MAX_IMAGE_SIDE).contains(&side_pixels),
        "QR raster side must be from 1 to {MAX_IMAGE_SIDE} pixels"
    );
    let version = choose_version(payload.len(), ec_level)?;
    let mut bits = Bits::new(Version::Normal(i16::from(version)));
    bits.push_byte_data(payload)?;
    bits.push_terminator(ec_level.level())?;
    let code = QrCode::with_bits(bits, ec_level.level())?;
    let modules = code.width() as u32;
    let module_pitch_pixels = side_pixels / (modules + 2 * QUIET_ZONE_MODULES);
    ensure!(
        module_pitch_pixels > 0,
        "QR modules do not fit the requested raster"
    );
    let occupied_side_pixels = (modules + 2 * QUIET_ZONE_MODULES) * module_pitch_pixels;
    let origin =
        (side_pixels - occupied_side_pixels) / 2 + QUIET_ZONE_MODULES * module_pitch_pixels;
    let mut image = RgbImage::from_pixel(side_pixels, side_pixels, Rgb([255; 3]));
    for module_y in 0..modules {
        for module_x in 0..modules {
            if code[(module_x as usize, module_y as usize)] == Color::Dark {
                for y in 0..module_pitch_pixels {
                    for x in 0..module_pitch_pixels {
                        image.put_pixel(
                            origin + module_x * module_pitch_pixels + x,
                            origin + module_y * module_pitch_pixels + y,
                            Rgb([0; 3]),
                        );
                    }
                }
            }
        }
    }
    Ok(QrEncoded {
        image,
        metadata: QrMetadata {
            format: "QR-Model-2-byte-mode",
            ec_level,
            version,
            modules,
            quiet_zone_modules: QUIET_ZONE_MODULES,
            module_pitch_pixels,
            raster_side_pixels: side_pixels,
            occupied_side_pixels,
            payload_bytes: payload.len(),
            payload_capacity_bytes: qr_byte_capacity(version, ec_level)?,
            net_bits_per_pixel: payload.len() as f64 * 8.0 / f64::from(side_pixels).powi(2),
        },
    })
}

pub fn decode_qr(image: &RgbImage) -> Result<Vec<u8>> {
    ensure!(
        image.width() > 0 && image.height() > 0,
        "QR capture is empty"
    );
    let gray = imageops::grayscale(image);
    let mut scanner = quircs::Quirc::default();
    let mut payloads = Vec::new();
    let mut last_error = None;
    for candidate in scanner.identify(gray.width() as usize, gray.height() as usize, gray.as_raw())
    {
        match candidate {
            Ok(code) => match code.decode() {
                Ok(decoded) => payloads.push(decoded.payload),
                Err(error) => last_error = Some(error.to_string()),
            },
            Err(error) => last_error = Some(error.to_string()),
        }
    }
    ensure!(
        payloads.len() <= 1,
        "capture contains multiple decoded QR codes"
    );
    payloads.pop().ok_or_else(|| {
        anyhow!(
            "automatic QR scan failed: {}",
            last_error.as_deref().unwrap_or("no QR code detected")
        )
    })
}

pub fn decode_qr_zxing(image: &RgbImage) -> Result<Vec<u8>> {
    use rxing::Reader;

    ensure!(
        image.width() > 0 && image.height() > 0,
        "QR capture is empty"
    );
    let gray = imageops::grayscale(image);
    let hints = rxing::DecodeHints {
        TryHarder: Some(true),
        PossibleFormats: Some(std::collections::HashSet::from([
            rxing::BarcodeFormat::QR_CODE,
        ])),
        ..rxing::DecodeHints::default()
    };
    let source = rxing::Luma8LuminanceSource::new(gray.into_raw(), image.width(), image.height())?;
    let mut bitmap = rxing::BinaryBitmap::new(rxing::common::HybridBinarizer::new(source));
    let decoded = rxing::qrcode::cpp_port::QrReader.decode_with_hints(&mut bitmap, &hints)?;
    Ok(decoded.getRawBytes().to_vec())
}
