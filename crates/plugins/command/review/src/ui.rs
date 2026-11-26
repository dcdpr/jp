//! The browser UI `jp review serve` answers `/` and `/reviews/{id}` with, and
//! the files it loads from `/ui/...`.
//!
//! The UI's bundle is in the binary when it was built with `JP_REVIEW_UI`
//! naming it (see `build.rs`), and [`BUNDLE`] holds it; without one, the pages
//! are a placeholder saying how to build it in.
//! The web app manifest and its icon are served either way.
//!
//! `jp review serve --ui-dir` serves the bundle from the directory `just
//! build-ui` writes instead, read on every request, for developing the UI.
//! `just build-ui` writes the file [`BUILD`] last, naming the build; the page
//! carries the build it started with in a `jp-ui-build` meta tag, and the UI
//! reloads itself when `/ui/build` names another.
//!
//! Both pages carry the name of the workspace the server serves, in a
//! `jp-workspace` meta tag, which the UI compares with the name each API answer
//! carries.

include!(concat!(env!("OUT_DIR"), "/ui.rs"));

/// The page the UI starts in, with `{workspace}` standing for the workspace's
/// name and `{head}` for what else its head holds.
const SHELL: &str = include_str!("ui/shell.html");

/// The file in a bundle directory that names its build, written last.
pub(crate) const BUILD: &str = "build";

/// The page served in place of the UI when the binary carries none, with
/// `{workspace}` standing for the workspace's name.
const PLACEHOLDER: &str = include_str!("ui/placeholder.html");

const MANIFEST: &str = include_str!("ui/manifest.webmanifest");

/// The app icon, SVG so it lives in the source tree as text.
const ICON: &str = include_str!("ui/icon.svg");

/// The browser UI's files, as `wasm-bindgen --target web` writes them, and its
/// stylesheet.
#[derive(Debug)]
pub(crate) struct Bundle {
    /// The JavaScript module the page imports, which loads `wasm`.
    pub script: &'static str,
    pub wasm: &'static [u8],
    pub style: &'static str,
}

/// A file served under `/ui/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Asset {
    pub content_type: &'static str,
    pub body: &'static [u8],
}

/// The page for the workspace named `workspace`: the shell that starts the UI
/// in `bundle`, or the placeholder without one.
pub(crate) fn page(workspace: &str, bundle: Option<&Bundle>) -> String {
    if bundle.is_none() {
        return PLACEHOLDER.replace("{workspace}", &escape(workspace));
    }

    shell(workspace, "")
}

/// The shell for the workspace named `workspace` that starts the UI in a bundle
/// directory, as the build `build` left it; the UI reloads itself once the
/// directory names another build.
pub(crate) fn watched_page(workspace: &str, build: &str) -> String {
    let head = format!(r#"<meta name="jp-ui-build" content="{}">"#, escape(build));

    shell(workspace, &head)
}

fn shell(workspace: &str, head: &str) -> String {
    // `{head}` first, so a workspace named `{head}` stays named that.
    SHELL
        .replace("{head}", head)
        .replace("{workspace}", &escape(workspace))
}

/// The content type of the bundle's file `name`; `None` for a name that is no
/// file of the bundle, which keeps any other path in a bundle directory out of
/// reach.
pub(crate) fn bundle_file(name: &str) -> Option<&'static str> {
    match name {
        "jp_review_ui.js" => Some("text/javascript"),
        "jp_review_ui_bg.wasm" => Some("application/wasm"),
        "style.css" => Some("text/css"),
        _ => None,
    }
}

/// The file `/ui/{name}` serves; `None` for a name that isn't one, and for the
/// bundle's files when there is no `bundle`.
pub(crate) fn asset(name: &str, bundle: Option<&Bundle>) -> Option<Asset> {
    let asset = |content_type, body| Some(Asset { content_type, body });

    match (name, bundle) {
        ("manifest.webmanifest", _) => asset("application/manifest+json", MANIFEST.as_bytes()),
        ("icon.svg", _) => asset("image/svg+xml", ICON.as_bytes()),
        ("jp_review_ui.js", Some(bundle)) => asset("text/javascript", bundle.script.as_bytes()),
        ("jp_review_ui_bg.wasm", Some(bundle)) => asset("application/wasm", bundle.wasm),
        ("style.css", Some(bundle)) => asset("text/css", bundle.style.as_bytes()),
        _ => None,
    }
}

/// `text` with the characters HTML gives meaning to escaped, for an element's
/// text or an attribute's value.
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }

    escaped
}

#[cfg(test)]
#[path = "ui_tests.rs"]
mod tests;
