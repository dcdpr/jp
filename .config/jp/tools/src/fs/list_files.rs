use camino::{Utf8Path, Utf8PathBuf};
use ignore::{WalkBuilder, WalkState};

use crate::{Error, util::OneOrMany};

#[derive(Debug)]
pub(crate) enum Files {
    Empty,
    List(Vec<String>),
}

impl Files {
    pub(crate) fn into_files(self) -> Vec<String> {
        match self {
            Files::Empty => vec![],
            Files::List(files) => files,
        }
    }
}

impl serde::Serialize for Files {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Files::Empty => serializer.serialize_str("No files found."),
            Files::List(files) => files.serialize(serializer),
        }
    }
}

pub(crate) async fn fs_list_files(
    root: &Utf8Path,
    prefixes: Option<OneOrMany<String>>,
    extensions: Option<OneOrMany<String>>,
) -> std::result::Result<Files, Error> {
    let prefixes = prefixes.unwrap_or(OneOrMany::One(String::new())).into_vec();

    let mut entries = vec![];
    for prefix in &prefixes {
        let prefixed = root.join(prefix.trim_start_matches('/'));

        let (tx, matches) = crossbeam_channel::unbounded();
        WalkBuilder::new(&prefixed)
            // Include hidden and otherwise ignored files.
            .standard_filters(false)
            .follow_links(false)
            // Respect `.ignore` files (also in parent directories).
            .ignore(true)
            .parents(true)
            .build_parallel()
            .run(|| {
                let tx = tx.clone();
                let extensions = extensions.clone();
                Box::new(move |entry| {
                    // Ignore invalid entries.
                    let Ok(entry) = entry else {
                        return WalkState::Continue;
                    };

                    // Ignore non-files.
                    if entry.file_type().is_none_or(|ft| !ft.is_file()) {
                        return WalkState::Continue;
                    }

                    // Ignore files that don't match the extension, if any.
                    if extensions.as_ref().is_some_and(|extensions| {
                        entry.path().extension().is_some_and(|ext| {
                            !extensions.contains(&ext.to_string_lossy().into_owned())
                        })
                    }) {
                        return WalkState::Continue;
                    }

                    let Ok(path) = Utf8PathBuf::try_from(entry.into_path()) else {
                        return WalkState::Continue;
                    };

                    // Strip non-workspace prefix from files.
                    let Ok(path) = path.strip_prefix(root) else {
                        return WalkState::Continue;
                    };

                    let _result = tx.send(path.to_string());

                    WalkState::Continue
                })
            });

        drop(tx);
        entries.extend(matches);
    }

    if entries.is_empty() {
        return Ok(Files::Empty);
    }

    entries.sort();
    entries.dedup();

    Ok(Files::List(entries))
}

#[cfg(test)]
#[path = "list_files_tests.rs"]
mod tests;
