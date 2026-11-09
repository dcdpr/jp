# RFD 065: Typed Resource Model for Attachments

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-20
- **Required by**: [RFD 058], [RFD 066], [RFD 067], [RFD 135]
- **Summary**: Replaces opaque attachments with typed MCP-aligned `Resource`
  snapshots stored on the `ChatRequest` of the turn that attached them.

## Summary

This RFD replaces the opaque `Attachment` type with a typed `Resource` model
carrying canonical URIs, MIME types, and MCP annotations aligned with [RFD 058].
Resource snapshots are stored on the `ChatRequest` at the turn where they are
attached instead of in the system prompt, and a built-in `refresh_resource` tool
lets the assistant request updated content on demand.

## Motivation

[RFD 058] introduces typed content blocks for tool responses, where tools return
`resource` blocks with URIs, MIME types, and structured content.
This metadata supports resource-level features such as formatting control and
deduplication ([RFD 067]).

Attachments do not expose this metadata, and JP resolves them on every turn
rather than recording them where they were attached.
This creates three problems:

### No shared identity space

Attachment handlers receive canonical URLs via `AttachmentConfig`, but the
resolved `Attachment` type only carries `source: String` (a human-readable
label), an optional `description`, and `AttachmentContent` (text or binary).
The `source` field is a display label whose format varies by handler: the file
handler sets it to the relative path (`"src/main.rs"`), the HTTP and MCP
handlers set it to the full URL, and the command handler sets it to the command
string (`"git diff --cached"`).

[RFD 067] needs to match resources across tool calls and attachments, which is
the primary use case in its Motivation section.
When a tool returns `resource { uri: "file:///project/src/main.rs" }` and a file
attachment has `source: "src/main.rs"`, both refer to the same file, but JP has
no reliable way to connect them.
Without a canonical URI on both types, deduplication cannot work across the tool
and attachment boundary, leaving a permanent asymmetry between typed tool
responses ([RFD 058]) and opaque attachments.

### Wrong insertion point

Attachments are currently resolved at the start of each `jp query` invocation
and prepended to the first user message, regardless of when they were added to
the conversation.
A resource attached at turn 30 appears at turn 0.
This has three consequences:

1. **Cache invalidation.** Unchanged content preserves the prompt cache (same
   tokens at the same position), but changing even a single byte at position 0
   invalidates the cache for the entire conversation history.

2. **Semantic mismatch.** The user's message at turn 30 ("look at this file")
   refers to content placed 30 turns earlier, forcing the LLM to connect a
   recent instruction with distant context.

3. **Inconsistency with tool calls.** Tool results appear at the turn where the
   tool was called, whereas attachments appear at turn 0.
   Two mechanisms for delivering resources to the LLM follow different rules.

### Dynamic resources force unnecessary invalidation

For resources with inherently dynamic content (command output with timestamps,
`git status`, web pages), re-resolution at turn 0 produces different content on
every invocation.
This invalidates the cache on every turn, even when the LLM does not need fresh
data.
The current model gives JP no way to distinguish between content that changed
because the user edited a file and content that changed because command output
includes a timestamp.

## Design

### Overview

The `Attachment` type is replaced by a `Resource` type aligned with MCP's
resource specification: canonical URI, MIME type, annotations, and typed
content.
Resources are stored on `ChatRequest` at the turn where they were attached.
The `conversation.attachments` config field continues to declare what should be
attached, and a built-in `refresh_resource` tool lets the LLM request fresh
content for any attached resource through its original handler.

### MCP-compatible superset

Any resource or tool-related type JP defines must be constructible from the
fields that MCP provides alone.
JP-specific extensions beyond MCP's specification must have defaults
(`Option<T>`, `Vec<T>`, `false`, etc.) so that MCP-sourced data passes through
without synthesis or lossy conversion.
Local tools and attachment handlers may populate the extended fields when
additional metadata is available.

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

    // JP extensions (all defaultable, absent for MCP-sourced resources)

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

