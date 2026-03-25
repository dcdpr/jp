# RFD 058: Typed Content Blocks for Tool Responses

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-15

## Summary

This RFD replaces `jp_tool::Outcome` with typed content blocks for tool
responses. Tools return an array of structured content blocks — `text`,
`resource`, and `question` — instead of opaque strings. This gives JP the
metadata it needs to own resource formatting, reason about resource identity,
and enable resource-level optimizations like deduplication.

The content block schema follows MCP's `CallToolResult` model because MCP has
already standardized the right primitives for `text` and `resource` blocks, and
it is the format MCP tools already speak. JP extends the model with `question`
blocks for the inquiry system, and with a stateful response envelope for
long-running tools.

## Motivation

JP's tool output is opaque. The `Outcome::Success { content: String }` type
gives JP no way to distinguish between "here is the content of `main.rs`" and
"check succeeded, no warnings." Both are strings. JP cannot:

- **Control resource formatting.** Each tool invents its own presentation. Some
  wrap content in markdown fences, others use XML, others return plain text.
  There is no consistency, and JP cannot adapt formatting per-model or
  per-configuration.

- **Reason about resource identity.** Without URIs, JP has no way to know that
  two tool calls returned the same file, or that a tool returned a file already
  provided as an attachment. This blocks resource-level optimizations like
  deduplication.

MCP tools already return typed content blocks with URIs, mimeTypes, and
annotations — exactly the metadata JP needs. But JP collapses these into flat
strings at the MCP boundary, discarding the structure:

```rust
// From jp_llm/src/tool.rs — execute_mcp()
let content = result
    .content
    .into_iter()
    .filter_map(|v| match v.raw {
        RawContent::Text(v) => Some(v.text),
        RawContent::Resource(v) => match v.resource {
            ResourceContents::TextResourceContents { text, .. } => Some(text),
            ResourceContents::BlobResourceContents { blob, .. } => Some(blob),
        },
        RawContent::Image(_) | RawContent::Audio(_)
            | RawContent::ResourceLink(_) => None,
    })
    .collect::<Vec<_>>()
    .join("\n\n");
```

The problem isn't that tools lack structure — MCP tools already have it. The
problem is that JP's internal model is a flat string, so all structure is lost
regardless of source. Typed content blocks fix this. Tools return structured
output with optional identity metadata (URIs, mimeTypes), and JP processes all
tool responses — local, MCP, and builtin — through a single typed pipeline.

### Why MCP's content model?

MCP's `CallToolResult` defines typed content blocks (`text`, `resource`) with
resource identity via URIs and mimeTypes. These are the primitives JP needs. If
we designed our own, we would arrive at something ~90% identical. Adopting MCP's
schema:

- Avoids reinventing well-designed types.
- Means MCP tool responses pass through without lossy conversion.
- Gives the content model a stable, externally-maintained specification to
  reference.

This is not an attempt to make local tools into MCP servers. Local tools remain
one-shot processes writing JSON to stdout. The transport is different; the
content schema is shared.

JP also has first-class support for **unstructured text output**. A local tool
can be a quick shell script or a direct call to `fd` without any output
processing. Raw stdout that isn't valid JSON is delivered to the LLM as-is. The
trade-off is that unstructured output cannot participate in resource-level
features like deduplication or JP-controlled formatting, because JP has no
metadata to work with.

## Design

> [!TIP]
> The `Resource` type and the MCP-compatible superset design principle that
> governs its shape are defined in [RFD 065]. The `ContentBlock::Resource`
> variant wraps that type directly.

### Response format

A tool's stdout is a JSON object with a `content` array of typed content blocks:

```json
{
  "content": [
    {
      "type": "text",
      "text": "Check succeeded."
    }
  ]
}
```

If the tool's stdout is not valid JSON, or does not contain a `content` array,
JP treats the entire stdout as a single `text` block. This is the raw text
fallback, and it is a permanent, supported path — not a deprecated compatibility
mode.

### Content block types

#### `text`

Informative text output. JP passes it through to the LLM as-is. The tool owns
the formatting.

```json
{
  "type": "text",
  "text": "Check succeeded. No warnings or errors found."
}
```

