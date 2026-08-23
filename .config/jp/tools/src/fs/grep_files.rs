use camino::{Utf8Path, Utf8PathBuf};
use grep_printer::StandardBuilder;
use grep_searcher::{BinaryDetection, SearcherBuilder};
use ignore::gitignore::Gitignore;
use jp_tool::AccessPolicy;
use matcher::FancyMatcher;

use super::fs_list_files;
use crate::{Error, util::OneOrMany};

mod matcher;

pub(crate) async fn fs_grep_files(
    root: &Utf8Path,
    access: Option<&AccessPolicy>,
    mut pattern: String,
    context: Option<usize>,
    paths: Option<OneOrMany<String>>,
    extensions: Option<OneOrMany<String>>,
    suppress: &Gitignore,
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
    // as a hard error from the shared path validation. The access policy is
    // threaded through so an approved external mount can be searched.
    let listing = fs_list_files(root, access, paths.clone(), extensions.clone(), suppress).await?;

    let notes = listing.notes();
    let files: Vec<Utf8PathBuf> = listing
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

    let matcher = FancyMatcher::new(&pattern)?;

    let mut printer = StandardBuilder::new()
        .max_columns(Some(1000))
        .max_columns_preview(true)
        .trim_ascii(true)
        .build_no_color(vec![]);

    let mut searcher = SearcherBuilder::new()
        .before_context(context.unwrap_or(0))
        .after_context(context.unwrap_or(0))
        .max_matches(Some(100))
        // Stop reading a file once a NUL byte appears, as ripgrep does. Without
        // this the searcher prints raw bytes from object files and archives,
        // and the UTF-8 decode below then fails the *whole* search rather than
        // the one file that caused it.
        .binary_detection(BinaryDetection::quit(0))
        .build();

    for file in files {
        let absolute = root.join(&file);
        searcher.search_path(&matcher, &absolute, printer.sink_with_path(&matcher, &file))?;
    }

    let matches = String::from_utf8(printer.into_inner().into_inner())?;

    let lines = matches.lines().count();
    let body = if matches.is_empty() {
        // A search whose requested paths were skipped finds nothing for a
        // completely different reason than a search that came up empty, and the
        // notes below say which happened.
        if notes.is_empty() {
            "No matches found. Broaden your search to see more.".to_owned()
        } else {
            "No matches found in the paths that were searched.".to_owned()
        }
    } else if lines > 200 && context.is_some() {
        // The inner call reproduces the notes, so they are not appended twice.
        return Box::pin(fs_grep_files(
            root, access, pattern, None, paths, extensions, suppress,
        ))
        .await
        .map(|v| {
            format!(
                "{v}\n[Hidden contextual lines due to excessive number of lines returned. Narrow \
                 down your search to see more.]"
            )
        });
    } else if lines > 100 {
        indoc::formatdoc! {"
            {}

            [Showing 100/{lines} lines of matches... Narrow down your search to see more.]
        ", matches.lines().take(100).collect::<Vec<_>>().join("\n"),}
    } else {
        matches
    };

    Ok(append_notes(body, &notes))
}

/// Append the listing's skip notes to a search result.
///
/// Without them, a search whose requested paths were skipped reports the same
/// empty result as a search that genuinely found nothing.
fn append_notes(body: String, notes: &[String]) -> String {
    if notes.is_empty() {
        return body;
    }

    let notes = notes
        .iter()
        .map(|note| format!("Note: {note}"))
        .collect::<Vec<_>>()
        .join("\n");

    format!("{body}\n\n{notes}")
}

#[cfg(test)]
#[path = "grep_files_tests.rs"]
mod tests;
