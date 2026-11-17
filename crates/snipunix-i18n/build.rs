//! Embeds the compiled gettext catalogues into the crate.
//!
//! The `.mo` files live in `locales/<lang>/LC_MESSAGES/snipunix.mo`. They are
//! copied into `OUT_DIR` and referenced from a generated table, so the
//! binary carries every translation it supports and does not depend on a
//! locale directory being present on the machine.

use std::fs;
use std::path::{Path, PathBuf};

/// Name of the catalogue file inside each locale directory.
const CATALOGUE_NAME: &str = "snipunix.mo";

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let locales = manifest.join("locales");
    println!("cargo:rerun-if-changed={}", locales.display());

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let embedded = out_dir.join("locales");
    fs::create_dir_all(&embedded).expect("could not create the output directory");

    let mut entries = Vec::new();
    for locale in sorted_locales(&locales) {
        let source = locales
            .join(&locale)
            .join("LC_MESSAGES")
            .join(CATALOGUE_NAME);
        println!("cargo:rerun-if-changed={}", source.display());
        if !source.exists() {
            continue;
        }
        let target = embedded.join(format!("{locale}.mo"));
        fs::copy(&source, &target).unwrap_or_else(|error| {
            panic!("could not copy {}: {error}", source.display());
        });
        entries.push((locale, target));
    }

    let mut generated = String::from(
        "/// Compiled catalogues, one entry per supported locale.\npub static CATALOGUES: &[(&str, &[u8])] = &[\n",
    );
    for (locale, path) in &entries {
        generated.push_str(&format!(
            "    ({:?}, include_bytes!({:?}) as &[u8]),\n",
            locale,
            path.to_string_lossy()
        ));
    }
    generated.push_str("];\n");

    fs::write(out_dir.join("catalogues.rs"), generated)
        .expect("could not write the catalogue table");
}

/// Lists the locale directories, sorted so the build is reproducible.
fn sorted_locales(locales: &Path) -> Vec<String> {
    let Ok(directory) = fs::read_dir(locales) else {
        return Vec::new();
    };
    let mut names: Vec<String> = directory
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}
