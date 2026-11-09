//! Command history for the annotation document.
//!
//! Commands store only object data and crop rectangles. The full-resolution
//! captured image lives behind one shared `Arc` and is never copied into
//! history, so a long pointer stroke is one small command rather than one image
//! snapshot per event.

use snipunix_core::ImageRect;
use thiserror::Error;

use crate::document::{Annotation, AnnotationId, Document, DocumentError};
use crate::geometry::DocRect;

/// One reversible document mutation.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Add {
        index: usize,
        annotation: Annotation,
    },
    Remove {
        index: usize,
        annotation: Annotation,
    },
    Replace {
        index: usize,
        before: Annotation,
        after: Annotation,
    },
    SetCrop {
        before: ImageRect,
        after: ImageRect,
    },
    Compound(Vec<Command>),
}

impl Command {
    pub fn apply(&self, document: &mut Document) {
        match self {
            Self::Add { index, annotation } => {
                document.insert_annotation(*index, annotation.clone())
            }
            Self::Remove { index, .. } => {
                document.remove_annotation_at(*index);
            }
            Self::Replace { index, after, .. } => {
                document.replace_annotation(*index, after.clone());
            }
            Self::SetCrop { after, .. } => {
                let _ = document.set_crop(*after);
            }
            Self::Compound(commands) => {
                for command in commands {
                    command.apply(document);
                }
            }
        }
    }

    pub fn revert(&self, document: &mut Document) {
        match self {
            Self::Add { index, .. } => {
                document.remove_annotation_at(*index);
            }
            Self::Remove {
                index, annotation, ..
            } => document.insert_annotation(*index, annotation.clone()),
            Self::Replace { index, before, .. } => {
                document.replace_annotation(*index, before.clone());
            }
            Self::SetCrop { before, .. } => {
                let _ = document.set_crop(*before);
            }
            Self::Compound(commands) => {
                for command in commands.iter().rev() {
                    command.revert(document);
                }
            }
        }
    }
}

/// Default number of undo steps retained by the editor.
pub const DEFAULT_HISTORY_LIMIT: usize = 100;

/// A document plus its undo and redo command stacks.
#[derive(Debug, Clone)]
pub struct Editor {
    document: Document,
    undo: Vec<Command>,
    redo: Vec<Command>,
    limit: usize,
}

impl Editor {
    #[must_use]
    pub fn new(document: Document) -> Self {
        Self {
            document,
            undo: Vec::new(),
            redo: Vec::new(),
            limit: DEFAULT_HISTORY_LIMIT,
        }
    }

    #[must_use]
    pub fn with_history_limit(document: Document, limit: usize) -> Self {
        Self {
            document,
            undo: Vec::new(),
            redo: Vec::new(),
            limit: limit.max(1),
        }
    }

