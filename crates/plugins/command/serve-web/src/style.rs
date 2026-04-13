//! Embedded CSS for the web UI.

use std::sync::OnceLock;

use sha2::{Digest as _, Sha256};

/// The CSS content, embedded at compile time.
pub(crate) const CSS: &str = include_str!("style.css");

/// Compute a stable `ETag` from the CSS content hash.
pub(crate) fn css_etag() -> String {
    static ETAG: OnceLock<String> = OnceLock::new();
    ETAG.get_or_init(|| {
        let hash = Sha256::digest(CSS.as_bytes());
        let hex: String = hash[..8].iter().fold(String::new(), |mut acc, b| {
            use std::fmt::Write as _;
            let _ = write!(acc, "{b:02x}");
            acc
        });
        format!("\"{hex}\"")
    })
    .clone()
}
