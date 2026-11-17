//! Gettext-style message catalogue runtime for Snipunix.
//!
//! # Design
//!
//! Snipunix keeps its translatable strings in `po/<lang>.po` files that follow
//! the GNU gettext conventions (`msgid` is the English source string,
//! `msgstr` is the translation). The compiled catalogues are stored next to
//! them as `.mo` files in the standard binary format and are embedded into
//! the binary at build time, so the application ships with every translation
//! it supports and never needs a locale directory on disk.
//!
//! Because the catalogues are embedded, the lookup is a plain hash map and
//! the application starts in the same state on every machine.
//!
//! # Usage
//!
//! ```no_run
//! use snipunix_i18n::{tr, tr_fmt};
//!
//! let button = tr!("Capture Region");
//! let message = tr_fmt!("Wrote {0}", "capture.png");
//! ```
//!
//! `tr!` returns the translated string, or the original text when no
//! translation exists. `tr_fmt!` substitutes `{0}`-style placeholders in the
//! translated template with the formatted arguments.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::OnceLock;

mod mo;

pub use mo::{Catalogue, CatalogueError};

/// Translations embedded at build time, one entry per supported locale.
mod catalogues {
    include!(concat!(env!("OUT_DIR"), "/catalogues.rs"));
}

/// Environment variables that can select a locale, in gettext's own order.
const LOCALE_VARIABLES: [&str; 4] = ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"];

/// A loaded translation catalogue.
#[derive(Debug, Clone, Default)]
struct Translations {
    /// Locale name the catalogue was loaded for, if any.
    locale: Option<String>,
    /// Message translations.
    entries: HashMap<String, String>,
}

static TRANSLATIONS: OnceLock<Translations> = OnceLock::new();

/// Returns the translation for `msgid`, or `msgid` itself when the message is
/// not translated.
///
/// This is the runtime behind the [`tr!`] macro. Prefer the macro so the call
/// site stays readable.
#[must_use]
pub fn translate(msgid: &str) -> String {
    let translations = TRANSLATIONS.get_or_init(load_translations);
    translations
        .entries
        .get(msgid)
        .cloned()
        .unwrap_or_else(|| msgid.to_string())
}

/// Translates `msgid` and substitutes `{0}`-style placeholders with `values`.
///
/// Placeholders are filled from left to right. A placeholder whose index is
/// out of range is left untouched so a mistranslation cannot silently drop
/// text.
#[must_use]
pub fn fill_placeholders(template: &str, values: &[String]) -> String {
    let mut output = String::with_capacity(template.len());
    let mut index = 0usize;
    let mut chars = template.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                output.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                output.push('}');
            }
            '{' => {
                let mut placeholder = String::new();
                let mut closed = false;
                for inner in chars.by_ref() {
                    if inner == '}' {
                        closed = true;
                        break;
                    }
                    placeholder.push(inner);
                }
                if !closed {
                    output.push('{');
                    output.push_str(&placeholder);
                    continue;
                }
                match values.get(index) {
                    Some(value) => output.push_str(value),
                    None => {
                        output.push('{');
                        output.push_str(&placeholder);
                        output.push('}');
                    }
                }
                index += 1;
            }
            other => output.push(other),
        }
    }
    output
}

/// Translates a message.
///
/// The macro returns a `String`. Use it wherever a `&str` is expected by
/// borrowing the result.
#[macro_export]
macro_rules! tr {
    ($msgid:expr) => {
        $crate::translate($msgid)
    };
}

/// Translates a message and substitutes `{0}`-style placeholders.
///
/// Each argument is formatted with [`ToString`]. Pass an already formatted
/// value when a specific format is needed, for example
/// `tr_fmt!("Wrote {0}", format!("{:?}", path))`.
#[macro_export]
macro_rules! tr_fmt {
    ($msgid:expr) => {
        $crate::translate($msgid)
    };
    ($msgid:expr, $($argument:expr),+ $(,)?) => {{
        let template = $crate::translate($msgid);
        let values: Vec<String> = vec![$($crate::ToText::to_text(&$argument)),+];
        $crate::fill_placeholders(&template, &values)
    }};
}

/// Helper trait so `tr_fmt!` can format arguments of any `Display` type.
pub trait ToText {
    /// Renders the value as text.
    fn to_text(&self) -> String;
}

impl<T: std::fmt::Display> ToText for T {
    fn to_text(&self) -> String {
        self.to_string()
    }
}

/// Loads the catalogue for the current locale.
fn load_translations() -> Translations {
    let requested = requested_locales();
    for candidate in &requested {
        if let Some((locale, catalogue)) = load_catalogue(candidate) {
            return Translations {
                locale: Some(locale),
                entries: catalogue.into_entries(),
            };
        }
    }
    Translations::default()
}

/// Loads the catalogue for one exact locale name.
fn load_catalogue(locale: &str) -> Option<(String, Catalogue)> {
    for (name, bytes) in catalogues::CATALOGUES {
        if *name == locale {
            return match Catalogue::parse(bytes) {
                Ok(catalogue) => Some(((*name).to_string(), catalogue)),
                Err(error) => {
                    tracing_warn(&format!("could not read the {name} catalogue: {error}"));
                    None
                }
            };
        }
    }
    None
}

/// Logs a warning without pulling in a logging dependency.
fn tracing_warn(message: &str) {
    eprintln!("snipunix-i18n: {message}");
}

