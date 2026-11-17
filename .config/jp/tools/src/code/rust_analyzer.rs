//! `rust-analyzer`- and Rust-specific configuration and parsing.
//!
//! Isolates server-specific details from the generic LSP client: startup
//! command and initialization options, the `experimental/serverStatus`
//! readiness notification, hover and `impl` symbol formatting, and Rust syntax
//! helpers for paths, doc comments, attributes, and test layout.

use jp_tool::Capability;
use serde::Deserialize;
use serde_json::{Value, json};

use super::lsp::ReadySignal;
use crate::util::root::{CARGO_MANIFEST, Marker};

/// Default language server executable when `options.command` is unset.
pub const COMMAND: &str = "rust-analyzer";

/// LSP `languageId` sent when syncing Rust documents.
pub const LANGUAGE_ID: &str = "rust";

/// Workspace root marker file (`Cargo.toml`).
pub const ROOT: Marker = CARGO_MANIFEST;

/// Access capabilities required to run `rust-analyzer`, which compiles and
/// executes build scripts and proc macros in the target directory during
/// workspace loading.
pub const CAPABILITIES: &[Capability] = &[
    Capability::Read,
    Capability::Create,
    Capability::Update,
    Capability::Delete,
    Capability::Execute,
];

/// Maximum results requested from `workspace/symbol`.
///
/// Because `workspace/symbol` performs a fuzzy search on the final identifier
/// segment, a short name like `iter` can return thousands of fuzzy hits before
/// exact matches are filtered.
pub const SYMBOL_LIMIT: usize = 100_000;

pub const PATH_SEPARATOR: &str = "::";

/// Default `initializationOptions` sent to `rust-analyzer`, before merging the
/// tool's `settings` option.
#[must_use]
pub fn settings() -> Value {
    json!({
        // Disable flycheck so `rust-analyzer` does not run `cargo check` in the
        // background and contend with the `cargo_check` tool for the Cargo
        // build lock.
        "checkOnSave": false,
        // Exclude `use` imports from reference counts so widely imported types
        // only report actual usage sites.
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

/// Non-standard client capabilities advertised in `initialize`.
#[must_use]
pub fn experimental_capabilities() -> Value {
    json!({ "serverStatusNotification": true })
}

/// Readiness signal based on `rust-analyzer`'s `experimental/serverStatus`
/// notification.
pub const READY: ReadySignal = ReadySignal {
    method: "experimental/serverStatus",
    ready: server_status,
};

/// Payload of the `experimental/serverStatus` notification.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct ServerStatus {
    /// Server health: `"ok"`, `"warning"`, or `"error"`.
    health: String,

    /// Whether workspace loading and initial indexing have completed.
    quiescent: bool,

    #[serde(default)]
    message: Option<String>,
}

/// Evaluate a `ServerStatus` notification for [`READY`].
///
/// Returns `Some(Err(...))` when `quiescent` is true but `health` is `"error"`
/// (for example, when `cargo metadata` fails), since queries against a broken
/// workspace return incomplete results.
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

/// Parse an `impl` symbol name into `(trait_name, self_type)`.
///
/// Returns `Some((Some("Render"), "Stream"))` for `"impl Render for Stream"`
/// and `Some((None, "Stream<T>"))` for `"impl<T> Stream<T>"`.
#[must_use]
pub fn impl_parts(name: &str) -> Option<(Option<&str>, &str)> {
    let rest = skip_generics(name.strip_prefix("impl")?.trim_start()).trim();

    Some(match split_top_level(rest, " for ") {
        Some((trait_, self_ty)) => (Some(trait_.trim()), self_ty.trim()),
        None => (None, rest),
    })
}

/// Format a qualified trait-impl path segment (`<SelfType as Trait>`).
#[must_use]
pub fn impl_segment(trait_: &str, self_ty: &str) -> String {
    format!("<{self_ty} as {trait_}>")
}

/// Strip a leading `<...>` generic parameter list from `s`.
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

/// Split `s` at the first occurrence of `separator` outside angle brackets.
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

/// Extract the base type identifier from a type expression (e.g. `"Stream"`
/// from `"&mut crate::Stream<T>"`).
#[must_use]
pub fn base_ident(ty: &str) -> &str {
    let ty = ty.trim_start_matches('&').trim_start();
    let ty = ty.strip_prefix("mut ").unwrap_or(ty);
    let ty = ty.strip_prefix("dyn ").unwrap_or(ty);
    let ty = ty.split('<').next().unwrap_or(ty);

    ty.rsplit(PATH_SEPARATOR).next().unwrap_or(ty).trim()
}

#[must_use]
pub fn is_test_module(name: &str) -> bool {
    matches!(name, "tests" | "test")
}

/// Return whether a workspace-relative path is a test or benchmark file by
/// naming convention (`tests/`, `benches/`, `test.rs`, `tests.rs`, `*_test.rs`,
/// or `*_tests.rs`).
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

/// Return whether a leading-trimmed source line is an outer/inner doc comment
/// or attribute preceding an item's declaration.
#[must_use]
pub fn is_preamble(line: &str) -> bool {
    line.starts_with("///")
        || line.starts_with("//!")
        || line.starts_with("#[")
        || line.starts_with("#![")
}

/// Strip the `///` prefix from a leading-trimmed outer doc comment line.
#[must_use]
pub fn doc_text(line: &str) -> Option<&str> {
    let text = line.strip_prefix("///")?;
    Some(text.strip_prefix(' ').unwrap_or(text).trim_end())
}

/// Extracted header fields from a `rust-analyzer` hover response.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct HoverParts {
    /// Enclosing module or type path (e.g. `fixture::stream::Stream`).
    pub container: Option<String>,
    pub signature: Vec<String>,
}

/// Parse the container path and item signature (truncated to `max` lines) from
/// a `rust-analyzer` hover markdown string.
///
/// `rust-analyzer` formats the container path and item signature as up to two
/// fenced code blocks before the first `---` horizontal rule.
/// Everything after the rule (doc comments, dyn-compatibility notes, memory
/// layout) is ignored; doc comments are read directly from the source file
/// instead.
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
