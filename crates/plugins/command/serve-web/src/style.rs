//! Embedded CSS for the web UI.

use std::sync::OnceLock;

use sha2::{Digest as _, Sha256};

/// The CSS content, embedded at compile time.
pub(crate) const CSS: &str = include_str!("style.css");

/// A short hex hash of the CSS content, used to cache-bust the stylesheet URL.
pub(crate) fn css_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        let hash = Sha256::digest(CSS.as_bytes());
        hash[..8].iter().fold(String::new(), |mut acc, b| {
            use std::fmt::Write as _;
            let _ = write!(acc, "{b:02x}");
            acc
        })
    })
}

/// The `ETag` header value for the CSS: the content hash in quotes.
pub(crate) fn css_etag() -> String {
    format!("\"{}\"", css_version())
}
