# RFD 065: Typed Resource Model for Attachments

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-20

## Summary

This RFD replaces the opaque `Attachment` type with a typed `Resource` model
aligned with MCP's resource specification and [RFD 058]'s content block schema.
Resources carry canonical URIs, MIME types, and MCP-standard annotations.
Resource content moves from the system prompt into the `ChatRequest` at the turn
where the resource is attached. Resources are snapshots — content is captured at
attachment time, only updated when explicitly requested. A built-in
`refresh_resource` tool lets the LLM request fresh content through the
attachment handler when needed.

## Motivation

[RFD 058] introduces typed content blocks for tool responses. Tools return
`resource` blocks with URIs, MIME types, and structured content. This gives JP
the metadata it needs for resource-level features like formatting control and
deduplication ([RFD 067]).

Attachments don't expose this metadata. The `Attachment` type has a
`source: String` (a human-readable label like `"src/main.rs"`), optional
`description`, and `AttachmentContent` (text or binary). Attachment handlers
receive canonical URLs via `AttachmentConfig`, but the resolved `Attachment`
type has no structured URI field — only a `source: String` whose content varies
by handler. The file handler sets `source` to the relative path
(`"src/main.rs"`), the HTTP and MCP handlers set it to the full URL, and the
command handler sets it to the command string (`"git diff --cached"`). Without
a guaranteed canonical URI on the resolved type, JP cannot reliably match an
attachment against a tool response that returns the same resource. This creates
three problems:

### No shared identity space

[RFD 067] needs to match resources across tool calls and attachments. A tool
returns `resource { uri: "file:///project/src/main.rs" }`; the file attachment
has `source: "src/main.rs"`. These refer to the same file, but JP has no
reliable way to connect them — the `source` field is a display label with
handler-specific formatting, not a canonical URI. Without a shared identity
model, deduplication cannot work across the tool/attachment boundary.

### Wrong insertion point

Attachments are currently resolved at the start of each `jp query` invocation
and prepended to the first user message, regardless of when they were added to
the conversation. A resource attached at turn 30 appears at turn 0. This has
three consequences:

1. **Cache invalidation.** When content changes, inserting the updated content
   at position 0 invalidates the cache for the entire conversation history. If
   the content is unchanged the cache is preserved (same tokens at the same
   position), but any change — even a single byte — invalidates everything after
   that point.

2. **Semantic mismatch.** The user's message at turn 30 ("look at this file")
   refers to content that appears 30 turns earlier. The LLM must connect a
   recent instruction with distant context.

3. **Inconsistency with tool calls.** Tool results appear at the turn where the
   tool was called. Attachments appear at turn 0. Two mechanisms for delivering
   resources to the LLM follow different rules.

### Dynamic resources force unnecessary invalidation

For resources with inherently dynamic content (command output with timestamps,
`git status`, web pages), re-resolution at turn 0 produces different content on
every invocation, guaranteeing cache invalidation every turn — even when the LLM
doesn't need the fresh data. The current model gives JP no way to distinguish
between "content changed because the user edited the file" and "content changed
because the command output includes a timestamp."

### What happens if we do nothing

Without a shared resource model, [RFD 067] cannot deduplicate across tool calls
and attachments — the primary use case described in its Motivation section. Tool
responses gain typed metadata (via [RFD 058]) while attachments remain opaque,
creating a permanent asymmetry in the resource pipeline.

## Design

### MCP-compatible superset

Any resource or tool-related type JP defines must be constructible from only the
fields that MCP provides. JP-specific extensions (fields beyond what MCP
defines) must have sane defaults (`Option<T>`, `Vec<T>`, `false`, etc.) so that
MCP-sourced data requires no synthesis or fabrication. Local tools and
attachment handlers may populate the extended fields for richer functionality.

This ensures MCP tool responses pass through without lossy conversion, while
local tools and handlers can provide additional metadata when it is available.

### Overview

The `Attachment` type is replaced by a `Resource` type aligned with MCP's
resource specification: canonical URI, MIME type, annotations, and typed
content. Resources are stored as a field on `ChatRequest`, placing them at the
turn where they were attached. The `conversation.attachments` config field
continues to declare what should be attached. A built-in `refresh_resource` tool
lets the LLM request fresh content for any attached resource through its
original handler.