`Resource` is the single canonical type for resource content across JP:
`ChatRequest.resources` carries `Vec<Resource>` for attachments, and [RFD 058]'s
`ContentBlock::Resource` wraps `Resource` for tool responses.
The `name` field replaces `Attachment.source`, and `uri` exposes the canonical
identifier that was previously available at the config layer but discarded
during resolution.

Following the MCP-compatible superset principle, the fields split into two
tiers:

- **MCP core (`uri`, `content`, `mime_type`, `annotations`):** Sufficient to
  construct a `Resource` from any MCP tool response or resource read.
- **JP extensions (`name`, `title`, `description`, `formatted`):** All
  `Option<T>`, defaulting to `None` for MCP-sourced resources so
  `From<rmcp::ResourceContents>` and `From<rmcp::EmbeddedResource>` conversions
  require no fabrication.
  Attachment handlers populate `name` (and optionally `title`, `description`,
  and `formatted`), and local tools may set `formatted`.

Because a single type serves all resource paths without separate stripped-down
variants, [RFD 067]'s content tagging works identically regardless of source: it
uses the same URI space, computes checksums against raw `content` rather than
`formatted`, and applies the same matching algorithm.

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

JP extension fields are omitted when `None` (via
`#[serde(skip_serializing_if)]`).
Providers that need display metadata (e.g., Anthropic's `Document.title`) read
`name` or `title` directly from the `Resource`.

### Handler canonicalization and metadata

Each attachment handler produces the canonical URI and metadata for its scheme:

- **File handler:** Resolves relative paths against the workspace root to
  produce `file:///absolute/path`, normalizes `.` and `..` segments, and infers
  the MIME type from the file extension.
- **HTTP handler:** Normalizes the URL (lowercases scheme and host, resolves the
  path), uses the `Content-Type` header for the MIME type, and populates
  `annotations` from `Last-Modified` when present.
- **MCP handler:** Uses the MCP resource URI as-is (already canonical per the
  MCP spec) along with the resource's declared MIME type and `annotations`.
- **Command handler:** Uses the original `cmd://...` URL as the canonical URI.

Handlers are expected to populate `name` with the most human-readable identifier
for their scheme (relative path for files, URL for HTTP, note title for Bear,
command string for `cmd`), even though the type leaves `name` as
`Option<String>` so MCP tool responses can omit it.

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
attached, following the same pattern as `schema`.
Old conversations without a `resources` field deserialize with an empty `Vec`.

### Snapshot semantics

A resource attached at turn N captures the content as it exists at turn N. The
content is frozen in the `ChatRequest` event and never changes, matching tool
call results (`fs_read_file` at turn 3 shows the file as it was at turn 3,
regardless of subsequent edits).

This model provides four guarantees:

- **Immutable history.** Content never changes retroactively, avoiding cache
  invalidation from re-resolution.
- **Semantic correctness.** The user's message at turn N refers to the content
  as it was at turn N. If the user says "review this file" at turn 5, the
  resource content matches what the user saw at turn 5.
- **Consistency with tools.** Attachments and tool results follow the same
  snapshot pattern.
- **Stable dynamic resources.** A `cmd://git status` snapshot at turn 3 captures
  the status at turn 3 instead of re-running on every subsequent turn and
  invalidating the cache.

### Resolution logic

At each `jp query` invocation, JP resolves resources at the turn where they are
first attached or explicitly re-attached, rather than at turn 0 on every
invocation:

- **New conversation (`jp query --new`):** Resolves all attachments declared in
  `conversation.attachments` and includes them in turn 0's `ChatRequest`.
- **Explicit `--attach` on a follow-up:** Resolves the resource fresh, includes
  it in the current turn's `ChatRequest`, and emits a `ConfigDelta` as before.
  For example, `--attach bar.rs` at turn 5 resolves `bar.rs` fresh and records
  it at turn 5.
  If the URI already appears in the conversation history (such as `--attach
  foo.rs` at turn 10 when `foo.rs` was already attached at turn 0), JP still
  includes the fresh snapshot because the user explicitly requested the latest
  content.
  The LLM sees both the turn-0 and turn-10 snapshots; if the content has not
  changed, [RFD 067] deduplication handles it, and if it has changed, the LLM
  has both versions.
- **Follow-up with no new attachments:** Skips config-declared attachments whose
  URIs already appear in the conversation history (for example, `bar.rs` at turn
  6 after being attached at turn 5).
- **Forked conversation (`jp conversation fork`):** The new conversation
  inherits the config, including attachment declarations.
  Any declared attachment whose URI does not appear in the forked stream is
  resolved fresh for the fork's first turn.

### Config and `--attach`

`conversation.attachments` remains the declaration mechanism, and `--attach` on
a follow-up query continues to write a `ConfigDelta` that adds the attachment
declaration to the config:

- `jp attachment ls` reads from the accumulated config as before.
- `jp attachment rm` removes the declaration from the config while leaving
  content already in the stream untouched.
- `jp conversation fork` carries attachment declarations forward via the config.

### Refreshing resources

Resources are snapshots, but the LLM sometimes needs current content.
The refresh path must go through the attachment handler because only the handler
knows how to produce content for its scheme.
Tools like `fs_read_file` are configured tools that may not be enabled, and many
attachment schemes (Bear notes, command output, MCP resources) have no
corresponding tool.

JP ships a built-in `refresh_resource` tool:

```
refresh_resource(uri: "file:///project/src/main.rs")
```

When called, JP:

1. Parses the URI scheme.
2. Finds the registered handler for that scheme.
3. Calls the handler's resolve method for that specific URI.
4. Returns the content as a `resource` content block (per [RFD 058]).

The result is a normal tool call response in the conversation stream.
[RFD 067] deduplication applies: if the content has not changed since the
original attachment (or a previous refresh), the response is replaced with a
reference; otherwise, the full content is delivered.

#### Superseded resource handling

When `refresh_resource` delivers new content for a URI that already exists in
the stream, the earlier snapshot becomes stale.
The stale content remains in the stored conversation (history is immutable), but
JP can omit it when building the LLM request, similar to how [RFD 036]
compaction drops old tool responses.

Omitting a superseded resource trades token savings against cache invalidation
across several heuristics:

- **Token size threshold.** Small resources (< N tokens) may cost less to keep
  than the cache invalidation caused by removing them.
- **Cache position.** Resources within the provider's cache lookback window
  (e.g., Anthropic's ~30 turns) are expensive to remove, whereas resources
  outside the window can be removed without cache cost.
- **Content delta.** Nearly identical old and new content saves few tokens on
  removal, while completely different content benefits from removal to avoid
  confusing the LLM with contradictory versions.
- **Turn distance.** Recent resources are likely in the LLM's active attention;
  distant resources may not be.

The specific policy (defaults, configuration, and which heuristics to implement)
is deferred to [RFD 067] and [RFD 036], which own the deduplication and
compaction logic respectively.

#### Handler trait change

The handler trait needs to resolve a single resource by URI.
For the current trait, this means adding a method:

```rust
async fn resolve_one(
    &self,
    uri: &Url,
    cwd: &Utf8Path,
    mcp: Client,
) -> Result<Resource>;
```

For the [RFD 015] trait (which already takes `&[Url]`), passing a single-element
slice to `resolve()` is sufficient.
The handler returns one `Resource` for the one URI.

> [!TIP]
> [RFD 015] currently defines the handler return type as `Vec<Attachment>`.
> It will land first without the `Resource` change.
> Once this RFD is accepted, the handler return type should be updated to
> `Vec<Resource>` as a follow-up.

#### When the LLM refreshes

The LLM decides when to refresh; JP does not automatically re-resolve
attachments or rewrite history.
The LLM calls `refresh_resource` when it expects the content to be stale, such
as after modifying a file or when the user mentions that something has changed.

Future extensions can inform the LLM about stale resources without automatic
refresh:

- **[RFD 011] notifications.** JP could check declared attachments at the start
  of each turn (by re-resolving and comparing checksums) and emit a system
  notification if content has changed: "The attached resource src/main.rs has
  changed since turn 5."
  The LLM then decides whether to call `refresh_resource`.
- **Config-level refresh policy.** An optional `refresh` field on attachment
  declarations (`manual`, `notify`) could control whether JP checks for changes,
  defaulting to `manual` (no checking).
  This is not designed in this RFD.

### Provider migration

Each provider currently handles attachments in its `build_request` method,
receiving them as a separate `ThreadParts::attachments` `Vec` and prepending
them to the first user message.
With resources on `ChatRequest`, providers process them inline as part of the
event stream:

- **Anthropic:** `convert_event` currently converts a `ChatRequest` into a
  single `MessageContent::Text` block.
  With resources, it produces a `MessageContent::Text` for the user's query
  followed by one `MessageContent::Document` per resource block (`Text`
  resources become `DocumentSource::Text`; binary resources become
  `DocumentSource::Base64`).
  `Resource.name` (preserved in event metadata) maps to the document `title`.
  Cache breakpoint logic applies to the document blocks at their actual turn
  position rather than always at message 0.
- **Google/Gemini:** Text resources serialize to XML inline at their turn (using
  the same format as `text_attachments_to_xml`), and binary resources become
  `ContentPart::InlineData`.
- **OpenAI:** Text resources become XML-wrapped `ContentItem::Text`, and binary
  resources map to `ContentItem::Image` or `ContentItem::File` depending on MIME
  type.
- **OpenRouter:** Same as OpenAI, adapted for the OpenRouter content block
  types.

`Thread` and `ThreadParts` lose the `attachments` field.
The `text_attachments_to_xml` helper may remain for providers that render text
resources as XML, operating on `&[Resource]` from `ChatRequest` rather than a
separate `Vec<Attachment>`.

## Drawbacks

- **Breaking change to the handler trait.** All handlers must be updated to
  return `Resource` instead of `Attachment`.
  Because handlers are internal and [RFD 015] is already redesigning the trait,
  this is acceptable.
  [RFD 015] will land first with `Vec<Attachment>`, and the migration to
  `Vec<Resource>` will follow once this RFD is accepted.
- **Breaking change to provider implementations.** Every provider's
  `build_request` method changes because attachments arrive as `Resource` fields
  on `ChatRequest` events instead of a separate `Vec`.
- **Snapshot semantics may surprise users.** Users accustomed to attachments
  always showing the latest content will find that attached files are frozen at
  attachment time.
  The `refresh_resource` tool and explicit `--attach` mitigate this, but the
  behavioral change needs documentation.
- **Re-attaching an existing URI creates duplicate content.** When a user
  re-attaches a resource with `--attach`, the conversation contains two
  snapshots.
  If the content has not changed, [RFD 067] deduplication avoids wasting tokens;
  if the content has changed, both versions remain, which lets the LLM see the
  evolution at the risk of confusion.
- **Canonical URI correctness is critical.** If a handler produces an incorrect
  canonical URI, deduplication fails silently by missing valid matches or
  producing false positives.

## Alternatives

### Keep attachments at turn 0, add metadata

Leave the insertion point at the first user message, but add URI and MIME type
to `Attachment` so deduplication works without refactoring providers.

**Rejected because:** Adding a resource at turn 30 still modifies turn 0 and
invalidates the cache for the entire conversation.
Fixing the insertion point is a primary motivator for this RFD, not just adding
metadata.

### Separate `AttachmentResolved` event type

Add an `EventKind::AttachmentResolved` variant to record when attachment content
is delivered, instead of embedding resources in `ChatRequest`.

**Rejected because:** `EventKind` follows a request/response pattern, and
`AttachmentResolved` has no request counterpart.
It would also require special handling in `is_provider_visible()` and in every
provider's event conversion logic.
Embedding resources in `ChatRequest` is simpler and matches how providers
already process user messages.

### Auto-refresh changed resources

Automatically re-resolve attachments at each turn and insert updated resource
blocks into the current `ChatRequest` when content changes.

**Rejected because:** Dynamic resources such as command output with timestamps
would change and invalidate the cache on every turn, even when the LLM does not
need fresh data.
The snapshot model with `refresh_resource` gives the LLM control over when to
pay the refresh cost.

### Use a JP-specific resource type instead of MCP alignment

Define a custom resource model optimized for JP without regard for MCP
compatibility.

**Rejected because:** MCP already standardizes the right primitives for resource
identity (URIs, MIME types, annotations).
Adopting MCP's model lets MCP tool responses and attachment resources share the
same type, enabling [RFD 067] deduplication across both without a translation
layer.

## Non-Goals

- **Automatic resource refresh.** JP does not re-resolve attached resources or
  check for staleness; the LLM requests fresh content via `refresh_resource`.
  Automatic staleness detection via [RFD 011] notifications is a future
  extension.
- **Resource deduplication logic.** This RFD establishes the shared identity
  model (`Resource`, canonical URIs, MIME types); [RFD 067] defines the
  deduplication algorithm.
- **Handler trait redesign.** [RFD 015] redesigns the handler trait structure.
  This RFD only changes the return type from `Attachment` to `Resource` and adds
  single-resource resolution.
- **Attachment config syntax changes.** The `conversation.attachments` config
  field and `--attach` CLI flag are unchanged; only resolution timing and
  placement change.
- **Superseded resource removal policy.** This RFD outlines heuristics for
  omitting superseded resources from LLM requests, but defers the specific
  defaults and configuration to [RFD 067] and [RFD 036].

## Risks and Open Questions

### URI canonicalization edge cases

File URIs require careful handling of symlinks, case-insensitive filesystems,
Unicode normalization, and Windows path separators.
The file handler needs thorough test coverage for these cases because a
malformed canonical URI silently breaks deduplication.

### Inline content bloats the conversation file

Resource content is stored inline in `events.json` inside `ChatRequest` and
`ToolCallResponse` events.
Running `--attach ./src` on a modest project can resolve to hundreds of files,
and binary resources inflate further with base64 encoding, increasing disk
usage, `jp query` parse times, and search noise in developer tools.

This is a blocking concern that a future RFD can address by externalizing
content payloads from `events.json`.
Because externalization is a persistence-layer serialization concern, the
`Resource` and `ContentBlock` domain types are unaffected, and this RFD proceeds
with inline content as the initial storage format.

### Interaction with conversation compaction

When [RFD 036] conversation compaction operates on turns with attached
resources, it should preserve the `Resource` metadata (URI, MIME type) even if
the content is dropped, so deduplication can still identify the resource.
The exact behavior depends on the compaction design.

### Migration of existing conversations

Existing conversations lack a `resources` field on `ChatRequest`, which
`#[serde(default)]` deserializes as an empty `Vec`.
However, attachments in existing conversations were previously prepended to turn
0 without being recorded in `ChatRequest.resources`.
If the conversation config still declares those attachments, the new resolution
logic will see no matching URIs in the event stream and resolve them again on
the next turn.
This one-time duplication is acceptable during migration.

### `refresh_resource` tool visibility

Registering `refresh_resource` when a conversation has no attached resources
wastes a tool definition in the context window.
JP can register the tool conditionally when `ChatRequest.resources` is non-empty
anywhere in the stream.

## Implementation Plan

### Phase 1: `Resource` type and `ResourceContent`

Define `Resource` (with canonical URI, MIME type, content, annotations, and JP
extensions `name`, `title`, `description`, `formatted`) in a shared location.
Align `ResourceContent` with [RFD 058]'s `ResourceContent` type, define
`Annotations` matching MCP's specification (`audience`, `priority`,
`lastModified`), and implement `From<rmcp::ResourceContents>` and
`From<rmcp::EmbeddedResource>`.

This phase introduces no behavioral changes and can be merged independently.

### Phase 2: Handler return type migration

Update the handler trait to return `Vec<Resource>` instead of `Vec<Attachment>`,
and update all built-in handlers to produce canonical URIs and MIME types.
If [RFD 015] lands first, this phase is smaller because the trait is already
being rewritten; otherwise, it is an additive change to the existing trait.

Depends on Phase 1 and can be merged independently.

### Phase 3: `ChatRequest.resources` and resolution logic

Add `resources: Vec<Resource>` to `ChatRequest`.
Implement the resolution logic to resolve config-declared attachments on new
conversations and `--attach` resources fresh at the current turn.
Update `build_thread` and `register_attachment` to use the new resolution path
instead of building a separate `Vec<Attachment>`.

Depends on Phase 2.

### Phase 4: Provider migration

Update all providers to read resources from `ChatRequest` events instead of
`ThreadParts::attachments`, remove `attachments` from `Thread` and
`ThreadParts`, and update or remove `text_attachments_to_xml` as needed.
This is the largest phase, and each provider can be migrated independently.

Depends on Phase 3.

### Phase 5: `refresh_resource` built-in tool

Implement the `refresh_resource` tool (URI parsing, handler dispatch,
single-resource resolution, resource block response) and register it
conditionally when the conversation has attached resources.
Add `resolve_one` to the handler trait, or pass a single-element slice to
`resolve` if [RFD 015] has landed.

Depends on Phases 2 and 3.

### Phase 6: Remove `Attachment` type

Once all consumers have migrated to `Resource`, remove `Attachment`,
`AttachmentContent`, and related code from `jp_attachment`, and update the
crate's public API to use `Resource` terminology.

Depends on all previous phases.

## References

- [MCP Resources Specification (2025-11-25)][MCP Resources]: Defines resource
  URIs, content types, annotations, and URI schemes.
  The `Resource` type in this RFD follows MCP's resource content format with JP
  extensions.
- [MCP Tools][MCP Tools]: Defines typed content blocks for tool responses.
- [MCP Tasks][MCP Tasks]: Defines the task lifecycle model relevant to stateful
  tool support.
- [RFD 058: Typed Content Blocks for Tool Responses][RFD 058]: Introduces the
  `ContentBlock` and `ResourceContent` types for tool responses.
  This RFD aligns attachments with the same model via the shared `Resource`
  type.
- [RFD 067: Resource Deduplication for Token Efficiency][RFD 067]: Defines
  deduplication logic that depends on the shared identity model (canonical URIs,
  checksums) established by this RFD and RFD 058.
- [RFD 015: Simplified Attachment Handler Trait][RFD 015]: Redesigns the handler
  trait to stateless `validate`/`resolve`.
  This RFD changes the return type; RFD 015 changes the trait structure.
  Both can proceed independently.
- [RFD 011: System Notification Queue][RFD 011]: Future path for notifying the
  LLM about stale resources without automatic refresh.
- [RFD 036: Conversation Compaction][RFD 036]: Relevant for how resource
  metadata interacts with compacted conversations, and for superseded resource
  removal heuristics.

[MCP Resources]: https://modelcontextprotocol.io/specification/2025-11-25/server/resources.md
[MCP Tasks]: https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/tasks.md
[MCP Tools]: https://modelcontextprotocol.io/specification/2025-11-25/server/tools.md
[RFD 011]: 011-system-notification-queue.md
[RFD 015]: 015-simplified-attachment-handler-trait.md
[RFD 036]: 036-conversation-compaction.md
[RFD 058]: 058-typed-content-blocks-for-tool-responses.md
[RFD 066]: 066-content-addressable-blob-store.md
[RFD 067]: 067-resource-deduplication-for-token-efficiency.md
[RFD 135]: 135-jp-decide.md
