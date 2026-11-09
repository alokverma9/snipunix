//! Deterministic annotation rasteriser.
//!
//! The editor canvas and every export call the same vector renderer. The canvas
//! only varies the `scale` and the visible source rectangle; geometry, text
//! shaping, arrowheads and compositing therefore stay identical.

use std::collections::HashMap;
use std::sync::Arc;

use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache, Wrap};
use snipunix_capture::CapturedImage;
use thiserror::Error;
use tiny_skia::{
    FillRule, FilterQuality, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, PixmapPaint,
    Stroke, Transform,
};

use crate::color::Color;
use crate::document::{Annotation, AnnotationId, AnnotationKind, Document, arrow_head_length};
use crate::geometry::{DocPoint, DocRect};
use crate::raster::{RasterError, RenderedImage};

const TEXT_PADDING: u32 = 2;
const TEXT_CACHE_LIMIT: usize = 96;
pub const HIGHLIGHTER_ALPHA: f64 = 0.36;

/// An unfinished object drawn on top of the committed document.
#[derive(Debug, Clone, Copy)]
pub enum Preview<'a> {
    None,
    /// A new object that will be appended on release.
    Add(&'a Annotation),
    /// A moved, resized or edited object replacing the committed one.
    Replace(&'a Annotation),
}

impl<'a> Preview<'a> {
    fn replacement_for(self, id: AnnotationId) -> Option<&'a Annotation> {
        match self {
            Self::Replace(annotation) if annotation.id == id => Some(annotation),
            _ => None,
        }
    }
}

/// Renders a document region at a chosen scale.
pub struct RenderEngine {
    font_system: FontSystem,
    swash_cache: SwashCache,
    text_cache: HashMap<TextKey, TextBitmap>,
    base_cache: Option<BaseCache>,
}

struct BaseCache {
    image: Arc<CapturedImage>,
    pixmap: Pixmap,
}

struct TextBitmap {
    pixmap: Pixmap,
    offset_x: f32,
    offset_y: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TextKey {
    text: String,
    font_size_bits: u32,
    width_bits: u32,
    color: Color,
}

impl RenderEngine {
    #[must_use]
    pub fn new() -> Self {
        Self {
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
            text_cache: HashMap::new(),
            base_cache: None,
        }
    }

    /// Renders the captured pixels for `source` without any annotations.
    pub fn render_base(
        &mut self,
        document: &Document,
        source: DocRect,
        scale: f64,
    ) -> Result<RenderedImage, RenderError> {
        let (width, height) = output_size(source, scale)?;
        let mut pixmap = Pixmap::new(width, height).ok_or(RenderError::TooLarge)?;
        self.draw_base(&mut pixmap, document, source, scale)?;
        RenderedImage::new(width, height, pixmap.take_demultiplied()).map_err(RenderError::Raster)
    }

    /// Draws the annotations on top of an already rendered base layer.
    pub fn compose(
        &mut self,
        document: &Document,
        preview: Preview<'_>,
        source: DocRect,
        scale: f64,
        base: &RenderedImage,
    ) -> Result<RenderedImage, RenderError> {
        let (width, height) = output_size(source, scale)?;
        if base.width() != width || base.height() != height {
            return Err(RenderError::LayerSizeMismatch);
        }
        let mut pixmap = premultiplied_pixmap(base.rgba(), width, height)?;
        let transform = document_to_output(source, scale);
        for annotation in document.annotations() {
            if let Some(replacement) = preview.replacement_for(annotation.id) {
                self.draw_annotation(&mut pixmap, replacement, transform);
            } else {
                self.draw_annotation(&mut pixmap, annotation, transform);
            }
        }
        if let Preview::Add(annotation) = preview {
            self.draw_annotation(&mut pixmap, annotation, transform);
        }
        RenderedImage::new(width, height, pixmap.take_demultiplied()).map_err(RenderError::Raster)
    }

    /// Convenience path for export: base plus annotations in one call.
    pub fn render(
        &mut self,
        document: &Document,
        preview: Preview<'_>,
        source: DocRect,
        scale: f64,
    ) -> Result<RenderedImage, RenderError> {
        let base = self.render_base(document, source, scale)?;
        self.compose(document, preview, source, scale, &base)
    }

