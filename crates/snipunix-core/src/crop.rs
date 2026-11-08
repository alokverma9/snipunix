//! Pixel extraction from an RGBA8 buffer.

use thiserror::Error;

use crate::mapping::ImageRect;

/// Copies a rectangular region out of a tightly packed RGBA8 buffer.
///
/// The returned buffer is row-major and tightly packed (no stride padding).
pub fn crop_rgba(
    source: &[u8],
    source_width: u32,
    source_height: u32,
    rect: ImageRect,
) -> Result<Vec<u8>, CropError> {
    let expected = usize::try_from(source_width)
        .ok()
        .and_then(|w| usize::try_from(source_height).ok().map(|h| (w, h)))
        .and_then(|(w, h)| w.checked_mul(h))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(CropError::BufferTooLarge)?;
    if source.len() != expected {
        return Err(CropError::BufferSizeMismatch {
            expected,
            actual: source.len(),
        });
    }
    if rect.width == 0 || rect.height == 0 {
        return Err(CropError::EmptyRect);
    }
    let right = u64::from(rect.x) + u64::from(rect.width);
    let bottom = u64::from(rect.y) + u64::from(rect.height);
    if right > u64::from(source_width) || bottom > u64::from(source_height) {
        return Err(CropError::OutOfBounds {
            rect,
            width: source_width,
            height: source_height,
        });
    }

    let source_stride = source_width as usize * 4;
    let row_bytes = rect.width as usize * 4;
    let mut output = Vec::with_capacity(row_bytes * rect.height as usize);
    for row in rect.y..bounds_end(rect.y, rect.height) {
        let start = row as usize * source_stride + rect.x as usize * 4;
        output.extend_from_slice(&source[start..start + row_bytes]);
    }
    Ok(output)
}

fn bounds_end(start: u32, length: u32) -> u32 {
    // Guarded by the bounds check above.
    start.saturating_add(length)
}

#[derive(Debug, Error, PartialEq)]
pub enum CropError {
    #[error("source buffer is too large to address")]
    BufferTooLarge,
    #[error("source buffer has {actual} bytes, expected {expected}")]
    BufferSizeMismatch { expected: usize, actual: usize },
    #[error("crop rectangle is empty")]
    EmptyRect,
    #[error("crop rectangle {rect:?} exceeds {width}x{height} image")]
    OutOfBounds {
        rect: ImageRect,
        width: u32,
        height: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 3x2 image with a unique byte value per pixel:
    /// 0 1 2
    /// 3 4 5
    fn fixture() -> Vec<u8> {
        let mut data = Vec::new();
        for value in 0u8..6 {
            data.extend_from_slice(&[value, value.wrapping_add(10), value.wrapping_add(20), 255]);
        }
        data
    }

    #[test]
    fn crops_full_image() {
        let source = fixture();
        let out = crop_rgba(&source, 3, 2, ImageRect::new(0, 0, 3, 2)).unwrap();
        assert_eq!(out, source);
    }

    #[test]
    fn crops_single_pixel_from_bottom_right() {
        let source = fixture();
        let out = crop_rgba(&source, 3, 2, ImageRect::new(2, 1, 1, 1)).unwrap();
        assert_eq!(out, vec![5, 15, 25, 255]);
    }

    #[test]
    fn crops_inner_rows_without_padding() {
        let source = fixture();
        let out = crop_rgba(&source, 3, 2, ImageRect::new(1, 0, 2, 2)).unwrap();
        assert_eq!(
            out,
            vec![
                1, 11, 21, 255, 2, 12, 22, 255, 4, 14, 24, 255, 5, 15, 25, 255
            ]
        );
    }

    #[test]
    fn rejects_bad_inputs() {
        assert!(matches!(
            crop_rgba(&[0; 3], 3, 2, ImageRect::new(0, 0, 1, 1)),
            Err(CropError::BufferSizeMismatch { .. })
        ));
        assert!(matches!(
            crop_rgba(&[0; 24], 3, 2, ImageRect::new(0, 0, 0, 1)),
            Err(CropError::EmptyRect)
        ));
        assert!(matches!(
            crop_rgba(&[0; 24], 3, 2, ImageRect::new(2, 1, 2, 1)),
            Err(CropError::OutOfBounds { .. })
        ));
    }
}
