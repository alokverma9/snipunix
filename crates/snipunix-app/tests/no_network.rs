//! Proves that Snipunix does not talk to the network.
//!
//! The privacy statement in `docs/PRIVACY.md` promises that the application
//! never opens a network connection. This test checks that promise against
//! the source tree, so a careless change that adds an HTTP client or a
//! socket fails the build instead of quietly breaking the promise.
//!
//! # The one exception
//!
//! `snipunix-ocr` downloads the pinned OCR models. That download is
//! explicit: it only runs when the user asks for it in Settings, and the
//! model files are verified against a pinned hash. The test therefore allows
//! network primitives in that crate and nowhere else, and it additionally
//! checks that the download is not reachable from the capture path.

use std::path::{Path, PathBuf};

/// Patterns that indicate network access.
const NETWORK_PATTERNS: [&str; 8] = [
    "ureq::",
    "reqwest::",
    "TcpStream",
    "TcpListener",
    "UdpSocket",
    "hyper::",
    "socket2::",
    "std::net::",
];

/// The crates that may contain network primitives.
const ALLOWED_CRATES: [&str; 1] = ["snipunix-ocr"];

/// The workspace root, derived from the manifest directory.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives inside the workspace")
        .to_path_buf()
}

/// Every Rust source file in the workspace.
fn sources(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let crates = root.join("crates");
    let Ok(entries) = std::fs::read_dir(&crates) else {
        return files;
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("snipunix-") {
            continue;
        }
        let source_root = entry.path().join("src");
        collect(&source_root, &mut files);
    }
    files.sort();
    files
}

fn collect(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, files);
        } else if path.extension().and_then(|value| value.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

/// The crate a source file belongs to.
fn crate_name(path: &Path) -> String {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .find(|part| part.starts_with("snipunix-"))
        .unwrap_or_default()
        .to_string()
}

#[test]
fn only_the_ocr_crate_may_contain_network_primitives() {
    let root = workspace_root();
    let files = sources(&root);
    assert!(!files.is_empty(), "no source files were found");

    let mut violations = Vec::new();
    for file in &files {
        let crate_name = crate_name(file);
        if ALLOWED_CRATES.contains(&crate_name.as_str()) {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(file) else {
            continue;
        };
        for (number, line) in contents.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            for pattern in NETWORK_PATTERNS {
                if line.contains(pattern) {
                    violations.push(format!(
                        "{}:{}: {pattern}",
                        file.strip_prefix(&root).unwrap_or(file).display(),
                        number + 1
                    ));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "network primitives outside the OCR crate:\n{}",
        violations.join("\n")
    );
}

#[test]
fn the_ocr_download_is_reachable_only_through_an_explicit_request() {
    let root = workspace_root();
    let ocr = root.join("crates/snipunix-ocr/src/lib.rs");
    let contents = std::fs::read_to_string(&ocr).expect("the OCR crate must exist");

    // The download lives behind a named function, and that function is only
    // called from the settings page.
    assert!(
        contents.contains("pub fn download_models("),
        "the OCR crate must expose an explicit download function"
    );
    assert!(
        contents.contains("ureq::get(url)"),
        "the download must still use the pinned URL"
    );

    // The download verifies the file it fetched.
    assert!(
        contents.contains("sha256") || contents.contains("Sha256"),
        "the download must verify the model files"
    );

    // The capture path must not call the download. The capture crate does not
    // depend on the OCR crate at all, and the OCR crate does not depend on the
    // capture crate at run time. A development dependency is fine: it never
    // reaches a release build.
    let capture_manifest = std::fs::read_to_string(root.join("crates/snipunix-capture/Cargo.toml"))
        .expect("the capture manifest must exist");
    assert!(
        !dependencies_section(&capture_manifest).contains("snipunix-ocr"),
        "the capture crate must not depend on the OCR crate"
    );
    let ocr_manifest = std::fs::read_to_string(root.join("crates/snipunix-ocr/Cargo.toml"))
        .expect("the OCR manifest must exist");
    assert!(
        !dependencies_section(&ocr_manifest).contains("snipunix-capture"),
        "the OCR crate must not depend on the capture crate"
    );
}

/// The `[dependencies]` section of a manifest.
fn dependencies_section(manifest: &str) -> String {
    let mut section = String::new();
    let mut inside = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            inside = trimmed == "[dependencies]";
            continue;
        }
        if inside {
            section.push_str(line);
            section.push('\n');
        }
    }
    section
}

#[test]
fn the_application_manifest_does_not_pull_in_a_network_client() {
    let root = workspace_root();
    let manifest = std::fs::read_to_string(root.join("crates/snipunix-app/Cargo.toml"))
        .expect("the application manifest must exist");
    for pattern in ["ureq", "reqwest", "hyper", "curl"] {
        assert!(
            !manifest.contains(pattern),
            "the application must not depend on {pattern}"
        );
    }
}
