//! Non-destructive annotation document.
//!
//! The captured image is immutable and shared through an `Arc`. Annotations are
//! structured objects in original-image coordinates; crop is a rectangle over
//! the same image. Nothing here rasterises pixels, so the document stays cheap
//! to clone for render requests and cheap to undo.

use std::sync::Arc;

use snipunix_capture::CapturedImage;
use snipunix_core::ImageRect;
use thiserror::Error;

use crate::color::Color;
use crate::geometry::{DocPoint, DocRect, DocSize};

/// Stable identity for one annotation for the lifetime of a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AnnotationId(pub u64);

/// A freehand pen or highlighter stroke.
#[derive(Debug, Clone, PartialEq)]
pub struct Stroke {
    pub points: Vec<DocPoint>,
    pub color: Color,
    pub width: f64,
    /// Highlighter strokes use a translucent colour and a broad round cap.
    pub highlighter: bool,
    /// Blend opacity used when `highlighter` is true. Pen strokes ignore it and
    /// use the colour's own alpha.
    pub opacity: f64,
}

/// A straight line.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub start: DocPoint,
    pub end: DocPoint,
    pub color: Color,
    pub width: f64,
}

/// A line with a filled arrowhead at `end`.
#[derive(Debug, Clone, PartialEq)]
pub struct Arrow {
    pub start: DocPoint,
    pub end: DocPoint,
    pub color: Color,
    pub width: f64,
}

/// A stroked rectangle or ellipse.
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub rect: DocRect,
    pub color: Color,
    pub width: f64,
}

/// An opaque filled rectangle used to remove sensitive pixels from output.
#[derive(Debug, Clone, PartialEq)]
pub struct Redaction {
    pub rect: DocRect,
}

/// A multiline text box.
#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    pub origin: DocPoint,
    /// Wrap width in image pixels.
    pub width: f64,
    /// Approximate height in image pixels. Updated after the editor measures the
    /// laid-out text, and used for selection bounds.
    pub height: f64,
    pub text: String,
    pub color: Color,
    pub font_size: f64,
}

/// Structured annotation data. The enum is the canonical document model; no
/// tool paints directly into the captured image.
#[derive(Debug, Clone, PartialEq)]
pub enum AnnotationKind {
    Stroke(Stroke),
    Line(Line),
    Arrow(Arrow),
    Rectangle(Shape),
    Ellipse(Shape),
    Text(Text),
    /// Opaque black fill. Used by OCR quick-redact; the pixels really are
    /// removed from the rendered output, not merely covered by a translucent
    /// highlight.
    Redaction(Redaction),
}

/// A document object with stable identity and image-space geometry.
#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    pub id: AnnotationId,
    pub kind: AnnotationKind,
}

/// Smallest crop edge accepted by the editor, in image pixels.
pub const MIN_CROP_SIZE: f64 = 8.0;

impl Annotation {
    #[must_use]
    pub fn bounds(&self) -> DocRect {
        match &self.kind {
            AnnotationKind::Stroke(stroke) => {
                let Some(points) = DocRect::from_points(stroke.points.iter().copied()) else {
                    return DocRect::new(0.0, 0.0, 0.0, 0.0);
                };
                points.expand(stroke.width.max(1.0) / 2.0)
            }
            AnnotationKind::Line(line) => {
                DocRect::from_corners(line.start, line.end).expand(line.width.max(1.0) / 2.0)
            }
            AnnotationKind::Arrow(arrow) => {
                let head = arrow_head_length(arrow.width);
                DocRect::from_corners(arrow.start, arrow.end).expand(head)
            }
            AnnotationKind::Rectangle(shape) | AnnotationKind::Ellipse(shape) => {
                shape.rect.expand(shape.width.max(1.0) / 2.0)
            }
            AnnotationKind::Text(text) => DocRect::new(
                text.origin.x,
                text.origin.y,
                text.width.max(1.0),
                text.height.max(text.font_size),
            ),
            AnnotationKind::Redaction(redaction) => redaction.rect,
        }
    }

