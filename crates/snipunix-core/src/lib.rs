//! Toolkit-independent logic for Snipunix.
//!
//! This crate deliberately has no GTK dependency so geometry, selection and
//! crop rules can be tested headlessly. Coordinate spaces are represented by
//! distinct types so that logical desktop pixels and captured image pixels
//! cannot be mixed accidentally.

#![forbid(unsafe_code)]

pub mod crop;
pub mod geometry;
pub mod layout;
pub mod mapping;
pub mod selection;

pub use crop::{CropError, crop_rgba};
pub use geometry::{LogicalPoint, LogicalRect, LogicalSize};
pub use layout::{DesktopLayout, LayoutError, Monitor};
pub use mapping::{CaptureMapping, ImageRect, ImageSize, MappingError};
pub use selection::{MIN_SELECTION_SIZE, Selection, SelectionPhase};
