//! Region-to-stream-pixel mapping for recording.
//!
//! Portal stream sizes are in compositor-logical coordinates, while the
//! negotiated video frames are in device pixels. A 2× HiDPI monitor reports
//! 1920×1080 logically but delivers 3840×2160 frames, so the crop has to be
//! scaled after the pipeline has prerolled and the real frame size is known.

/// A region in compositor-logical coordinates, relative to the stream origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogicalCrop {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl LogicalCrop {
    /// Creates a crop, rejecting empty or negative rectangles.
    #[must_use]
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Option<Self> {
        if width <= 0 || height <= 0 {
            return None;
        }
        Some(Self {
            x,
            y,
            width,
            height,
        })
    }

    /// Converts a desktop-logical region into a crop relative to one stream.
    ///
    /// The portal reports the selected monitor's position and size in the
    /// compositor coordinate space, while the selection overlay works in
    /// virtual-desktop coordinates. The selection must lie fully inside the
    /// selected stream; a selection spanning two monitors cannot be recorded
    /// from a single monitor stream and returns `None`.
    #[must_use]
    pub fn from_desktop(
        region: snipunix_core::LogicalRect,
        stream_origin: snipunix_core::LogicalPoint,
        stream_size: snipunix_core::LogicalSize,
    ) -> Option<Self> {
        if region.is_empty()
            || !stream_size.is_positive()
            || !region.x.is_finite()
            || !region.y.is_finite()
            || !region.width.is_finite()
            || !region.height.is_finite()
        {
            return None;
        }
        let left = region.x - stream_origin.x;
        let top = region.y - stream_origin.y;
        // Allow half a logical pixel of rounding before rejecting a selection
        // that starts just outside the monitor.
        if left < -0.5 || top < -0.5 {
            return None;
        }
        let width = region.width.round() as i32;
        let height = region.height.round() as i32;
        let left = left.round() as i32;
        let top = top.round() as i32;
        if left < 0 || top < 0 {
            return None;
        }
        let right = i64::from(left) + i64::from(width);
        let bottom = i64::from(top) + i64::from(height);
        let stream_width = stream_size.width.round() as i64;
        let stream_height = stream_size.height.round() as i64;
        if right > stream_width || bottom > stream_height {
            return None;
        }
        Self::new(left, top, width, height)
    }
}

/// A crop in frame pixels, including the trailing margins for `videocrop`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelCrop {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
    pub right: i32,
    pub bottom: i32,
}

impl PixelCrop {
    /// Maps a logical crop onto a negotiated frame size.
    ///
    /// The result is clipped to the frame, and width/height are rounded down to
    /// even numbers because H.264 encoders reject odd dimensions. Returns
    /// `None` when the clipped region is smaller than 2×2.
    #[must_use]
    pub fn map(
        crop: LogicalCrop,
        logical_size: (i32, i32),
        pixel_size: (i32, i32),
    ) -> Option<Self> {
        let (logical_width, logical_height) = logical_size;
        let (pixel_width, pixel_height) = pixel_size;
        if logical_width <= 0
            || logical_height <= 0
            || pixel_width < 2
            || pixel_height < 2
            || crop.width <= 0
            || crop.height <= 0
        {
            return None;
        }

        let scale_x = f64::from(pixel_width) / f64::from(logical_width);
        let scale_y = f64::from(pixel_height) / f64::from(logical_height);
        let left = (f64::from(crop.x) * scale_x).round() as i32;
        let top = (f64::from(crop.y) * scale_y).round() as i32;
        let width = (f64::from(crop.width) * scale_x).round() as i32;
        let height = (f64::from(crop.height) * scale_y).round() as i32;

        Self::from_pixels(left, top, width, height, pixel_size)
    }

