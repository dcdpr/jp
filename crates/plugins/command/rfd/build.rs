//! Stamp the revision of `jp vite`'s site files into the binary.
//!
//! The site's loaders read what this plugin writes, so `jp rfd --version`
//! reports the revision of the loaders it was built alongside.
//! `jp vite serve` refuses to trust a site whose plugins disagree on it.

use std::{env, path::PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let site = manifest.join("../vite/site");

    println!("cargo:rerun-if-changed={}", site.display());

    let revision = site_revision::revision(&site)
        .unwrap_or_else(|error| panic!("can't read the site files in {}: {error}", site.display()));
    println!("cargo:rustc-env=JP_SITE_REVISION={revision}");
}
