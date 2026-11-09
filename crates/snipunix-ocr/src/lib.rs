//! On-device text recognition, layout results, and redaction.
//!
//! The engine runs the `ocrs` text detection and recognition models. Nothing in
//! this crate touches the network except the explicit [`download_models`]
//! opt-in; images and recognised text stay in memory.

#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};

use ocrs::{ImageSource, OcrEngine, OcrEngineParams, TextItem};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Detection model file name.
pub const DETECTION_MODEL_FILE: &str = "text-detection.onnx";
/// Recognition model file name.
pub const RECOGNITION_MODEL_FILE: &str = "text-recognition.onnx";
/// SHA-256 of the pinned detection model.
pub const DETECTION_MODEL_SHA256: &str =
    "a917b23dbd9524b465df7e922641b3ff2981623df4ded5a0234004ef2fee7cfe";
/// SHA-256 of the pinned recognition model.
pub const RECOGNITION_MODEL_SHA256: &str =
    "86c145c2edb96c8caed5b1ebb8f44d706408922211451c309c625157dd6061c5";
/// Environment override for model discovery.
pub const MODEL_DIR_ENV: &str = "SNIPUNIX_OCR_MODEL_DIR";
/// Environment variable used by the model-gated OCR test.
pub const TEST_MODEL_DIR_ENV: &str = "SNIPUNIX_OCR_TEST_MODELS";

const DETECTION_MODEL_URL: &str =
    "https://ocrs-models.s3-accelerate.amazonaws.com/text-detection.onnx";
const RECOGNITION_MODEL_URL: &str =
    "https://ocrs-models.s3-accelerate.amazonaws.com/text-recognition.onnx";
const MODEL_DOWNLOAD_LIMIT: u64 = 64 * 1024 * 1024;

/// A rectangle in original-image pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OcrRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl OcrRect {
    #[must_use]
    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    #[must_use]
    pub fn right(self) -> u32 {
        self.x.saturating_add(self.width)
    }

    #[must_use]
    pub fn bottom(self) -> u32 {
        self.y.saturating_add(self.height)
    }

    #[must_use]
    pub fn contains(self, x: u32, y: u32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    #[must_use]
    pub fn union(self, other: Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Self::new(x, y, right.saturating_sub(x), bottom.saturating_sub(y))
    }

    #[must_use]
    pub fn expand(self, padding: u32) -> Self {
        Self::new(
            self.x.saturating_sub(padding),
            self.y.saturating_sub(padding),
            self.width.saturating_add(padding.saturating_mul(2)),
            self.height.saturating_add(padding.saturating_mul(2)),
        )
    }

    #[must_use]
    pub fn clamp(self, width: u32, height: u32) -> Self {
        let x = self.x.min(width);
        let y = self.y.min(height);
        let right = self.right().min(width);
        let bottom = self.bottom().min(height);
        Self::new(x, y, right.saturating_sub(x), bottom.saturating_sub(y))
    }
}

/// One recognised character and its box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrChar {
    pub character: char,
    pub rect: OcrRect,
}

/// One recognised word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrWord {
    pub text: String,
    pub rect: OcrRect,
}

/// One recognised text line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrLine {
    pub text: String,
    pub rect: OcrRect,
    pub words: Vec<OcrWord>,
    pub chars: Vec<OcrChar>,
}

impl OcrLine {
    fn rect_for_char_span(&self, start: usize, end: usize) -> Option<OcrRect> {
        self.chars
            .get(start..end)?
            .iter()
            .map(|character| character.rect)
            .reduce(OcrRect::union)
    }
}

/// Complete OCR output for one image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrResult {
    pub width: u32,
    pub height: u32,
    pub lines: Vec<OcrLine>,
}

impl OcrResult {
    /// True when no text was recognised.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Recognised text in reading order. When `remove_line_breaks` is true,
    /// lines are joined with a single space instead of a newline.
    #[must_use]
    pub fn text(&self, remove_line_breaks: bool) -> String {
        self.lines
            .iter()
            .map(|line| line.text.trim())
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(if remove_line_breaks { " " } else { "\n" })
    }

    /// Bounding boxes in image pixels.
    #[must_use]
    pub fn boxes(&self) -> Vec<OcrRect> {
        self.lines.iter().map(|line| line.rect).collect()
    }

