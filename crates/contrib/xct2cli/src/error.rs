use std::path::PathBuf;

use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("trace bundle not found: {0}")]
    BundleMissing(PathBuf),

    #[error("`xctrace` not found on PATH or at {0}")]
    XctraceMissing(PathBuf),

    #[error("`xctrace {subcommand}` exited with status {status}: {stderr}")]
    XctraceFailed {
        subcommand: &'static str,
        status: std::process::ExitStatus,
        stderr: String,
    },

    #[error("XML parse error: {0}")]
    Xml(#[from] quick_xml::Error),

    #[error("XML encoding error: {0}")]
    XmlEncoding(#[from] quick_xml::encoding::EncodingError),

    #[error("XML escape error: {0}")]
    XmlEscape(#[from] quick_xml::escape::EscapeError),

    #[error("XML deserialize error: {0}")]
    XmlDe(#[from] quick_xml::DeError),

    #[error("malformed trace XML: {0}")]
    Schema(String),

    #[error("unresolved cell reference: id={0}")]
    UnresolvedRef(u64),

    #[error("table with schema {0:?} not found in TOC")]
    TableMissing(String),

    #[error("Mach-O parse error: {0}")]
    MachO(#[from] object::Error),

    #[error("DWARF parse error: {0}")]
    Dwarf(#[from] gimli::Error),

    #[error("addr2line error: {0}")]
    Addr2Line(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Utf8(#[from] std::str::Utf8Error),

    #[error(transparent)]
    ParseInt(#[from] std::num::ParseIntError),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
