use camino_tempfile::Utf8TempDir;

use super::*;

fn fill() -> Fill {
    Fill {
        title: "Acme \"Docs\"".to_owned(),
        description: "Plans: what's next".to_owned(),
        nav: vec![("RFDs".to_owned(), "/rfd/".to_owned())],
    }
}

fn template(path: &str) -> Template {
    TEMPLATES
        .into_iter()
        .find(|template| template.path == path)
        .unwrap()
}

#[test]
fn an_empty_site_is_missing_every_file() {
    let dir = Utf8TempDir::new().unwrap();

    let paths: Vec<&str> = missing(dir.path())
        .into_iter()
        .map(|template| template.path)
        .collect();

    assert_eq!(paths, [
        "package.json",
        ".vitepress/config.mts",
        ".vitepress/theme/index.js",
        "index.md",
    ]);
}

/// Running it again adds only what is missing.
#[test]
fn a_file_that_exists_is_not_missing() {
    let dir = Utf8TempDir::new().unwrap();
    fs::write(dir.path().join("package.json"), "{}").unwrap();

    assert_eq!(missing(dir.path()).len(), 3);
}

/// The configuration merges in the fragment `sync` writes, and quotes the title
/// and description whatever they hold.
#[test]
fn the_configuration_takes_the_title_and_the_navigation() {
    assert_eq!(
        render(&template(".vitepress/config.mts"), &fill()),
        r#"import { defineConfig } from 'vitepress'

import { withJpVite } from './jp-vite/config.mjs'

// https://vitepress.dev/reference/site-config
//
// `withJpVite` adds what the RFD and ticket pages need, from the files
// `jp vite sync` keeps in `./jp-vite/`. Everything else here is yours.
export default defineConfig(withJpVite({
    title: "Acme \"Docs\"",
    description: "Plans: what's next",
    cleanUrls: true,
    themeConfig: {
        nav: [
            { text: 'Home', link: '/' },
            { text: "RFDs", link: "/rfd/" },
        ],
    },
}))
"#
    );
}

/// A double-quoted YAML scalar reads the same escapes JSON writes, so a colon
/// or a quote in the title can't break the front matter.
#[test]
fn the_homepage_quotes_what_it_shows() {
    assert_eq!(
        render(&template("index.md"), &fill()),
        "---\nlayout: home\n\nhero:\n  name: \"Acme \\\"Docs\\\"\"\n  tagline: \"Plans: what's \
         next\"\n---\n"
    );
}

#[test]
fn writing_creates_the_files_and_their_directories() {
    let root = Utf8TempDir::new().unwrap();
    let site = root.path().join("docs");

    let written = write(root.path(), &site, &missing(&site), &fill()).unwrap();

    assert_eq!(written, [
        "docs/package.json",
        "docs/.vitepress/config.mts",
        "docs/.vitepress/theme/index.js",
        "docs/index.md",
    ]);
    assert!(
        fs::read_to_string(site.join(".vitepress/theme/index.js"))
            .unwrap()
            .contains("import { docBefore } from '../jp-vite/theme/index.mjs'")
    );
}

/// A file that appeared after the templates were picked is not replaced.
#[test]
fn writing_refuses_a_file_that_exists() {
    let root = Utf8TempDir::new().unwrap();
    let site = root.path().join("docs");
    let templates = missing(&site);
    fs::create_dir_all(&site).unwrap();
    fs::write(site.join("index.md"), "Mine").unwrap();

    let error = write(root.path(), &site, &templates, &fill()).unwrap_err();

    assert!(
        error.starts_with(&format!("{}: ", site.join("index.md"))),
        "{error}"
    );
    assert_eq!(fs::read_to_string(site.join("index.md")).unwrap(), "Mine");
}