    pub fn translate(&mut self, dx: f64, dy: f64) {
        match &mut self.kind {
            AnnotationKind::Stroke(stroke) => {
                for point in &mut stroke.points {
                    point.x += dx;
                    point.y += dy;
                }
            }
            AnnotationKind::Line(line) => {
                line.start.x += dx;
                line.start.y += dy;
                line.end.x += dx;
                line.end.y += dy;
            }
            AnnotationKind::Arrow(arrow) => {
                arrow.start.x += dx;
                arrow.start.y += dy;
                arrow.end.x += dx;
                arrow.end.y += dy;
            }
            AnnotationKind::Rectangle(shape) | AnnotationKind::Ellipse(shape) => {
                shape.rect = shape.rect.translate(dx, dy);
            }
            AnnotationKind::Text(text) => {
                text.origin.x += dx;
                text.origin.y += dy;
            }
            AnnotationKind::Redaction(redaction) => {
                redaction.rect = redaction.rect.translate(dx, dy);
            }
        }
    }

    /// Resizes the object so its outer bounds become `to`.
    pub fn resize_to(&mut self, to: DocRect) {
        let from = self.bounds();
        if to.is_empty() {
            return;
        }
        match &mut self.kind {
            AnnotationKind::Stroke(stroke) => {
                let inset = stroke.width.max(1.0) / 2.0;
                let from_inner = from.expand(-inset);
                let to_inner = to.expand(-inset);
                if from_inner.width <= f64::EPSILON || from_inner.height <= f64::EPSILON {
                    let dx = to_inner.x - from_inner.x;
                    let dy = to_inner.y - from_inner.y;
                    for point in &mut stroke.points {
                        point.x += dx;
                        point.y += dy;
                    }
                    return;
                }
                for point in &mut stroke.points {
                    let mapped = DocPoint::new(point.x, point.y);
                    let x = to_inner.x
                        + (mapped.x - from_inner.x) * (to_inner.width / from_inner.width);
                    let y = to_inner.y
                        + (mapped.y - from_inner.y) * (to_inner.height / from_inner.height);
                    *point = DocPoint::new(x, y);
                }
            }
            AnnotationKind::Line(line) => {
                let inset = line.width.max(1.0) / 2.0;
                let (start, end) =
                    remap_line(line.start, line.end, from.expand(-inset), to.expand(-inset));
                line.start = start;
                line.end = end;
            }
            AnnotationKind::Arrow(arrow) => {
                let inset = arrow_head_length(arrow.width);
                let (start, end) = remap_line(
                    arrow.start,
                    arrow.end,
                    from.expand(-inset),
                    to.expand(-inset),
                );
                arrow.start = start;
                arrow.end = end;
            }
            AnnotationKind::Rectangle(shape) | AnnotationKind::Ellipse(shape) => {
                let inner = to.expand(-(shape.width.max(1.0) / 2.0));
                shape.rect = if inner.width > 0.0 && inner.height > 0.0 {
                    inner
                } else {
                    to
                };
            }
            AnnotationKind::Text(text) => {
                text.origin = DocPoint::new(to.x, to.y);
                text.width = to.width.max(text.font_size * 2.0);
                text.height = to.height.max(text.font_size);
            }
            AnnotationKind::Redaction(redaction) => {
                redaction.rect = to;
            }
        }
    }

    /// Hit-tests selection and eraser clicks. Strokes and lines use their
    /// geometry; filled/text objects use their bounds.
    #[must_use]
    pub fn hit_test(&self, point: DocPoint, tolerance: f64) -> bool {
        match &self.kind {
            AnnotationKind::Stroke(stroke) => {
                let reach = stroke.width.max(1.0) / 2.0 + tolerance;
                if stroke.points.len() == 1 {
                    return stroke.points[0].distance(point) <= reach;
                }
                stroke
                    .points
                    .windows(2)
                    .any(|pair| distance_to_segment(point, pair[0], pair[1]) <= reach)
            }
            AnnotationKind::Line(line) => {
                distance_to_segment(point, line.start, line.end)
                    <= line.width.max(1.0) / 2.0 + tolerance
            }
            AnnotationKind::Arrow(arrow) => {
                distance_to_segment(point, arrow.start, arrow.end)
                    <= arrow.width.max(1.0) / 2.0 + tolerance
                    || self.bounds().expand(tolerance).contains(point)
            }
            AnnotationKind::Rectangle(shape) | AnnotationKind::Ellipse(shape) => shape
                .rect
                .expand(shape.width.max(1.0) / 2.0 + tolerance)
                .contains(point),
            AnnotationKind::Text(_) => self.bounds().expand(tolerance).contains(point),
            AnnotationKind::Redaction(redaction) => {
                redaction.rect.expand(tolerance).contains(point)
            }
        }
    }

