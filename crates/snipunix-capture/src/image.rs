//! Decoding and encoding of captured images.

use std::path::Path;

use image::{ImageFormat, RgbaImage};
use snipunix_core::ImageSize;
use thiserror::Error;

/// Hard ceiling for one in-memory capture. A larger image is rejected instead
/// of letting a malformed portal response exhaust memory. 512 MiB covers 8K
/// RGBA with generous headroom while keeping the process bounded.
pub const MAX_IMAGE_BYTES: usize = 512 * 1024 * 1024;

/// A decoded, tightly packed RGBA8 image.
#[derive(Clone)]
pub struct CapturedImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    source: Option<String>,
}

impl std::fmt::Debug for CapturedImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CapturedImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.rgba.len())
            .field("source", &self.source)
            .finish()
    }
}

impl CapturedImage {
    pub fn new(
        width: u32,
        height: u32,
        rgba: Vec<u8>,
        source: Option<String>,
    ) -> Result<Self, ImageError> {
        let expected = usize::try_from(width)
            .ok()
            .and_then(|w| usize::try_from(height).ok().map(|h| (w, h)))
            .and_then(|(w, h)| w.checked_mul(h))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(ImageError::TooLarge)?;
        if expected > MAX_IMAGE_BYTES {
            return Err(ImageError::TooLarge);
        }
        if rgba.len() != expected {
            return Err(ImageError::BufferSizeMismatch {
                expected,
                actual: rgba.len(),
            });
        }
        Ok(Self {
            width,
            height,
            rgba,
            source,
        })
    }

    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    #[must_use]
    pub fn size(&self) -> ImageSize {
        ImageSize::new(self.width, self.height)
    }

    #[must_use]
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    #[must_use]
    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    /// Crops using the already validated image rectangle and returns a new
    /// image. The source image is left untouched.
    pub fn crop(&self, rect: snipunix_core::ImageRect) -> Result<Self, ImageError> {
        let bytes = snipunix_core::crop_rgba(&self.rgba, self.width, self.height, rect)
            .map_err(ImageError::Crop)?;
        Self::new(rect.width, rect.height, bytes, self.source.clone())
    }

    /// Encodes the image as PNG at `path`.
    pub fn save_png(&self, path: &Path) -> Result<(), ImageError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(ImageError::Io)?;
            }
        }
        let buffer = RgbaImage::from_raw(self.width, self.height, self.rgba.clone())
            .ok_or(ImageError::TooLarge)?;
        buffer
            .save_with_format(path, ImageFormat::Png)
            .map_err(ImageError::Encode)
    }

    pub fn encode_png(&self) -> Result<Vec<u8>, ImageError> {
        let buffer = RgbaImage::from_raw(self.width, self.height, self.rgba.clone())
            .ok_or(ImageError::TooLarge)?;
        let mut out = std::io::Cursor::new(Vec::new());
        buffer
            .write_to(&mut out, ImageFormat::Png)
            .map_err(ImageError::Encode)?;
        Ok(out.into_inner())
    }
}

/// Decodes a PNG (or any format enabled in the `image` crate) into RGBA8.
pub fn decode_image(bytes: &[u8], source: Option<String>) -> Result<CapturedImage, ImageError> {
    let decoded = image::load_from_memory(bytes).map_err(ImageError::Decode)?;
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    CapturedImage::new(width, height, rgba.into_raw(), source)
}

#[derive(Debug, Error)]
pub enum ImageError {
    #[error("image dimensions are too large")]
    TooLarge,
    #[error("RGBA buffer has {actual} bytes, expected {expected}")]
    BufferSizeMismatch { expected: usize, actual: usize },
    #[error("failed to decode image: {0}")]
    Decode(#[from] image::ImageError),
    #[error("failed to encode image: {0}")]
    Encode(image::ImageError),
    #[error("failed to access image file: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid crop: {0}")]
    Crop(snipunix_core::CropError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_png_without_changing_pixels() {
        let image = CapturedImage::new(2, 2, vec![255; 16], None).unwrap();
        let encoded = image.encode_png().unwrap();
        let decoded = decode_image(&encoded, Some("memory".into())).unwrap();
        assert_eq!(decoded.width(), 2);
        assert_eq!(decoded.height(), 2);
        assert_eq!(decoded.rgba(), &[255; 16]);
    }

    #[test]
    fn rejects_wrong_buffer_length() {
        assert!(matches!(
            CapturedImage::new(2, 2, vec![0; 15], None),
            Err(ImageError::BufferSizeMismatch { .. })
        ));
    }

    #[test]
    fn crop_returns_new_image() {
        let image = CapturedImage::new(2, 2, (0u8..16).collect(), None).unwrap();
        let cropped = image
            .crop(snipunix_core::ImageRect::new(1, 0, 1, 2))
            .unwrap();
        assert_eq!(cropped.width(), 1);
        assert_eq!(cropped.height(), 2);
        assert_eq!(cropped.rgba(), &[4, 5, 6, 7, 12, 13, 14, 15]);
    }
}
