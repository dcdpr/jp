//! Attachment handler for JP-internal resources.
//!
//! Scheme: `jp://<jp-id>[?<query>]`
//!
//! The variant character of the JP ID dispatches to a specific resource
//! type. Today only conversations (`jp-c<digits>`) are supported. Each
//! resource type owns its own set of query parameters.
//!
//! See the crate README for full documentation of supported resources and
//! their parameters.

use std::{
    error::Error,
    fmt::{self, Write as _},
    str::FromStr,
};

use jp_attachment::Attachment;
use jp_conversation::{
    ConversationEvent, ConversationId, ConversationStream,
    event::{ChatResponse, EventKind, ToolCallRequest, ToolCallResponse},
};
use jp_workspace::{ConversationHandle, Workspace};
use serde::Serialize;
use serde_json::Value;
use url::Url;

mod selector;
use selector::{Content, Selector};

/// Variant character used by [`ConversationId`] in `jp_id`.
const CONVERSATION_VARIANT: char = 'c';

/// Query parameter name for the selector DSL.
const PARAM_SELECT: &str = "select";

/// Query parameter name for raw-mode output.
const PARAM_RAW: &str = "raw";

/// What raw-mode output to include.
///
/// Triggered by the `raw` query parameter. Absent = rendered markdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum RawMode {
    /// Selected conversation events as JSON.
    Events,
    /// Selected events plus base config and metadata.
    All,
}

impl FromStr for RawMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "" | "events" => Ok(Self::Events),
            "all" => Ok(Self::All),
            other => Err(format!(
                "invalid raw mode '{other}' (expected 'events' or 'all')"
            )),
        }
    }
}

/// A single `jp://<id>[?<query>]` entry.
///
/// `id` is stored as a plain string so the on-disk form doesn't depend on the
/// serde representation of any specific JP ID type, and so that adding new
/// variants doesn't require migrating existing data.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Entry {
    /// The full JP ID, e.g. `jp-c17013123456`.
    id: String,

    /// Selector DSL value. The grammar depends on the ID's variant. For
    /// conversations this is the `CONTENT[:RANGE]` DSL. Empty = resource
    /// default.
    select: String,

    /// Raw-mode output flag. `None` = rendered output (markdown for
    /// conversations).
    raw: Option<RawMode>,
}

impl Entry {
    fn to_url(&self) -> Result<Url, Box<dyn Error + Send + Sync>> {
        let mut url = Url::parse(&format!("jp://{}", self.id))?;
        if self.select.is_empty() && self.raw.is_none() {
            return Ok(url);
        }

        let mut query = url.query_pairs_mut();
        if !self.select.is_empty() {
            query.append_pair(PARAM_SELECT, &self.select);
        }
        match self.raw {
            Some(RawMode::Events) => {
                query.append_key_only(PARAM_RAW);
            }
            Some(RawMode::All) => {
                query.append_pair(PARAM_RAW, "all");
            }
            None => {}
        }
        drop(query);

        Ok(url)
    }

    /// Inspect the variant of this entry's ID without claiming a specific
    /// resource type.
    fn variant(&self) -> Result<char, Box<dyn Error + Send + Sync>> {
        let parts = jp_id::parts::Parts::from_str(&self.id)
            .map_err(|e| format!("invalid jp id '{}': {e}", self.id))?;
        Ok(*parts.variant)
    }
}

/// Validate a `jp://` URI without performing I/O.
pub fn validate(uri: &Url) -> Result<(), Box<dyn Error + Send + Sync>> {
    let entry = uri_to_entry(uri)?;
    validate_entry(&entry)
}

/// Resolve a `jp://` URI against the already-open workspace.
pub fn resolve(
    workspace: &Workspace,
    uri: &Url,
) -> Result<Vec<Attachment>, Box<dyn Error + Send + Sync>> {
    let entry = uri_to_entry(uri)?;
    validate_entry(&entry)?;
    fetch_entry(&entry, workspace)
}

/// Validate an entry without performing any I/O.
fn validate_entry(entry: &Entry) -> Result<(), Box<dyn Error + Send + Sync>> {
    match entry.variant()? {
        CONVERSATION_VARIANT => {
            // Round-trip through the typed ID to verify the timestamp is
            // representable.
            ConversationId::from_str(&entry.id)
                .map_err(|e| format!("invalid conversation id '{}': {e}", entry.id))?;
            parse_selector(&entry.select)?;
            Ok(())
        }
        v => Err(unsupported_variant_error(v)),
    }
}

/// Resolve an entry into one or more [`Attachment`]s using the given storage
/// backend.
fn fetch_entry(
    entry: &Entry,
    workspace: &Workspace,
) -> Result<Vec<Attachment>, Box<dyn Error + Send + Sync>> {
    match entry.variant()? {
        CONVERSATION_VARIANT => fetch_conversation(entry, workspace),
        v => Err(unsupported_variant_error(v)),
    }
}

