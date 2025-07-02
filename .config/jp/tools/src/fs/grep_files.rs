use std::{io, path::PathBuf};

use grep_printer::Standard;
use grep_regex::RegexMatcher;
use grep_searcher::{Searcher, SearcherBuilder, Sink, SinkContext, SinkError as _, SinkMatch};

use crate::Error;

pub(crate) async fn fs_grep_files(
    root: PathBuf,
    pattern: String,
    paths: Option<Vec<String>>,
    return_entire_file: Option<bool>,
) -> std::result::Result<String, Error> {
    let paths: Vec<_> = paths
        .unwrap_or(vec![String::new()])
        .into_iter()
        .map(|v| root.join(v.trim_start_matches('/')))
        .filter(|v| v.exists())
        .collect();

    let matcher = RegexMatcher::new(&pattern)?;
    let mut printer = Standard::new_no_color(vec![]);
    let mut searcher = SearcherBuilder::new()
        .before_context(5)
        .after_context(5)
        .passthru(return_entire_file.unwrap_or_default())
        .build();

    for path in paths {
        let files = if path.is_dir() {
            super::fs_list_files(path.clone(), None, None)
                .await?
                .0
                .into_iter()
                .map(PathBuf::from)
                .map(|p| root.join(&path).join(p))
                .filter(|path| path.exists())
                .collect()
        } else {
            vec![path]
        };

        for file in files {
            let Ok(path) = file.strip_prefix(&root).map(PathBuf::from) else {
                continue;
            };

            searcher.search_path(&matcher, &file, printer.sink_with_path(&matcher, &path))?;
        }
    }

    Ok(String::from_utf8(printer.into_inner().into_inner())?)
}

#[derive(Clone, Debug)]
pub struct LossyWithCtx<F>(pub F)
where
    F: FnMut(u64, &str) -> Result<bool, io::Error>;

impl<F> Sink for LossyWithCtx<F>
where
    F: FnMut(u64, &str) -> Result<bool, io::Error>,
{
    type Error = io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, io::Error> {
        use std::borrow::Cow;

        let matched = match std::str::from_utf8(mat.bytes()) {
            Ok(matched) => Cow::Borrowed(matched),
            Err(_) => String::from_utf8_lossy(mat.bytes()),
        };
        let Some(line_number) = mat.line_number() else {
            let msg = "line numbers not enabled";
            return Err(io::Error::error_message(msg));
        };

        (self.0)(line_number, &matched)
    }

    fn context(&mut self, _searcher: &Searcher, mat: &SinkContext<'_>) -> Result<bool, io::Error> {
        use std::borrow::Cow;

        let matched = match std::str::from_utf8(mat.bytes()) {
            Ok(matched) => Cow::Borrowed(matched),
            Err(_) => String::from_utf8_lossy(mat.bytes()),
        };
        let Some(line_number) = mat.line_number() else {
            let msg = "line numbers not enabled";
            return Err(io::Error::error_message(msg));
        };

        (self.0)(line_number, &matched)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use test_log::test;

    use super::*;

    #[test(tokio::test)]
    async fn test_grep_files() {
        struct TestCase {
            pattern: &'static str,
            paths: Vec<&'static str>,
            return_entire_file: bool,
            given: Vec<(&'static str, &'static str)>,
            expected: Vec<&'static str>,
        }

        let cases = HashMap::from([
            ("pattern", TestCase {
                pattern: "hi",
                paths: vec!["test/a.txt"],
                return_entire_file: false,
                given: vec![("test/a.txt", "hello\nhi\ngoodbye")],
                expected: vec![
                    "test/a.txt-1-hello\n",
                    "test/a.txt:2:hi\n",
                    "test/a.txt-3-goodbye\n",
                ],
            }),
            ("return-entire-file", TestCase {
                pattern: "1|2|3",
                paths: vec!["test/a.txt"],
                return_entire_file: true,
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
                    "test/a.txt-9-9\n",
                ],
            }),
            ("dont-return-entire-file", TestCase {
                pattern: "1|2|3",
                paths: vec!["test/a.txt"],
                return_entire_file: false,
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
                return_entire_file: false,
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
                return_entire_file: false,
                given: vec![("test/a.txt", "foo"), ("test/b.txt", "bar")],
                expected: vec!["test/a.txt:1:foo\n"],
            }),
            ("search-in-subdir", TestCase {
                pattern: "foo",
                paths: vec!["test/subdir"],
                return_entire_file: false,
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
                return_entire_file,
                given,
                expected,
            },
        ) in cases
        {
            let tmp = tempfile::tempdir().unwrap();
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

            let matches = fs_grep_files(
                PathBuf::from(root),
                pattern.to_owned(),
                paths,
                Some(return_entire_file),
            )
            .await
            .unwrap();

            assert_eq!(matches, expected.join(""), "test case: {name}");
        }
    }
}