    /// The line under an image-space point, if any.
    #[must_use]
    pub fn line_at(&self, x: u32, y: u32) -> Option<&OcrLine> {
        self.lines.iter().find(|line| line.rect.contains(x, y))
    }

    /// Emails and phone numbers found in the recognised lines.
    #[must_use]
    pub fn sensitive_matches(&self) -> Vec<SensitiveMatch> {
        let mut matches = Vec::new();
        for line in &self.lines {
            for (start, end, kind) in find_sensitive_spans(&line.text) {
                if let Some(rect) = line.rect_for_char_span(start, end) {
                    matches.push(SensitiveMatch {
                        kind,
                        text: line.text.chars().skip(start).take(end - start).collect(),
                        rect,
                    });
                }
            }
        }
        matches
    }

    /// Padded boxes that cover every detected email and phone number.
    #[must_use]
    pub fn redaction_rects(&self) -> Vec<OcrRect> {
        self.sensitive_matches()
            .into_iter()
            .map(|found| found.rect.expand(2).clamp(self.width, self.height))
            .filter(|rect| rect.width > 0 && rect.height > 0)
            .collect()
    }
}

/// The kind of sensitive text found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensitiveKind {
    Email,
    Phone,
}

impl SensitiveKind {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Email => "email",
            Self::Phone => "phone",
        }
    }
}

/// A sensitive span recognised in the image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensitiveMatch {
    pub kind: SensitiveKind,
    pub text: String,
    pub rect: OcrRect,
}

/// Model file locations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPaths {
    pub detection: PathBuf,
    pub recognition: PathBuf,
}

impl ModelPaths {
    /// Finds the pinned pair of model files.
    ///
    /// Search order: an explicit path, [`MODEL_DIR_ENV`], the configured model
    /// directory, XDG data directories, and the Flatpak `/app` prefix.
    pub fn discover(explicit: Option<&Path>) -> Result<Self, OcrError> {
        let mut searched = Vec::new();
        for directory in model_search_paths(explicit) {
            searched.push(directory.clone());
            let paths = Self {
                detection: directory.join(DETECTION_MODEL_FILE),
                recognition: directory.join(RECOGNITION_MODEL_FILE),
            };
            if paths.detection.is_file() && paths.recognition.is_file() {
                return Ok(paths);
            }
        }
        Err(OcrError::ModelsMissing { searched })
    }

    /// Directory overrides to show in diagnostics.
    #[must_use]
    pub fn search_paths(explicit: Option<&Path>) -> Vec<PathBuf> {
        model_search_paths(explicit)
    }
}

fn model_search_paths(explicit: Option<&Path>) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(directory) = explicit {
        directories.push(directory.to_path_buf());
    }
    if let Some(value) = std::env::var_os(MODEL_DIR_ENV) {
        directories.push(PathBuf::from(value));
    }
    let settings = snipunix_settings::Settings::default();
    directories.push(settings.model_directory());
    if let Some(value) = std::env::var_os("XDG_DATA_HOME") {
        let path = PathBuf::from(value);
        if path.is_absolute() {
            directories.push(path.join("snipunix/models"));
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        directories.push(PathBuf::from(home).join(".local/share/snipunix/models"));
    }
    if let Some(value) = std::env::var_os("XDG_DATA_DIRS") {
        for entry in std::env::split_paths(&value) {
            directories.push(entry.join("snipunix/models"));
        }
    } else {
        directories.push(PathBuf::from("/usr/local/share/snipunix/models"));
        directories.push(PathBuf::from("/usr/share/snipunix/models"));
    }
    directories.push(PathBuf::from("/app/share/snipunix/models"));
    directories.dedup();
    directories
}

