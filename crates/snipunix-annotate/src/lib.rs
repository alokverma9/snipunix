//! Annotation document model, edit history, and deterministic renderer.
//!
//! This crate is UI-independent. GTK owns input, presentation and file
//! dialogs; the document, undo history, view transform, rasteriser and export
//! encoders live here so the editor canvas and file export share one pipeline.

#![forbid(unsafe_code)]

pub mod color;
pub mod command;
pub mod document;
pub mod export;
pub mod geometry;
pub mod raster;
pub mod render;
pub mod view;

pub use color::Color;
pub use command::{Command, DEFAULT_HISTORY_LIMIT, EditError, Editor};
pub use document::{
    Annotation, AnnotationId, AnnotationKind, Arrow, Document, DocumentError, Line, MIN_CROP_SIZE,
    Shape, Stroke, Text,
};
pub use export::{DEFAULT_JPEG_QUALITY, ExportError, ExportFormat, encode, save};
pub use geometry::{DocPoint, DocRect, DocSize};
pub use raster::{RasterError, RenderedImage};
pub use render::{Preview, RenderEngine, RenderError};
pub use view::{
    FIT_MARGIN, MAX_ZOOM, MIN_ZOOM, ScreenPoint, ScreenRect, ScreenSize, ViewTransform,
};
