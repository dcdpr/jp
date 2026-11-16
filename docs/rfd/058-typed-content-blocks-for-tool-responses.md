# RFD 058: Typed Content Blocks for Tool Responses

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-15
- **Requires**: [RFD 065]
- **Required by**: [RFD 066], [RFD 067]
- **Extended by**: [RFD 094]
- **Summary**: Replaces opaque tool output strings with typed `text`,
  `resource`, and `question` content blocks aligned with MCP's `CallToolResult`.

## Summary

This RFD replaces `jp_tool::Outcome` with typed content blocks (`text`,
`resource`, and `question`) aligned with MCP's `CallToolResult` model, plus a
stateful response envelope for long-running tools.
Returning structured blocks instead of opaque strings gives JP the resource URIs
and metadata needed for consistent formatting and cross-turn deduplication.

## Motivation

`jp_tool::Outcome::Success { content: String }` represents all tool output as an
opaque string, so JP cannot distinguish between the contents of `main.rs` and a
status message like `"check succeeded, no warnings."` As a result, JP cannot:

- **Control resource formatting.** Each tool invents its own presentation,
  wrapping content in Markdown fences, XML, or plain text.
  JP cannot enforce consistent formatting or adapt it per model or
  configuration.

- **Reason about resource identity.** Without URIs, JP cannot tell when two tool
  calls return the same file, or when a tool returns a file already provided as
  an attachment.
  This blocks resource-level optimizations such as deduplication.

MCP tools already return typed content blocks with URIs, `mimeType`s, and
annotations, but JP collapses them into a flat string at the MCP boundary:

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

Because JP's internal model is a flat string, structure is lost regardless of
the tool's source.
Replacing `Outcome` with typed content blocks lets tools return structured
output with optional identity metadata (URIs and `mimeType`s), and lets JP
process local, MCP, and builtin tool responses through a single typed pipeline.

### Why MCP's content model?

MCP's `CallToolResult` already defines `text` and `resource` content blocks
identified by URIs and `mimeType`s.
Designing a custom schema would produce something nearly identical.
Adopting MCP's schema:

- Avoids duplicating existing types.
- Lets MCP tool responses pass through without lossy conversion.
- Grounds the content model in a stable, externally maintained specification.

Local tools do not become MCP servers.
They remain one-shot processes that write JSON to stdout; only the content
schema is shared with MCP, not the transport.

JP also supports **unstructured text output** as a first-class path.
A local tool can be a shell script or a direct call to `fd`: any stdout that is
not valid JSON is delivered to the LLM as-is.
Because unstructured output carries no metadata, it cannot participate in
resource-level features such as deduplication or JP-controlled formatting.

## Design

> [!TIP]
> [RFD 065] defines the `Resource` type and the MCP-compatible superset
> principle that governs its shape.
> `ContentBlock::Resource` wraps that type directly.

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

If stdout is not valid JSON or lacks a `content` array, JP treats the entire
output as a single `text` block.
This raw-text fallback is a permanent, supported path, not a deprecated
compatibility mode.

### Content block types

#### `text`

A `text` block carries informative text that the tool formats itself and JP
passes to the LLM as-is:

```json
{
  "type": "text",
  "text": "Check succeeded. No warnings or errors found."
}
```

Text blocks may carry an optional `mimeType` describing the format of the text:

```json
{
  "type": "text",
  "text": "## Summary\n\nAll checks passed.",
  "mimeType": "text/markdown"
}
```

