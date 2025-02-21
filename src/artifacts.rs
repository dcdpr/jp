//! Module for managing file artifacts in conversations

use std::{fs, path::PathBuf};

use exodus_trace::error;
use ignore::WalkBuilder;

use crate::context::Context;

/// Represents a file artifact to be included in a conversation
#[derive(Debug, Clone)]
pub struct FileArtifact {
    pub path: PathBuf,
    pub content: String,
    pub relative_path: PathBuf,
}

pub struct ArtifactIterator {
    walk: Option<ignore::Walk>,
    root: Option<PathBuf>,
}

impl Iterator for ArtifactIterator {
    type Item = FileArtifact;

    fn next(&mut self) -> Option<Self::Item> {
        let Some(walk) = &mut self.walk else {
            return None;
        };

        let Some(root) = &self.root else {
            return None;
        };

        let path = match walk.next()? {
            Ok(e) => e.path().to_path_buf(),
            Err(e) => {
                error!("Failed to read file: {e:?}");
                return self.next();
            }
        };

        // Skip directories
        if path.is_dir() {
            return self.next();
        }

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to read file content: {e:?}");
                return self.next();
            }
        };

        let relative_path = match path.strip_prefix(root) {
            Ok(p) => p.to_path_buf(),
            Err(e) => {
                error!("Failed to strip prefix: {e:?}");
                return self.next();
            }
        };

        Some(FileArtifact {
            path,
            content,
            relative_path,
        })
    }
}

pub fn iter(ctx: &Context) -> ArtifactIterator {
    let root_opt = ctx.workspace.as_ref().map(|w| w.root.clone());

    if let Some(root) = &root_opt {
        let mut walker = WalkBuilder::new(root);
        walker.standard_filters(false);

        let ignore_path = root.join(&ctx.config.artifacts.ignorefile);
        if ignore_path.exists() {
            walker.add_ignore(ignore_path);
        }

        // // TODO: make this configurable.
        // walker.filter_entry(|entry| {
        //     entry.depth() < 6 && entry.path().exists() && entry.path().is_file()
        // });

        ArtifactIterator {
            walk: Some(walker.build()),
            root: Some(root.clone()),
        }
    } else {
        ArtifactIterator {
            walk: None,
            root: None,
        }
    }
}
