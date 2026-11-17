//! Reader for the GNU gettext binary catalogue format (`.mo`).
//!
//! The format is a small, stable, documented binary layout, so Snipunix reads
//! it directly instead of linking against the system `libintl`. That keeps the
//! runtime dependency-free and makes the parser testable.
//!
//! Layout, all integers little-endian unless the file says otherwise:
//!
//! ```text
//! offset  size  field
//!      0     4  magic (0x950412de, or byte-swapped 0xde120495)
//!      4     4  revision
//!      8     4  number of strings
//!     12     4  offset of the original-string table
//!     16     4  offset of the translation-string table
//!     20     4  size of the hash table (unused here)
//!     24     4  offset of the hash table (unused here)
//! ```
//!
//! Each table entry is a length and an offset. The original table holds the
//! `msgid`s, the translation table the matching `msgstr`s, in the same order.

use std::collections::HashMap;

use thiserror::Error;

/// Magic number of a little-endian catalogue.
const MAGIC_LITTLE_ENDIAN: u32 = 0x9504_12de;
/// Magic number of a big-endian catalogue.
const MAGIC_BIG_ENDIAN: u32 = 0xde12_0495;

/// Errors that can occur while reading a catalogue.
#[derive(Debug, Error)]
pub enum CatalogueError {
    /// The file does not start with the gettext magic number.
    #[error("not a gettext catalogue: bad magic number")]
    BadMagic,
    /// The file is shorter than the fixed header.
    #[error("truncated gettext catalogue header")]
    Truncated,
    /// A table entry points outside the file.
    #[error("gettext catalogue entry points outside the file")]
    OutOfBounds,
    /// The declared format version is not supported.
    #[error("unsupported gettext catalogue revision {0}")]
    UnsupportedRevision(u32),
}

/// A parsed message catalogue.
#[derive(Debug, Clone, Default)]
pub struct Catalogue {
    entries: HashMap<String, String>,
}

impl Catalogue {
    /// Parses a `.mo` file.
    ///
    /// # Errors
    ///
    /// Returns an error when the magic number, revision, or table offsets are
    /// not usable.
    pub fn parse(bytes: &[u8]) -> Result<Self, CatalogueError> {
        if bytes.len() < 28 {
            return Err(CatalogueError::Truncated);
        }
        let magic = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let big_endian = match magic {
            MAGIC_LITTLE_ENDIAN => false,
            MAGIC_BIG_ENDIAN => true,
            _ => return Err(CatalogueError::BadMagic),
        };
        let read_u32 = |offset: usize| -> u32 {
            let raw = [
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ];
            if big_endian {
                u32::from_be_bytes(raw)
            } else {
                u32::from_le_bytes(raw)
            }
        };

        let revision = read_u32(4);
        if revision > 1 {
            return Err(CatalogueError::UnsupportedRevision(revision));
        }
        let count = read_u32(8) as usize;
        let originals = read_u32(12) as usize;
        let translations = read_u32(16) as usize;

        let mut entries = HashMap::with_capacity(count);
        for index in 0..count {
            let original = read_string(bytes, originals, index, read_u32)?;
            let translated = read_string(bytes, translations, index, read_u32)?;
            // An empty translation means "not translated"; the caller keeps the
            // original string in that case.
            if !translated.is_empty() {
                entries.insert(original, translated);
            }
        }
        Ok(Self { entries })
    }

    /// Looks up a message.
    #[must_use]
    pub fn get(&self, msgid: &str) -> Option<&str> {
        self.entries.get(msgid).map(String::as_str)
    }

    /// Consumes the catalogue and returns its entries.
    #[must_use]
    pub fn into_entries(self) -> HashMap<String, String> {
        self.entries
    }

    /// Number of translated messages.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the catalogue holds no translations.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Reads one string out of a table.
fn read_string(
    bytes: &[u8],
    table_offset: usize,
    index: usize,
    read_u32: impl Fn(usize) -> u32,
) -> Result<String, CatalogueError> {
    let entry = table_offset + index * 8;
    if entry + 8 > bytes.len() {
        return Err(CatalogueError::OutOfBounds);
    }
    let length = read_u32(entry) as usize;
    let offset = read_u32(entry + 4) as usize;
    let end = offset
        .checked_add(length)
        .ok_or(CatalogueError::OutOfBounds)?;
    if end > bytes.len() {
        return Err(CatalogueError::OutOfBounds);
    }
    Ok(String::from_utf8_lossy(&bytes[offset..end]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a little-endian catalogue with the standard entry layout.
    fn build(entries: &[(&str, &str)]) -> Vec<u8> {
        let count = entries.len();
        let header = 28;
        let originals_table = header;
        let translations_table = originals_table + count * 8;
        let strings_start = translations_table + count * 8;

        let mut blobs = Vec::new();
        let mut original_offsets = Vec::new();
        let mut original_lengths = Vec::new();
        let mut translation_offsets = Vec::new();
        let mut translation_lengths = Vec::new();
        for (original, translation) in entries {
            original_offsets.push(strings_start + blobs.len());
            original_lengths.push(original.len());
            blobs.extend_from_slice(original.as_bytes());
            blobs.push(0);
            translation_offsets.push(strings_start + blobs.len());
            translation_lengths.push(translation.len());
            blobs.extend_from_slice(translation.as_bytes());
            blobs.push(0);
        }

        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC_LITTLE_ENDIAN.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(count as u32).to_le_bytes());
        out.extend_from_slice(&(originals_table as u32).to_le_bytes());
        out.extend_from_slice(&(translations_table as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());

        for (length, offset) in original_lengths.iter().zip(&original_offsets) {
            out.extend_from_slice(&(*length as u32).to_le_bytes());
            out.extend_from_slice(&(*offset as u32).to_le_bytes());
        }
        for (length, offset) in translation_lengths.iter().zip(&translation_offsets) {
            out.extend_from_slice(&(*length as u32).to_le_bytes());
            out.extend_from_slice(&(*offset as u32).to_le_bytes());
        }
        out.extend_from_slice(&blobs);
        out
    }

    #[test]
    fn parses_a_simple_catalogue() {
        let bytes = build(&[("Hello", "Hola"), ("Bye", "Adiós")]);
        let catalogue = Catalogue::parse(&bytes).unwrap();
        assert_eq!(catalogue.len(), 2);
        assert_eq!(catalogue.get("Hello"), Some("Hola"));
        assert_eq!(catalogue.get("Bye"), Some("Adiós"));
        assert_eq!(catalogue.get("Missing"), None);
    }

    #[test]
    fn rejects_bad_magic_and_truncated_files() {
        let mut bad_magic = vec![0u8; 28];
        bad_magic[0..4].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        assert!(matches!(
            Catalogue::parse(&bad_magic),
            Err(CatalogueError::BadMagic)
        ));
        assert!(matches!(
            Catalogue::parse(&[0xde, 0x12, 0x04, 0x95]),
            Err(CatalogueError::Truncated)
        ));
    }

    #[test]
    fn rejects_out_of_bounds_entries() {
        let mut bytes = build(&[("Hello", "Hola")]);
        // Point the first original entry past the end of the file.
        bytes[12 + 4..12 + 8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            Catalogue::parse(&bytes),
            Err(CatalogueError::OutOfBounds)
        ));
    }

    #[test]
    fn empty_translations_are_treated_as_untranslated() {
        let bytes = build(&[("Hello", "")]);
        let catalogue = Catalogue::parse(&bytes).unwrap();
        assert!(catalogue.is_empty());
        assert_eq!(catalogue.get("Hello"), None);
    }
}
