use pretty_assertions::assert_eq;

use super::*;

const BUILT: Bundle = Bundle {
    script: "export default function init() {}\n",
    wasm: b"\0asm\x01\0\0\0",
    style: "body { margin: 0; }\n",
};

#[test]
fn the_page_is_the_shell_that_starts_the_bundle() {
    let page = page("notes", Some(&BUILT));

    assert!(
        page.contains(r#"<meta name="jp-workspace" content="notes">"#),
        "{page}"
    );
    assert!(
        page.contains(
            r#"<script type="module">import init from "/ui/jp_review_ui.js"; init();</script>"#
        ),
        "{page}"
    );
    assert!(
        page.contains(r#"<link rel="manifest" href="/ui/manifest.webmanifest">"#),
        "{page}"
    );
}

#[test]
fn a_page_from_a_bundle_directory_names_the_build_it_started() {
    let page = watched_page("notes", "1834927105");

    assert!(
        page.contains(r#"<meta name="jp-ui-build" content="1834927105">"#),
        "{page}"
    );
    assert!(
        page.contains(
            r#"<script type="module">import init from "/ui/jp_review_ui.js"; init();</script>"#
        ),
        "{page}"
    );
    assert!(
        page.contains(r#"<meta name="jp-workspace" content="notes">"#),
        "{page}"
    );
}

/// The build tag is what tells the UI to reload itself, so only a page served
/// from a bundle directory carries it.
#[test]
fn a_page_from_the_built_in_bundle_names_no_build() {
    let page = page("notes", Some(&BUILT));

    assert!(!page.contains("jp-ui-build"), "{page}");
    assert!(!page.contains("{head}"), "{page}");
}

/// A workspace named after a placeholder stays named that.
#[test]
fn a_workspace_name_is_not_read_as_a_placeholder() {
    let page = watched_page("{head}", "7");

    assert!(
        page.contains(r#"<meta name="jp-workspace" content="{head}">"#),
        "{page}"
    );
}

#[test]
fn only_the_bundle_files_are_read_from_a_bundle_directory() {
    assert_eq!(bundle_file("jp_review_ui.js"), Some("text/javascript"));
    assert_eq!(
        bundle_file("jp_review_ui_bg.wasm"),
        Some("application/wasm")
    );
    assert_eq!(bundle_file("style.css"), Some("text/css"));
    assert_eq!(bundle_file("manifest.webmanifest"), None);
    assert_eq!(bundle_file(BUILD), None);
    assert_eq!(bundle_file("../Cargo.toml"), None);
    assert_eq!(bundle_file(""), None);
}

#[test]
fn without_a_bundle_the_page_is_the_placeholder() {
    let page = page("notes", None);

    assert!(!page.contains("jp_review_ui.js"), "{page}");
    assert!(
        page.contains("<code>JP_REVIEW_UI</code>"),
        "the placeholder says how to build the UI in: {page}"
    );
    assert!(
        page.contains(r#"<meta name="jp-workspace" content="notes">"#),
        "{page}"
    );
}

/// A workspace is named after its directory, which can hold any character.
#[test]
fn the_workspace_name_is_escaped() {
    let page = page(r#"a<b>&"c'"#, None);

    assert!(
        page.contains(r#"content="a&lt;b&gt;&amp;&quot;c&#39;""#),
        "{page}"
    );
    assert!(!page.contains("a<b>"), "{page}");
}

#[test]
fn the_manifest_and_icon_are_served_with_or_without_a_bundle() {
    for bundle in [None, Some(&BUILT)] {
        assert_eq!(
            asset("manifest.webmanifest", bundle).map(|asset| asset.content_type),
            Some("application/manifest+json")
        );
        assert_eq!(
            asset("icon.svg", bundle).map(|asset| asset.content_type),
            Some("image/svg+xml")
        );
    }
}

#[test]
fn the_bundle_is_served_only_when_there_is_one() {
    assert_eq!(
        asset("jp_review_ui_bg.wasm", Some(&BUILT)),
        Some(Asset {
            content_type: "application/wasm",
            body: b"\0asm\x01\0\0\0",
        })
    );
    assert_eq!(
        asset("jp_review_ui.js", Some(&BUILT)).map(|asset| asset.content_type),
        Some("text/javascript")
    );
    assert_eq!(
        asset("style.css", Some(&BUILT)).map(|asset| asset.content_type),
        Some("text/css")
    );
    assert_eq!(asset("jp_review_ui_bg.wasm", None), None);
    assert_eq!(asset("jp_review_ui.js", None), None);
    assert_eq!(asset("style.css", None), None);
    assert_eq!(asset("../Cargo.toml", Some(&BUILT)), None);
}

/// The manifest is what a phone reads to add the UI to its home screen.
#[test]
fn the_manifest_starts_the_ui_full_screen() {
    let manifest: serde_json::Value = serde_json::from_str(MANIFEST).unwrap();

    assert_eq!(manifest["start_url"], "/");
    assert_eq!(manifest["display"], "standalone");
    assert_eq!(manifest["icons"][0]["src"], "/ui/icon.svg");
}