/// Runs text detection and recognition over tightly packed RGBA8 pixels.
pub fn recognize(
    rgba: &[u8],
    width: u32,
    height: u32,
    models: &ModelPaths,
) -> Result<OcrResult, OcrError> {
    let expected = usize::try_from(width)
        .ok()
        .and_then(|w| usize::try_from(height).ok().map(|h| (w, h)))
        .and_then(|(w, h)| w.checked_mul(h))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(OcrError::ImageTooLarge)?;
    if rgba.len() != expected {
        return Err(OcrError::ImageSizeMismatch {
            expected,
            actual: rgba.len(),
        });
    }
    if width == 0 || height == 0 {
        return Err(OcrError::ImageTooLarge);
    }

    let detection = rten::Model::load_file(&models.detection)
        .map_err(|error| OcrError::Model(format!("detection model: {error}")))?;
    let recognition = rten::Model::load_file(&models.recognition)
        .map_err(|error| OcrError::Model(format!("recognition model: {error}")))?;
    let engine = OcrEngine::new(OcrEngineParams {
        detection_model: Some(detection),
        recognition_model: Some(recognition),
        ..Default::default()
    })
    .map_err(|error| OcrError::Engine(error.to_string()))?;

    let source = ImageSource::from_bytes(rgba, (width, height))
        .map_err(|error| OcrError::Engine(error.to_string()))?;
    let input = engine
        .prepare_input(source)
        .map_err(|error| OcrError::Engine(error.to_string()))?;
    let words = engine
        .detect_words(&input)
        .map_err(|error| OcrError::Engine(error.to_string()))?;
    let lines = engine.find_text_lines(&input, &words);
    let recognised = engine
        .recognize_text(&input, &lines)
        .map_err(|error| OcrError::Engine(error.to_string()))?;

    let mut result = Vec::new();
    for line in recognised.into_iter().flatten() {
        let text = line.to_string();
        if text.trim().is_empty() {
            continue;
        }
        let rect = rect_from(line.bounding_rect()).clamp(width, height);
        let chars = line
            .chars()
            .iter()
            .map(|character| OcrChar {
                character: character.char,
                rect: rect_from(character.rect).clamp(width, height),
            })
            .collect::<Vec<_>>();
        let words = line
            .words()
            .filter_map(|word| {
                let text = word.to_string();
                let rect = rect_from(word.bounding_rect()).clamp(width, height);
                (!text.trim().is_empty() && rect.width > 0 && rect.height > 0)
                    .then_some(OcrWord { text, rect })
            })
            .collect::<Vec<_>>();
        result.push(OcrLine {
            text,
            rect,
            words,
            chars,
        });
    }
    result.sort_by(|left, right| {
        left.rect
            .y
            .cmp(&right.rect.y)
            .then(left.rect.x.cmp(&right.rect.x))
    });
    Ok(OcrResult {
        width,
        height,
        lines: result,
    })
}

fn rect_from(rect: rten_imageproc::Rect) -> OcrRect {
    OcrRect::new(
        u32::try_from(rect.left().max(0)).unwrap_or(0),
        u32::try_from(rect.top().max(0)).unwrap_or(0),
        u32::try_from(rect.width().max(0)).unwrap_or(0),
        u32::try_from(rect.height().max(0)).unwrap_or(0),
    )
}

/// Finds email and phone spans in one line of text. Returned indices are
/// character indices, not byte offsets.
#[must_use]
pub fn find_sensitive_spans(text: &str) -> Vec<(usize, usize, SensitiveKind)> {
    let characters: Vec<char> = text.chars().collect();
    let mut spans = Vec::new();
    let mut index = 0;
    while index < characters.len() {
        if !is_token_char(characters[index]) {
            index += 1;
            continue;
        }
        let start = index;
        while index < characters.len() && is_token_char(characters[index]) {
            index += 1;
        }
        let token: String = characters[start..index].iter().collect();
        if is_email(&token) {
            spans.push((start, index, SensitiveKind::Email));
        }
    }

    // Phone numbers can contain spaces, so scan the raw line and map the byte
    // offsets back to character indices.
    let mut byte = 0;
    let mut run_start: Option<(usize, usize)> = None;
    let mut digits = 0;
    for (offset, character) in text.char_indices() {
        let allowed =
            character.is_ascii_digit() || matches!(character, '+' | '-' | '(' | ')' | '.' | ' ');
        if allowed {
            if run_start.is_none() {
                run_start = Some((offset, byte));
            }
            if character.is_ascii_digit() {
                digits += 1;
            }
        } else {
            if let Some((start_byte, _)) = run_start.take() {
                push_phone_span(text, start_byte, byte, digits, &mut spans);
            }
            digits = 0;
        }
        byte = offset + character.len_utf8();
    }
    if let Some((start_byte, _)) = run_start {
        push_phone_span(text, start_byte, byte, digits, &mut spans);
    }

    spans.sort_by_key(|(start, end, _)| (*start, *end));
    spans.dedup_by(|left, right| left.0 == right.0 && left.1 == right.1);
    spans
}

