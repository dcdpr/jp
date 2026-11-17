//! Subset of Language Server Protocol types used by the code tools.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use url::Url;

/// Zero-based line and character offset in the server's negotiated encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

impl Range {
    #[must_use]
    pub fn contains(&self, position: Position) -> bool {
        self.start <= position && position <= self.end
    }

    #[must_use]
    pub fn spans_line(&self, line: u32) -> bool {
        self.start.line <= line && line <= self.end.line
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
pub struct Location {
    pub uri: Url,
    pub range: Range,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocationLink {
    pub target_uri: Url,
    pub target_selection_range: Range,
}

/// Untyped union of responses returned by `textDocument/definition` and
/// `textDocument/implementation`.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum GotoResponse {
    One(Location),
    Many(Vec<Location>),
    Links(Vec<LocationLink>),
}

impl GotoResponse {
    #[must_use]
    pub fn into_locations(self) -> Vec<Location> {
        match self {
            Self::One(location) => vec![location],
            Self::Many(locations) => locations,
            Self::Links(links) => links
                .into_iter()
                .map(|link| Location {
                    uri: link.target_uri,
                    range: link.target_selection_range,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSymbol {
    pub name: String,
    pub kind: u32,
    pub range: Range,
    pub selection_range: Range,
    #[serde(default)]
    pub children: Vec<DocumentSymbol>,
}

/// Nested syntax node ranges around a position, returned by
/// `textDocument/selectionRange`.
#[derive(Debug, Clone, Deserialize)]
pub struct SelectionRange {
    pub range: Range,
    #[serde(default)]
    pub parent: Option<Box<SelectionRange>>,
}

impl SelectionRange {
    /// Collect this range and its ancestors, from innermost to outermost.
    #[must_use]
    pub fn chain(&self) -> Vec<Range> {
        let mut ranges = vec![self.range];
        let mut parent = self.parent.as_deref();
        while let Some(node) = parent {
            ranges.push(node.range);
            parent = node.parent.as_deref();
        }

        ranges
    }
}

/// Function or method node returned by `textDocument/prepareCallHierarchy`.
///
/// Unrecognized fields are preserved in `rest` so the item can be echoed back
/// to the server in `callHierarchy/incomingCalls` and `outgoingCalls`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallHierarchyItem {
    pub name: String,
    pub kind: u32,
    pub uri: Url,
    pub selection_range: Range,
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

/// Incoming call hierarchy edge returned by `callHierarchy/incomingCalls`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomingCall {
    pub from: CallHierarchyItem,
    pub from_ranges: Vec<Range>,
}

/// Outgoing call hierarchy edge returned by `callHierarchy/outgoingCalls`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutgoingCall {
    pub to: CallHierarchyItem,
    pub from_ranges: Vec<Range>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextEdit {
    pub range: Range,
    pub new_text: String,
}

/// Workspace edit returned by `textDocument/rename`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceEdit {
    #[serde(default)]
    pub changes: Option<HashMap<String, Vec<TextEdit>>>,
    #[serde(default)]
    pub document_changes: Option<Vec<Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentChange {
    text_document: DocumentId,
    edits: Vec<TextEdit>,
}

#[derive(Debug, Deserialize)]
struct DocumentId {
    uri: Url,
}

impl WorkspaceEdit {
    /// Extract per-file text edits sorted by file URI, rejecting resource
    /// operations (file creation, rename, or deletion).
    pub fn into_text_edits(self) -> Result<Vec<(Url, Vec<TextEdit>)>, String> {
        let mut files = vec![];
        for (uri, edits) in self.changes.unwrap_or_default() {
            let uri = Url::parse(&uri).map_err(|error| format!("`{uri}`: {error}"))?;
            files.push((uri, edits));
        }

        for change in self.document_changes.unwrap_or_default() {
            if let Some(kind) = change.get("kind").and_then(Value::as_str) {
                return Err(format!("it would also {kind} a file"));
            }

            let change = serde_json::from_value::<DocumentChange>(change)
                .map_err(|error| format!("unexpected document change: {error}"))?;
            files.push((change.text_document.uri, change.edits));
        }

        files.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        Ok(files)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolInformation {
    pub name: String,
    pub kind: u32,
    pub location: Location,
    #[serde(default)]
    pub container_name: Option<String>,
}

/// LSP `SymbolKind` constants.
pub mod kind {
    pub const MODULE: u32 = 2;
    pub const METHOD: u32 = 6;
    pub const FIELD: u32 = 8;
    pub const CONSTRUCTOR: u32 = 9;
    pub const ENUM: u32 = 10;
    pub const INTERFACE: u32 = 11;
    pub const FUNCTION: u32 = 12;
    pub const VARIABLE: u32 = 13;
    pub const CONSTANT: u32 = 14;
    pub const OBJECT: u32 = 19;
    pub const ENUM_MEMBER: u32 = 22;
    pub const STRUCT: u32 = 23;
    pub const TYPE_PARAMETER: u32 = 26;
}

/// Map an LSP `SymbolKind` to its Rust keyword label.
///
/// `rust-analyzer` maps `impl` blocks to `Object` and type aliases to
/// `TypeParameter`.
#[must_use]
pub fn kind_label(value: u32) -> &'static str {
    match value {
        kind::MODULE => "mod",
        kind::METHOD | kind::CONSTRUCTOR | kind::FUNCTION => "fn",
        kind::FIELD => "field",
        kind::ENUM => "enum",
        kind::INTERFACE => "trait",
        kind::VARIABLE => "static",
        kind::CONSTANT => "const",
        kind::OBJECT => "impl",
        kind::ENUM_MEMBER => "variant",
        kind::STRUCT => "struct",
        kind::TYPE_PARAMETER => "type",
        _ => "item",
    }
}

/// Extract the markdown or plain-text body from a `textDocument/hover`
/// response.
#[must_use]
pub fn hover_text(hover: &Value) -> Option<String> {
    let contents = hover.get("contents")?;
    let text = match contents {
        Value::String(text) => text.clone(),
        Value::Object(markup) => markup.get("value")?.as_str()?.to_owned(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| match part {
                Value::String(text) => Some(text.clone()),
                Value::Object(marked) => marked.get("value")?.as_str().map(str::to_owned),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => return None,
    };

    Some(text)
}
