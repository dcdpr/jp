//! What the code tools know about rust-analyzer and the Rust it reads.
//!
//! Everything else in the code tools speaks plain LSP.
//! This module holds what is true only of rust-analyzer: the command and
//! settings it starts with, the notification it reports a loaded workspace
//! through, and how it names items in its document symbols and hovers.
//! It also holds the Rust syntax the tools read from source text: `::` paths,
//! doc comments and attributes, and how test code is laid out.

use jp_tool::Capability;
use serde::Deserialize;
use serde_json::{Value, json};

use super::lsp::ReadySignal;
use crate::util::root::{CARGO_MANIFEST, Marker};

/// The server command, unless the `command` tool option names another.
pub const COMMAND: &str = "rust-analyzer";

/// The language a document is opened as, when the tools send it its text.
pub const LANGUAGE_ID: &str = "rust";

/// The entry that marks a directory the server can load as a workspace.
pub const ROOT: Marker = CARGO_MANIFEST;

/// rust-analyzer runs build scripts and proc macros while it loads the
/// workspace, and builds them into the target directory: everything cargo's
/// compiling commands need.
pub const CAPABILITIES: &[Capability] = &[
    Capability::Read,
    Capability::Create,
    Capability::Update,
    Capability::Delete,
    Capability::Execute,
];

/// The most results the server's symbol search returns.
///
/// The search matches fuzzily and is searched for a name's last segment alone,
/// so a common name like `iter` returns thousands of results, few of them named
/// exactly `iter`.
/// On this workspace `iter` returns about 5,000 in under a second.
pub const SYMBOL_LIMIT: usize = 100_000;

/// What separates the segments of a path: `Stream::iter`.
pub const PATH_SEPARATOR: &str = "::";

/// The settings rust-analyzer starts with, sent as `initializationOptions`.
///
/// The `settings` tool option is merged over these.
#[must_use]
pub fn settings() -> Value {
    json!({
        // A check would run `cargo check` behind the call and contend with
        // `cargo_check` for the build lock.
        "checkOnSave": false,
        // An import is not a use: it says where a name comes from, and a
        // widely used type is imported in nearly every file.
        "references": { "excludeImports": true },
        "workspace": { "symbol": { "search": { "kind": "all_symbols", "limit": SYMBOL_LIMIT } } },
        "hover": {
            "memoryLayout": { "enable": false },
            "dropGlue": { "enable": false },
            "actions": { "enable": false },
        },
        "lens": { "enable": false },
    })
}

/// The client capabilities outside the standard that the tools rely on.
#[must_use]
pub fn experimental_capabilities() -> Value {
    json!({ "serverStatusNotification": true })
}

/// How rust-analyzer reports that it has loaded and indexed the workspace.
pub const READY: ReadySignal = ReadySignal {
    method: "experimental/serverStatus",
    ready: server_status,
};

/// The progress rust-analyzer reports through `experimental/serverStatus`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct ServerStatus {
    /// `ok`, `warning`, or `error`.
    health: String,

    /// Whether the server has finished loading and indexing the workspace.
    quiescent: bool,

    #[serde(default)]
    message: Option<String>,
}

/// Whether a status notification says the workspace is loaded.
///
/// A server that is done but in `error` health, such as rust-analyzer after
/// `cargo metadata` failed, is not ready: its answers would be incomplete.
fn server_status(params: &Value) -> Option<Result<(), String>> {
    let status = serde_json::from_value::<ServerStatus>(params.clone()).ok()?;
    if !status.quiescent {
        return None;
    }

    Some(if status.health == "error" {
        Err(status.message.unwrap_or_default())
    } else {
        Ok(())
    })
}

/// The trait and self type an `impl` symbol names.
///
/// `impl Render for Stream` gives `(Some("Render"), "Stream")` and `impl<T>
/// Stream<T>` gives `(None, "Stream<T>")`.
#[must_use]
pub fn impl_parts(name: &str) -> Option<(Option<&str>, &str)> {
    let rest = skip_generics(name.strip_prefix("impl")?.trim_start()).trim();

    Some(match split_top_level(rest, " for ") {
        Some((trait_, self_ty)) => (Some(trait_.trim()), self_ty.trim()),
        None => (None, rest),
    })
}