### The `Resource` type

Handlers return `Vec<Resource>` instead of `Vec<Attachment>`:

```rust
/// A resolved resource with identity metadata.
///
/// Returned by attachment handlers after resolving a URL. Carries both
/// the content (for delivery to the LLM) and metadata (for display and
/// resource management).
pub struct Resource {
    /// Canonical URI identifying this resource.
    ///
    /// The handler produces this from the attachment URL. File handlers
    /// resolve relative paths against the workspace root to produce
    /// absolute `file:///...` URIs. HTTP handlers normalize the URL.
    /// The canonical URI is the identity key for deduplication.
    pub uri: String,

    /// The resource content.
    pub content: ResourceContent,

    /// MIME type of the content (e.g., "text/rust", "image/png").
    pub mime_type: Option<String>,

    /// Optional MCP annotations (audience, priority, lastModified).
    pub annotations: Option<Annotations>,

    // --- JP extensions (all defaultable, absent for MCP-sourced resources) ---

    /// Short name of the resource.
    ///
    /// Populated by attachment handlers for display in `jp attachment ls`
    /// and as the document title in providers that support it. Examples:
    /// - File handler: relative path from workspace root (`"src/main.rs"`)
    /// - HTTP handler: the URL (`"https://example.com/doc"`)
    /// - Bear handler: the note title
    /// - Command handler: the command string (`"git diff --cached"`)
    ///
    /// `None` when the resource originates from an MCP tool response,
    /// since MCP's `EmbeddedResource` (the content delivery type) does
    /// not carry a `name` field.
    pub name: Option<String>,

    /// Optional human-readable title for display purposes.
    ///
    /// When absent, providers fall back to `name`, then to `uri`.
    pub title: Option<String>,

    /// Optional description of the resource.
    pub description: Option<String>,

    /// Optional pre-formatted content for LLM delivery.
    ///
    /// When present, JP uses this for the LLM instead of formatting
    /// from `content` + `mime_type`. The raw `content` is still used
    /// for checksums and identity matching. See [RFD 058] for details.
    ///
    /// Attachment handlers and local tools may set this for custom
    /// presentation. When absent, JP formats the resource from
    /// `content` + `mime_type`. MCP tool responses never populate
    /// this field (MCP does not define it).
    pub formatted: Option<String>,
}

