use camino::{Utf8Path, Utf8PathBuf};
use grep_printer::StandardBuilder;
use grep_regex::RegexMatcher;
use grep_searcher::SearcherBuilder;

use super::fs_list_files;
use crate::{Error, util::OneOrMany};

pub(crate) async fn fs_grep_files(
    root: &Utf8Path,
    mut pattern: String,
    context: Option<usize>,
    paths: Option<OneOrMany<String>>,
    extensions: Option<OneOrMany<String>>,
) -> std::result::Result<String, Error> {
    // Resolve the file set via `fs_list_files`, which always walks from the
    // workspace root. Anchoring the walk there is what makes the root
    // `.ignore` whitelist apply consistently: its anchored patterns (e.g.
    // `docs/.vitepress/dist/`) don't prune reliably when the walk is rooted
    // below the `.ignore` file, so scoping by re-rooting would leak ignored
    // build output into the results.
    //
    // `paths` carries the same scoping semantics as `fs_list_files`'s
    // prefixes: `None` searches the whole workspace, `Some([])` searches
    // nothing, and `""`/`.` mean the workspace root. Escape attempts surface
    // as a hard error from the shared path validation.
    let files: Vec<Utf8PathBuf> = fs_list_files(root, paths.clone(), extensions.clone())
        .await?
        .into_files()
        .into_iter()
        .map(Utf8PathBuf::from)
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

    for file in files {
        let absolute = root.join(&file);
        searcher.search_path(&matcher, &absolute, printer.sink_with_path(&matcher, &file))?;
    }

    let matches = String::from_utf8(printer.into_inner().into_inner())?;

    let lines = matches.lines().count();
    if matches.is_empty() {
        Ok("No matches found. Broaden your search to see more.".to_owned())
    } else if lines > 200 && context.is_some() {
        Box::pin(fs_grep_files(root, pattern, None, paths, extensions))
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