#### `resource`

Identified content with a URI and mimeType. JP owns formatting and may apply
resource-level optimizations.

```json
{
  "type": "resource",
  "resource": {
    "uri": "file:///project/src/main.rs",
    "mimeType": "text/rust",
    "text": "fn main() {}"
  }
}
```

Resource blocks carry content inline, as either text or binary (base64 `blob`):

```json
{
  "type": "resource",
  "resource": {
    "uri": "file:///project/logo.png",
    "mimeType": "image/png",
    "blob": "<base64-encoded data>"
  }
}
```

The `uri` field identifies the resource using standard URI schemes:

- `file:///path` for files (canonicalized by JP relative to the workspace root)
- `https://...` for web resources
- Custom schemes for other resource types

The `mimeType` field tells JP how to format the content. For `text/rust`, JP
wraps the content in a ```` ```rs ```` fenced code block. If the mimeType is
unknown or absent, JP falls back to a plain code fence with no language tag.

MCP servers may also return `resource_link` blocks (URI without inline content).
JP handles these through the MCP client's `resources/read` capability. Local
tools cannot return `resource_link` blocks since they exit after responding and
cannot serve follow-up fetches. Stateful local tools could in principle support
this pattern (the process is still alive), but the interaction mechanism is not
defined in this RFD. Tools that have content to share should embed it inline as
a `resource` block.

Resource blocks support MCP's standard annotations (`audience`, `priority`,
`lastModified`). These are optional. JP defines the annotation types for
type-level compatibility with MCP but does not act on them initially.

Local tools may also provide optional JP extension fields (`name`, `title`,
`description`) for richer display metadata. These fields are `Option<T>` on
the internal `Resource` type and default to `None` when absent from the JSON
output (e.g., from MCP tools). See [RFD 065] for the full type definition and
the MCP-compatible superset principle.

##### Tool-provided formatting

By default, JP formats resource blocks based on mimeType. Tools that need custom
presentation can provide a `formatted` field alongside the raw content:

```json
{
  "type": "resource",
  "resource": {
    "uri": "file:///project/src/main.rs",
    "mimeType": "text/rust",
    "text": "fn main() {\n    println!(\"Hello world!\");\n}"
  },
  "formatted": "```rs (src/main.rs, lines 1-3)\nfn main() {\n    println!(\"Hello world!\");\n}\n```"
}
```

When `formatted` is present, JP uses it for LLM delivery but uses the raw
`resource.text` (or `resource.blob`) for checksums and identity matching. This
separates presentation from identity — tools can provide rich formatting without
opting out of resource-level features like deduplication.

When `formatted` is absent, JP formats the resource from the raw content using
the mimeType-to-language mapping.

#### `question`

A request for input. Questions are content blocks, not a separate response type.
A tool can return questions alongside other content:

```json
{
  "content": [
    {
      "type": "resource",
      "resource": {
        "uri": "file:///project/src/main.rs",
        "mimeType": "text/rust",
        "text": "fn main() {}"
      }
    },
    {
      "type": "resource",
      "resource": {
        "uri": "file:///project/src/lib.rs",
        "mimeType": "text/rust",
        "text": "pub mod config;"
      }
    },
    {
      "type": "question",
      "question": {
        "id": "confirm",
        "text": "Apply these changes to a third file?",
        "schema": {
          "type": "boolean"
        },
        "default": true
      }
    }
  ]
}
```

In this example, the two resource blocks provide context for the question — the
user (or LLM) can see what the tool has produced so far while deciding how to
answer. Content blocks preceding a question are displayed above the terminal
prompt (for user-targeted inquiries) or included in the request to the assistant
(for LLM-targeted inquiries). The question's `text` field is a short label shown
alongside the input — it should be a one-liner, not a formatted document.
Supporting material (diffs, file contents, explanations) belongs in `text` or
`resource` content blocks, not in the question text.

JP presents the question, collects the answer, and re-executes the tool with the
accumulated answer. On re-execution, the tool returns its complete output (e.g.,
all four resource blocks). JP uses the final invocation's content as the tool
call result.

Every tool response is the complete answer for that invocation. JP does not
stitch content across invocations. One-shot tools produce their full output on
each execution. Stateful tools produce the response appropriate to the current
`fetch`/`apply` call. Content blocks alongside questions serve as context during
the inquiry flow but do not persist independently.

##### Question schema

Questions use JSON Schema to define the expected answer:

```json
{
  "type": "question",
  "question": {
    "id": "target_branch",
    "text": "Which branch should I merge into?",
    "schema": {
      "type": "string",
      "enum": [
        "main",
        "develop",
        "staging"
      ]
    },
    "default": "main"
  }
}
```

A tool may return multiple questions in a single response. They are presented in
array order. Content blocks between questions serve as context for the question
that follows them. Content blocks after the final question are rendered normally
— JP displays all blocks in array order regardless of position relative to
question blocks.

JP maps common schema patterns to terminal prompts:

| Schema                                | Terminal prompt |
|---------------------------------------|-----------------|
| `{ "type": "boolean" }`               | Yes/No prompt   |
| `{ "type": "string", "enum": [...] }` | Select prompt   |
| `{ "type": "string" }`                | Text input      |

For complex schemas (nested objects, arrays, etc.) that don't map to a simple
prompt, JP opens the user's editor with a JSON template matching the schema. The
LLM path always works regardless of schema complexity — JP sends the schema and
gets a conforming response.

`jp_tool` provides an `AnswerType` convenience enum (`Boolean`, `Select`,
`Text`) that generates the corresponding JSON Schema. Tool authors can use
either `AnswerType` or raw JSON Schema.

##### Relationship with MCP elicitation

MCP's elicitation system (`elicitation/create`) serves the same purpose: the
server asks the client for input via a JSON Schema form. JP normalizes both MCP
elicitation requests and local tool `question` blocks into a single internal
input-request type. MCP elicitation requests pass through with minimal
transformation. Local tool questions are converted from `AnswerType` or raw
schema into the same internal type.

JP extends MCP's user-only elicitation model with assistant-targeted inquiry —
routing questions to the LLM instead of the user. This is a JP-side
annotation from tool configuration, not a field the tool sets.

### Error responses

Errors use `isError: true`, following MCP's convention. The tool sets this
flag in its JSON output. The content blocks carry the error message:

```json
{
  "content": [
    {
      "type": "text",
      "text": "File not found: foo.rs"
    }
  ],
  "isError": true
}
```

JP extends this with optional metadata via MCP's `_meta` field:

```json
{
  "content": [
    {
      "type": "text",
      "text": "File not found: foo.rs"
    }
  ],
  "isError": true,
  "_meta": {
    "computer.jp/error": {
      "transient": true,
      "trace": [
        "io error: No such file or directory (os error 2)"
      ]
    }
  }
}
```

- `transient`: Whether the error is retryable.
- `trace`: Error source chain for debugging.

When `_meta."computer.jp/error"` is absent, errors are treated as non-transient
with no trace.

### Stateful tool responses

The content block format also supports long-running tools that persist across
multiple interactions (e.g., interactive git staging, persistent shell
sessions). A stateful tool response includes a `computer.jp/status` field
in `_meta`:

```json
{
  "content": [
    {
      "type": "text",
      "text": "Stage this hunk? [y/n/...]"
    }
  ],
  "_meta": {
    "computer.jp/status": "running"
  }
}
```

The `computer.jp/status` field uses the following states (see [RFD 009]):

| Status    | Meaning                                  |
|-----------|------------------------------------------|
| `running` | Tool is active, may have partial output  |
| `waiting` | Tool needs input (content includes       |
|           | `question` blocks)                       |
| `stopped` | Tool has finished (content is the final  |
|           | result)                                  |

When `computer.jp/status` is absent, the response is implicitly `stopped` — the
tool completed in a single invocation. This means one-shot tools never need to
set `status`. The stateful envelope is purely opt-in for tools that declare
stateful support.

A `waiting` response combines content and questions naturally:

```json
{
  "content": [
    {
      "type": "text",
      "text": "Diff for hunk 3/5:"
    },
    {
      "type": "text",
      "text": "@@ -10,3 +10,4 @@\n+    new_line();"
    },
    {
      "type": "question",
      "question": {
        "id": "hunk_3",
        "text": "Stage this hunk?",
        "schema": {
          "type": "string",
          "enum": [
            "y",
            "n",
            "s",
            "q"
          ]
        },
        "default": "y"
      }
    }
  ],
  "_meta": {
    "computer.jp/status": "waiting"
  }
}
```

Each stateful interaction produces its own `ToolCallResponse` for the LLM. The
tool process manages its own state — JP relays responses, it does not stitch
them together. When the tool returns `stopped`, the content blocks in that final
response are the tool's result.

A `stopped` response with `isError: true` signals that the tool finished with an
error.

The stateful protocol's execution model (handle registry, spawn/fetch/apply/
abort actions, one-shot wrapping) is out of scope for this RFD. This RFD defines
the content format; the lifecycle mechanics are a separate concern defined in
[RFD 009].

### Internal representation

JP's internal type for tool results, defined in `jp_tool`:

```rust
/// A typed content block from a tool response.
pub enum ContentBlock {
    Text {
        text: String,
    },
    Resource(Resource),
    Question {
        question: InputRequest,
    },
}