    #[must_use]
    pub fn color(&self) -> Color {
        match &self.kind {
            AnnotationKind::Stroke(stroke) => stroke.color,
            AnnotationKind::Line(line) => line.color,
            AnnotationKind::Arrow(arrow) => arrow.color,
            AnnotationKind::Rectangle(shape) | AnnotationKind::Ellipse(shape) => shape.color,
            AnnotationKind::Text(text) => text.color,
            AnnotationKind::Redaction(_) => Color::rgb(0, 0, 0),
        }
    }

    #[must_use]
    pub fn with_color(&self, color: Color) -> Self {
        let mut next = self.clone();
        match &mut next.kind {
            AnnotationKind::Stroke(stroke) => stroke.color = color,
            AnnotationKind::Line(line) => line.color = color,
            AnnotationKind::Arrow(arrow) => arrow.color = color,
            AnnotationKind::Rectangle(shape) | AnnotationKind::Ellipse(shape) => {
                shape.color = color;
            }
            AnnotationKind::Text(text) => text.color = color,
            AnnotationKind::Redaction(_) => {}
        }
        next
    }

    #[must_use]
    pub fn with_stroke_width(&self, width: f64) -> Self {
        let width = width.clamp(0.5, 128.0);
        let mut next = self.clone();
        match &mut next.kind {
            AnnotationKind::Stroke(stroke) => stroke.width = width,
            AnnotationKind::Line(line) => line.width = width,
            AnnotationKind::Arrow(arrow) => arrow.width = width,
            AnnotationKind::Rectangle(shape) | AnnotationKind::Ellipse(shape) => {
                shape.width = width;
            }
            AnnotationKind::Text(text) => text.font_size = width.clamp(6.0, 256.0),
            AnnotationKind::Redaction(_) => {}
        }
        next
    }

    #[must_use]
    pub fn with_highlighter_opacity(&self, opacity: f64) -> Self {
        let mut next = self.clone();
        if let AnnotationKind::Stroke(stroke) = &mut next.kind {
            stroke.opacity = opacity.clamp(0.0, 1.0);
        }
        next
    }

    #[must_use]
    pub fn with_text(&self, value: String, height: f64) -> Self {
        let mut next = self.clone();
        if let AnnotationKind::Text(text) = &mut next.kind {
            text.text = value;
            text.height = height.max(text.font_size);
        }
        next
    }

    #[must_use]
    pub fn is_text(&self) -> bool {
        matches!(self.kind, AnnotationKind::Text(_))
    }
}

fn remap_line(start: DocPoint, end: DocPoint, from: DocRect, to: DocRect) -> (DocPoint, DocPoint) {
    if from.width <= f64::EPSILON || from.height <= f64::EPSILON {
        let dx = to.x - from.x;
        let dy = to.y - from.y;
        return (
            DocPoint::new(start.x + dx, start.y + dy),
            DocPoint::new(end.x + dx, end.y + dy),
        );
    }
    let sx = to.width / from.width;
    let sy = to.height / from.height;
    (
        DocPoint::new(
            to.x + (start.x - from.x) * sx,
            to.y + (start.y - from.y) * sy,
        ),
        DocPoint::new(to.x + (end.x - from.x) * sx, to.y + (end.y - from.y) * sy),
    )
}

fn distance_to_segment(point: DocPoint, a: DocPoint, b: DocPoint) -> f64 {
    let vx = b.x - a.x;
    let vy = b.y - a.y;
    let len_sq = vx * vx + vy * vy;
    if len_sq <= f64::EPSILON {
        return point.distance(a);
    }
    let t = (((point.x - a.x) * vx + (point.y - a.y) * vy) / len_sq).clamp(0.0, 1.0);
    let nearest = DocPoint::new(a.x + vx * t, a.y + vy * t);
    point.distance(nearest)
}

#[must_use]
pub fn arrow_head_length(width: f64) -> f64 {
    (width.max(1.0) * 4.0).clamp(9.0, 42.0)
}

/// Immutable captured image plus crop, ordered annotations and identity counter.
#[derive(Debug, Clone)]
pub struct Document {
    base: Arc<CapturedImage>,
    crop: ImageRect,
    annotations: Vec<Annotation>,
    next_id: u64,
}

impl Document {
    pub fn new(base: CapturedImage) -> Result<Self, DocumentError> {
        let width = base.width();
        let height = base.height();
        if width == 0 || height == 0 {
            return Err(DocumentError::EmptyImage);
        }
        Ok(Self {
            base: Arc::new(base),
            crop: ImageRect::new(0, 0, width, height),
            annotations: Vec::new(),
            next_id: 1,
        })
    }