/// Resource content, either UTF-8 text or binary data.
///
/// Matches MCP's resource content model.
pub enum ResourceContent {
    /// UTF-8 text content (source code, markdown, etc.)
    Text(String),
    /// Binary content (images, PDFs, etc.)
    Blob(Vec<u8>),
}
```

`Resource` replaces `Attachment` as the type that handlers return and that the
resolution pipeline consumes. The `name` field replaces `Attachment.source`. The
`uri` field exposes the canonical identifier that was previously available at
the config layer but discarded during resolution.

The fields are grouped into two tiers following the MCP-compatible superset
principle:

- **MCP core:** `uri`, `content`, `mime_type`, `annotations` — these fields are
  sufficient to construct a `Resource` from any MCP tool response or resource
  read.
- **JP extensions:** `name`, `title`, `description`, `formatted` — all
  `Option<T>`, defaulting to `None` for MCP-sourced resources. Attachment
  handlers and local tools populate these when richer metadata is available.

This means `From<rmcp::ResourceContents>` and `From<rmcp::EmbeddedResource>`
conversions require no fabrication — all JP extension fields default to `None`.

### Shared across all resource paths

`Resource` is the single canonical type for resource content across JP.
`ChatRequest.resources` carries `Vec<Resource>` for attachments, and [RFD 058]'s
`ContentBlock::Resource` wraps `Resource` for tool responses. The same type
flows through both paths.

This is possible because the JP extension fields (`name`, `title`,
`description`, `formatted`) are all `Option<T>`. Attachment handlers populate
`name` and possibly `title`/`description`. Local tools may set `formatted`. MCP
tool responses leave all four as `None`. A single type serves all sources
without requiring separate stripped-down variants for different contexts.

[RFD 067]'s content tagging works identically regardless of source — same URI
space, same checksum computation (always against the raw `content`, never
against `formatted`), same matching algorithm.

When serialized to JSON, the MCP-core fields follow MCP's resource content
structure:

```json
{
  "uri": "file:///project/src/main.rs",
  "mimeType": "text/x-rust",
  "text": "fn main() {}",
  "annotations": {
    "lastModified": "2026-03-20T10:00:00Z"
  }
}
```

JP extension fields are omitted when `None` (via `#[serde(skip_serializing_if)]`).
Providers that need display metadata (e.g., Anthropic's `Document.title`) read
`name` or `title` directly from the `Resource`.

### Resources on `ChatRequest`

`ChatRequest` gains a `resources` field:

```rust
pub struct ChatRequest {
    /// The user's query or message content.
    pub content: String,

    /// Optional JSON schema constraining the assistant's response format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Map<String, Value>>,

    /// Resources attached to this message.
    ///
    /// Each resource is a snapshot of its content at the time this
    /// ChatRequest was created. Resources are never re-resolved or
    /// updated retroactively.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<Resource>,
}
```

Resources appear inline in the user message at the turn where they were
attached, the same pattern as `schema`. Old conversations without a `resources`
field deserialize with an empty vec.

Attachment handlers populate the JP extension fields (`name`, `title`,
`description`) and may optionally set `formatted` for custom presentation. In
all cases, `content` carries the raw, unformatted data — checksums and identity
matching ([RFD 067]) always operate on `content`, never on `formatted`.
MCP-sourced resources (e.g., from `refresh_resource` via an MCP handler) carry
only the MCP-core fields.

### Resources are snapshots

A resource attached at turn N captures the content as it exists at turn N. The
content is frozen in the `ChatRequest` event and never changes. This is the
same model as tool call results — `fs_read_file` at turn 3 shows the file as
it was at turn 3, regardless of subsequent edits.

This means:

- **History is immutable.** No retroactive content changes, no cache
  invalidation from re-resolution.
- **Semantic correctness.** The user's message at turn N refers to the content
  as it was at turn N. If the user says "review this file" at turn 5, the
  resource content matches what the user saw at turn 5.
- **Consistency with tools.** Attachments and tool results follow the same
  snapshot pattern.
- **Dynamic resources handled correctly.** A `cmd://git status` snapshot at
  turn 3 captures the status at turn 3. It doesn't re-run on every subsequent
  turn, which would produce different output and bust the cache.

### Resolution logic

At each `jp query` invocation, JP determines which resources to resolve:

**New conversation (`jp query --new`):** All attachments declared in
`conversation.attachments` are resolved and included in turn 0's `ChatRequest`.

**Explicit `--attach` on a follow-up:** The resource is resolved fresh and
included in the current turn's `ChatRequest`. The `ConfigDelta` is emitted as
before (so `jp attachment ls` works, forks carry the declaration forward). If the
URI already appears in the conversation history, the fresh content is still
included — the user explicitly asked to attach it, so they expect the latest
content.

**Follow-up with no new attachments:** Config-declared attachments are already in
the conversation history from their initial resolution. Nothing to resolve.

This means:

- **New conversation:** All config-declared attachments resolved at turn 0. ✓
- **`--attach bar.rs` at turn 5:** bar.rs resolved fresh, included at turn 5.
  ConfigDelta emitted. ✓
- **`--attach foo.rs` at turn 10 (foo.rs was attached at turn 0):** foo.rs
  resolved fresh at turn 10. The LLM sees both the turn-0 snapshot and the
  turn-10 snapshot. If the content hasn't changed, [RFD 067] dedup handles it.
  If it has changed, the LLM has both versions. ✓
- **Turn 6 with no `--attach`:** Config still declares bar.rs, but it's already
  in the stream at turn 5. Skip. ✓
- **`jp conversation fork`:** New conversation inherits config (including
  attachment declarations). Resolution finds no matching URIs in the forked
  stream and resolves them fresh for the fork's first turn. ✓

### Config and `--attach`

`conversation.attachments` remains the declaration mechanism. `--attach` on a
follow-up query continues to write a `ConfigDelta` that adds the attachment
declaration to the config. This keeps the declaration persistent:

- `jp attachment ls` reads from the accumulated config. Works as before.
- `jp attachment rm` removes from the config. Content already in the stream is
  unaffected (history is immutable).
- `jp conversation fork` carries attachment declarations forward via the config.

The change is in *when and where* content is resolved — at the turn where the
resource is first attached (or explicitly re-attached), not at turn 0 on every
invocation.

### Refreshing resources

Resources are snapshots, but the LLM sometimes needs current content. The
refresh path must go through the attachment handler because the handler is the
only component that knows how to produce content for its scheme. Tools like
`fs_read_file` are configured tools that may not be enabled, and many attachment
schemes (Bear notes, command output, MCP resources) have no corresponding tool.

JP ships a built-in `refresh_resource` tool:

```
refresh_resource(uri: "file:///project/src/main.rs")
```

When called:

1. JP parses the URI scheme.
2. Finds the registered handler for that scheme.
3. Calls the handler's resolve method for that specific URI.
4. Returns the content as a `resource` content block (per [RFD 058]).

The result is a normal tool call response in the conversation stream. [RFD 067]
dedup applies: if the content hasn't changed since the original attachment (or a
previous refresh), the response is replaced with a reference. If the content has
changed, the full content is delivered.