/// A request for input from the user or assistant.
pub struct InputRequest {
    /// Unique ID for correlating answers on re-execution.
    pub id: String,
    /// Short prompt label (one-liner). Displayed alongside the input field
    /// in the terminal, or as the question text sent to the assistant.
    /// Supporting context belongs in content blocks, not here.
    pub text: String,
    /// JSON Schema defining the expected answer shape.
    pub schema: serde_json::Map,
    /// Default value, if any.
    pub default: Option<serde_json::Value>,
}
```

`ContentBlock` lives in `jp_tool` because it is the contract type shared
between tool authors and JP. The `Resource` type (defined in [RFD 065]) carries
both MCP-standard fields (`uri`, `content`, `mime_type`, `annotations`) and
optional JP extensions (`name`, `title`, `description`, `formatted`). MCP tool
responses populate only the MCP fields; local tools may populate the extensions.
`ResourceContent` is defined alongside `Resource` — see [RFD 065] for the
full type definition.

All three execution paths (`execute_local`, `execute_mcp`, `execute_builtin`)
produce `Vec<ContentBlock>`. The formatting step that converts content blocks
into the final string delivered to the LLM is shared across all paths.

### `ToolCallResponse` migration

The current `ToolCallResponse` in `jp_conversation` carries `Result<String,
String>`. This changes to carry `Vec<ContentBlock>` internally (along with an
`is_error` flag).

Backward compatibility with stored conversations is handled through
`ToolCallResponse`'s existing custom `Deserialize` impl. The deserializer
detects the old format (flat `content` + `is_error` fields) and converts it to a
single `ContentBlock::Text` block. On serialization, the new format is written.
Old conversations remain readable; re-saved conversations use the new format.

### Response parsing

`parse_command_output` in `jp_llm/src/tool.rs` is the single parsing point for
local tool stdout. The new parsing order:

1. Try JSON with `content` array → parse typed content blocks (including any
   `question` blocks and optional `_meta."computer.jp/status"` envelope).
2. Fall back to raw stdout as a single `ContentBlock::Text`.

`Outcome` is not preserved as a parsing target. Since JP is the only consumer of
the `jp_tool` API, all tools are migrated in one sweep when the new types land.
There is no incremental migration period.

### JP-controlled resource formatting

When a tool returns `resource` blocks without a `formatted` field, JP formats
them for the LLM. For a resource with `mimeType: "text/rust"`:

````txt
```rs
fn main() {
    println!("Hello world!");
}
```
````

JP may prepend metadata (filename, line range) or adapt the format based on
model preferences or conversation configuration. The mimeType-to-language
mapping follows established conventions.

When a tool provides a `formatted` field, JP uses it verbatim for LLM delivery.
The raw content in `resource.text`/`resource.blob` is still used for checksums
and identity matching.

For `text` blocks, JP passes the text through as-is. The tool owns the
formatting of informative output.

## Drawbacks

**mimeType-to-language mapping.** JP needs a mapping from MIME types to code
fence language tags. This is a well-understood problem with existing libraries,
but it is a new dependency and maintenance surface.

**`ToolCallResponse` format change.** Stored conversations written with the old
format need to be readable. The custom deserializer handles this, but it is
added complexity.

**Questions as content blocks add processing complexity.** JP must scan the
content array for `question` blocks to decide whether to trigger the inquiry
flow. This is more complex than the current dedicated `NeedsInput` variant, but
the composability gains (questions alongside content) justify it.

## Alternatives

### Keep `Outcome` and add a `Resources` variant

Add `Outcome::Resources { items: Vec<Resource> }` alongside `Success`.

**Rejected because:** This creates a JP-specific resource model that duplicates
what MCP has already standardized. It also doesn't address the MCP metadata loss
— `execute_mcp` would still need separate handling.

### Add optional metadata fields to `Outcome::Success`

Keep the existing format but let tools annotate output with resource identifiers
and checksums.

**Rejected because:** It bolts resource semantics onto an opaque string. JP
still can't format resources, can't decompose multi-resource responses, and
tools still own presentation.

### Make local tools full MCP servers

Instead of adopting just the content model, make every local tool implement the
full MCP protocol (JSON-RPC, initialize, tools/list, tools/call).

**Rejected because:** The authoring burden is disproportionate. Writing a local
tool today is trivially simple — read CLI args, write JSON, exit. Full MCP
server implementation requires a JSON-RPC stack, lifecycle management, and an
SDK. The content model is the valuable part; the transport protocol is not worth
the cost for one-shot tools.

### Inquiry as a separate response type

Have `question` be a top-level response field rather than a content block type.

**Rejected because:** It prevents tools from returning content alongside
questions. A tool that has produced partial results and needs input before
continuing must be able to express both in one response. Making `question` a
content block enables natural composition: "here are 3 resource blocks I'm done
with, plus a question about the 4th."

## Non-Goals

- **Resource deduplication.** Using resource URIs and content to avoid redundant
  delivery to the LLM. This RFD establishes the content model that makes
  deduplication possible, but the deduplication logic itself is a separate
  concern.

- **Mandating resource blocks.** Tools are free to return `text` blocks for any
  output, or to skip JSON entirely and write plain text to stdout. The
  `resource` type is available for tools that return identified content; it is
  not required.

- **Prescribing per-tool migration.** This RFD defines the protocol. How
  specific tools adopt it — and whether they return `resource` or `text` blocks
  is decided per-tool during implementation.

- **Stateful tool execution model.** The handle registry, spawn/fetch/apply/
  abort actions, and one-shot wrapping are separate concerns. This RFD defines
  the content format that stateful tools use in their responses.

- **Image and audio blocks.** MCP supports `image` and `audio` content block
  types. JP does not use these today. They can be added later since the content
  block model is extensible.

## Risks and Open Questions

### mimeType inference

For file-reading tools, the mimeType can be inferred from the file extension.
For HTTP tools, it comes from the `Content-Type` header. For other tools, the
mimeType may not be obvious. The fallback is no mimeType, which means JP uses a
plain code fence.

### URI canonicalization

File URIs need to be canonical for cross-tool and cross-attachment matching.
`file:///project/./src/../src/main.rs` and `file:///project/src/main.rs` must
resolve to the same identity. JP canonicalizes URIs after receiving them. The
rules need to be documented: resolve `.` and `..`, normalize trailing slashes,
ensure paths are relative to the project root.