    #[must_use]
    pub fn base(&self) -> &Arc<CapturedImage> {
        &self.base
    }

    #[must_use]
    pub fn base_bounds(&self) -> DocRect {
        DocRect::new(
            0.0,
            0.0,
            f64::from(self.base.width()),
            f64::from(self.base.height()),
        )
    }

    #[must_use]
    pub fn crop(&self) -> ImageRect {
        self.crop
    }

    #[must_use]
    pub fn crop_rect(&self) -> DocRect {
        DocRect::new(
            f64::from(self.crop.x),
            f64::from(self.crop.y),
            f64::from(self.crop.width),
            f64::from(self.crop.height),
        )
    }

    #[must_use]
    pub fn viewport_size(&self) -> DocSize {
        DocSize::new(f64::from(self.crop.width), f64::from(self.crop.height))
    }

    pub fn set_crop(&mut self, rect: ImageRect) -> Result<(), DocumentError> {
        self.validate_crop(rect)?;
        self.crop = rect;
        Ok(())
    }

    pub fn validate_crop(&self, rect: ImageRect) -> Result<(), DocumentError> {
        let right = u64::from(rect.x) + u64::from(rect.width);
        let bottom = u64::from(rect.y) + u64::from(rect.height);
        if rect.width == 0 || rect.height == 0 {
            return Err(DocumentError::EmptyCrop);
        }
        if f64::from(rect.width) < MIN_CROP_SIZE || f64::from(rect.height) < MIN_CROP_SIZE {
            return Err(DocumentError::CropTooSmall);
        }
        if right > u64::from(self.base.width()) || bottom > u64::from(self.base.height()) {
            return Err(DocumentError::CropOutOfBounds);
        }
        Ok(())
    }

    #[must_use]
    pub fn annotations(&self) -> &[Annotation] {
        &self.annotations
    }

    #[must_use]
    pub fn annotation(&self, id: AnnotationId) -> Option<&Annotation> {
        self.annotations
            .iter()
            .find(|annotation| annotation.id == id)
    }

    #[must_use]
    pub fn annotation_index(&self, id: AnnotationId) -> Option<usize> {
        self.annotations
            .iter()
            .position(|annotation| annotation.id == id)
    }

    /// Returns the topmost annotation under `point`.
    #[must_use]
    pub fn find_topmost(&self, point: DocPoint, tolerance: f64) -> Option<&Annotation> {
        self.annotations
            .iter()
            .rev()
            .find(|annotation| annotation.hit_test(point, tolerance))
    }

    /// Creates an annotation with a fresh stable identity. The caller commits it
    /// with [`crate::command::Editor::add_annotation`].
    pub fn create_annotation(&mut self, kind: AnnotationKind) -> Annotation {
        let id = AnnotationId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        Annotation { id, kind }
    }

    pub(crate) fn insert_annotation(&mut self, index: usize, annotation: Annotation) {
        let index = index.min(self.annotations.len());
        self.annotations.insert(index, annotation);
    }

    pub(crate) fn remove_annotation_at(&mut self, index: usize) -> Option<Annotation> {
        (index < self.annotations.len()).then(|| self.annotations.remove(index))
    }