#### Superseded resource handling

When `refresh_resource` delivers new content for a URI that already exists in
the stream, the earlier snapshot becomes stale. The stale content remains in the
stored conversation (history is immutable), but JP can choose to omit it when
building the LLM request — similar to how [RFD 036] compaction drops old tool
responses.

Whether to omit a superseded resource involves trade-offs between token savings
and cache invalidation. Several heuristics are relevant:

- **Token size threshold.** Small resources (< N tokens) may cost less to keep
  than the cache invalidation caused by removing them.
- **Cache position.** Resources within the provider's cache lookback window
  (e.g., Anthropic's ~30 turns) are expensive to remove. Resources outside the
  window can be removed without cache cost.
- **Content delta.** Nearly identical old/new content saves few tokens on
  removal. Completely different content benefits from removal to avoid
  confusing the LLM with contradictory versions.
- **Turn distance.** Recent resources are likely in the LLM's active attention;
  distant resources may not be.

The specific policy (defaults, configuration, and which heuristics to implement)
is deferred to [RFD 067] and [RFD 036], which own the deduplication and
compaction logic respectively.

#### Handler trait change

The handler trait needs to resolve a single resource by URI. For the current
trait, this means adding a method:

```rust
async fn resolve_one(
    &self,
    uri: &Url,
    cwd: &Utf8Path,
    mcp: Client,
) -> Result<Resource>;
```

For the [RFD 015] trait (which already takes `&[Url]`), passing a single-element
slice to `resolve()` is sufficient. The handler returns one `Resource` for the
one URI.

> [!TIP]
> [RFD 015] currently defines the handler return type as `Vec<Attachment>`. It
> will land first without the `Resource` change. Once this RFD is accepted, the
> handler return type should be updated to `Vec<Resource>` as a follow-up.

#### When the LLM refreshes

The LLM decides when to refresh. JP does not automatically re-resolve
attachments or rewrite history. The LLM calls `refresh_resource` when it has
reason to believe the content is stale — for example, after modifying a file, or
when the user mentions something has changed.

Future extensions can inform the LLM about stale resources without automatic
refresh:

- **[RFD 011] notifications.** JP could check declared attachments at the start
  of each turn (by re-resolving and comparing checksums) and emit a system
  notification if content has changed: "The attached resource src/main.rs has
  changed since turn 5." The LLM decides whether to call `refresh_resource`.
- **Config-level refresh policy.** An optional `refresh` field on attachment
  declarations (`manual`, `notify`) could control whether JP checks for changes.
  Default is `manual` (no checking). Not designed in this RFD.

### Provider migration

Each provider currently handles attachments in its `build_request` method,
receiving them as a separate `ThreadParts::attachments` vec and prepending them
to the first user message. With resources on `ChatRequest`, providers process
them inline as part of the event stream.

**Anthropic:** The `convert_event` function currently converts a `ChatRequest`
into a single `MessageContent::Text` block. With resources, it produces a
`MessageContent::Text` for the user's query followed by one
`MessageContent::Document` per resource block. Text resources become
`DocumentSource::Text`; binary resources become `DocumentSource::Base64`. The
`Resource.name` (preserved in event metadata) maps to the document `title`.
Cache breakpoint logic applies to the document blocks at their actual position
rather than always at message 0.

**Google/Gemini:** Text resources are serialized to XML (same as today's
`text_attachments_to_xml`, but inline at the correct turn). Binary resources
become `ContentPart::InlineData`.

**OpenAI:** Text resources become `ContentItem::Text` (XML-wrapped). Binary
resources map to `ContentItem::Image` or `ContentItem::File` depending on MIME
type.

**OpenRouter:** Same as OpenAI, adapted for the OpenRouter content block types.

The `Thread` and `ThreadParts` types lose the `attachments` field. The
`text_attachments_to_xml` helper may remain as a utility for providers that
render text resources as XML, but it operates on `&[Resource]` from the
`ChatRequest` rather than on a separate `Vec<Attachment>`.

### Handler return type

Handlers return `Vec<Resource>` instead of `Vec<Attachment>`. The handler is
responsible for producing the canonical URI because only the handler knows the
scheme-specific canonicalization rules:

- **File handler:** Resolves relative paths against the workspace root to
  produce `file:///absolute/path`. Normalizes `.` and `..` segments. Infers
  MIME type from file extension.
- **HTTP handler:** Normalizes the URL (lowercase scheme and host, resolve
  path). Uses `Content-Type` header for MIME type.
- **MCP handler:** Uses the MCP resource URI as-is (already canonical per the
  MCP spec). Uses the resource's declared MIME type.
- **Command handler:** Uses the original `cmd://...` URL as the canonical URI.

The `name` field replaces what is currently `Attachment.source`. Each handler
populates it with the most human-readable identifier: relative path for files,
URL for HTTP, note title for Bear, command string for cmd. Since `name` is
`Option<String>`, handlers are expected to always set it, but the type does not
enforce this — MCP tool responses legitimately omit it. The `annotations` field
carries MCP-standard metadata; handlers that support it (MCP handler, HTTP
handler via `Last-Modified`) populate it.

## Drawbacks

**Breaking change to the handler trait.** All handlers must be updated to return
`Resource` instead of `Attachment`. Since handlers are internal (and [RFD 015]
is already redesigning the trait), this is acceptable. [RFD 015] will land first
with `Vec<Attachment>`; the migration to `Vec<Resource>` is a follow-up after
this RFD is accepted.

**Breaking change to provider implementations.** Every provider's
`build_request` method changes — attachments no longer arrive as a separate vec
but as `Resource` fields on `ChatRequest` events. This is a significant refactor
touching all providers.

**Snapshot semantics may surprise users.** Users accustomed to attachments
always showing the latest content will find that attached files are frozen at
attachment time. The `refresh_resource` tool and explicit `--attach` mitigate
this, but the behavioral change needs documentation.

**Re-attaching an existing URI creates duplicate content.** When a user
`--attach`es a resource that's already in the stream, the conversation contains
two snapshots. If the content hasn't changed, this wastes tokens (mitigated by
[RFD 067] dedup). If the content has changed, both versions remain — useful for
the LLM to see the evolution, but potentially confusing.

