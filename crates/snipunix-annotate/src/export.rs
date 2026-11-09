//! PNG, JPEG and WebP export with metadata stripped.
//!
//! The `image` backend writes only pixel data. No EXIF, XMP, IPTC or textual
//! metadata from the captured source is copied into the exported file.

use std::io::Cursor;
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::codecs::webp::WebPEncoder;
use image::{ExtendedColorType, ImageEncoder};
use thiserror::Error;

use crate::raster::RenderedImage;

/// A supported export container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Png,
    Jpeg,
    WebP,
}

impl ExportFormat {
    pub const ALL: [Self; 3] = [Self::Png, Self::Jpeg, Self::WebP];

    #[must_use]
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "webp" => Some(Self::WebP),
            _ => None,
        }
    }

    #[must_use]
    pub fn from_path(path: &Path) -> Option<Self> {
        path.extension()
            .and_then(|extension| extension.to_str())
            .and_then(Self::from_extension)
    }

    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::WebP => "webp",
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::WebP => "WebP",
        }
    }

    #[must_use]
    pub fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::WebP => "image/webp",
        }
    }

    /// `true` when this format supports a lossy quality setting.
    #[must_use]
    pub fn is_lossy(self) -> bool {
        matches!(self, Self::Jpeg)
    }
}

/// Default JPEG quality used by the primary Export action.
pub const DEFAULT_JPEG_QUALITY: u8 = 90;

/// Encodes a rendered image. JPEG quality is clamped to 1..=100. WebP uses the
/// lossless encoder provided by the selected `image` backend.
pub fn encode(
    image: &RenderedImage,
    format: ExportFormat,
    quality: u8,
) -> Result<Vec<u8>, ExportError> {
    let mut output = Cursor::new(Vec::new());
    match format {
        ExportFormat::Png => {
            PngEncoder::new(&mut output)
                .write_image(
                    image.rgba(),
                    image.width(),
                    image.height(),
                    ExtendedColorType::Rgba8,
                )
                .map_err(ExportError::Encode)?;
        }
        ExportFormat::Jpeg => {
            let quality = quality.clamp(1, 100);
            let rgb = image.to_rgb_on([255, 255, 255]);
            JpegEncoder::new_with_quality(&mut output, quality)
                .write_image(&rgb, image.width(), image.height(), ExtendedColorType::Rgb8)
                .map_err(ExportError::Encode)?;
        }
        ExportFormat::WebP => {
            WebPEncoder::new_lossless(&mut output)
                .write_image(
                    image.rgba(),
                    image.width(),
                    image.height(),
                    ExtendedColorType::Rgba8,
                )
                .map_err(ExportError::Encode)?;
        }
    }
    Ok(output.into_inner())
}

/// Encodes and writes an image to `path`. The caller chooses the path through
/// a native dialog, so this function never invents or silently reuses a name.
pub fn save(
    path: &Path,
    image: &RenderedImage,
    format: ExportFormat,
    quality: u8,
) -> Result<(), ExportError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(ExportError::Io)?;
        }
    }
    let bytes = encode(image, format, quality)?;
    std::fs::write(path, bytes).map_err(ExportError::Io)?;
    Ok(())
}

#[derive(Debug, Error)]
pub enum ExportError {
    #[error("unsupported export format; choose PNG, JPEG or WebP")]
    UnsupportedFormat,
    #[error("failed to encode image: {0}")]
    Encode(image::ImageError),
    #[error("failed to write image file: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;

    fn gradient(width: u32, height: u32) -> RenderedImage {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                rgba.extend_from_slice(&[
                    (x * 255 / width.max(1)) as u8,
                    (y * 255 / height.max(1)) as u8,
                    128,
                    255,
                ]);
            }
        }
        RenderedImage::new(width, height, rgba).unwrap()
    }

    #[test]
    fn every_format_has_the_expected_dimensions() {
        let image = gradient(37, 19);
        for format in ExportFormat::ALL {
            let bytes = encode(&image, format, DEFAULT_JPEG_QUALITY).unwrap();
            let decoded = image::load_from_memory(&bytes).unwrap();
            assert_eq!(decoded.dimensions(), (37, 19), "format {format:?}");
        }
    }

    #[test]
    fn extension_detection_is_case_insensitive() {
        assert_eq!(ExportFormat::from_extension("PNG"), Some(ExportFormat::Png));
        assert_eq!(
            ExportFormat::from_extension("JPEG"),
            Some(ExportFormat::Jpeg)
        );
        assert_eq!(
            ExportFormat::from_extension("webp"),
            Some(ExportFormat::WebP)
        );
        assert_eq!(ExportFormat::from_extension("gif"), None);
    }

    #[test]
    fn exports_carry_no_metadata_markers() {
        let image = gradient(8, 8);
        for format in ExportFormat::ALL {
            let bytes = encode(&image, format, DEFAULT_JPEG_QUALITY).unwrap();
            let haystack = String::from_utf8_lossy(&bytes);
            assert!(!haystack.contains("Exif"), "EXIF in {format:?}");
            assert!(!haystack.contains("xmp"), "XMP in {format:?}");
            assert!(!haystack.contains("tEXt"), "PNG text chunk in {format:?}");
        }
    }

    #[test]
    fn jpeg_quality_changes_output_size() {
        let image = gradient(128, 128);
        let low = encode(&image, ExportFormat::Jpeg, 20).unwrap();
        let high = encode(&image, ExportFormat::Jpeg, 95).unwrap();
        assert_ne!(low.len(), high.len());
    }

    #[test]
    fn transparent_pixels_are_flattened_for_jpeg() {
        let image = RenderedImage::new(2, 1, vec![255, 0, 0, 0, 0, 0, 255, 255]).unwrap();
        let bytes = encode(&image, ExportFormat::Jpeg, 100).unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap().to_rgb8();
        assert!(decoded.get_pixel(0, 0)[0] > 240);
        assert!(decoded.get_pixel(0, 0)[1] > 240);
        assert!(decoded.get_pixel(0, 0)[2] > 240);
    }
}
