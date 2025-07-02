use std::path::PathBuf;

use ignore::Walk;

use crate::Error;

#[derive(Debug, serde::Serialize)]
pub(crate) struct Files(pub Vec<String>);

pub(crate) async fn fs_list_files(
    root: PathBuf,
    prefixes: Option<Vec<String>>,
    extensions: Option<Vec<String>>,
) -> std::result::Result<Files, Error> {
    let prefixes = prefixes.unwrap_or(vec![String::new()]);

    let mut entries = vec![];
    for prefix in &prefixes {
        let prefixed = root.join(prefix.trim_start_matches('/'));
        let matches = Walk::new(&prefixed)
            // Ignore invalid entries.
            .filter_map(Result::ok)
            // Ignore non-files.
            .filter(|entry| entry.file_type().is_some_and(|ft| ft.is_file()))
            // Ignore files that don't match the extension, if any.
            .filter(|entry| {
                extensions.as_ref().is_none_or(|extensions| {
                    entry
                        .path()
                        .extension()
                        .is_some_and(|ext| extensions.contains(&ext.to_string_lossy().into_owned()))
                })
            })
            // Strip non-workspace prefix from files.
            .filter_map(|entry| {
                entry
                    .into_path()
                    .strip_prefix(&root)
                    .ok()
                    .map(|path| path.to_string_lossy().into_owned())
            });

        entries.extend(matches);
    }

    entries.sort();
    entries.dedup();

    Ok(Files(entries))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use test_log::test;

    use super::*;

    #[test(tokio::test)]
    async fn test_list_files() {
        struct TestCase {
            prefixes: Vec<&'static str>,
            extensions: Vec<&'static str>,
            given: Vec<&'static str>,
            expected: Vec<&'static str>,
        }

        let cases = HashMap::from([
            ("sorted", TestCase {
                prefixes: vec![],
                extensions: vec![],
                given: vec!["test/a.txt", "test/b.txt"],
                expected: vec!["test/a.txt", "test/b.txt"],
            }),
            ("prefixed", TestCase {
                prefixes: vec!["test2"],
                extensions: vec![],
                given: vec!["test/a.txt", "test2/b.txt"],
                expected: vec!["test2/b.txt"],
            }),
            ("multiple-prefixes", TestCase {
                prefixes: vec!["one", "two"],
                extensions: vec![],
                given: vec!["one/a.txt", "two/b.txt", "nope/c.txt"],
                expected: vec!["one/a.txt", "two/b.txt"],
            }),
            ("extension", TestCase {
                prefixes: vec![],
                extensions: vec!["txt"],
                given: vec!["test/a.txt", "test/b.txt", "test/c.md"],
                expected: vec!["test/a.txt", "test/b.txt"],
            }),
            ("extension-multiple", TestCase {
                prefixes: vec![],
                extensions: vec!["rs", "md"],
                given: vec!["test/a.rs", "test/b.txt", "test/c.md"],
                expected: vec!["test/a.rs", "test/c.md"],
            }),
            ("nested-files", TestCase {
                prefixes: vec![],
                extensions: vec![],
                given: vec!["test/b.txt", "test/c.md", "test/d/e.txt"],
                expected: vec!["test/b.txt", "test/c.md", "test/d/e.txt"],
            }),
        ]);

        for (
            name,
            TestCase {
                prefixes,
                extensions,
                given,
                expected,
            },
        ) in cases
        {
            eprintln!("test {name}");
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();

            for path in given {
                let path = root.join(path);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).unwrap();
                }
                std::fs::write(path, "").unwrap();
            }

            let prefixes =
                (!prefixes.is_empty()).then_some(prefixes.into_iter().map(str::to_owned).collect());

            let extensions = (!extensions.is_empty())
                .then_some(extensions.into_iter().map(str::to_owned).collect());

            let files = fs_list_files(PathBuf::from(root), prefixes, extensions)
                .await
                .unwrap();

            assert_eq!(files.0, expected);
        }
    }
}
