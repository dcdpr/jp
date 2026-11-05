//! `jp vite init`: set up a VitePress site in the source directory.
//!
//! Writes the project's own files, which `jp vite sync` never touches:
//! `package.json`, `.vitepress/config.mts`, the theme entry, and a homepage.
//! The configuration merges in the fragment `sync` writes, and the theme entry
//! renders what the RFD and ticket pages add above a document.
//! Running it again writes them again, overwriting a file that holds something
//! else only as the `overwrite` crate allows.
//!
//! The title and description go into the configuration and the homepage.

use std::{fs, io::Write as _};

use camino::Utf8Path;
use overwrite::Existing;

use crate::settings::below;

/// A file `init` writes, below the source directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Template {
    /// Where it goes, below the source directory.
    pub path: &'static str,

    /// What it holds, before [`Fill`] is applied.
    pub contents: &'static str,
}

/// The files `init` writes.
pub(crate) const TEMPLATES: [Template; 4] = [
    Template {
        path: "package.json",
        contents: include_str!("../init/package.json"),
    },
    Template {
        path: ".vitepress/config.mts",
        contents: include_str!("../init/config.mts"),
    },
    Template {
        path: ".vitepress/theme/index.js",
        contents: include_str!("../init/theme.js"),
    },
    Template {
        path: "index.md",
        contents: include_str!("../init/index.md"),
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

/// Each template, with what is in its file's place below `site_dir`, compared
/// with what it renders to with `fill`.
pub(crate) fn plan(site_dir: &Utf8Path, fill: &Fill) -> Result<Vec<(Template, Existing)>, String> {
    TEMPLATES
        .into_iter()
        .map(|template| {
            let path = site_dir.join(template.path);
            let existing = overwrite::existing(&path, render(&template, fill).as_bytes())?;

            Ok((template, existing))
        })
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

/// Write the templates `planned` found missing or different below `site_dir`.
///
/// A missing file that appeared since it was planned is not replaced.
/// Returns one line per file written, `Created` or `Overwrote` and its path
/// relative to `root`.
pub(crate) fn write(
    root: &Utf8Path,
    site_dir: &Utf8Path,
    planned: &[(Template, Existing)],
    fill: &Fill,
) -> Result<Vec<String>, String> {
    let mut written = vec![];

    for (template, existing) in planned {
        let path = site_dir.join(template.path);
        let contents = render(template, fill);
        let verb = match existing {
            Existing::Same => continue,
            Existing::Differs => {
                fs::write(&path, contents).map_err(|error| format!("{path}: {error}"))?;
                "Overwrote"
            }
            Existing::Missing => {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|error| format!("{parent}: {error}"))?;
                }
                fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .and_then(|mut file| file.write_all(contents.as_bytes()))
                    .map_err(|error| format!("{path}: {error}"))?;
                "Created"
            }
        };

        let shown = below(root, &path).unwrap_or_else(|| path.to_string());
        written.push(format!("{verb} {shown}"));
    }

    Ok(written)
}

#[cfg(test)]
#[path = "init_tests.rs"]
mod tests;