### Content block ordering

When a tool returns multiple content blocks, JP preserves the order from the
tool's response. For mixed text and resource blocks, the tool may intend a
specific order (e.g., a text explanation followed by a resource). JP should not
reorder blocks.

### Malformed content blocks

If a local tool returns JSON with a `content` array but individual blocks are
malformed (e.g., a `resource` block missing `uri`, or an unknown `type` value),
JP should log a warning and skip the malformed block rather than falling through
to raw-stdout parsing. A tool that produces a valid `content` array is clearly
trying to use the new format — a silent fallback to raw text would be a
confusing failure mode.

### Content alongside questions in one-shot tools

When a one-shot tool returns content blocks alongside questions, the content
serves as context for the inquiry — the user or LLM can see what the tool has
produced while deciding how to answer. On re-execution with the answer, the tool
produces its complete output (including any content that was previously shown
alongside the question). JP uses the final invocation's response as the tool
call result.

This means one-shot tools must re-emit all content on each invocation. This is
natural for stateless processes — they compute their full output each time. The
content-alongside-questions pattern is still valuable because it shows the
user/LLM relevant context during the inquiry, even though JP doesn't persist it
across invocations.

### Complex JSON Schema questions in the terminal

The JSON Schema > terminal prompt mapping covers simple cases (`boolean`,
`string`, `enum`). For complex schemas, JP falls back to opening the user's
editor with a JSON template. This is functional but not a great UX for
multi-field forms. We may want to invest in a richer terminal form renderer
later, but the editor fallback is sufficient for now.