    /// Measures the laid-out text box at document scale. The editor uses this
    /// to keep selection bounds honest after multiline edits.
    pub fn measure_text(&mut self, text: &str, font_size: f64, wrap_width: f64) -> (f64, f64) {
        let key = TextKey {
            text: text.to_string(),
            font_size_bits: (font_size.max(1.0) as f32).to_bits(),
            width_bits: (wrap_width.max(1.0) as f32).to_bits(),
            color: Color::rgb(0, 0, 0),
        };
        let (width, height) = self.text_bounds(&key);
        (f64::from(width), f64::from(height))
    }

    fn draw_base(
        &mut self,
        pixmap: &mut Pixmap,
        document: &Document,
        source: DocRect,
        scale: f64,
    ) -> Result<(), RenderError> {
        let base = document.base();
        let cache_is_current = self
            .base_cache
            .as_ref()
            .is_some_and(|cache| Arc::ptr_eq(&cache.image, base));
        if !cache_is_current {
            let data = premultiply_rgba(base.rgba());
            let base_pixmap = Pixmap::from_vec(
                data,
                tiny_skia::IntSize::from_wh(base.width(), base.height())
                    .ok_or(RenderError::TooLarge)?,
            )
            .ok_or(RenderError::TooLarge)?;
            self.base_cache = Some(BaseCache {
                image: Arc::clone(base),
                pixmap: base_pixmap,
            });
        }

        if scale_close_to_one(scale) && is_integer_aligned(source) {
            copy_integer_base(pixmap, base, source);
            return Ok(());
        }

        let Some(cache) = self.base_cache.as_ref() else {
            return Ok(());
        };
        let transform = Transform::from_scale(scale as f32, scale as f32)
            .post_translate((-source.x * scale) as f32, (-source.y * scale) as f32);
        let paint = PixmapPaint {
            quality: FilterQuality::Bilinear,
            ..PixmapPaint::default()
        };
        pixmap.draw_pixmap(0, 0, cache.pixmap.as_ref(), &paint, transform, None);
        Ok(())
    }

    fn draw_annotation(
        &mut self,
        pixmap: &mut Pixmap,
        annotation: &Annotation,
        transform: Transform,
    ) {
        match &annotation.kind {
            AnnotationKind::Stroke(stroke) => {
                let color = if stroke.highlighter {
                    stroke.color.with_alpha(stroke.opacity.clamp(0.0, 1.0))
                } else {
                    stroke.color
                };
                draw_stroke(pixmap, &stroke.points, color, stroke.width, transform);
            }
            AnnotationKind::Line(line) => {
                draw_line(
                    pixmap, line.start, line.end, line.color, line.width, transform,
                );
            }
            AnnotationKind::Arrow(arrow) => {
                draw_arrow(
                    pixmap,
                    arrow.start,
                    arrow.end,
                    arrow.color,
                    arrow.width,
                    transform,
                );
            }
            AnnotationKind::Rectangle(shape) => {
                let path = PathBuilder::from_rect(shape.rect.to_tiny_skia());
                stroke_path(pixmap, &path, shape.color, shape.width, transform);
            }
            AnnotationKind::Ellipse(shape) => {
                if let Some(path) = PathBuilder::from_oval(shape.rect.to_tiny_skia()) {
                    stroke_path(pixmap, &path, shape.color, shape.width, transform);
                }
            }
            AnnotationKind::Redaction(redaction) => {
                let path = PathBuilder::from_rect(redaction.rect.to_tiny_skia());
                fill_path(pixmap, &path, Color::rgb(0, 0, 0), transform);
            }
            AnnotationKind::Text(text) => {
                if text.text.is_empty() {
                    return;
                }
                let key = TextKey {
                    text: text.text.clone(),
                    font_size_bits: (text.font_size.max(1.0) as f32).to_bits(),
                    width_bits: (text.width.max(1.0) as f32).to_bits(),
                    color: text.color,
                };
                let Some(bitmap) = self.text_bitmap(&key) else {
                    return;
                };
                let text_transform = transform.pre_translate(
                    text.origin.x as f32 - bitmap.offset_x,
                    text.origin.y as f32 - bitmap.offset_y,
                );
                let paint = PixmapPaint {
                    quality: FilterQuality::Bilinear,
                    ..PixmapPaint::default()
                };
                pixmap.draw_pixmap(0, 0, bitmap.pixmap.as_ref(), &paint, text_transform, None);
            }
        }
    }

