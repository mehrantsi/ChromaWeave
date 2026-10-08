pub(crate) fn bgra_to_rgb(input: &[u8], width: u32, height: u32, stride: usize) -> Vec<u8> {
    let width = width as usize;
    let height = height as usize;
    let row_bytes = width * 4;
    let rgb_stride = width * 3;
    debug_assert!(stride >= row_bytes);
    debug_assert!(input.len() >= stride * height);

    let mut rgb = vec![0; rgb_stride * height];
    for row in 0..height {
        let source = &input[row * stride..row * stride + row_bytes];
        let destination = &mut rgb[row * rgb_stride..(row + 1) * rgb_stride];
        convert_row(source, destination);
    }
    rgb
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn convert_row(input: &[u8], output: &mut [u8]) {
    use std::arch::aarch64::{uint8x16x3_t, vld4q_u8, vst3q_u8};

    let pixels = input.len() / 4;
    let vector_pixels = pixels / 16 * 16;
    for pixel in (0..vector_pixels).step_by(16) {
        unsafe {
            let bgra = vld4q_u8(input.as_ptr().add(pixel * 4));
            vst3q_u8(
                output.as_mut_ptr().add(pixel * 3),
                uint8x16x3_t(bgra.2, bgra.1, bgra.0),
            );
        }
    }
    for pixel in vector_pixels..pixels {
        output[pixel * 3] = input[pixel * 4 + 2];
        output[pixel * 3 + 1] = input[pixel * 4 + 1];
        output[pixel * 3 + 2] = input[pixel * 4];
    }
}

#[cfg(not(target_arch = "aarch64"))]
fn convert_row(input: &[u8], output: &mut [u8]) {
    for (bgra, rgb) in input
        .as_chunks::<4>()
        .0
        .iter()
        .zip(output.as_chunks_mut::<3>().0)
    {
        rgb.copy_from_slice(&[bgra[2], bgra[1], bgra[0]]);
    }
}

#[cfg(test)]
#[path = "tests/pixels.rs"]
mod tests;