### Interaction with conversation compaction

[RFD 036] describes conversation compaction. When compaction drops old tool
responses, it should retain the resource metadata (URIs, mimeTypes) even if the
content is dropped, so that future resource-level reasoning remains possible.
The specifics depend on the compaction design.

## Implementation Plan

### Phase 1: Types and internal representation

Define `ContentBlock`, `ResourceContent`, `InputRequest`, and `Annotations` in
`jp_tool`. Update `ToolCallResponse` in `jp_conversation` to carry
`Vec<ContentBlock>` with backward-compatible deserialization. Update
`ExecutionOutcome` in `jp_llm` to carry `Vec<ContentBlock>` instead of
`Result<String, String>`.

Can be merged independently. No tool-side changes yet.

### Phase 2: MCP tool passthrough

Update `execute_mcp` to preserve typed content blocks from MCP tools instead of
collapsing to strings. Convert `rmcp` types to `ContentBlock` at the MCP
boundary.

Can be merged independently. MCP tools immediately benefit from preserved
metadata.

### Phase 3: Resource formatting

Implement the mimeType-to-language mapping and the formatting layer that wraps
resource content in code fences (or other presentation). Support the `formatted`
field override. This replaces the per-tool formatting currently embedded in each
tool's implementation.

Can be merged independently. Initially applies only to MCP tool responses.

