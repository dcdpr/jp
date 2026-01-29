use std::path::PathBuf;

use camino::{Utf8Path, Utf8PathBuf};
use grep_printer::StandardBuilder;
use grep_regex::RegexMatcher;
use grep_searcher::SearcherBuilder;

use crate::{Error, util::OneOrMany};

pub(crate) async fn fs_grep_files(
    root: &Utf8Path,
    pattern: String,
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

    let matcher = RegexMatcher::new(&pattern)?;

    let mut printer = StandardBuilder::new()
        .max_columns(Some(1000))
        .max_columns_preview(true)
        .max_matches(Some(100))
        .trim_ascii(true)
        .build_no_color(vec![]);

    let mut searcher = SearcherBuilder::new()
        .before_context(context.unwrap_or(0))
        .after_context(context.unwrap_or(0))
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
mod tests {
    use std::collections::HashMap;

    use camino_tempfile::tempdir;

    use super::*;

    #[tokio::test]
    #[test_log::test]
    async fn test_grep_files() {
        struct TestCase {
            pattern: &'static str,
            paths: Vec<&'static str>,
            given: Vec<(&'static str, &'static str)>,
            expected: Vec<&'static str>,
        }

        let cases = HashMap::from([
            ("pattern", TestCase {
                pattern: "hi",
                paths: vec!["test/a.txt"],
                given: vec![("test/a.txt", "hello\nhi\ngoodbye")],
                expected: vec![
                    "test/a.txt-1-hello\n",
                    "test/a.txt:2:hi\n",
                    "test/a.txt-3-goodbye\n",
                ],
            }),
            ("dont-return-entire-file", TestCase {
                pattern: "1|2|3",
                paths: vec!["test/a.txt"],
                given: vec![("test/a.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9")],
                expected: vec![
                    "test/a.txt:1:1\n",
                    "test/a.txt:2:2\n",
                    "test/a.txt:3:3\n",
                    "test/a.txt-4-4\n",
                    "test/a.txt-5-5\n",
                    "test/a.txt-6-6\n",
                    "test/a.txt-7-7\n",
                    "test/a.txt-8-8\n",
                ],
            }),
            ("multiple-files", TestCase {
                pattern: "1|2|3",
                paths: vec!["test/a.txt", "test/b.txt"],
                given: vec![
                    ("test/a.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9"),
                    ("test/b.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9"),
                ],
                expected: vec![
                    "test/a.txt:1:1\n",
                    "test/a.txt:2:2\n",
                    "test/a.txt:3:3\n",
                    "test/a.txt-4-4\n",
                    "test/a.txt-5-5\n",
                    "test/a.txt-6-6\n",
                    "test/a.txt-7-7\n",
                    "test/a.txt-8-8\n",
                    "test/b.txt:1:1\n",
                    "test/b.txt:2:2\n",
                    "test/b.txt:3:3\n",
                    "test/b.txt-4-4\n",
                    "test/b.txt-5-5\n",
                    "test/b.txt-6-6\n",
                    "test/b.txt-7-7\n",
                    "test/b.txt-8-8\n",
                ],
            }),
            ("multiple-files", TestCase {
                pattern: "foo",
                paths: vec![],
                given: vec![("test/a.txt", "foo"), ("test/b.txt", "bar")],
                expected: vec!["test/a.txt:1:foo\n"],
            }),
            ("search-in-subdir", TestCase {
                pattern: "foo",
                paths: vec!["test/subdir"],
                given: vec![
                    ("test/a.txt", "baz"),
                    ("test/b.txt", "bar"),
                    ("test/subdir/c.txt", "foo"),
                ],
                expected: vec!["test/subdir/c.txt:1:foo\n"],
            }),
        ]);

        for (
            name,
            TestCase {
                pattern,
                paths,
                given,
                expected,
            },
        ) in cases
        {
            let tmp = tempdir().unwrap();
            let root = tmp.path();

            for (path, content) in given {
                let path = root.join(path);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).unwrap();
                }
                std::fs::write(path, content).unwrap();
            }

            let paths =
                (!paths.is_empty()).then_some(paths.into_iter().map(str::to_owned).collect());

            let matches = fs_grep_files(root, pattern.to_owned(), Some(5), paths)
                .await
                .unwrap();

            assert_eq!(matches, expected.join(""), "test case: {name}");
        }
    }
}
