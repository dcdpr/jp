use std::path::PathBuf;

use camino::{Utf8Path, Utf8PathBuf};
use grep_printer::StandardBuilder;
use grep_regex::RegexMatcher;
use grep_searcher::SearcherBuilder;

use crate::{Error, util::OneOrMany};

pub(crate) async fn fs_grep_files(
    root: &Utf8Path,
    mut pattern: String,
    context: Option<usize>,
    paths: Option<OneOrMany<String>>,
) -> std::result::Result<String, Error> {
    let absolute_paths: Vec<_> = paths
        .as_deref()
        .unwrap_or(&[String::new()])
        .iter()
        .map(|v| root.join(v.trim_start_matches('/')))
        .filter(|v| v.exists())
        .collect();

    // Guard against a common mistake LLMs seem to make when using this tool.
    // Often the pattern ends with an escaped double quote, which will cause the
    // pattern to not match anything.
    if let Some(pat) = pattern.strip_suffix('"') {
        pattern = format!("{pattern}|{pat}");
    }

    let matcher = RegexMatcher::new(&pattern)?;

    let mut printer = StandardBuilder::new()
        .max_columns(Some(1000))
        .max_columns_preview(true)
        .trim_ascii(true)
        .build_no_color(vec![]);

    let mut searcher = SearcherBuilder::new()
        .before_context(context.unwrap_or(0))
        .after_context(context.unwrap_or(0))
        .max_matches(Some(100))
        .build();

    for path in absolute_paths {
        let files = if path.is_dir() {
            super::fs_list_files(&path, None, None)
                .await?
                .into_files()
                .into_iter()
                .map(Utf8PathBuf::from)
                .map(|p| root.join(&path).join(p))
                .filter(|path| path.exists())
                .collect()
        } else {
            vec![path]
        };

        for file in files {
            let Ok(path) = file.strip_prefix(root).map(PathBuf::from) else {
                continue;
            };

            searcher.search_path(&matcher, &file, printer.sink_with_path(&matcher, &path))?;
        }
    }

    let matches = String::from_utf8(printer.into_inner().into_inner())?;

    let lines = matches.lines().count();
    if matches.is_empty() {
        Ok("No matches found. Broaden your search to see more.".to_owned())
    } else if lines > 200 && context.is_some() {
        Box::pin(fs_grep_files(root, pattern, None, paths))
            .await
            .map(|v| {
                format!(
                    "{v}\n[Hidden contextual lines due to excessive number of lines returned. \
                     Narrow down your search to see more.]"
                )
            })
    } else if lines > 100 {
        Ok(indoc::formatdoc! {"
            {}

            [Showing 100/{lines} lines of matches... Narrow down your search to see more.]
        ", matches.lines().take(100).collect::<Vec<_>>().join("\n"),})
    } else {
        Ok(matches)
    }
}

#[cfg(test)]
#[path = "grep_files_tests.rs"]
mod tests;