    #[must_use]
    pub fn document(&self) -> &Document {
        &self.document
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    #[must_use]
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    #[must_use]
    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    /// Creates an annotation with a document-stable identity without adding it
    /// to the history yet. Call [`Self::add_annotation`] when the gesture ends.
    pub fn create_annotation(&mut self, kind: crate::document::AnnotationKind) -> Annotation {
        self.document.create_annotation(kind)
    }

    /// Adds an annotation created by [`Document::create_annotation`].
    pub fn add_annotation(&mut self, annotation: Annotation) -> AnnotationId {
        let id = annotation.id;
        let index = self.document.annotations().len();
        self.execute(Command::Add { index, annotation });
        id
    }

    pub fn remove_annotation(&mut self, id: AnnotationId) -> Result<(), EditError> {
        let index = self
            .document
            .annotation_index(id)
            .ok_or(EditError::MissingAnnotation(id))?;
        let annotation = self.document.annotations()[index].clone();
        self.execute(Command::Remove { index, annotation });
        Ok(())
    }

    /// Removes several annotations as one undoable operation.
    pub fn remove_annotations(&mut self, ids: &[AnnotationId]) -> usize {
        let mut targets: Vec<(usize, Annotation)> = ids
            .iter()
            .filter_map(|id| {
                self.document
                    .annotation_index(*id)
                    .map(|index| (index, self.document.annotations()[index].clone()))
            })
            .collect();
        if targets.is_empty() {
            return 0;
        }
        targets.sort_by_key(|(index, _)| std::cmp::Reverse(*index));
        let commands = targets
            .into_iter()
            .map(|(index, annotation)| Command::Remove { index, annotation })
            .collect::<Vec<_>>();
        let removed = commands.len();
        self.execute(Command::Compound(commands));
        removed
    }

    /// Replaces an annotation with a new value, e.g. after moving, resizing,
    /// recolouring or editing text.
    pub fn replace_annotation(&mut self, after: Annotation) -> Result<(), EditError> {
        let index = self
            .document
            .annotation_index(after.id)
            .ok_or(EditError::MissingAnnotation(after.id))?;
        let before = self.document.annotations()[index].clone();
        if before == after {
            return Ok(());
        }
        self.execute(Command::Replace {
            index,
            before,
            after,
        });
        Ok(())
    }

    pub fn translate_annotation(&mut self, id: AnnotationId, dx: f64, dy: f64) -> bool {
        let Some(mut annotation) = self.document.annotation(id).cloned() else {
            return false;
        };
        if dx == 0.0 && dy == 0.0 {
            return false;
        }
        annotation.translate(dx, dy);
        self.replace_annotation(annotation).is_ok()
    }

    pub fn resize_annotation(&mut self, id: AnnotationId, bounds: DocRect) -> bool {
        let Some(mut annotation) = self.document.annotation(id).cloned() else {
            return false;
        };
        annotation.resize_to(bounds);
        self.replace_annotation(annotation).is_ok()
    }

    pub fn edit_text(&mut self, id: AnnotationId, text: String, height: f64) -> bool {
        let Some(annotation) = self.document.annotation(id).cloned() else {
            return false;
        };
        if !annotation.is_text() {
            return false;
        }
        self.replace_annotation(annotation.with_text(text, height))
            .is_ok()
    }

    pub fn set_crop(&mut self, rect: ImageRect) -> Result<(), EditError> {
        self.document
            .validate_crop(rect)
            .map_err(EditError::Document)?;
        let before = self.document.crop();
        if before == rect {
            return Ok(());
        }
        self.execute(Command::SetCrop {
            before,
            after: rect,
        });
        Ok(())
    }

    pub fn undo(&mut self) -> bool {
        let Some(command) = self.undo.pop() else {
            return false;
        };
        command.revert(&mut self.document);
        self.redo.push(command);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(command) = self.redo.pop() else {
            return false;
        };
        command.apply(&mut self.document);
        self.push_undo(command);
        true
    }

    fn execute(&mut self, command: Command) {
        command.apply(&mut self.document);
        self.redo.clear();
        self.push_undo(command);
    }

    fn push_undo(&mut self, command: Command) {
        self.undo.push(command);
        if self.undo.len() > self.limit {
            self.undo.remove(0);
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EditError {
    #[error("annotation {0:?} does not exist")]
    MissingAnnotation(AnnotationId),
    #[error(transparent)]
    Document(#[from] DocumentError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;
    use crate::document::{AnnotationKind, Shape};
    use crate::geometry::DocRect;
    use snipunix_capture::CapturedImage;

    fn editor() -> Editor {
        let image = CapturedImage::new(200, 120, vec![255; 200 * 120 * 4], None).unwrap();
        Editor::new(Document::new(image).unwrap())
    }

    fn rectangle(doc: &mut Document, x: f64, y: f64) -> AnnotationId {
        let annotation = doc.create_annotation(AnnotationKind::Rectangle(Shape {
            rect: DocRect::new(x, y, 20.0, 20.0),
            color: Color::rgb(255, 0, 0),
            width: 2.0,
        }));
        let id = annotation.id;
        doc.insert_annotation(doc.annotations().len(), annotation);
        id
    }

    #[test]
    fn add_undo_redo_round_trip_has_no_duplicates() {
        let mut editor = editor();
        let annotation = editor
            .document
            .create_annotation(AnnotationKind::Rectangle(Shape {
                rect: DocRect::new(10.0, 10.0, 20.0, 20.0),
                color: Color::rgb(255, 0, 0),
                width: 2.0,
            }));
        let id = editor.add_annotation(annotation);
        assert_eq!(editor.document().annotations().len(), 1);
        assert!(editor.undo());
        assert_eq!(editor.document().annotations().len(), 0);
        assert!(editor.redo());
        assert_eq!(editor.document().annotations().len(), 1);
        assert_eq!(editor.document().annotations()[0].id, id);
        assert_eq!(editor.document().annotations()[0].id, id);
    }

    #[test]
    fn new_operation_clears_redo() {
        let mut editor = editor();
        let annotation = editor
            .document
            .create_annotation(AnnotationKind::Rectangle(Shape {
                rect: DocRect::new(10.0, 10.0, 20.0, 20.0),
                color: Color::rgb(255, 0, 0),
                width: 2.0,
            }));
        editor.add_annotation(annotation);
        assert!(editor.undo());
        assert!(editor.can_redo());
        let annotation = editor
            .document
            .create_annotation(AnnotationKind::Rectangle(Shape {
                rect: DocRect::new(60.0, 10.0, 20.0, 20.0),
                color: Color::rgb(0, 0, 255),
                width: 2.0,
            }));
        editor.add_annotation(annotation);
        assert!(!editor.can_redo());
        assert_eq!(editor.document().annotations().len(), 1);
    }

    #[test]
    fn crop_is_undoable_and_annotations_keep_image_coordinates() {
        let mut editor = editor();
        let id = rectangle(&mut editor.document, 150.0, 100.0);
        editor.set_crop(ImageRect::new(10, 10, 50, 40)).unwrap();
        assert_eq!(editor.document().crop(), ImageRect::new(10, 10, 50, 40));
        assert_eq!(editor.document().annotation(id).unwrap().bounds().x, 149.0);
        assert!(editor.undo());
        assert_eq!(editor.document().crop(), ImageRect::new(0, 0, 200, 120));
        assert!(editor.redo());
        assert_eq!(editor.document().crop(), ImageRect::new(10, 10, 50, 40));
    }

    #[test]
    fn invalid_crops_are_rejected_without_history() {
        let mut editor = editor();
        assert!(editor.set_crop(ImageRect::new(0, 0, 0, 0)).is_err());
        assert_eq!(editor.undo_depth(), 0);
        assert_eq!(editor.document().crop(), ImageRect::new(0, 0, 200, 120));
    }

    #[test]
    fn compound_erase_is_one_undo_step() {
        let mut editor = editor();
        let a = rectangle(&mut editor.document, 10.0, 10.0);
        let b = rectangle(&mut editor.document, 40.0, 10.0);
        assert_eq!(editor.remove_annotations(&[a, b]), 2);
        assert_eq!(editor.document().annotations().len(), 0);
        assert_eq!(editor.undo_depth(), 1);
        assert!(editor.undo());
        assert_eq!(editor.document().annotations().len(), 2);
        assert!(editor.document().annotation(a).is_some());
        assert!(editor.document().annotation(b).is_some());
    }

    #[test]
    fn move_and_resize_are_individually_reversible() {
        let mut editor = editor();
        let id = rectangle(&mut editor.document, 10.0, 10.0);
        assert!(editor.translate_annotation(id, 15.0, 5.0));
        assert_eq!(
            editor.document().annotation(id).unwrap().bounds(),
            DocRect::new(24.0, 14.0, 22.0, 22.0)
        );
        assert!(editor.resize_annotation(id, DocRect::new(0.0, 0.0, 60.0, 40.0)));
        assert!(editor.undo());
        assert_eq!(
            editor.document().annotation(id).unwrap().bounds(),
            DocRect::new(24.0, 14.0, 22.0, 22.0)
        );
        assert!(editor.undo());
        assert_eq!(
            editor.document().annotation(id).unwrap().bounds(),
            DocRect::new(9.0, 9.0, 22.0, 22.0)
        );
    }

    #[test]
    fn text_edit_is_reversible() {
        let mut editor = editor();
        let annotation =
            editor
                .document
                .create_annotation(AnnotationKind::Text(crate::document::Text {
                    origin: DocPoint::new(10.0, 10.0),
                    width: 100.0,
                    height: 30.0,
                    text: "one".into(),
                    color: Color::rgb(0, 0, 0),
                    font_size: 18.0,
                }));
        let id = editor.add_annotation(annotation);
        assert!(editor.edit_text(id, "one\ntwo".into(), 45.0));
        if let AnnotationKind::Text(text) = &editor.document().annotation(id).unwrap().kind {
            assert_eq!(text.text, "one\ntwo");
            assert_eq!(text.height, 45.0);
        } else {
            panic!("expected text");
        }
        assert!(editor.undo());
        if let AnnotationKind::Text(text) = &editor.document().annotation(id).unwrap().kind {
            assert_eq!(text.text, "one");
        } else {
            panic!("expected text");
        }
    }

    use crate::geometry::DocPoint;

    #[test]
    fn history_limit_drops_old_commands() {
        let image = CapturedImage::new(50, 50, vec![255; 50 * 50 * 4], None).unwrap();
        let mut editor = Editor::with_history_limit(Document::new(image).unwrap(), 2);
        for index in 0..5 {
            let annotation = editor
                .document
                .create_annotation(AnnotationKind::Rectangle(Shape {
                    rect: DocRect::new(f64::from(index), 0.0, 5.0, 5.0),
                    color: Color::rgb(255, 0, 0),
                    width: 1.0,
                }));
            editor.add_annotation(annotation);
        }
        assert_eq!(editor.undo_depth(), 2);
        assert!(editor.undo());
        assert!(editor.undo());
        assert!(!editor.undo());
    }
}