    fn text_bitmap(&mut self, key: &TextKey) -> Option<&TextBitmap> {
        if !self.text_cache.contains_key(key) {
            if self.text_cache.len() >= TEXT_CACHE_LIMIT {
                self.text_cache.clear();
            }
            let (width, height) = self.text_bounds(key);
            let pixmap = self.rasterise_text(key, width, height)?;
            self.text_cache.insert(
                key.clone(),
                TextBitmap {
                    pixmap,
                    offset_x: TEXT_PADDING as f32,
                    offset_y: TEXT_PADDING as f32,
                },
            );
        }
        self.text_cache.get(key)
    }

    fn text_bounds(&mut self, key: &TextKey) -> (u32, u32) {
        let font_size = f32::from_bits(key.font_size_bits).max(1.0);
        let wrap_width = f32::from_bits(key.width_bits).max(1.0);
        let buffer = self.buffer_for(key, font_size, wrap_width);
        let mut max_x = 0.0_f32;
        let mut max_y = font_size * 1.3;
        for run in buffer.layout_runs() {
            max_x = max_x.max(run.line_w);
            max_y = max_y.max(run.line_top + run.line_height);
        }
        (
            max_x.ceil().max(1.0) as u32 + TEXT_PADDING * 2,
            max_y.ceil().max(1.0) as u32 + TEXT_PADDING * 2,
        )
    }

    fn buffer_for(&mut self, key: &TextKey, font_size: f32, wrap_width: f32) -> Buffer {
        let metrics = Metrics::new(font_size, font_size * 1.3);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_wrap(Wrap::WordOrGlyph);
        buffer.set_size(Some(wrap_width), None);
        let attrs = Attrs::new().family(Family::SansSerif);
        buffer.set_text(&key.text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);
        buffer
    }