    pub(crate) fn replace_annotation(&mut self, index: usize, annotation: Annotation) -> bool {
        if let Some(slot) = self.annotations.get_mut(index) {
            *slot = annotation;
            true
        } else {
            false
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DocumentError {
    #[error("the captured image has a zero dimension")]
    EmptyImage,
    #[error("the crop rectangle is empty")]
    EmptyCrop,
    #[error("the crop rectangle is smaller than {MIN_CROP_SIZE} pixels")]
    CropTooSmall,
    #[error("the crop rectangle is outside the captured image")]
    CropOutOfBounds,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32) -> CapturedImage {
        CapturedImage::new(
            width,
            height,
            vec![128; (width * height * 4) as usize],
            None,
        )
        .expect("valid image")
    }

    fn stroke(points: &[(f64, f64)]) -> AnnotationKind {
        AnnotationKind::Stroke(Stroke {
            points: points.iter().map(|&(x, y)| DocPoint::new(x, y)).collect(),
            color: Color::rgb(255, 0, 0),
            width: 4.0,
            highlighter: false,
            opacity: 1.0,
        })
    }

    #[test]
    fn identities_are_stable_and_monotonic() {
        let mut doc = Document::new(image(100, 80)).unwrap();
        let a = doc.create_annotation(stroke(&[(10.0, 10.0), (20.0, 20.0)]));
        let b = doc.create_annotation(stroke(&[(30.0, 30.0), (40.0, 40.0)]));
        assert_ne!(a.id, b.id);
        assert_eq!(a.id.0 + 1, b.id.0);
    }

    #[test]
    fn bounds_include_stroke_width() {
        let annotation = Annotation {
            id: AnnotationId(1),
            kind: stroke(&[(10.0, 10.0), (20.0, 30.0)]),
        };
        assert_eq!(annotation.bounds(), DocRect::new(8.0, 8.0, 14.0, 24.0));
    }

    #[test]
    fn moving_preserves_shape_and_identity() {
        let mut annotation = Annotation {
            id: AnnotationId(9),
            kind: AnnotationKind::Rectangle(Shape {
                rect: DocRect::new(10.0, 20.0, 30.0, 40.0),
                color: Color::rgb(0, 0, 255),
                width: 2.0,
            }),
        };
        annotation.translate(5.0, -5.0);
        assert_eq!(annotation.id, AnnotationId(9));
        if let AnnotationKind::Rectangle(shape) = annotation.kind {
            assert_eq!(shape.rect, DocRect::new(15.0, 15.0, 30.0, 40.0));
        } else {
            panic!("expected rectangle");
        }
    }

    #[test]
    fn resize_remaps_stroke_geometry() {
        let mut annotation = Annotation {
            id: AnnotationId(1),
            kind: stroke(&[(10.0, 10.0), (20.0, 20.0)]),
        };
        let before = annotation.bounds();
        annotation.resize_to(DocRect::new(0.0, 0.0, 40.0, 40.0));
        let after = annotation.bounds();
        assert!((after.width - 40.0).abs() < 0.01);
        assert!((after.height - 40.0).abs() < 0.01);
        assert_ne!(before.width, after.width);
    }

    #[test]
    fn hit_testing_uses_stroke_geometry() {
        let annotation = Annotation {
            id: AnnotationId(1),
            kind: stroke(&[(0.0, 0.0), (100.0, 0.0)]),
        };
        assert!(annotation.hit_test(DocPoint::new(50.0, 1.0), 1.0));
        assert!(!annotation.hit_test(DocPoint::new(50.0, 20.0), 1.0));
    }

    #[test]
    fn redaction_bounds_move_resize_and_hit_test() {
        let mut annotation = Annotation {
            id: AnnotationId(11),
            kind: AnnotationKind::Redaction(Redaction {
                rect: DocRect::new(10.0, 20.0, 30.0, 40.0),
            }),
        };
        assert_eq!(annotation.bounds(), DocRect::new(10.0, 20.0, 30.0, 40.0));
        assert_eq!(annotation.color(), Color::rgb(0, 0, 0));
        assert!(annotation.hit_test(DocPoint::new(20.0, 30.0), 0.0));
        assert!(!annotation.hit_test(DocPoint::new(80.0, 80.0), 0.0));
        annotation.translate(5.0, -5.0);
        assert_eq!(annotation.bounds(), DocRect::new(15.0, 15.0, 30.0, 40.0));
        annotation.resize_to(DocRect::new(0.0, 0.0, 60.0, 30.0));
        assert_eq!(annotation.bounds(), DocRect::new(0.0, 0.0, 60.0, 30.0));
    }

    #[test]
    fn crop_validation_rejects_invalid_rectangles() {
        let mut doc = Document::new(image(100, 80)).unwrap();
        assert_eq!(
            doc.set_crop(ImageRect::new(0, 0, 0, 10)),
            Err(DocumentError::EmptyCrop)
        );
        assert_eq!(
            doc.set_crop(ImageRect::new(0, 0, 4, 4)),
            Err(DocumentError::CropTooSmall)
        );
        assert_eq!(
            doc.set_crop(ImageRect::new(90, 70, 20, 20)),
            Err(DocumentError::CropOutOfBounds)
        );
        assert!(doc.set_crop(ImageRect::new(10, 10, 50, 40)).is_ok());
        assert_eq!(doc.crop(), ImageRect::new(10, 10, 50, 40));
    }

    #[test]
    fn empty_images_are_rejected() {
        assert_eq!(
            Document::new(image(0, 10)).unwrap_err(),
            DocumentError::EmptyImage
        );
    }
}