fn fetch_conversation(
    entry: &Entry,
    workspace: &Workspace,
) -> Result<Vec<Attachment>, Box<dyn Error + Send + Sync>> {
    let id = ConversationId::from_str(&entry.id)
        .map_err(|e| format!("invalid conversation id '{}': {e}", entry.id))?;

    let selector = parse_selector(&entry.select)?;

    let handle = workspace
        .acquire_conversation(&id)
        .map_err(|e| format!("failed to find conversation '{id}': {e}"))?;
    let stream = workspace
        .events(&handle)
        .map_err(|e| format!("failed to load conversation '{id}': {e}"))?;

    match entry.raw {
        None => {
            let rendered = render_stream(&stream, selector);
            let source = format!("jp:{id} ({selector})");
            Ok(vec![Attachment::text(source, rendered)])
        }
        Some(mode) => fetch_conversation_raw(workspace, &handle, &stream, selector, mode),
    }
}

fn fetch_conversation_raw(
    workspace: &Workspace,
    handle: &ConversationHandle,
    stream: &ConversationStream,
    selector: Selector,
    mode: RawMode,
) -> Result<Vec<Attachment>, Box<dyn Error + Send + Sync>> {
    let events = collect_selected_events(stream, selector);
    let raw = RawConversation {
        events,
        base_config: if mode == RawMode::All {
            let (base_config, _) = stream
                .to_parts()
                .map_err(|e| format!("failed to serialize base config: {e}"))?;
            Some(base_config)
        } else {
            None
        },
        metadata: if mode == RawMode::All {
            let metadata = workspace
                .metadata(handle)
                .map_err(|e| format!("failed to load conversation metadata: {e}"))?;
            Some(
                serde_json::to_value(&*metadata)
                    .map_err(|e| format!("failed to serialize conversation metadata: {e}"))?,
            )
        } else {
            None
        },
    };
    let body = serde_json::to_string_pretty(&raw)
        .map_err(|e| format!("failed to serialize raw conversation: {e}"))?;

    Ok(vec![Attachment::text(
        format!("jp:{} (json, {selector})", handle.id()),
        body,
    )])
}

#[derive(Serialize)]
struct RawConversation<'a> {
    events: Vec<&'a ConversationEvent>,

    #[serde(skip_serializing_if = "Option::is_none")]
    base_config: Option<Value>,

    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Value>,
}

fn parse_selector(s: &str) -> Result<Selector, Box<dyn Error + Send + Sync>> {
    if s.is_empty() {
        return Ok(Selector::default());
    }
    s.parse::<Selector>()
        .map_err(|e| format!("invalid selector '{s}': {e}").into())
}

fn unsupported_variant_error(variant: char) -> Box<dyn Error + Send + Sync> {
    format!(
        "unsupported jp:// resource type '{variant}' (only conversations (variant 'c') are \
         supported)"
    )
    .into()
}

fn uri_to_entry(uri: &Url) -> Result<Entry, Box<dyn Error + Send + Sync>> {
    // JP IDs (`jp-<variant><target_id>`) contain only lowercase ASCII
    // letters, digits, and hyphens — all of which survive `Url` host
    // normalization unchanged.
    let id = uri
        .host_str()
        .filter(|s| !s.is_empty())
        .ok_or("missing jp id in URI")?
        .to_owned();

    let mut select = String::new();
    let mut raw: Option<RawMode> = None;
    for (key, value) in uri.query_pairs() {
        match key.as_ref() {
            PARAM_SELECT => select = value.into_owned(),
            PARAM_RAW => raw = Some(value.parse()?),
            other => {
                return Err(format!("unknown query parameter '{other}' on jp:// URI").into());
            }
        }
    }

    Ok(Entry { id, select, raw })
}

/// Render a conversation stream into markdown, honoring the selector.
///
/// Returns an empty string only when no events match the selection; callers
/// should treat that as a valid but empty attachment rather than an error, so
/// the assistant can still see the source reference.
fn render_stream(stream: &ConversationStream, selector: Selector) -> String {
    let turns: Vec<_> = stream.iter_turns().collect();
    let Some((start, end)) = selector.range.resolve(turns.len()) else {
        return String::new();
    };

    let mut out = String::new();
    for (idx, turn) in turns[start..end].iter().enumerate() {
        let turn_number = start + idx + 1;

        // Pair tool call requests with their matching responses so they can be
        // rendered together. We collect pending requests and flush them when
        // the matching response arrives (or at end of turn if no match).
        let mut pending_tool_calls: Vec<&ToolCallRequest> = Vec::new();
        let mut turn_body = String::new();

        for event in turn {
            match &event.event.kind {
                EventKind::ChatRequest(req) if selector.content.user => {
                    write_section(&mut turn_body, "User", &req.content);
                }
                EventKind::ChatResponse(ChatResponse::Message { message })
                    if selector.content.assistant =>
                {
                    write_section(&mut turn_body, "Assistant", message);
                }
                EventKind::ChatResponse(ChatResponse::Structured { data })
                    if selector.content.assistant =>
                {
                    let rendered =
                        serde_json::to_string_pretty(data).unwrap_or_else(|_| data.to_string());
                    write_code_section(&mut turn_body, "Assistant (structured)", "json", &rendered);
                }
                EventKind::ChatResponse(ChatResponse::Reasoning { reasoning })
                    if selector.content.reasoning =>
                {
                    write_section(&mut turn_body, "Reasoning", reasoning);
                }
                EventKind::ToolCallRequest(req) if selector.content.tools => {
                    pending_tool_calls.push(req);
                }
                EventKind::ToolCallResponse(resp) if selector.content.tools => {
                    if let Some(pos) = pending_tool_calls.iter().position(|r| r.id == resp.id) {
                        let req = pending_tool_calls.remove(pos);
                        write_tool_pair(&mut turn_body, req, Some(resp));
                    } else {
                        write_tool_pair_response_only(&mut turn_body, resp);
                    }
                }
                // Inquiries and everything else are skipped.
                _ => {}
            }
        }

        // Flush any tool requests that never got a response.
        for req in pending_tool_calls {
            write_tool_pair(&mut turn_body, req, None);
        }

        if turn_body.is_empty() {
            continue;
        }

        if !out.is_empty() {
            out.push('\n');
        }
        let _ = writeln!(&mut out, "## Turn {turn_number}\n");
        out.push_str(&turn_body);
    }

    out
}