`mimeType` on `text` is a JP extension following the MCP-compatible superset
principle from [RFD 065] (MCP's `TextContent` has no `mimeType`).
MCP tools never set it, and omitting it means plain text.
The field is defined for future JP-side presentation; JP does not act on it
initially, and LLM delivery always passes the text through unchanged.

Matching MCP's `TextContent` schema, `text` blocks also accept standard MCP
annotations (`audience`, `priority`, `lastModified`):

```json
{
  "type": "text",
  "text": "Summary for the user.",
  "annotations": {
    "audience": [
      "user"
    ]
  }
}
```

JP defines the annotation types for MCP compatibility but does not act on them
initially.

#### `resource`

A `resource` block carries content identified by a URI and `mimeType`.
JP owns its formatting and can apply resource-level optimizations:

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

The `mimeType` field tells JP how to format the content.
For `text/rust`, JP wraps the content in a ` ```rs ` fenced code block.
If `mimeType` is unknown or absent, JP falls back to a plain code fence with no
language tag.

MCP servers may also return `resource_link` blocks (a URI without inline
content), which JP resolves through the MCP client's `resources/read`
capability.
Local tools must embed content inline as a `resource` block instead: one-shot
tools exit after responding and cannot serve follow-up fetches, and this RFD
does not define a fetch mechanism for stateful local tools.

Resource blocks also accept MCP's optional annotations (`audience`, `priority`,
`lastModified`) and the optional JP extension fields (`name`, `title`,
`description`) defined in [RFD 065].
Extension fields are `Option<T>` on the internal `Resource` type and default to
`None` when omitted (such as in MCP tool responses).

##### Tool-provided formatting

When a tool needs custom presentation instead of JP's default `mimeType`-based
formatting, it can provide a `formatted` field alongside the raw content:

````json
{
  "type": "resource",
  "resource": {
    "uri": "file:///project/src/main.rs",
    "mimeType": "text/rust",
    "text": "fn main() {\n    println!(\"Hello world!\");\n}"
  },
  "formatted": "```rs (src/main.rs, lines 1-3)\nfn main() {\n    println!(\"Hello world!\");\n}\n```"
}
````

JP delivers `formatted` to the LLM verbatim while still using `resource.text` or
`resource.blob` for checksums and identity matching.
Separating presentation from identity lets tools supply custom formatting
without opting out of deduplication.

#### `question`

A `question` block requests input from the user or assistant.
Because questions are content blocks rather than a separate response type, a
tool can return them alongside other content:

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

Here, the two `resource` blocks provide context while the user or LLM decides
how to answer.
Content blocks preceding a question appear above the terminal prompt for
user-targeted inquiries, or in the assistant request for LLM-targeted inquiries.
The question's `text` field is a one-line prompt label shown next to the input
field; supporting material (diffs, file contents, explanations) belongs in
accompanying `text` or `resource` blocks.

JP presents the question, collects the answer, and re-executes the tool with the
accumulated answers.
JP does not stitch content across invocations: each response is the complete
output for that invocation, and content blocks returned alongside a question
serve only as context during the inquiry.
On re-execution, a one-shot tool returns its full output (for example, all four
`resource` blocks), which becomes the tool call result; a stateful tool returns
the response for the current `fetch` or `apply` call.

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

A response may contain multiple questions.
JP displays all blocks in array order: content blocks before a question provide
context for that question, and content blocks after the final question render
normally.

JP maps common schema patterns to terminal prompts:

| Schema                                | Terminal prompt |
| ------------------------------------- | --------------- |
| `{ "type": "boolean" }`               | Yes/No prompt   |
| `{ "type": "string", "enum": [...] }` | Select prompt   |
| `{ "type": "string" }`                | Text input      |

For complex schemas (nested objects, arrays) that do not map to a simple prompt,
JP opens the user's editor with a JSON template matching the schema.
LLM-targeted inquiries work with any schema, since JP sends the schema directly
to the model.

`jp_tool` provides an `AnswerType` convenience enum (`Boolean`, `Select`,
`Text`) that generates the corresponding JSON Schema.
Tool authors can use either `AnswerType` or raw JSON Schema.

##### Relationship with MCP elicitation

MCP's elicitation system (`elicitation/create`) serves the same purpose: the
server asks the client for input using a JSON Schema form.
JP normalizes MCP elicitation requests and local tool `question` blocks (whether
built from `AnswerType` or raw JSON Schema) into a single internal
`InputRequest` type.

While MCP elicitation targets only the user, JP also supports assistant-targeted
inquiries that route questions to the LLM.
Target selection comes from JP's tool configuration, not from the tool output.

### Error responses

Following MCP's convention, a tool reports an error by setting `isError: true`
in its JSON output and placing the error message in `content`:

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
sessions).
A stateful tool response includes a `computer.jp/status` field in `_meta`:

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

| Status    | Meaning                                 |
| --------- | --------------------------------------- |
| `running` | Tool is active, may have partial output |
| `waiting` | Tool needs input (content includes      |
|           | `question` blocks)                      |
| `stopped` | Tool has finished (content is the final |
|           | result)                                 |

When `computer.jp/status` is omitted, the status defaults to `stopped`
(completed in a single invocation), so one-shot tools never need to set it.
The stateful envelope is opt-in for tools that declare stateful support.

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

Each stateful interaction produces its own `ToolCallResponse` for the LLM.
The tool process manages its own state while JP relays responses without
stitching them together.
When the tool returns `stopped`, that response's content blocks are the final
result (or the final error if `isError: true`).

This RFD defines only the response content format; [RFD 009] defines the
stateful execution model (handle registry, `spawn`/`fetch`/`apply`/`abort`
actions, and one-shot wrapping).

### Internal representation

`jp_tool` defines the contract types shared between tool authors and JP:

```rust
/// A typed content block from a tool response.
pub enum ContentBlock {
    Text {
        text: String,
        /// Optional MIME type describing the text format (JP extension; MCP
        /// tools never set it).
        /// Absent means plain text.
        mime_type: Option<String>,
        /// MCP-standard annotations (`audience`, `priority`, `lastModified`).
        /// Carried for MCP compatibility; not acted on initially.
        annotations: Option<Annotations>,
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
    /// Short prompt label (one-liner).
    /// Displayed alongside the input field in the terminal, or as the question
    /// text sent to the assistant.
    /// Supporting context belongs in content blocks, not here.
    pub text: String,
    /// JSON Schema defining the expected answer shape.
    pub schema: serde_json::Map,
    /// Default value, if any.
    pub default: Option<serde_json::Value>,
}
```

`Resource` and `ResourceContent` (defined in [RFD 065]) carry the MCP-standard
fields (`uri`, `content`, `mime_type`, `annotations`) alongside optional JP
extensions (`name`, `title`, `description`, `formatted`).
MCP tool responses populate only the MCP fields; local tools may also populate
the extensions.

All three execution paths (`execute_local`, `execute_mcp`, `execute_builtin`)
produce `Vec<ContentBlock>` and share the formatting step that renders blocks
for the LLM.

### `ToolCallResponse` migration

`ToolCallResponse` in `jp_conversation` changes from `Result<String, String>` to
`Vec<ContentBlock>` plus an `is_error` flag.

`ToolCallResponse`'s custom `Deserialize` implementation preserves backward
compatibility with stored conversations: it detects the legacy flat `content`
and `is_error` fields and converts them into a single `ContentBlock::Text`
block.
Serialization always writes the block format, so existing conversations remain
readable and upgrade when re-saved.

Downstream consumers of `ToolCallResponse`, including the terminal result
renderer and the `serve-web` plugin, become block-aware, and `inline_results`
applies uniformly across a response's blocks.

### Response parsing

`parse_command_output` in `jp_llm/src/tool.rs` is the single entry point for
parsing local tool stdout:

1. If stdout is JSON with a `content` array, parse typed content blocks
   (including `question` blocks and any `_meta."computer.jp/status"` field).
2. Otherwise, wrap raw stdout in a single `ContentBlock::Text`.

`Outcome` is removed rather than kept as a legacy parsing target.
Because JP is the sole consumer of `jp_tool`, all tools migrate in a single
change.

### JP-controlled resource formatting

When a `resource` block omits `formatted`, JP renders it for the LLM using a
conventional `mimeType`-to-language mapping.
For a resource with `mimeType: "text/rust"`:

````txt
```rs
fn main() {
    println!("Hello world!");
}
```
````

JP may also prepend metadata (filename, line range) or adapt the format to model
preferences and conversation configuration.

## Drawbacks

**`mimeType`-to-language mapping.** Mapping MIME types to code-fence language
tags is a solved problem with existing libraries, but it adds a dependency and
maintenance surface.

**`ToolCallResponse` format change.** Supporting stored conversations written in
the legacy flat format requires keeping a custom deserializer.

**Scanning for `question` blocks.** JP must scan the `content` array for
`question` blocks to trigger the inquiry flow, which is more complex than
matching a dedicated `NeedsInput` variant.
Returning context alongside questions justifies the cost.

## Alternatives

### Keep `Outcome` and add a `Resources` variant

Add `Outcome::Resources { items: Vec<Resource> }` alongside `Success`.

**Rejected because:** This duplicates MCP's resource model with a JP-specific
one and leaves `execute_mcp` requiring separate handling.

### Add optional metadata fields to `Outcome::Success`

Keep the existing format but let tools annotate output with resource identifiers
and checksums.

**Rejected because:** Attaching metadata to an opaque string still leaves
formatting to individual tools and cannot decompose multi-resource responses.

### Make local tools full MCP servers

Instead of adopting only the content model, require every local tool to
implement the full MCP protocol (JSON-RPC, `initialize`, `tools/list`,
`tools/call`).

**Rejected because:** A local tool is a simple process that reads CLI arguments,
writes JSON, and exits.
Implementing a full MCP server requires a JSON-RPC stack, lifecycle management,
and an SDK.
The content model provides the value; the transport protocol is unnecessary
overhead for one-shot tools.

### Inquiry as a separate response type

Represent `question` as a top-level response field rather than a content block
type.

**Rejected because:** A tool that has produced partial results and needs input
before continuing must be able to return both in one response (for example,
three completed `resource` blocks followed by a `question` about the fourth).

## Non-Goals

- **Resource deduplication.** This RFD establishes the resource identity
  metadata that deduplication requires, but deduplication logic itself is a
  separate concern.

- **Mandating resource blocks.** Tools may return `text` blocks for any output
  or skip JSON entirely and write plain text to stdout.
  `resource` blocks are optional.

- **Prescribing per-tool migration.** This RFD defines the protocol; whether an
  individual tool returns `resource` or `text` blocks is decided per tool during
  implementation.

- **Stateful tool execution model.** The handle registry,
  `spawn`/`fetch`/`apply`/`abort` actions, and one-shot wrapping are separate
  concerns.
  This RFD defines only the content format that stateful tools use in their
  responses.

- **Image and audio blocks.** MCP defines `image` and `audio` content block
  types, which JP does not use today.
  The content block enum can be extended with them later.

## Risks and Open Questions

### `mimeType` inference

File-reading tools can infer `mimeType` from the file extension, and HTTP tools
can read the `Content-Type` header.
When a tool has no obvious `mimeType`, omitting the field falls back to a plain
code fence.

### URI canonicalization

Cross-tool and cross-attachment matching requires canonical file URIs so that
`file:///project/./src/../src/main.rs` and `file:///project/src/main.rs` resolve
to the same identity.
JP canonicalizes URIs on receipt by resolving `.` and `..`, normalizing trailing
slashes, and resolving paths relative to the project root.

### Content block ordering

JP preserves the block order from the tool's response, since a tool returning
mixed `text` and `resource` blocks may rely on a specific sequence (such as an
explanation followed by a resource).

### Malformed content blocks

If a local tool returns JSON with a `content` array containing malformed entries
(such as a `resource` block missing `uri` or an unknown `type` value), JP logs a
warning and skips the malformed block rather than falling back to raw-stdout
parsing.
A response with a valid `content` array is opting into the structured format, so
falling back to raw text would obscure the error.

### Content alongside questions in one-shot tools

Because JP uses only the final invocation's response as the tool call result, a
one-shot tool that returns context blocks alongside a `question` must re-emit
its full output when re-executed with the answer.
This fits stateless processes, which recompute their output on each run, while
still giving the user or LLM context during the inquiry.

### Complex JSON Schema questions in the terminal

Direct terminal prompts cover simple schemas (`boolean`, `string`, `enum`),
while complex schemas fall back to opening the user's editor with a JSON
template.
The editor fallback is awkward for multi-field forms, and a richer terminal form
renderer may be worth adding later.

### Interaction with conversation compaction

When conversation compaction ([RFD 036]) drops old tool response content, it
should retain resource metadata (URIs and `mimeType`s) so resource-level
reasoning remains possible across compacted turns.

## Implementation Plan

### Phase 1: Types and internal representation

Define `ContentBlock`, `ResourceContent`, `InputRequest`, and `Annotations` in
`jp_tool`.
Update `ToolCallResponse` in `jp_conversation` to carry `Vec<ContentBlock>` with
backward-compatible deserialization.
Update `ExecutionOutcome` in `jp_llm` to carry `Vec<ContentBlock>` instead of
`Result<String, String>`.

This phase requires no tool-side changes and can be merged independently.

### Phase 2: MCP tool passthrough

Update `execute_mcp` to preserve typed content blocks from MCP tools instead of
collapsing them to strings, converting `rmcp` types to `ContentBlock` at the MCP
boundary.

This phase can be merged independently and immediately preserves metadata from
MCP tools.

### Phase 3: Resource formatting

Implement the `mimeType`-to-language mapping and the formatting layer that wraps
resource content in code fences, including the `formatted` field override.
This replaces the per-tool formatting embedded in individual tools.

This phase can be merged independently and initially applies to MCP tool
responses.

### Phase 4: Local tool parsing and migration

Update `parse_command_output` to parse the content block format, migrate all
tools in `.config/jp/tools/` and builtin tools to return the new format, and
remove `Outcome` from `jp_tool`.

Because JP is the only consumer of `jp_tool`, all tools migrate in a single
coordinated change.

### Phase 5: Question blocks and inquiry migration

Update the inquiry system to detect `question` content blocks and trigger the
input flow.
Implement the JSON Schema-to-terminal prompt mapping (`boolean`, `select`,
`text`) with the editor fallback for complex schemas, and migrate existing
`NeedsInput` tools to `question` blocks.

This phase depends on Phase 4.

### Phase 6 (deferred): Stateful response envelope

Parse `_meta."computer.jp/status"` and integrate it with the handle registry and
lifecycle management for long-running tools, bridging this RFD's content format
with the stateful execution model.

This phase depends on Phase 5 and [RFD 009].
Because `_meta."computer.jp/status"` is optional and defaults to `stopped`, all
one-shot tools work through Phases 1 to 5 without it.

## References

- [MCP Tools Specification (2025-11-25)]: Defines `CallToolResult` content
  blocks.
- [MCP Resources Specification (2025-11-25)]: Defines resource URIs, content
  types, and annotations.
- [MCP Tasks Specification (2025-11-25)]: Defines the task lifecycle model
  relevant to stateful tool support.
- [RFD 009: Stateful Tool Protocol][RFD 009]: Defines the stateful tool
  execution model whose response content format is specified here.
- [RFD 036: Conversation Compaction][RFD 036]: Defines conversation compaction,
  which interacts with retained resource metadata.
- [RFD 065: Typed Resource Model for Attachments][RFD 065]: Defines the
  `Resource` type wrapped by `ContentBlock::Resource` and the MCP-compatible
  superset principle.

[MCP Resources Specification (2025-11-25)]: https://modelcontextprotocol.io/specification/2025-11-25/server/resources
[MCP Tasks Specification (2025-11-25)]: https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/tasks
[MCP Tools Specification (2025-11-25)]: https://modelcontextprotocol.io/specification/2025-11-25/server/tools
[RFD 009]: 009-stateful-tool-protocol.md
[RFD 036]: 036-conversation-compaction.md
[RFD 065]: 065-typed-resource-model-for-attachments.md
[RFD 066]: 066-content-addressable-blob-store.md
[RFD 067]: 067-resource-deduplication-for-token-efficiency.md
[RFD 094]: 094-built-in-tell_user-tool-for-mid-turn-user-addressed-messages.md