**Canonical URI correctness is critical.** If a handler produces an incorrect
canonical URI, deduplication fails silently (either missing valid matches or
producing false positives). Handlers must be tested carefully for URI
canonicalization edge cases (symlinks, case sensitivity, trailing slashes).

## Alternatives

### Keep attachments at turn 0, add metadata

Leave the insertion point unchanged (always first user message) but add URI
and MIME type to the `Attachment` type. This enables deduplication without
the provider refactor.

**Rejected because:** It perpetuates the cache invalidation problem on content
changes. Adding a resource at turn 30 still modifies turn 0, invalidating
everything after that point. The insertion-point fix is the primary motivator
for this RFD, not just the metadata.

### Separate `AttachmentResolved` event type

Add a new `EventKind::AttachmentResolved` variant to record when attachment
content is delivered, instead of embedding resources in `ChatRequest`.

**Rejected because:** `EventKind` follows a request/response pattern.
`AttachmentResolved` has no request counterpart and would be a metadata event
that doesn't fit the model. It would also need special handling in
`is_provider_visible()` and in every provider's event conversion logic.
Embedding resources in `ChatRequest` is simpler and aligns with how providers
already process user messages.

### Auto-refresh changed resources

Automatically re-resolve attachments at each turn and re-insert content when
it changes. The fresh content would appear in the current `ChatRequest` as an
updated resource block.