fn write_section(buf: &mut String, title: &str, body: &str) {
    let _ = writeln!(buf, "### {title}\n");
    buf.push_str(body.trim_end());
    buf.push_str("\n\n");
}

fn write_code_section(buf: &mut String, title: &str, lang: &str, body: &str) {
    let _ = writeln!(buf, "### {title}\n");
    write_fenced_block(buf, lang, body);
}

fn write_fenced_block(buf: &mut String, lang: &str, body: &str) {
    let body = body.trim_end();
    let fence = markdown_fence(body);
    if lang.is_empty() {
        let _ = writeln!(buf, "{fence}");
    } else {
        let _ = writeln!(buf, "{fence}{lang}");
    }
    buf.push_str(body);
    let _ = writeln!(buf, "\n{fence}\n");
}

fn markdown_fence(body: &str) -> String {
    let mut current = 0;
    let mut max = 0;
    for ch in body.chars() {
        if ch == '`' {
            current += 1;
            max = max.max(current);
            continue;
        }
        current = 0;
    }

    "`".repeat(3.max(max + 1))
}

fn write_tool_pair(buf: &mut String, req: &ToolCallRequest, resp: Option<&ToolCallResponse>) {
    let _ = writeln!(buf, "### Tool: {}\n", req.name);

    if !req.arguments.is_empty() {
        let args =
            serde_json::to_string_pretty(&req.arguments).unwrap_or_else(|_| "{}".to_string());
        let _ = writeln!(buf, "**Arguments**");
        write_fenced_block(buf, "json", &args);
    }

    match resp {
        Some(resp) => {
            let label = if resp.result.is_err() {
                "**Error**"
            } else {
                "**Result**"
            };
            let _ = writeln!(buf, "{label}");
            write_fenced_block(buf, "", resp.content());
        }
        None => {
            let _ = writeln!(buf, "_(no response)_\n");
        }
    }
}

fn write_tool_pair_response_only(buf: &mut String, resp: &ToolCallResponse) {
    let _ = writeln!(buf, "### Tool result (orphan: {})\n", resp.id);
    write_fenced_block(buf, "", resp.content());
}

// Display is provided for diagnostic logging.
impl fmt::Display for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Reuse `to_url` so the diagnostic output matches the canonical form
        // exactly. If URL building fails (it shouldn't, given validation),
        // fall back to a best-effort representation.
        match self.to_url() {
            Ok(url) => write!(f, "{url}"),
            Err(_) => write!(f, "jp://{}", self.id),
        }
    }
}

/// Collect events that match the given selector, preserving conversation order.
///
/// Used by raw output mode. `TurnStart` events that fall within the selected
/// range are always included (they're cheap context for debugging).
fn collect_selected_events(
    stream: &ConversationStream,
    selector: Selector,
) -> Vec<&ConversationEvent> {
    let turns: Vec<_> = stream.iter_turns().collect();
    let Some((start, end)) = selector.range.resolve(turns.len()) else {
        return vec![];
    };

    let mut out = vec![];
    for turn in &turns[start..end] {
        for event in turn {
            if content_matches(&event.event.kind, selector.content) {
                out.push(event.event);
            }
        }
    }
    out
}

/// Returns `true` if the event kind matches the content filter. `TurnStart`
/// is always considered a match — it's a free, low-cost context marker.
fn content_matches(kind: &EventKind, content: Content) -> bool {
    match kind {
        EventKind::TurnStart(_) => true,
        EventKind::ChatRequest(_) => content.user,
        EventKind::ChatResponse(ChatResponse::Message { .. } | ChatResponse::Structured { .. }) => {
            content.assistant
        }
        EventKind::ChatResponse(ChatResponse::Reasoning { .. }) => content.reasoning,
        EventKind::ToolCallRequest(_) | EventKind::ToolCallResponse(_) => content.tools,
        EventKind::InquiryRequest(_) | EventKind::InquiryResponse(_) => false,
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
