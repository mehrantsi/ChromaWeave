use super::*;

#[test]
fn converts_unaligned_bgra_with_padded_rows_and_vector_tails() {
    for width in [1, 15, 16, 17, 31, 32, 33] {
        let height = 3;
        let stride = width * 4 + 7;
        let length = stride * height;
        let mut storage = vec![0xa5; length + 1];
        let input = &mut storage[1..length + 1];
        let mut expected = Vec::with_capacity(width * height * 3);
        for y in 0..height {
            for x in 0..width {
                let offset = y * stride + x * 4;
                let bgra = [
                    (x * 17 + y * 31) as u8,
                    (x * 7 + y * 53) as u8,
                    (x * 29 + y * 11) as u8,
                    0xff,
                ];
                input[offset..offset + 4].copy_from_slice(&bgra);
                expected.extend_from_slice(&[bgra[2], bgra[1], bgra[0]]);
            }
        }
        assert_eq!(
            bgra_to_rgb(input, width as u32, height as u32, stride),
            expected
        );
    }
}

#[test]
fn ignores_trailing_bytes_after_validated_rows() {
    let width = 17;
    let height = 2;
    let stride = width * 4 + 3;
    let required = stride * height;
    let mut input = vec![0x7b; required + 19];
    for y in 0..height {
        for x in 0..width {
            let offset = y * stride + x * 4;
            input[offset..offset + 4].copy_from_slice(&[x as u8, y as u8, 0x35, 0xff]);
        }
    }
    let output = bgra_to_rgb(&input, width as u32, height as u32, stride);
    assert_eq!(&output[..3], &[0x35, 0, 0]);
    assert_eq!(&output[(width * 3)..(width * 3 + 3)], &[0x35, 1, 0]);
}