**Rejected because:** It forces cache invalidation for dynamic resources on
every turn, even when the LLM doesn't need fresh data. Resources like command
output with timestamps would bust the cache unconditionally. The snapshot model
with LLM-driven refresh via `refresh_resource` gives the LLM control over when
to pay the refresh cost.

### Use a JP-specific resource type instead of MCP alignment

Define a custom resource model optimized for JP's needs without regard for MCP
compatibility.

**Rejected because:** MCP has already standardized the right primitives for
resource identity (URIs, MIME types, annotations). Adopting MCP's model means
MCP tool responses and attachment resources share the same type, enabling
[RFD 067] dedup across both without translation. A custom model would require
a mapping layer and risk subtle mismatches.

## Non-Goals

- **Automatic resource refresh.** JP does not re-resolve attached resources or
  check for staleness. The LLM requests fresh content when it needs it via
  `refresh_resource`. Automatic staleness detection via [RFD 011] notifications
  is a future extension.

- **Resource deduplication logic.** This RFD establishes the shared identity
  model (canonical URIs, MIME types, `Resource`) that makes deduplication
  possible. The deduplication algorithm itself is defined in [RFD 067].

- **Handler trait redesign.** [RFD 015] redesigns the handler trait. This RFD
  changes the return type from `Attachment` to `Resource` and adds
  single-resource resolution, but does not redesign the trait structure itself.

- **Attachment config syntax changes.** The `conversation.attachments` config
  field and `--attach` CLI flag continue to work as before. The change is in
  how declarations are resolved, not in how they are declared.

- **Superseded resource removal policy.** This RFD documents the heuristics
  for when to omit superseded resources from the LLM request. The specific
  defaults and configuration are implementation details deferred to [RFD 067]
  and [RFD 036].

## Risks and Open Questions

### URI canonicalization edge cases

File URIs need careful handling: symlinks, case-insensitive filesystems,
Unicode normalization, Windows path separators. The file handler needs thorough
testing for these cases. A malformed canonical URI silently breaks deduplication.

### Inline content bloats the conversation file

Resource content is stored inline in `events.json` as part of the `ChatRequest`
or `ToolCallResponse` event. A `--attach ./src` on a modest project can resolve
to hundreds of files, and binary resources inflate further with base64 encoding.
This causes disk bloat, increased parse time on every `jp query` invocation, and
search pollution in developer tools.

This is a blocking concern. A future RFD can design a system that externalizes
all content payloads from `events.json`. The `Resource` and `ContentBlock` types
in the domain model are not affected — externalization is a serialization
concern handled by the persistence layer. This RFD proceeds with inline content
as the initial storage format.

### Interaction with conversation compaction

[RFD 036] describes conversation compaction. When compaction operates on turns
that contain attached resources, it should preserve the `Resource` metadata
(URI, MIME type) even if the content is dropped, so that deduplication can still
identify the resource. The specifics depend on the compaction design.

### Migration of existing conversations

Existing conversations have no `resources` field on `ChatRequest`. The
`#[serde(default)]` annotation handles deserialization. However, existing
conversations also have attachments that were resolved via the old model
(prepended to turn 0). After this change, those attachments won't be in any
`ChatRequest.resources`. If the config still declares them, the resolution logic
will resolve them as new resources on the next turn — even though the content
was previously delivered (invisibly, at turn 0). This is a one-time duplication
that's acceptable for the migration period.

### `refresh_resource` tool visibility

The `refresh_resource` tool should be available whenever the conversation has
attached resources. If no resources are attached, registering the tool wastes a
tool definition in the context window. JP could conditionally register it based
on whether `ChatRequest.resources` is non-empty anywhere in the stream.

