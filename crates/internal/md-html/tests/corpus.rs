//! Renders JP's own documents, as a check on comrak's source positions that
//! fails when a comrak upgrade or a new document leaves text unmapped.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// The directories, relative to the workspace root, whose Markdown files are
/// rendered, subdirectories included.
const DIRS: [&str; 3] = ["docs/rfd", "docs/wiki", "docs/ticket"];

const UNMAPPED: &str = "<span class=\"unmapped\">";

/// How many unmapped spans of one document the failure message shows.
const SHOWN_PER_DOCUMENT: usize = 3;

#[test]
fn jp_documents_render_without_unmapped_text() {
    let root = workspace_root();
    let mut files = vec![];
    for dir in DIRS {
        markdown_files(&root.join(dir), &mut files);
    }
    files.sort();
    // Moved documents would otherwise leave nothing to check.
    assert!(files.len() > 100, "found only {} documents", files.len());

    let mut failures = vec![];
    for path in &files {
        let source = fs::read_to_string(path).unwrap();
        let html = md_html::render(&source).html;
        let name = path.strip_prefix(&root).unwrap_or(path).display();
        for (index, _) in html.match_indices(UNMAPPED).take(SHOWN_PER_DOCUMENT) {
            let text: String = html[index + UNMAPPED.len()..].chars().take(60).collect();
            failures.push(format!("{name}: {text}"));
        }
    }

    assert!(
        failures.is_empty(),
        "unmapped text in {} places:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The root of JP's repository.
///
/// Read at run time: a target directory shared between worktrees can run a
/// binary another worktree compiled, so a path baked in with `env!` can name
/// the wrong checkout.
fn workspace_root() -> PathBuf {
    let manifest = env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR: run tests via `cargo test` or `cargo nextest`");

    PathBuf::from(manifest).join("../../..")
}

fn markdown_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            markdown_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "md") {
            files.push(path);
        }
    }
}
