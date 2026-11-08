//! Portal URI handling.

use std::path::{Path, PathBuf};

use thiserror::Error;

/// A file returned by the Screenshot portal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalFile {
    pub path: PathBuf,
    /// The original `file://` URI as returned by the portal.
    pub uri: String,
    /// `true` when the file was created by the portal for this request and may
    /// be removed after reading.
    pub deletable: bool,
}

impl PortalFile {
    /// Decides whether the portal-created source file should be removed after
    /// its pixels are safely in memory.
    ///
    /// The GNOME backend saves non-interactive screenshots into the Pictures
    /// directory as `Screenshot*.png`. Those are copies created solely for this
    /// request, so Snipunix removes them instead of leaving full-desktop
    /// captures behind. A file elsewhere is never touched.
    #[must_use]
    pub fn should_remove_after_read(&self) -> bool {
        if self.deletable {
            return true;
        }
        let name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        pictures_dir().is_some_and(|pictures| is_portal_artifact(&self.path, &pictures, &name))
    }
}

fn is_portal_artifact(path: &Path, pictures: &Path, name: &str) -> bool {
    path.starts_with(pictures) && name.starts_with("Screenshot") && name.ends_with(".png")
}

fn pictures_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_PICTURES_DIR") {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Pictures"))
}

/// Converts a portal URI to a local path.
pub fn portal_uri_to_file(uri: &str) -> Result<PortalFile, UriError> {
    let parsed = url::Url::parse(uri).map_err(|e| UriError::Malformed(e.to_string()))?;
    if parsed.scheme() != "file" {
        return Err(UriError::UnsupportedScheme(parsed.scheme().to_string()));
    }
    let path = parsed
        .to_file_path()
        .map_err(|()| UriError::NotLocal(uri.to_string()))?;
    let deletable = is_temporary(&path);
    Ok(PortalFile {
        path,
        uri: uri.to_string(),
        deletable,
    })
}

fn is_temporary(path: &Path) -> bool {
    let temp = std::env::temp_dir();
    if path.starts_with(&temp) {
        return true;
    }
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .is_some_and(|runtime| path.starts_with(runtime))
}

#[derive(Debug, Error, PartialEq)]
pub enum UriError {
    #[error("portal returned a malformed URI: {0}")]
    Malformed(String),
    #[error("portal returned a non-file URI with scheme {0}")]
    UnsupportedScheme(String),
    #[error("portal URI is not a local file path: {0}")]
    NotLocal(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_percent_escaped_paths() {
        let file = portal_uri_to_file("file:///tmp/Screen%20Shot.png").unwrap();
        assert!(file.path.ends_with("Screen Shot.png"));
    }

    #[test]
    fn marks_tmp_files_deletable() {
        let file = portal_uri_to_file("file:///tmp/snipunix-test.png").unwrap();
        assert!(file.deletable);
    }

    #[test]
    fn keeps_user_pictures_files() {
        let file = portal_uri_to_file("file:///home/alice/Pictures/Screenshot.png").unwrap();
        assert!(!file.deletable);
    }

    #[test]
    fn recognises_gnome_portal_screenshot_in_pictures() {
        let pictures = Path::new("/home/alice/Pictures");
        assert!(is_portal_artifact(
            Path::new("/home/alice/Pictures/Screenshot-4.png"),
            pictures,
            "Screenshot-4.png"
        ));
        assert!(!is_portal_artifact(
            Path::new("/home/alice/Downloads/Screenshot-4.png"),
            pictures,
            "Screenshot-4.png"
        ));
        assert!(!is_portal_artifact(
            Path::new("/home/alice/Pictures/holiday.png"),
            pictures,
            "holiday.png"
        ));
    }

    #[test]
    fn rejects_non_file_uris() {
        assert_eq!(
            portal_uri_to_file("https://example.invalid/a.png"),
            Err(UriError::UnsupportedScheme("https".into()))
        );
    }
}
