//! Build the site files in `site/` into the binary, and stamp their revision.
//!
//! Writes `site.rs` to `OUT_DIR`, holding every file under `site/` by its path
//! there, which `src/site.rs` includes.
//! The revision is the one `jp ticket` and `jp rfd` stamp from the same files,
//! so `jp vite serve` can tell whether the three were built from one site.

use std::{
    env,
    fmt::Write as _,
    fs, io,
    path::{Path, PathBuf},
};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets it"));
    let site = manifest.join("site");

    println!("cargo:rerun-if-changed={}", site.display());

    let files = site_revision::files(&site).unwrap_or_else(|error| unreadable(&site, &error));
    let revision = site_revision::revision(&site).unwrap_or_else(|error| unreadable(&site, &error));

    let mut source = String::from(
        "/// Every file under the crate's `site/`, by its path there.\npub(crate) static FILES: \
         &[(&str, &[u8])] = &[\n",
    );
    for file in &files {
        let path = file.source.to_str().unwrap_or_else(|| {
            panic!("{} is not a UTF-8 path", file.source.display());
        });
        writeln!(source, "    ({:?}, include_bytes!({path:?})),", file.path)
            .expect("writes to a String");
    }
    source.push_str("];\n");

    fs::write(out.join("site.rs"), source).expect("OUT_DIR is writable");
    println!("cargo:rustc-env=JP_SITE_REVISION={revision}");
}

fn unreadable(site: &Path, error: &io::Error) -> ! {
    panic!("can't read the site files in {}: {error}", site.display());
}
