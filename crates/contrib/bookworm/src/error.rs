use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("missing parameter: {0}")]
    MissingParameter(&'static str),

    #[error("invalid parameter: {0}")]
    InvalidParameter(String),

    #[error("crate not found: {0}")]
    CrateNotFound(String),

    #[error("version {version} not found for crate {crate_name}")]
    VersionNotFound { crate_name: String, version: String },

    #[error("invalid resource URI: {0}")]
    InvalidResourceUri(String),

    #[error("resource not found: {0}")]
    ResourceNotFound(String),

    #[error("documentation not found at path: {0}")]
    DocNotFoundAtPath(PathBuf),

    #[error("source path does not exist: {0}")]
    SourceNotFound(PathBuf),

    #[error("source path is not a directory: {0}")]
    SourceNotDirectory(PathBuf),

    #[error("missing docs")]
    MissingDocs,

    #[error("not found")]
    NotFound,

    #[error("invalid configuration: {0}")]
    Config(String),

    #[error("invalid response from crates.io")]
    InvalidResponse,

    #[error("HTML parsing error: {0}")]
    HtmlParsing(String),

    #[error("unknown entry type: {0}")]
    UnknownEntryType(String),

    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),

    #[error(transparent)]
    Url(#[from] url::ParseError),

    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),

    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),

    #[error("HTML to markdown conversion error: {0}")]
    HtmlToMarkdown(String),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    ParseInt(#[from] std::num::ParseIntError),
}