    fn rasterise_text(&mut self, key: &TextKey, width: u32, height: u32) -> Option<Pixmap> {
        let font_size = f32::from_bits(key.font_size_bits).max(1.0);
        let wrap_width = f32::from_bits(key.width_bits).max(1.0);
        let mut buffer = self.buffer_for(key, font_size, wrap_width);
        let mut rgba = vec![
            0_u8;
            (width as usize)
                .checked_mul(height as usize)?
                .checked_mul(4)?
        ];
        let text_color = key.color.to_cosmic();
        buffer.draw(
            &mut self.font_system,
            &mut self.swash_cache,
            text_color,
            |x, y, w, h, color| {
                blend_rect(
                    &mut rgba,
                    width,
                    height,
                    x + TEXT_PADDING as i32,
                    y + TEXT_PADDING as i32,
                    w,
                    h,
                    Color::rgba(color.r(), color.g(), color.b(), color.a()),
                );
            },
        );
        premultiplied_pixmap(&rgba, width, height).ok()
    }
}

impl Default for RenderEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn output_size(source: DocRect, scale: f64) -> Result<(u32, u32), RenderError> {
    if source.is_empty() {
        return Err(RenderError::EmptySource);
    }
    if !scale.is_finite() || scale <= 0.0 || scale > 64.0 {
        return Err(RenderError::InvalidScale);
    }
    let width = (source.width * scale).round().max(1.0);
    let height = (source.height * scale).round().max(1.0);
    if width > f64::from(u32::MAX) || height > f64::from(u32::MAX) {
        return Err(RenderError::TooLarge);
    }
    Ok((width as u32, height as u32))
}

fn document_to_output(source: DocRect, scale: f64) -> Transform {
    Transform::from_scale(scale as f32, scale as f32)
        .post_translate((-source.x * scale) as f32, (-source.y * scale) as f32)
}

fn scale_close_to_one(scale: f64) -> bool {
    (scale - 1.0).abs() < 1e-6
}

fn is_integer_aligned(rect: DocRect) -> bool {
    let aligned = |value: f64| (value - value.round()).abs() < 1e-6;
    aligned(rect.x) && aligned(rect.y) && aligned(rect.width) && aligned(rect.height)
}

fn premultiply_rgba(rgba: &[u8]) -> Vec<u8> {
    let mut out = rgba.to_vec();
    for pixel in out.chunks_exact_mut(4) {
        let alpha = u32::from(pixel[3]);
        if alpha != 255 {
            for channel in &mut pixel[..3] {
                *channel = ((u32::from(*channel) * alpha + 127) / 255) as u8;
            }
        }
    }
    out
}

fn premultiplied_pixmap(rgba: &[u8], width: u32, height: u32) -> Result<Pixmap, RenderError> {
    let data = premultiply_rgba(rgba);
    let size = tiny_skia::IntSize::from_wh(width, height).ok_or(RenderError::TooLarge)?;
    Pixmap::from_vec(data, size).ok_or(RenderError::TooLarge)
}

fn copy_integer_base(pixmap: &mut Pixmap, base: &CapturedImage, source: DocRect) {
    let left = source.x.round() as i64;
    let top = source.y.round() as i64;
    if left < 0 || top < 0 {
        return;
    }
    let left = left as u32;
    let top = top as u32;
    let width = pixmap.width().min(base.width().saturating_sub(left));
    let height = pixmap.height().min(base.height().saturating_sub(top));
    if width == 0 || height == 0 {
        return;
    }
    let output_stride = pixmap.width() as usize * 4;
    let base_stride = base.width() as usize * 4;
    let output = pixmap.data_mut();
    for row in 0..height as usize {
        let source_start = (top as usize + row) * base_stride + left as usize * 4;
        let target_start = row * output_stride;
        let source_slice = &base.rgba()[source_start..source_start + width as usize * 4];
        let target_slice = &mut output[target_start..target_start + width as usize * 4];
        for (target, source_pixel) in target_slice
            .chunks_exact_mut(4)
            .zip(source_slice.chunks_exact(4))
        {
            let alpha = u32::from(source_pixel[3]);
            target[3] = source_pixel[3];
            if alpha == 255 {
                target[..3].copy_from_slice(&source_pixel[..3]);
            } else {
                for channel in 0..3 {
                    target[channel] =
                        ((u32::from(source_pixel[channel]) * alpha + 127) / 255) as u8;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn blend_rect(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    rect_width: u32,
    rect_height: u32,
    color: Color,
) {
    if color.a == 0 {
        return;
    }
    let x_start = x.max(0);
    let y_start = y.max(0);
    let x_end = ((x as i64) + i64::from(rect_width)).min(i64::from(width)) as i32;
    let y_end = ((y as i64) + i64::from(rect_height)).min(i64::from(height)) as i32;
    if x_end <= x_start || y_end <= y_start {
        return;
    }
    let source_alpha = u32::from(color.a);
    let inverse = 255 - source_alpha;
    let width_usize = width as usize;
    for row in y_start..y_end {
        for column in x_start..x_end {
            let index = (row as usize * width_usize + column as usize) * 4;
            let destination_alpha = u32::from(rgba[index + 3]);
            // Straight-alpha source-over in integer arithmetic:
            //   out_a = sa + da * (1 - sa)
            //   out_c = (src_c * sa + dst_c * da * (1 - sa)) / out_a
            // Scaling the numerator and denominator by 255 keeps everything
            // integral and exact enough for glyph coverage values.
            let source_weight = source_alpha * 255;
            let destination_weight = destination_alpha * inverse;
            let denominator = source_weight + destination_weight;
            if denominator == 0 {
                continue;
            }
            for (channel, source) in [color.r, color.g, color.b].into_iter().enumerate() {
                let destination = u32::from(rgba[index + channel]);
                let blended = (u32::from(source) * source_weight
                    + destination * destination_weight
                    + denominator / 2)
                    / denominator;
                rgba[index + channel] = blended as u8;
            }
            rgba[index + 3] = ((denominator + 127) / 255) as u8;
        }
    }
}

fn stroke_path(pixmap: &mut Pixmap, path: &Path, color: Color, width: f64, transform: Transform) {
    let mut paint = Paint::default();
    paint.set_color(color.to_tiny_skia());
    paint.anti_alias = true;
    let stroke = Stroke {
        width: width.max(0.5) as f32,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    pixmap.stroke_path(path, &paint, &stroke, transform, None);
}

fn fill_path(pixmap: &mut Pixmap, path: &Path, color: Color, transform: Transform) {
    let mut paint = Paint::default();
    paint.set_color(color.to_tiny_skia());
    paint.anti_alias = true;
    pixmap.fill_path(path, &paint, FillRule::Winding, transform, None);
}

fn draw_stroke(
    pixmap: &mut Pixmap,
    points: &[DocPoint],
    color: Color,
    width: f64,
    transform: Transform,
) {
    if points.is_empty() {
        return;
    }
    if points.len() == 1 {
        if let Some(path) = PathBuilder::from_circle(
            points[0].x as f32,
            points[0].y as f32,
            (width.max(1.0) / 2.0) as f32,
        ) {
            fill_path(pixmap, &path, color, transform);
        }
        return;
    }
    let mut builder = PathBuilder::new();
    builder.move_to(points[0].x as f32, points[0].y as f32);
    if points.len() == 2 {
        builder.line_to(points[1].x as f32, points[1].y as f32);
    } else {
        for index in 1..points.len() - 1 {
            let control = points[index];
            let middle = control.midpoint(points[index + 1]);
            builder.quad_to(
                control.x as f32,
                control.y as f32,
                middle.x as f32,
                middle.y as f32,
            );
        }
        let last = points[points.len() - 1];
        builder.line_to(last.x as f32, last.y as f32);
    }
    if let Some(path) = builder.finish() {
        stroke_path(pixmap, &path, color, width, transform);
    }
}

fn draw_line(
    pixmap: &mut Pixmap,
    start: DocPoint,
    end: DocPoint,
    color: Color,
    width: f64,
    transform: Transform,
) {
    let mut builder = PathBuilder::new();
    builder.move_to(start.x as f32, start.y as f32);
    builder.line_to(end.x as f32, end.y as f32);
    if let Some(path) = builder.finish() {
        stroke_path(pixmap, &path, color, width, transform);
    }
}

fn draw_arrow(
    pixmap: &mut Pixmap,
    start: DocPoint,
    end: DocPoint,
    color: Color,
    width: f64,
    transform: Transform,
) {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let length = dx.hypot(dy);
    if length < 1e-6 {
        draw_stroke(pixmap, &[start], color, width, transform);
        return;
    }
    let unit_x = dx / length;
    let unit_y = dy / length;
    let head = arrow_head_length(width).min(length);
    let base = DocPoint::new(end.x - unit_x * head, end.y - unit_y * head);
    let spread = head * 0.45;
    let left = DocPoint::new(base.x - unit_y * spread, base.y + unit_x * spread);
    let right = DocPoint::new(base.x + unit_y * spread, base.y - unit_x * spread);

    draw_line(pixmap, start, base, color, width, transform);

    let mut builder = PathBuilder::new();
    builder.move_to(end.x as f32, end.y as f32);
    builder.line_to(left.x as f32, left.y as f32);
    builder.line_to(right.x as f32, right.y as f32);
    builder.close();
    if let Some(path) = builder.finish() {
        fill_path(pixmap, &path, color, transform);
    }
}

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("the render source rectangle is empty")]
    EmptySource,
    #[error("the render scale is invalid")]
    InvalidScale,
    #[error("the rendered image is too large")]
    TooLarge,
    #[error("the cached base layer has a different size than the requested render")]
    LayerSizeMismatch,
    #[error("rendered raster is invalid: {0}")]
    Raster(#[from] RasterError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Redaction, Shape, Stroke, Text};

    fn base(width: u32, height: u32, color: [u8; 4]) -> Document {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            rgba.extend_from_slice(&color);
        }
        Document::new(CapturedImage::new(width, height, rgba, None).unwrap()).unwrap()
    }

    fn pixel(image: &RenderedImage, x: u32, y: u32) -> [u8; 4] {
        let index = ((y * image.width() + x) * 4) as usize;
        let data = image.rgba();
        [
            data[index],
            data[index + 1],
            data[index + 2],
            data[index + 3],
        ]
    }

    fn add(doc: &mut Document, kind: AnnotationKind) -> AnnotationId {
        let annotation = doc.create_annotation(kind);
        let id = annotation.id;
        doc.insert_annotation(doc.annotations().len(), annotation);
        id
    }

    #[test]
    fn export_render_has_crop_dimensions() {
        let mut doc = base(100, 80, [255, 255, 255, 255]);
        doc.set_crop(snipunix_core::ImageRect::new(10, 20, 30, 40))
            .unwrap();
        let mut engine = RenderEngine::new();
        let image = engine
            .render(&doc, Preview::None, doc.crop_rect(), 1.0)
            .unwrap();
        assert_eq!(image.width(), 30);
        assert_eq!(image.height(), 40);
    }

    #[test]
    fn rectangle_pixels_are_drawn_at_image_coordinates() {
        let mut doc = base(50, 50, [255, 255, 255, 255]);
        add(
            &mut doc,
            AnnotationKind::Rectangle(Shape {
                rect: DocRect::new(10.0, 10.0, 20.0, 20.0),
                color: Color::rgb(255, 0, 0),
                width: 2.0,
            }),
        );
        let mut engine = RenderEngine::new();
        let image = engine
            .render(&doc, Preview::None, doc.crop_rect(), 1.0)
            .unwrap();
        let border = pixel(&image, 10, 10);
        assert!(border[0] > 200 && border[1] < 80);
        let inside = pixel(&image, 20, 20);
        assert!(inside[0] > 240 && inside[1] > 240);
    }

    #[test]
    fn highlighter_is_translucent() {
        let mut doc = base(40, 40, [255, 255, 255, 255]);
        add(
            &mut doc,
            AnnotationKind::Stroke(Stroke {
                points: vec![DocPoint::new(5.0, 20.0), DocPoint::new(35.0, 20.0)],
                color: Color::rgb(255, 230, 0),
                width: 12.0,
                highlighter: true,
                opacity: HIGHLIGHTER_ALPHA,
            }),
        );
        let mut engine = RenderEngine::new();
        let image = engine
            .render(&doc, Preview::None, doc.crop_rect(), 1.0)
            .unwrap();
        let highlighted = pixel(&image, 20, 20);
        assert!(highlighted[2] > 90, "blue channel should retain some white");
        assert!(highlighted[0] > 240);
    }

    #[test]
    fn arrow_has_a_filled_head() {
        let mut doc = base(80, 40, [255, 255, 255, 255]);
        add(
            &mut doc,
            AnnotationKind::Arrow(crate::document::Arrow {
                start: DocPoint::new(5.0, 20.0),
                end: DocPoint::new(70.0, 20.0),
                color: Color::rgb(0, 0, 0),
                width: 3.0,
            }),
        );
        let mut engine = RenderEngine::new();
        let image = engine
            .render(&doc, Preview::None, doc.crop_rect(), 1.0)
            .unwrap();
        let head = pixel(&image, 66, 20);
        assert!(head[0] < 40);
    }

    #[test]
    fn redaction_covers_its_rectangle_with_opaque_black() {
        let mut doc = base(60, 40, [40, 90, 200, 255]);
        add(
            &mut doc,
            AnnotationKind::Redaction(Redaction {
                rect: DocRect::new(10.0, 8.0, 20.0, 12.0),
            }),
        );
        let mut engine = RenderEngine::new();
        let image = engine
            .render(&doc, Preview::None, doc.crop_rect(), 1.0)
            .unwrap();
        assert_eq!(pixel(&image, 15, 12), [0, 0, 0, 255]);
        assert_eq!(pixel(&image, 29, 19), [0, 0, 0, 255]);
        assert_eq!(pixel(&image, 5, 5), [40, 90, 200, 255]);
        assert_eq!(pixel(&image, 40, 30), [40, 90, 200, 255]);
    }

    #[test]
    fn preview_is_visible_before_commit() {
        let mut doc = base(40, 40, [255, 255, 255, 255]);
        let annotation = doc.create_annotation(AnnotationKind::Ellipse(Shape {
            rect: DocRect::new(5.0, 5.0, 30.0, 30.0),
            color: Color::rgb(0, 0, 255),
            width: 3.0,
        }));
        let mut engine = RenderEngine::new();
        let image = engine
            .render(&doc, Preview::Add(&annotation), doc.crop_rect(), 1.0)
            .unwrap();
        let edge = pixel(&image, 5, 20);
        assert!(edge[2] > 180);
    }

    #[test]
    fn text_draws_pixels_and_measurement_is_positive() {
        let mut doc = base(240, 120, [255, 255, 255, 255]);
        add(
            &mut doc,
            AnnotationKind::Text(Text {
                origin: DocPoint::new(10.0, 10.0),
                width: 200.0,
                height: 40.0,
                text: "Hello\nSnipunix".into(),
                color: Color::rgb(0, 0, 0),
                font_size: 20.0,
            }),
        );
        let mut engine = RenderEngine::new();
        let (width, height) = engine.measure_text("Hello\nSnipunix", 20.0, 200.0);
        assert!(width > 20.0);
        assert!(height > 20.0);
        let image = engine
            .render(&doc, Preview::None, doc.crop_rect(), 1.0)
            .unwrap();
        let mut dark = false;
        for y in 10..80 {
            for x in 10..220 {
                let p = pixel(&image, x, y);
                if p[0] < 100 {
                    dark = true;
                }
            }
        }
        assert!(dark, "text should produce dark pixels");
    }

    #[test]
    fn text_pixels_match_between_screen_region_and_export() {
        let mut doc = base(260, 140, [255, 255, 255, 255]);
        add(
            &mut doc,
            AnnotationKind::Text(Text {
                origin: DocPoint::new(25.0, 20.0),
                width: 200.0,
                height: 60.0,
                text: "Parity\ncheck ✓".into(),
                color: Color::rgb(30, 30, 30),
                font_size: 22.0,
            }),
        );
        let mut engine = RenderEngine::new();
        let full = engine
            .render(&doc, Preview::None, doc.crop_rect(), 1.0)
            .unwrap();
        let region = engine
            .render(
                &doc,
                Preview::None,
                DocRect::new(40.0, 30.0, 80.0, 50.0),
                1.0,
            )
            .unwrap();
        for y in 0..region.height() {
            for x in 0..region.width() {
                let local = pixel(&region, x, y);
                let global = pixel(&full, x + 40, y + 30);
                for channel in 0..4 {
                    assert!(
                        local[channel].abs_diff(global[channel]) <= 2,
                        "text channel {channel} differs at {x},{y}: {local:?} vs {global:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn screen_region_matches_export_crop_pixels() {
        let mut doc = base(80, 60, [20, 120, 220, 255]);
        add(
            &mut doc,
            AnnotationKind::Ellipse(Shape {
                rect: DocRect::new(10.0, 10.0, 40.0, 30.0),
                color: Color::rgb(250, 10, 10),
                width: 4.0,
            }),
        );
        let mut engine = RenderEngine::new();
        let full = engine
            .render(&doc, Preview::None, doc.crop_rect(), 1.0)
            .unwrap();
        let region = engine
            .render(
                &doc,
                Preview::None,
                DocRect::new(20.0, 15.0, 30.0, 25.0),
                1.0,
            )
            .unwrap();
        let mut exact = 0_usize;
        let mut total = 0_usize;
        for y in 0..region.height() {
            for x in 0..region.width() {
                let local = pixel(&region, x, y);
                let global = pixel(&full, x + 20, y + 15);
                for channel in 0..4 {
                    total += 1;
                    if local[channel] == global[channel] {
                        exact += 1;
                    }
                    assert!(
                        local[channel].abs_diff(global[channel]) <= 40,
                        "channel {channel} differs by more than antialiasing tolerance at {x},{y}: {local:?} vs {global:?}"
                    );
                }
            }
        }
        // Vector anti-aliasing is subpixel-stable but tiny-skia's fixed-point
        // coverage can differ by a hair when the same path is translated. The
        // overwhelming majority of pixels must still be byte-identical.
        assert!(
            exact * 100 >= total * 98,
            "only {exact}/{total} channels matched"
        );
    }
}