fn push_phone_span(
    text: &str,
    byte_start: usize,
    byte_end: usize,
    digits: usize,
    spans: &mut Vec<(usize, usize, SensitiveKind)>,
) {
    if digits < 7 {
        return;
    }
    let raw = &text[byte_start..byte_end];
    let trimmed = raw.trim_matches(|character: char| {
        character == ' ' || character == '.' || character == '-' || character == '+'
    });
    if trimmed.is_empty() {
        return;
    }
    let leading = raw.len() - raw.trim_start_matches(' ').len();
    let trailing = raw.len() - raw.trim_end_matches(' ').len();
    let start_byte = byte_start + leading;
    let end_byte = byte_end.saturating_sub(trailing).max(start_byte);
    let start = text[..start_byte].chars().count();
    let end = text[..end_byte].chars().count();
    let digit_count = text[start_byte..end_byte]
        .chars()
        .filter(char::is_ascii_digit)
        .count();
    if digit_count >= 7 {
        spans.push((start, end, SensitiveKind::Phone));
    }
}

fn is_token_char(character: char) -> bool {
    !character.is_whitespace()
        && !matches!(
            character,
            ',' | ';' | ':' | '!' | '?' | '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}'
        )
}

fn is_email(token: &str) -> bool {
    let token = token.trim_matches(|character: char| {
        matches!(
            character,
            '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '<' | '>'
        )
    });
    let Some((local, domain)) = token.split_once('@') else {
        return false;
    };
    if local.is_empty() || domain.is_empty() || domain.contains('@') {
        return false;
    }
    let Some((_, tld)) = domain.rsplit_once('.') else {
        return false;
    };
    local
        .chars()
        .all(|character| character.is_alphanumeric() || "._%+-".contains(character))
        && domain
            .chars()
            .all(|character| character.is_alphanumeric() || ".-".contains(character))
        && tld.len() >= 2
        && tld.chars().all(|character| character.is_alphabetic())
}

/// Fills every rectangle with opaque black, returning new RGBA pixels.
pub fn redact_rgba(
    rgba: &[u8],
    width: u32,
    height: u32,
    rectangles: &[OcrRect],
) -> Result<Vec<u8>, OcrError> {
    let expected = usize::try_from(width)
        .ok()
        .and_then(|w| usize::try_from(height).ok().map(|h| (w, h)))
        .and_then(|(w, h)| w.checked_mul(h))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(OcrError::ImageTooLarge)?;
    if rgba.len() != expected {
        return Err(OcrError::ImageSizeMismatch {
            expected,
            actual: rgba.len(),
        });
    }
    let mut out = rgba.to_vec();
    for rectangle in rectangles {
        let rectangle = rectangle.clamp(width, height);
        for y in rectangle.y..rectangle.bottom() {
            let row = usize::try_from(y).unwrap_or(0) * usize::try_from(width).unwrap_or(0) * 4;
            for x in rectangle.x..rectangle.right() {
                let offset = row + usize::try_from(x).unwrap_or(0) * 4;
                out[offset..offset + 4].copy_from_slice(&[0, 0, 0, 255]);
            }
        }
    }
    Ok(out)
}

/// Downloads and verifies both pinned models into `directory`.
///
/// This is the only function in the crate that uses the network. It is invoked
/// only by an explicit user action or an explicit CLI flag.
pub fn download_models(directory: &Path) -> Result<ModelPaths, OcrError> {
    fs::create_dir_all(directory).map_err(|source| OcrError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    let detection = directory.join(DETECTION_MODEL_FILE);
    let recognition = directory.join(RECOGNITION_MODEL_FILE);
    download_and_verify(DETECTION_MODEL_URL, &detection, DETECTION_MODEL_SHA256)?;
    download_and_verify(
        RECOGNITION_MODEL_URL,
        &recognition,
        RECOGNITION_MODEL_SHA256,
    )?;
    Ok(ModelPaths {
        detection,
        recognition,
    })
}

fn download_and_verify(url: &str, destination: &Path, expected: &str) -> Result<(), OcrError> {
    let mut response = ureq::get(url)
        .call()
        .map_err(|error| OcrError::Download(error.to_string()))?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(MODEL_DOWNLOAD_LIMIT)
        .read_to_vec()
        .map_err(|error| OcrError::Download(error.to_string()))?;
    verify_sha256(&bytes, expected)?;
    let temporary = destination.with_extension("onnx.part");
    fs::write(&temporary, &bytes).map_err(|source| OcrError::Io {
        path: temporary.clone(),
        source,
    })?;
    fs::rename(&temporary, destination).map_err(|source| OcrError::Io {
        path: destination.to_path_buf(),
        source,
    })?;
    Ok(())
}

/// Verifies a model payload before it is installed.
pub fn verify_sha256(bytes: &[u8], expected: &str) -> Result<(), OcrError> {
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(OcrError::HashMismatch {
            expected: expected.to_string(),
            actual,
        })
    }
}

