//! Straight (non-premultiplied) sRGB colours used by the document model.

/// An sRGB colour with straight alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    #[must_use]
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    #[must_use]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    #[must_use]
    pub fn with_alpha(self, alpha: f64) -> Self {
        Self {
            a: (f64::from(self.a) * alpha.clamp(0.0, 1.0)).round() as u8,
            ..self
        }
    }

    #[must_use]
    pub fn to_tiny_skia(self) -> tiny_skia::Color {
        tiny_skia::Color::from_rgba8(self.r, self.g, self.b, self.a)
    }

    #[must_use]
    pub fn to_cosmic(self) -> cosmic_text::Color {
        cosmic_text::Color::rgba(self.r, self.g, self.b, self.a)
    }
}

/// Small, well-contrasted preset palette shared by the editor UI and tests.
pub const PRESETS: [(&str, Color); 8] = [
    ("Red", Color::rgb(0xE0, 0x1B, 0x24)),
    ("Orange", Color::rgb(0xF5, 0x7C, 0x00)),
    ("Yellow", Color::rgb(0xF6, 0xD3, 0x2C)),
    ("Green", Color::rgb(0x2E, 0xA0, 0x43)),
    ("Blue", Color::rgb(0x1C, 0x71, 0xD6)),
    ("Magenta", Color::rgb(0xC0, 0x39, 0x8B)),
    ("Black", Color::rgb(0x1A, 0x1A, 0x1A)),
    ("White", Color::rgb(0xFF, 0xFF, 0xFF)),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_scaling_is_clamped() {
        let c = Color::rgb(10, 20, 30);
        assert_eq!(c.with_alpha(0.5).a, 128);
        assert_eq!(c.with_alpha(-1.0).a, 0);
        assert_eq!(c.with_alpha(2.0).a, 255);
    }
}