/// The path segment that names a trait impl's items at a call site: `<Stream as
/// Render>`.
#[must_use]
pub fn impl_segment(trait_: &str, self_ty: &str) -> String {
    format!("<{self_ty} as {trait_}>")
}

/// `s` without a leading `<...>` parameter list.
fn skip_generics(s: &str) -> &str {
    if !s.starts_with('<') {
        return s;
    }

    let mut depth = 0usize;
    for (at, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return &s[at + 1..];
                }
            }
            _ => {}
        }
    }

    s
}

/// Split `s` at the first `separator` outside angle brackets.
fn split_top_level<'a>(s: &'a str, separator: &str) -> Option<(&'a str, &'a str)> {
    let mut depth = 0usize;
    for (at, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 && s[at..].starts_with(separator) => {
                return Some((&s[..at], &s[at + separator.len()..]));
            }
            _ => {}
        }
    }

    None
}

/// The name a type is written with: `Stream` for `&mut crate::Stream<T>`.
#[must_use]
pub fn base_ident(ty: &str) -> &str {
    let ty = ty.trim_start_matches('&').trim_start();
    let ty = ty.strip_prefix("mut ").unwrap_or(ty);
    let ty = ty.strip_prefix("dyn ").unwrap_or(ty);
    let ty = ty.split('<').next().unwrap_or(ty);

    ty.rsplit(PATH_SEPARATOR).next().unwrap_or(ty).trim()
}

/// Whether a module of this name holds test code: `tests` or `test`.
#[must_use]
pub fn is_test_module(name: &str) -> bool {
    matches!(name, "tests" | "test")
}

/// Whether a workspace-relative path holds test code by its name alone: a
/// `tests` or `benches` directory, or a file named `tests.rs`, `test.rs`, or
/// `*_tests.rs`.
#[must_use]
pub fn is_test_path(path: &str) -> bool {
    let mut components = path.split('/').collect::<Vec<_>>();
    let file = components.pop().unwrap_or_default();
    let stem = file.strip_suffix(".rs").unwrap_or(file);

    components
        .iter()
        .any(|dir| matches!(*dir, "tests" | "benches"))
        || is_test_module(stem)
        || stem.ends_with("_tests")
        || stem.ends_with("_test")
}

/// Whether a line, already trimmed at the start, comes before an item's code: a
/// doc comment or an attribute.
///
/// rust-analyzer's item range starts at the item's doc comment.
#[must_use]
pub fn is_preamble(line: &str) -> bool {
    line.starts_with("///")
        || line.starts_with("//!")
        || line.starts_with("#[")
        || line.starts_with("#![")
}

/// The text of an outer doc comment line, already trimmed at the start: `A
/// stream.` for `/// A stream.`.
#[must_use]
pub fn doc_text(line: &str) -> Option<&str> {
    let text = line.strip_prefix("///")?;
    Some(text.strip_prefix(' ').unwrap_or(text).trim_end())
}

/// The parts of a hover that the header shows.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct HoverParts {
    /// The path of the enclosing module or type: `fixture::stream::Stream`.
    pub container: Option<String>,
    pub signature: Vec<String>,
}

/// Read the container path and signature from a hover, at most `max` signature
/// lines.
///
/// rust-analyzer writes the container and the signature as two fenced code
/// blocks before the first `---` rule.
/// The sections after the rule (doc comment, dyn-compatibility, memory layout)
/// vary by item kind and are not read; the header takes the doc comment from
/// the source instead.
#[must_use]
pub fn parse_hover(markdown: &str, max: usize) -> HoverParts {
    let mut blocks: Vec<Vec<String>> = vec![];
    let mut in_code = false;

    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            if in_code {
                blocks.push(vec![]);
            }
        } else if in_code {
            if let Some(block) = blocks.last_mut() {
                block.push(line.to_owned());
            }
        } else if matches!(line.trim(), "---" | "___") {
            break;
        }
    }

    let (container, signature) = match blocks.len() {
        0 => (None, vec![]),
        1 => (None, blocks.swap_remove(0)),
        _ => {
            let signature = blocks.swap_remove(1);
            (Some(blocks.swap_remove(0).join(" ")), signature)
        }
    };

    HoverParts {
        container: container.filter(|container| !container.is_empty()),
        signature: super::render::cap(signature, max),
    }
}

#[cfg(test)]
#[path = "rust_analyzer_tests.rs"]
mod tests;
