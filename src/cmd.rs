use std::{env, path::PathBuf};

use clap::error::ErrorKind;
use path_clean::PathClean as _;

pub mod ask;
pub mod config;
pub mod serve;

// Custom value parser for paths
pub fn canonical_path(s: &str) -> Result<PathBuf, clap::Error> {
    #[allow(clippy::obfuscated_if_else)]
    let path = s
        .starts_with("~/")
        .then_some(
            env::var("HOME")
                .map(|home| PathBuf::from(s.replacen("~", &home, 1)))
                .map_err(|_| {
                    clap::Error::raw(
                        ErrorKind::InvalidValue,
                        "Could not expand '~': HOME environment variable not set",
                    )
                }),
        )
        .unwrap_or(Ok(PathBuf::from(s)))?
        .clean();

    Ok(std::fs::canonicalize(&path).unwrap_or(path))
}
