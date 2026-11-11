//! `jp vite init`: initialize a VitePress site in the source directory.
//!
//! Writes the project-owned site files (`package.json`,
//! `.vitepress/config.mts`, `.vitepress/theme/index.js`, and `index.md`), which
//! `jp vite sync` never modifies.
//! Re-running `init` overwrites existing files with different contents only
//! when git can restore them or `--force` is passed (see [`overwrite`]).

use std::{fs, io::Write as _};

use camino::Utf8Path;
use overwrite::Existing;

use crate::settings::below;

/// A template file written by `jp vite init`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Template {
    /// Path relative to the VitePress source directory.
    pub path: &'static str,

    /// Raw template contents before [`Fill`] substitution.
    pub contents: &'static str,
}

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

/// Placeholder values substituted into [`TEMPLATES`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fill {
    pub title: String,
    pub description: String,

    /// Navigation bar links after `Home`, as `(label, link)` pairs.
    pub nav: Vec<(String, String)>,
}

/// Return the templates from [`TEMPLATES`] whose target files do not exist in
/// `site_dir`.
pub(crate) fn missing(site_dir: &Utf8Path) -> Vec<Template> {
    TEMPLATES
        .into_iter()
        .filter(|template| !site_dir.join(template.path).exists())
        .collect()
}

/// Render each template with `fill` and compare it against the file in
/// `site_dir`.
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

/// Render `template` with `fill`.
///
/// `title` and `description` are JSON-quoted, which is valid in both the
/// TypeScript config and YAML front matter.
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

/// Write the missing or differing templates from `planned` into `site_dir`.
///
/// Files that were missing during planning are opened with `create_new` so a
/// file created in the meantime is not overwritten.
/// Returns a status line (`Created <path>` or `Overwrote <path>`, relative to
/// `root`) for each file written.
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