    /// Clips a pixel rectangle and removes odd trailing pixels.
    #[must_use]
    pub fn from_pixels(
        left: i32,
        top: i32,
        width: i32,
        height: i32,
        pixel_size: (i32, i32),
    ) -> Option<Self> {
        let (pixel_width, pixel_height) = pixel_size;
        if pixel_width < 2 || pixel_height < 2 || width <= 0 || height <= 0 {
            return None;
        }

        let left = left.clamp(0, pixel_width - 1);
        let top = top.clamp(0, pixel_height - 1);
        let width = width.min(pixel_width - left) & !1;
        let height = height.min(pixel_height - top) & !1;
        if width < 2 || height < 2 {
            return None;
        }

        Some(Self {
            left,
            top,
            width,
            height,
            right: pixel_width - left - width,
            bottom: pixel_height - top - height,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snipunix_core::{LogicalPoint, LogicalRect, LogicalSize};

    #[test]
    fn desktop_regions_are_relative_to_the_stream_origin() {
        let region = LogicalRect::new(2020.0, 110.0, 640.0, 480.0);
        let crop = LogicalCrop::from_desktop(
            region,
            LogicalPoint::new(1920.0, 0.0),
            LogicalSize::new(1920.0, 1080.0),
        )
        .unwrap();
        assert_eq!(crop, LogicalCrop::new(100, 110, 640, 480).unwrap());
    }

    #[test]
    fn desktop_regions_are_rejected_when_they_leave_the_stream() {
        let size = LogicalSize::new(1920.0, 1080.0);
        let origin = LogicalPoint::new(1920.0, 0.0);
        assert!(
            LogicalCrop::from_desktop(LogicalRect::new(1800.0, 100.0, 640.0, 480.0), origin, size)
                .is_none()
        );
        assert!(
            LogicalCrop::from_desktop(LogicalRect::new(2000.0, 900.0, 100.0, 300.0), origin, size)
                .is_none()
        );
        assert!(
            LogicalCrop::from_desktop(LogicalRect::new(2000.0, 100.0, 0.0, 480.0), origin, size)
                .is_none()
        );
    }

    #[test]
    fn fractional_region_coordinates_round_to_logical_pixels() {
        let crop = LogicalCrop::from_desktop(
            LogicalRect::new(10.4, 20.6, 333.0, 217.0),
            LogicalPoint::new(0.0, 0.0),
            LogicalSize::new(1920.0, 1080.0),
        )
        .unwrap();
        assert_eq!(crop, LogicalCrop::new(10, 21, 333, 217).unwrap());
    }

    #[test]
    fn one_to_one_crops_are_pixel_exact() {
        let crop = LogicalCrop::new(100, 50, 640, 480).unwrap();
        let pixel = PixelCrop::map(crop, (1920, 1080), (1920, 1080)).unwrap();
        assert_eq!(
            pixel,
            PixelCrop {
                left: 100,
                top: 50,
                width: 640,
                height: 480,
                right: 1180,
                bottom: 550,
            }
        );
    }

    #[test]
    fn hidpi_crops_scale_to_device_pixels() {
        let crop = LogicalCrop::new(100, 50, 640, 480).unwrap();
        let pixel = PixelCrop::map(crop, (1920, 1080), (3840, 2160)).unwrap();
        assert_eq!(pixel.left, 200);
        assert_eq!(pixel.top, 100);
        assert_eq!(pixel.width, 1280);
        assert_eq!(pixel.height, 960);
        assert_eq!(pixel.right, 2360);
        assert_eq!(pixel.bottom, 1100);
    }

    #[test]
    fn fractional_scales_round_to_even_sizes() {
        let crop = LogicalCrop::new(10, 10, 333, 217).unwrap();
        let pixel = PixelCrop::map(crop, (1536, 864), (1920, 1080)).unwrap();
        assert_eq!(pixel.width % 2, 0);
        assert_eq!(pixel.height % 2, 0);
        assert!(pixel.left + pixel.width + pixel.right == 1920);
        assert!(pixel.top + pixel.height + pixel.bottom == 1080);
    }

    #[test]
    fn crops_are_clipped_to_the_frame() {
        let crop = LogicalCrop::new(1800, 1000, 500, 500).unwrap();
        let pixel = PixelCrop::map(crop, (1920, 1080), (1920, 1080)).unwrap();
        assert_eq!(pixel.left, 1800);
        assert_eq!(pixel.width, 120);
        assert_eq!(pixel.top, 1000);
        assert_eq!(pixel.height, 80);
        assert_eq!(pixel.right, 0);
        assert_eq!(pixel.bottom, 0);
    }

    #[test]
    fn degenerate_crops_are_rejected() {
        assert!(LogicalCrop::new(0, 0, 0, 10).is_none());
        assert!(LogicalCrop::new(0, 0, 10, -1).is_none());
        assert!(
            PixelCrop::map(
                LogicalCrop::new(0, 0, 1, 1).unwrap(),
                (1920, 1080),
                (1920, 1080)
            )
            .is_none()
        );
        assert!(
            PixelCrop::map(
                LogicalCrop::new(0, 0, 100, 100).unwrap(),
                (0, 1080),
                (1920, 1080)
            )
            .is_none()
        );
    }

    #[test]
    fn margins_reconstruct_the_full_frame() {
        let pixel = PixelCrop::from_pixels(3, 5, 641, 481, (1920, 1080)).unwrap();
        assert_eq!(pixel.left + pixel.width + pixel.right, 1920);
        assert_eq!(pixel.top + pixel.height + pixel.bottom, 1080);
        assert_eq!(pixel.width, 640);
        assert_eq!(pixel.height, 480);
    }
}