/// OCR errors.
#[derive(Debug, Error)]
pub enum OcrError {
    #[error("OCR models are not installed; searched {searched:?}")]
    ModelsMissing { searched: Vec<PathBuf> },
    #[error("image is too large to address")]
    ImageTooLarge,
    #[error("image buffer has {actual} bytes, expected {expected}")]
    ImageSizeMismatch { expected: usize, actual: usize },
    #[error("could not load OCR model: {0}")]
    Model(String),
    #[error("OCR engine failed: {0}")]
    Engine(String),
    #[error("could not download OCR models: {0}")]
    Download(String),
    #[error("model checksum mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },
    #[error("could not access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, x: u32, y: u32, width: u32, height: u32) -> OcrLine {
        OcrLine {
            text: text.to_string(),
            rect: OcrRect::new(x, y, width, height),
            words: Vec::new(),
            chars: text
                .chars()
                .enumerate()
                .map(|(index, character)| OcrChar {
                    character,
                    rect: OcrRect::new(x + index as u32 * 8, y, 8, height),
                })
                .collect(),
        }
    }

    #[test]
    fn text_joins_lines_and_can_remove_breaks() {
        let result = OcrResult {
            width: 400,
            height: 200,
            lines: vec![
                line("Hello world", 0, 0, 100, 20),
                line("Second line", 0, 30, 90, 20),
            ],
        };
        assert_eq!(result.text(false), "Hello world\nSecond line");
        assert_eq!(result.text(true), "Hello world Second line");
        assert!(result.line_at(5, 35).is_some());
        assert!(result.line_at(5, 100).is_none());
    }

    #[test]
    fn rectangles_union_expand_and_clamp() {
        let a = OcrRect::new(10, 10, 20, 20);
        let b = OcrRect::new(40, 5, 10, 10);
        let union = a.union(b);
        assert_eq!(union, OcrRect::new(10, 5, 40, 25));
        assert_eq!(union.expand(3), OcrRect::new(7, 2, 46, 31));
        assert_eq!(union.clamp(30, 20), OcrRect::new(10, 5, 20, 15));
    }

    #[test]
    fn finds_emails_and_phone_numbers() {
        let found =
            find_sensitive_spans("Mail me at jane.doe+work@example.co.uk or +1 (555) 123-4567.");
        assert!(
            found
                .iter()
                .any(|(_, _, kind)| *kind == SensitiveKind::Email)
        );
        assert!(
            found
                .iter()
                .any(|(_, _, kind)| *kind == SensitiveKind::Phone)
        );
        assert!(find_sensitive_spans("No private data here").is_empty());
        assert!(find_sensitive_spans("version 1.2.3").is_empty());
    }

    #[test]
    fn redaction_boxes_cover_matches() {
        let result = OcrResult {
            width: 400,
            height: 200,
            lines: vec![line("email jane@example.com", 0, 0, 180, 20)],
        };
        let matches = result.sensitive_matches();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].text, "jane@example.com");
        let rects = result.redaction_rects();
        assert_eq!(rects.len(), 1);
        assert!(rects[0].x > 0);
    }

    #[test]
    fn redaction_paints_opaque_black() {
        let width = 4;
        let height = 2;
        let pixels = vec![255_u8; width * height * 4];
        let redacted = redact_rgba(
            &pixels,
            width as u32,
            height as u32,
            &[OcrRect::new(1, 0, 2, 2)],
        )
        .unwrap();
        for y in 0..2 {
            for x in 0..4 {
                let offset = (y * 4 + x) * 4;
                if (1..3).contains(&x) {
                    assert_eq!(&redacted[offset..offset + 4], &[0, 0, 0, 255]);
                } else {
                    assert_eq!(&redacted[offset..offset + 4], &[255, 255, 255, 255]);
                }
            }
        }
    }

    #[test]
    fn sha256_verification_rejects_tampering() {
        let bytes = b"trusted model";
        let expected = format!("{:x}", Sha256::digest(bytes));
        assert!(verify_sha256(bytes, &expected).is_ok());
        assert!(matches!(
            verify_sha256(b"tampered", &expected),
            Err(OcrError::HashMismatch { .. })
        ));
    }

    #[test]
    fn missing_models_report_search_paths() {
        let error =
            ModelPaths::discover(Some(Path::new("/definitely/not/a/model/path"))).unwrap_err();
        match error {
            OcrError::ModelsMissing { searched } => {
                assert_eq!(searched[0], PathBuf::from("/definitely/not/a/model/path"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn recognize_runs_against_local_models_when_available() {
        use snipunix_annotate::{
            AnnotationKind, Color, DocPoint, Document, Editor, Preview, RenderEngine, Text,
        };
        use snipunix_capture::CapturedImage;

        let Some(directory) = std::env::var_os(TEST_MODEL_DIR_ENV) else {
            eprintln!("{TEST_MODEL_DIR_ENV} is not set; skipping local model test");
            return;
        };
        let models = ModelPaths::discover(Some(Path::new(&directory))).unwrap();
        let background = CapturedImage::new(700, 180, vec![255; 700 * 180 * 4], None).unwrap();
        let document = Document::new(background).unwrap();
        let mut editor = Editor::new(document);
        let text = AnnotationKind::Text(Text {
            origin: DocPoint::new(40.0, 40.0),
            width: 620.0,
            height: 90.0,
            text: "Hello Snipunix 123".to_string(),
            color: Color::rgb(0, 0, 0),
            font_size: 56.0,
        });
        let annotation = editor.create_annotation(text);
        editor.add_annotation(annotation);
        let rendered = RenderEngine::new()
            .render(
                editor.document(),
                Preview::None,
                editor.document().crop_rect(),
                1.0,
            )
            .unwrap();
        let result = recognize(
            rendered.rgba(),
            rendered.width(),
            rendered.height(),
            &models,
        )
        .unwrap();
        let text = result.text(false).to_lowercase();
        assert!(text.contains("snipunix"), "recognised text: {text}");
        assert!(text.contains("123"), "recognised text: {text}");
    }

    #[test]
    fn redaction_removes_sensitive_text_from_a_second_ocr_pass() {
        use snipunix_annotate::{
            AnnotationKind, Color, DocPoint, Document, Editor, Preview, RenderEngine, Text,
        };
        use snipunix_capture::CapturedImage;

        let Some(directory) = std::env::var_os(TEST_MODEL_DIR_ENV) else {
            eprintln!("{TEST_MODEL_DIR_ENV} is not set; skipping local model test");
            return;
        };
        let models = ModelPaths::discover(Some(Path::new(&directory))).unwrap();
        let background = CapturedImage::new(900, 200, vec![255; 900 * 200 * 4], None).unwrap();
        let document = Document::new(background).unwrap();
        let mut editor = Editor::new(document);
        let text = AnnotationKind::Text(Text {
            origin: DocPoint::new(40.0, 50.0),
            width: 820.0,
            height: 100.0,
            text: "Contact alice@example.com".to_string(),
            color: Color::rgb(0, 0, 0),
            font_size: 54.0,
        });
        let annotation = editor.create_annotation(text);
        editor.add_annotation(annotation);
        let rendered = RenderEngine::new()
            .render(
                editor.document(),
                Preview::None,
                editor.document().crop_rect(),
                1.0,
            )
            .unwrap();
        let first = recognize(
            rendered.rgba(),
            rendered.width(),
            rendered.height(),
            &models,
        )
        .unwrap();
        let matches = first.sensitive_matches();
        assert!(
            matches
                .iter()
                .any(|found| found.kind == SensitiveKind::Email),
            "recognised text: {}",
            first.text(false)
        );
        let rects = first.redaction_rects();
        let redacted = redact_rgba(rendered.rgba(), rendered.width(), rendered.height(), &rects)
            .expect("valid redaction");
        let second = recognize(&redacted, rendered.width(), rendered.height(), &models).unwrap();
        let text = second.text(false);
        assert!(
            second.sensitive_matches().is_empty(),
            "sensitive text survived redaction: {text}"
        );
        assert!(!text.to_lowercase().contains("alice@example"), "{text}");
    }
}
