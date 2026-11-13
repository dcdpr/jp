use std::{fs, io::Cursor};

use camino_tempfile::Utf8TempDir;
use serde_json::json;

use super::*;

fn build_at(root: &Utf8Path) -> (String, Result<(), String>) {
    let mut stdout = vec![];
    let result = build(
        Cursor::new(vec![]),
        &mut stdout,
        root,
        &Options::default(),
        &json!({}),
        &[],
    );

    (String::from_utf8(stdout).unwrap(), result)
}

#[test]
fn a_workspace_without_a_site_is_refused() {
    let root = Utf8TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("docs")).unwrap();

    let (_, result) = build_at(root.path());

    assert_eq!(
        result,
        Err("There is no VitePress site in docs: run `jp vite init` to set one up.".to_owned())
    );
}

/// Loaders from another release could render the site wrongly, so the build
/// stops before `vitepress` runs; the snapshot is brought up to date first, as
/// `serve` does.
#[test]
fn a_site_whose_files_are_out_of_date_is_refused() {
    let root = Utf8TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("docs/rfd")).unwrap();
    fs::write(root.path().join("docs/package.json"), "{}").unwrap();

    let (said, result) = build_at(root.path());

    let problem = result.unwrap_err();
    assert!(
        problem.starts_with(
            "These files aren't the ones this `jp vite` writes, so the site may run with files \
             from another release:\n"
        ),
        "{problem}"
    );
    assert!(
        problem.ends_with("Run `jp vite sync` to write them."),
        "{problem}"
    );
    assert!(
        said.contains("Wrote the site's settings to docs/.vitepress/jp-vite/settings.json."),
        "{said}"
    );
}
