//! Embedded CSS for the web UI.

use std::sync::OnceLock;

use sha2::{Digest as _, Sha256};

/// The CSS content, embedded at compile time.
pub(crate) const CSS: &str = include_str!("style.css");

/// The app icon, embedded at compile time.
///
/// SVG rather than PNG so it can live in the source tree as text.
/// Browsers take it for the tab icon and recent iOS takes it from the web
/// manifest for the home screen; older iOS wants a PNG `apple-touch-icon` and
/// falls back to a page screenshot without one.
pub(crate) const ICON: &str = include_str!("icon.svg");

/// The web app manifest, so the page installs to a home screen with a name and
/// an icon rather than as a bare bookmark.
pub(crate) const MANIFEST: &str = r##"{
  "name": "JP Conversations",
  "short_name": "JP",
  "start_url": "/conversations",
  "display": "standalone",
  "background_color": "#1a1a1a",
  "theme_color": "#1a1a1a",
  "icons": [
    { "src": "/assets/icon.svg", "sizes": "any", "type": "image/svg+xml", "purpose": "any" }
  ]
}
"##;

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