/// The locale names to try, most specific first.
///
/// The list follows gettext: `LANGUAGE` wins over `LC_ALL`, which wins over
/// `LC_MESSAGES`, which wins over `LANG`. Each value may hold several
/// colon-separated locales.
#[must_use]
pub fn requested_locales() -> Vec<String> {
    let mut candidates = Vec::new();
    for variable in LOCALE_VARIABLES {
        let Ok(value) = std::env::var(variable) else {
            continue;
        };
        for entry in value.split(':') {
            let trimmed = entry.trim();
            if trimmed.is_empty() {
                continue;
            }
            for name in locale_candidates(trimmed) {
                candidates.push(name);
            }
        }
        if !candidates.is_empty() {
            break;
        }
    }
    candidates
}

/// Expands one locale string into the names gettext would try.
///
/// `de_DE.UTF-8@euro` becomes `de_DE` and then `de`.
#[must_use]
pub fn locale_candidates(value: &str) -> Vec<String> {
    let mut base = value;
    if let Some((head, _)) = base.split_once('@') {
        base = head;
    }
    if let Some((head, _)) = base.split_once('.') {
        base = head;
    }
    let base = base.replace('-', "_");
    if base.is_empty() || base == "C" || base == "POSIX" {
        return Vec::new();
    }
    let mut candidates = vec![base.clone()];
    if let Some((language, _)) = base.split_once('_') {
        if !language.is_empty() && language != base {
            candidates.push(language.to_string());
        }
    }
    candidates
}

/// The locale the catalogue was loaded for, or `None` when the user runs in a
/// language Snipunix is not translated into.
#[must_use]
pub fn active_locale() -> Option<&'static str> {
    let translations = TRANSLATIONS.get_or_init(load_translations);
    translations.locale.as_deref()
}

/// Locales Snipunix ships translations for.
#[must_use]
pub fn available_locales() -> Vec<&'static str> {
    catalogues::CATALOGUES
        .iter()
        .map(|(name, _)| *name)
        .collect()
}

/// Overrides the active catalogue. Intended for tests.
#[doc(hidden)]
pub fn set_active_catalogue_for_test(locale: Option<&str>, entries: &[(&str, &str)]) {
    let mut map = HashMap::new();
    for (key, value) in entries {
        map.insert((*key).to_string(), (*value).to_string());
    }
    let _ = TRANSLATIONS.set(Translations {
        locale: locale.map(ToString::to_string),
        entries: map,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_locale_names() {
        assert_eq!(locale_candidates("de_DE.UTF-8"), vec!["de_DE", "de"]);
        assert_eq!(locale_candidates("de-DE"), vec!["de_DE", "de"]);
        assert_eq!(locale_candidates("fr"), vec!["fr"]);
        assert_eq!(locale_candidates("C"), Vec::<String>::new());
        assert_eq!(locale_candidates("POSIX"), Vec::<String>::new());
        assert_eq!(locale_candidates(""), Vec::<String>::new());
        assert_eq!(
            locale_candidates("sr_RS@latin"),
            vec!["sr_RS".to_string(), "sr".to_string()]
        );
    }

    #[test]
    fn fills_placeholders_in_order() {
        let template = "Wrote {0} at {1}";
        let values = vec!["a.png".to_string(), "12:00".to_string()];
        assert_eq!(fill_placeholders(template, &values), "Wrote a.png at 12:00");
    }

    #[test]
    fn leaves_missing_placeholders_untouched() {
        let template = "Wrote {0} at {1}";
        let values = vec!["a.png".to_string()];
        assert_eq!(fill_placeholders(template, &values), "Wrote a.png at {1}");
    }

    #[test]
    fn keeps_escaped_braces_literal() {
        let values = vec!["x".to_string()];
        assert_eq!(fill_placeholders("{{0}} {0}", &values), "{0} x");
    }

    #[test]
    fn unterminated_placeholder_is_kept() {
        let values = vec!["x".to_string()];
        assert_eq!(fill_placeholders("{0", &values), "{0");
    }

    #[test]
    fn translate_falls_back_to_the_original_text() {
        // The global catalogue is empty in the test process unless another
        // test installed one, so an unknown message must come back unchanged.
        let message = "a message that is not translated";
        assert_eq!(translate(message), message);
    }

    #[test]
    fn embedded_catalogues_are_valid() {
        assert!(!available_locales().is_empty());
        for (name, bytes) in catalogues::CATALOGUES {
            let catalogue = Catalogue::parse(bytes)
                .unwrap_or_else(|error| panic!("the {name} catalogue is invalid: {error}"));
            assert!(
                !catalogue.is_empty(),
                "the {name} catalogue has no translations"
            );
        }
    }

    #[test]
    fn the_embedded_catalogues_contain_the_expected_translations() {
        let catalogue = |name: &str| {
            let bytes = catalogues::CATALOGUES
                .iter()
                .find(|(locale, _)| *locale == name)
                .map(|(_, bytes)| *bytes)
                .unwrap_or_else(|| panic!("the {name} catalogue is missing"));
            Catalogue::parse(bytes).unwrap()
        };
        let german = catalogue("de");
        assert_eq!(german.get("Capture Region"), Some("Bereich aufnehmen"));
        assert_eq!(german.get("Settings…"), Some("Einstellungen…"));
        let spanish = catalogue("es");
        assert_eq!(spanish.get("Capture Region"), Some("Capturar región"));
        assert_eq!(spanish.get("Quit"), Some("Salir"));
    }
}
