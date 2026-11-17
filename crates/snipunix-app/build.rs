//! Makes the application identifier configurable at build time.
//!
//! The stable build uses `io.github.alokverma9.snipunix`. The beta channel
//! uses `io.github.alokverma9.snipunix.Beta`, which is a different
//! application as far as the desktop portals are concerned: the portal grants
//! permissions per application identifier, so a beta build must present the
//! beta identifier to keep its permissions separate from the stable build.
//!
//! The Flatpak manifests set `SNIPUNIX_APP_ID` in the build environment.

fn main() {
    println!("cargo:rerun-if-env-changed=SNIPUNIX_APP_ID");
    let identifier = std::env::var("SNIPUNIX_APP_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "io.github.alokverma9.snipunix".to_string());
    println!("cargo:rustc-env=SNIPUNIX_APP_ID={identifier}");
    let is_beta = identifier.ends_with(".Beta");
    println!("cargo:rustc-check-cfg=cfg(snipunix_beta)");
    if is_beta {
        println!("cargo:rustc-cfg=snipunix_beta");
    }
}
