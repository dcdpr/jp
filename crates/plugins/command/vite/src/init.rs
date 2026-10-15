//! `jp vite init`: set up a VitePress site in the source directory.
//!
//! Writes the project's own files, which `jp vite sync` never touches again:
//! `package.json`, `.vitepress/config.mts`, the theme entry, and a homepage.
//! The configuration merges in the fragment `sync` writes, and the theme entry
//! renders what the RFD and ticket pages add above a document.
//! A file that exists is left as it is, so running it again only adds what is
//! missing.
//!
//! The title and description go into the configuration and the homepage.

use std::{fs, io::Write as _};

use camino::Utf8Path;

/// A file `init` writes, below the source directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Template {
    /// Where it goes, below the source directory.
    pub path: &'static str,

    /// What it holds, before [`Fill`] is applied.
    pub contents: &'static str,

    /// Whether it names the site's title and description.
    pub titled: bool,
}

/// The files `init` writes.
pub(crate) const TEMPLATES: [Template; 4] = [
    Template {
        path: "package.json",
        contents: include_str!("../init/package.json"),
        titled: false,
    },
    Template {
        path: ".vitepress/config.mts",
        contents: include_str!("../init/config.mts"),
        titled: true,
    },
    Template {
        path: ".vitepress/theme/index.js",
        contents: include_str!("../init/theme.js"),
        titled: false,
    },
    Template {
        path: "index.md",
        contents: include_str!("../init/index.md"),
        titled: true,
    },
];

/// What a template's placeholders are filled with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fill {
    pub title: String,
    pub description: String,

    /// The navigation bar's links after Home, as label and site path.
    pub nav: Vec<(String, String)>,
}

/// The templates whose files `site_dir` doesn't have yet.
pub(crate) fn missing(site_dir: &Utf8Path) -> Vec<Template> {
    TEMPLATES
        .into_iter()
        .filter(|template| !site_dir.join(template.path).exists())
        .collect()
}

/// `template` with its placeholders filled.
///
/// The title and description are written as quoted strings, which both the
/// configuration and the homepage's front matter read, whatever they hold.
pub(crate) fn render(template: &Template, fill: &Fill) -> String {
    let quoted = |text: &str| serde_json::to_string(text).expect("a string serializes");

    let mut nav = String::from("            { text: 'Home', link: '/' },\n");
    for (label, link) in &fill.nav {
        nav.push_str(&format!(
            "            {{ text: {}, link: {} }},\n",
            quoted(label),
            quoted(link)
        ));
    }

    template
        .contents
        .replace("@@title@@", &quoted(&fill.title))
        .replace("@@description@@", &quoted(&fill.description))
        .replace("@@nav@@", &nav)
}

/// Write `templates` below `site_dir`, refusing to replace a file that appeared
/// since they were picked.
///
/// Returns each file written, relative to `root`.
pub(crate) fn write(
    root: &Utf8Path,
    site_dir: &Utf8Path,
    templates: &[Template],
    fill: &Fill,
) -> Result<Vec<String>, String> {
    let mut written = vec![];

    for template in templates {
        let path = site_dir.join(template.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("{parent}: {error}"))?;
        }

        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .and_then(|mut file| file.write_all(render(template, fill).as_bytes()))
            .map_err(|error| format!("{path}: {error}"))?;

        written.push(path.strip_prefix(root).unwrap_or(&path).to_string());
    }

    Ok(written)
}

#[cfg(test)]
#[path = "init_tests.rs"]
mod tests;