### Phase 4: Local tool parsing and migration

Update `parse_command_output` to parse the content block format. Migrate all
tools in `.config/jp/tools/` and builtin tools to return the new format. Remove
`Outcome` from `jp_tool`.

This is a single coordinated change — all tools migrate together since JP is the
only consumer.

### Phase 5: Question blocks and inquiry migration

Update the inquiry system to detect `question` content blocks and trigger the
input flow. Implement the JSON Schema → terminal prompt mapping (boolean,
select, text) with editor fallback for complex schemas. Migrate existing
`NeedsInput` tools to use `question` blocks.

Depends on Phase 4.

### Phase 6: Stateful response envelope

Add `_meta."computer.jp/status"` field parsing. Integrate with the handle
registry and lifecycle management for long-running tools. This phase bridges the
content format (this RFD) with the stateful execution model.

Depends on Phase 5.

## References

- [MCP Tools Specification
  (2025-11-25)](https://modelcontextprotocol.io/specification/2025-11-25/server/tools)
  — defines `CallToolResult` content blocks.
- [MCP Resources Specification
  (2025-11-25)](https://modelcontextprotocol.io/specification/2025-11-25/server/resources)
  — defines resource URIs, content types, and annotations.
- [MCP Tasks Specification
  (2025-11-25)](https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/tasks)
  — defines the task lifecycle model relevant to stateful tool support.
- [RFD 009: Stateful Tool Protocol][RFD 009] — defines the stateful tool
  execution model. This RFD defines the content format used in stateful
  responses.
- [RFD 036: Conversation Compaction][RFD 036] — relevant for how resource
  metadata interacts with compacted conversations.
- [RFD 065: Typed Resource Model for Attachments][RFD 065] — defines the
  `Resource` type that `ContentBlock::Resource` wraps, and the MCP-compatible
  superset design principle governing resource type shapes.

[RFD 009]: 009-stateful-tool-protocol.md
[RFD 036]: 036-conversation-compaction.md
[RFD 065]: 065-typed-resource-model-for-attachments.md
