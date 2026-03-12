use std::process::Command;

use rmcp::{
    ErrorData as McpError,
    handler::server::{tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::{
    BearDb,
    note::LineSpec,
    search::{SearchMatch, SearchParams},
};

/// Configuration for the grizzly MCP server.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Enable JP tool protocol (serialize outputs as `jp_tool::Outcome` JSON).
    pub jp_protocol: bool,

    /// Enable the `note_create` tool.
    pub note_create: bool,
}

#[derive(Clone)]
pub struct GrizzlyService {
    config: ServerConfig,
    tool_router: ToolRouter<Self>,
}

impl GrizzlyService {
    fn db() -> Result<BearDb, McpError> {
        BearDb::open().map_err(mcp_err)
    }

    fn format_output(&self, msg: String) -> CallToolResult {
        if self.config.jp_protocol {
            let outcome = jp_tool::Outcome::Success {
                content: msg.clone(),
            };
            let json = serde_json::to_string(&outcome).unwrap_or(msg);

            CallToolResult::success(vec![Content::text(json)])
        } else {
            CallToolResult::success(vec![Content::text(msg)])
        }
    }

    fn format_error(&self, msg: String) -> CallToolResult {
        if self.config.jp_protocol {
            let outcome = jp_tool::Outcome::Error {
                message: msg.clone(),
                trace: vec![],
                transient: true,
            };
            let json = serde_json::to_string(&outcome).unwrap_or(msg);

            CallToolResult::success(vec![Content::text(json)])
        } else {
            CallToolResult::error(vec![Content::text(msg)])
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct NoteGetRequest {
    /// One or more note IDs to fetch.
    pub ids: Vec<String>,

    /// Optional line ranges. Each element can be an integer (single line) or a
    /// string like "10:20" for a range.
    #[serde(default)]
    pub lines: Vec<Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct NoteSearchRequest {
    /// Search queries. Can be a single string or an array of strings.
    #[serde(deserialize_with = "deserialize_string_or_vec")]
    pub queries: Vec<String>,

    /// Number of context lines around each match (default: 3).
    #[serde(default = "default_context")]
    pub context: usize,

    /// Filter: only search notes with ALL of these tags.
    #[serde(default)]
    pub tags: Vec<String>,

    /// Filter: only search notes with ANY of these IDs.
    #[serde(default)]
    pub ids: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct NoteCreateRequest {
    /// Title of the new note.
    pub title: String,

    /// Tags to apply (without the # prefix).
    #[serde(default)]
    pub tags: Vec<String>,

    /// Note content body.
    #[serde(default)]
    pub content: String,
}

fn default_context() -> usize {
    3
}

fn deserialize_string_or_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrVec {
        String(String),
        Vec(Vec<String>),
    }

    match StringOrVec::deserialize(deserializer)? {
        StringOrVec::String(s) => Ok(vec![s]),
        StringOrVec::Vec(v) => Ok(v),
    }
}

#[tool_router]
impl GrizzlyService {
    #[must_use]
    pub fn new(config: ServerConfig) -> Self {
        Self {
            config,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Fetch one or more Bear notes by their unique ID. Returns the full note \
                       content with metadata."
    )]
    async fn note_get(
        &self,
        Parameters(req): Parameters<NoteGetRequest>,
    ) -> Result<CallToolResult, McpError> {
        let db = Self::db()?;

        let ids = req.ids.iter().map(String::as_str).collect::<Vec<_>>();
        let notes = db.get_notes(&ids).map_err(mcp_err)?;

        if notes.is_empty() {
            return Ok(self.format_error("No notes found for the given IDs".into()));
        }

        let line_specs = req
            .lines
            .iter()
            .filter_map(LineSpec::parse)
            .collect::<Vec<_>>();

        let output = notes
            .iter()
            .map(|note| {
                if line_specs.is_empty() {
                    note.to_xml()
                } else {
                    note.to_xml_with_lines(&line_specs)
                }
            })
            .collect::<Vec<_>>();

        Ok(self.format_output(output.join("\n")))
    }

    #[tool(
        description = "Search Bear notes by content. Returns matching lines with surrounding \
                       context. Results are formatted with line numbers."
    )]
    async fn note_search(
        &self,
        Parameters(req): Parameters<NoteSearchRequest>,
    ) -> Result<CallToolResult, McpError> {
        let db = Self::db()?;

        let params = SearchParams {
            queries: req.queries,
            tags: req.tags,
            ids: req.ids,
            context: req.context,
            ..Default::default()
        };

        let matches = db.search(&params).map_err(mcp_err)?;

        if matches.is_empty() {
            return Ok(self.format_output("No matches found.".into()));
        }

        let output = matches.iter().map(SearchMatch::to_xml).collect::<Vec<_>>();
        Ok(self.format_output(output.join("\n")))
    }

    #[tool(description = "Create a new note in Bear with a title, tags, and content.")]
    async fn note_create(
        &self,
        Parameters(req): Parameters<NoteCreateRequest>,
    ) -> Result<CallToolResult, McpError> {
        if !self.config.note_create {
            return Ok(self.format_error(
                "note_create is disabled. Start the server with --note-create to enable it.".into(),
            ));
        }

        let tags_param = req
            .tags
            .iter()
            .map(|tag| format!("#{tag}"))
            .collect::<Vec<_>>()
            .join(" ");

        // Build note body in Bear's expected format
        let body = format!("# {}\n{}\n\n{}", req.title, tags_param, req.content);

        // URL-encode the body for x-callback-url
        let encoded = urlencoded(&body);

        // Build the x-callback-url
        let url =
            format!("bear://x-callback-url/create?text={encoded}&open_note=no&show_window=no");

        // Open via macOS `open` command
        let status = Command::new("open").arg(&url).status();

        match status {
            Ok(s) if s.success() => {
                Ok(self.format_output(format!("Note '{}' created successfully.", req.title)))
            }
            Ok(status) => Ok(self.format_error(format!("open command exited with: {status}"))),
            Err(error) => Ok(self.format_error(format!(
                "Failed to create note (is Bear installed?): {error}"
            ))),
        }
    }
}

/// Minimal percent-encoding for x-callback-url parameters.
fn urlencoded(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

#[tool_handler]
impl rmcp::ServerHandler for GrizzlyService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("grizzly", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "A Bear.app MCP server. Search notes, retrieve note contents, and create new \
                 notes.",
            )
    }
}

#[allow(clippy::needless_pass_by_value)]
fn mcp_err(msg: impl ToString) -> McpError {
    McpError::internal_error(msg.to_string(), None)
}
