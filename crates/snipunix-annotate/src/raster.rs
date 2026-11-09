//! A plain, tightly packed RGBA8 raster returned by the renderer.

use thiserror::Error;

/// A rendered image in straight (non-premultiplied) RGBA8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl RenderedImage {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self, RasterError> {
        let expected = usize::try_from(width)
            .ok()
            .and_then(|w| usize::try_from(height).ok().map(|h| (w, h)))
            .and_then(|(w, h)| w.checked_mul(h))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(RasterError::TooLarge)?;
        if rgba.len() != expected {
            return Err(RasterError::SizeMismatch {
                expected,
                actual: rgba.len(),
            });
        }
        Ok(Self {
            width,
            height,
            rgba,
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
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    #[must_use]
    pub fn into_rgba(self) -> Vec<u8> {
        self.rgba
    }

    /// Composites the image over an opaque background, returning RGB8.
    #[must_use]
    pub fn to_rgb_on(&self, background: [u8; 3]) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.rgba.len() / 4 * 3);
        for pixel in self.rgba.chunks_exact(4) {
            let alpha = u32::from(pixel[3]);
            let inverse = 255 - alpha;
            for (index, channel) in background.iter().enumerate() {
                let value =
                    (u32::from(pixel[index]) * alpha + u32::from(*channel) * inverse + 127) / 255;
                out.push(value as u8);
            }
        }
        out
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RasterError {
    #[error("rendered raster is too large to address")]
    TooLarge,
    #[error("rendered raster has {actual} bytes, expected {expected}")]
    SizeMismatch { expected: usize, actual: usize },
}