## Implementation Plan

### Phase 1: `Resource` type and `ResourceContent`

Define `Resource` (with canonical URI, MIME type, content, annotations, plus JP
extensions `name`, `title`, `description`, `formatted`) in a shared location.
Align `ResourceContent` with [RFD 058]'s `ResourceContent` type. Define the
`Annotations` type matching MCP's specification (audience, priority,
lastModified). Implement `From<rmcp::ResourceContents>` and
`From<rmcp::EmbeddedResource>` conversions.

No behavioral changes yet. Can be merged independently.

### Phase 2: Handler return type migration

Update the handler trait to return `Vec<Resource>` instead of `Vec<Attachment>`.
Update all built-in handlers to produce canonical URIs and MIME types.

If [RFD 015] lands first, this phase is smaller — the trait is already being
rewritten. If not, this is an additive change to the existing trait.

Depends on Phase 1. Can be merged independently.

### Phase 3: `ChatRequest.resources` and resolution logic

Add `resources: Vec<Resource>` to `ChatRequest`. Implement the resolution logic:
resolve config-declared attachments on new conversations, resolve `--attach`
resources fresh at the current turn.

Update `build_thread` / `register_attachment` to use the new resolution path
instead of building a separate `Vec<Attachment>`.

Depends on Phase 2.

### Phase 4: Provider migration

Update all providers to read resources from `ChatRequest` events instead of
`ThreadParts::attachments`. Remove the `attachments` field from `Thread` and
`ThreadParts`. Update or remove `text_attachments_to_xml` as needed.

This is the largest phase. Each provider can be migrated independently.

Depends on Phase 3.

### Phase 5: `refresh_resource` built-in tool

Implement the `refresh_resource` tool: URI parsing, handler dispatch,
single-resource resolution, resource block response. Conditionally register the
tool when the conversation has attached resources.

Add `resolve_one` to the handler trait (or use single-element `resolve` if [RFD
015] has landed).

Depends on Phase 2 and Phase 3.

### Phase 6: Remove `Attachment` type

Once all consumers have migrated to `Resource`, remove the `Attachment` type,
`AttachmentContent`, and related code from `jp_attachment`. Rename the crate's
public API to use `Resource` terminology.

Depends on all previous phases.

## References

- [MCP Resources Specification (2025-11-25)][MCP Resources] — defines resource
  URIs, content types, annotations, and URI schemes. The `Resource` type in this
  RFD follows MCP's resource content format with JP extensions.
- [MCP Tools][MCP Tools] — defines typed content blocks for tool responses.
- [MCP Tasks][MCP Tasks] — defines the task lifecycle model relevant to stateful
  tool support.
- [RFD 058: Typed Content Blocks for Tool Responses][RFD 058] — introduces the
  `ContentBlock` and `ResourceContent` types for tool responses. This RFD aligns
  attachments with the same model via the shared `Resource` type.
- [RFD 067: Resource Deduplication for Token Efficiency][RFD 067] — defines
  deduplication logic that depends on the shared identity model (canonical URIs,
  checksums) established by this RFD and RFD 058.
- [RFD 015: Simplified Attachment Handler Trait][RFD 015] — redesigns the
  handler trait to stateless `validate`/`resolve`. This RFD changes the return
  type; RFD 015 changes the trait structure. Both can proceed independently.
- [RFD 011: System Notification Queue][RFD 011] — future path for notifying the
  LLM about stale resources without automatic refresh.
- [RFD 036: Conversation Compaction][RFD 036] — relevant for how resource
  metadata interacts with compacted conversations, and for superseded resource
  removal heuristics.

[MCP Resources]: https://modelcontextprotocol.io/specification/2025-11-25/server/resources.md
[MCP Tools]: https://modelcontextprotocol.io/specification/2025-11-25/server/tools.md
[MCP Tasks]: https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/tasks.md
[RFD 011]: 011-system-notification-queue.md
[RFD 015]: 015-simplified-attachment-handler-trait.md
[RFD 036]: 036-conversation-compaction.md
[RFD 058]: 058-typed-content-blocks-for-tool-responses.md
[RFD 067]: 067-resource-deduplication-for-token-efficiency.md
