# RFD D12: Large Attachment Size Policy

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-02
- **Summary**: Adds a configurable attachment size threshold and policy for
  prompting, truncating, allowing, or rejecting oversized attachments.

## Summary

This RFD introduces a size policy for attachments whose total resolved content
exceeds a byte threshold.
Depending on `conversation.attachment.size_policy`, JP prompts the user to
attach as-is, truncate, or cancel (`ask`), truncates text attachments
automatically (`truncate`), attaches without prompting (`allow`), or aborts with
an error (`reject`).
In non-interactive runs, `ask` attaches the content as-is.

## Motivation

Attachments are resolved lazily: `Handler::add()` records a URI, and
`Handler::get()` fetches the content at query time.
A single `file://src/**/*.rs` glob can expand to hundreds of files, and an HTTP
attachment's response size is unknown until fetched.

The only existing size guard is `MAX_BINARY_SIZE`, a 10 MiB hard limit on
individual binary files in the `file` handler.
Text attachments have no size limit.
A user can attach a large directory tree or verbose command output and send
hundreds of thousands of tokens to the LLM without warning.

Oversized attachments waste tokens and money, exceed provider context windows
with opaque API errors, and dilute the prompt with irrelevant content.

Users need a way to:

- Catch unexpectedly large attachments before sending a query.
- Truncate oversized text attachments in place.
- Configure the behavior for scripted and non-interactive runs.

## Design

### User Experience

When a user runs `jp query --attach src/` and the resolved content exceeds the
configured threshold, JP prompts before building the thread:

```
⚠ Attachments total 847 KB (threshold: 512 KB)

  src/**/*.rs  — 127 files, 623 KB
  Cargo.lock   — 224 KB

Attach large content? [y,t,n,?]?
```

- `y`: Attach as-is and proceed with the query.
- `t`: Truncate each text attachment to `truncate_to` bytes (default: half the
  threshold), then proceed.
- `n`: Cancel the query.
- `?`: Print help.

Binary attachments cannot be truncated.
Selecting `t` truncates only text attachments and leaves binary attachments
unchanged, even if binary content alone exceeds the threshold.
Dropping individual binary attachments is out of scope.

### Configuration

Three fields under `conversation.attachment` configure the policy:

```toml
[conversation.attachment]
# Total byte size across all resolved attachments that triggers the policy.
# Accepts human-readable sizes: "512KB", "1MB", "2MB".
# Default: "512KB".
size_threshold = "512KB"

# What to do when the threshold is exceeded.
# Values: "ask", "allow", "truncate", "reject".
# Default: "ask".
size_policy = "ask"

# Target size per text attachment when truncating.
# Accepts human-readable sizes. Default: half of size_threshold.
# Only meaningful when size_policy is "ask" or "truncate".
truncate_to = "256KB"
```

| Policy     | TTY behavior           | Non-TTY behavior            |
| ---------- | ---------------------- | --------------------------- |
| `ask`      | Interactive prompt     | Auto-approve (attach as-is) |
| `allow`    | No prompt              | No prompt                   |
| `truncate` | Auto-truncate silently | Auto-truncate silently      |
| `reject`   | Error                  | Error                       |

When no TTY is present, `ask` attaches the content as-is, matching the detached
prompt convention in [RFD 049].

### Policy Enforcement

`Query::run()` calls `apply_attachment_size_policy` after resolving attachment
futures and before building the thread:

```rust
// Resolve all attachment content (existing code).
let attachments: Vec<_> = futures::future::try_join_all(attachment_futs)
    .await?
    .into_iter()
    .flatten()
    .collect();

// NEW: apply the size policy.
let attachments = apply_attachment_size_policy(
    attachments,
    &cfg.conversation.attachment,
    ctx.term.is_tty,
    &ctx.printer,
)?;

// Build the thread (existing code).
let thread = build_thread(stream, attachments, &cfg.assistant, !tools.is_empty())?;
```

Running the check after resolution means all attachment content is already in
memory.
Reliable size information is only available after fetching, and the cost of
fetching content that the user later discards is low: file I/O is local, HTTP
attachments are uncommon and typically small, and command output is bounded by
command execution time.

`apply_attachment_size_policy` compares the aggregate byte size against
`config.size_threshold` and dispatches on `config.size_policy`:

```rust
fn apply_attachment_size_policy(
    attachments: Vec<Attachment>,
    config: &AttachmentSizeConfig,
    is_tty: bool,
    printer: &Printer,
) -> Result<Vec<Attachment>> {
    let total = attachment_byte_size(&attachments);
    if total <= config.size_threshold {
        return Ok(attachments);
    }

    match config.size_policy {
        SizePolicy::Allow => Ok(attachments),
        SizePolicy::Reject => Err(Error::AttachmentTooLarge { total, threshold: config.size_threshold }),
        SizePolicy::Truncate => Ok(truncate_attachments(attachments, config.truncate_to)),
        SizePolicy::Ask if !is_tty => Ok(attachments),
        SizePolicy::Ask => prompt_attachment_size(attachments, config, total, printer),
    }
}
```

### Size Calculation

`Attachment::byte_size()` returns the raw byte length of the resolved content:

```rust
impl Attachment {
    pub fn byte_size(&self) -> usize {
        match &self.content {
            AttachmentContent::Text(s) => s.len(),
            AttachmentContent::Binary { data, .. } => data.len(),
        }
    }
}
```

`attachment_byte_size` sums `byte_size()` across all resolved attachments.
The threshold compares raw byte counts rather than estimated token counts: byte
size is deterministic, fast to compute, and handler-agnostic, whereas token
estimation varies by model and requires a tokenizer dependency.

### Truncation

Each text attachment is truncated to `truncate_to` bytes at the nearest UTF-8
character boundary, with a size marker appended:

```
... [truncated, 623 KB → 256 KB]
```

Binary attachments are never truncated, since truncating an image or PDF
corrupts the file.

Each text attachment is truncated independently to `truncate_to` bytes rather
than proportionally across attachments.
Independent truncation is simpler and avoids shrinking small attachments.
When a query has many attachments, their combined size after truncation can
still exceed `size_threshold`; `truncate_to` is a per-attachment cap, not a hard
cap on total size.

### Prompt Rendering

The prompt uses the `InlineSelect` component from `jp_inquire` and groups
attachments by their `Attachment.source` field with size annotations:

```
⚠ Attachments total 847 KB (threshold: 512 KB)

  src/**/*.rs  — 127 files, 623 KB
  Cargo.lock   — 224 KB

Attach large content? [y,t,n,?]?
```

Glob-expanded attachments are grouped under their original pattern showing only
the file count and combined size, rather than listing every matched file.

## Drawbacks

- **Post-fetch check**: All attachment content is loaded into memory before the
  size check runs, so attaching a multi-gigabyte directory tree allocates that
  memory even if the user cancels.
  In practice, the `file` handler already skips binary files over 10 MiB, and
  text files large enough to cause memory pressure are rare.

- **Byte size vs. token cost**: The threshold is measured in bytes rather than
  tokens.
  A 512 KB text file is roughly 128K to 170K tokens depending on the content and
  tokenizer, so users who think in tokens must convert mentally.

- **Per-attachment truncation**: Every text attachment is capped at the same
  `truncate_to` limit.
  In a query with twenty small files and one large file, each file gets the same
  `truncate_to` budget; proportional allocation would distribute the budget more
  evenly at the cost of extra complexity.

## Alternatives

### Hardcoded threshold without configuration

`Query::run()` could enforce a fixed byte threshold and prompt without exposing
configuration fields.
This is rejected because it cannot be customized for non-interactive runs or
different workflows; a user attaching a codebase for review needs a different
limit than one attaching a single file.

### Two-phase handler protocol (`preflight` then `get`)

The `Handler` trait could add a `preflight()` method that estimates content size
before `get()` fetches it, allowing the CLI to prompt between the two phases.

This is rejected because:

- It is a breaking change to `Handler` across all five handler implementations.
- Several handlers cannot estimate size ahead of time (`cmd` output is unknown
  until the command finishes, and MCP resources carry no size metadata).
- The `file` handler would have to expand globs twice: once to estimate size and
  once to read content.
- Avoiding fetches for canceled attachments does not justify the added protocol
  complexity, since local file I/O is fast and HTTP attachments are uncommon.

### Token-based threshold

Measuring the threshold in estimated tokens would match the model's context
window directly.
This is rejected because token estimation requires a tokenizer dependency,
varies across models, and is slow on large inputs.
Byte size is a sufficient proxy because byte and token counts scale roughly
linearly for text.

## Non-Goals

- **Per-attachment or per-handler policies**: A single policy applies to the
  aggregate size of all resolved attachments.

- **Content-aware truncation**: Truncation takes a byte prefix aligned to a
  UTF-8 boundary.
  Syntax-aware reduction, such as keeping function signatures while stripping
  method bodies, is out of scope.

- **Token budget management**: Fitting a request into a model's context window
  also depends on the system prompt, conversation history, and tool definitions.
  This RFD only guards against unexpectedly large attachments.

- **Dropping or resizing binary attachments**: The policy only truncates text
  attachments; it does not resize or drop binary attachments.

## Risks and Open Questions

- **Default threshold value**: The 512 KB default is an initial estimate that
  needs validation against real usage.
  A threshold that is too low prompts on routine queries; one that is too high
  misses oversized attachments.
  Users can override `size_threshold` in configuration if the default does not
  fit their workflow.

- **Glob grouping in the prompt**: Grouping attachments by `source` combines all
  files expanded from a glob like `file://src/**/*.rs` under one line, which can
  be confusing when multiple glob patterns overlap.
  This grouping needs testing with glob-heavy workflows.

- **Interaction with [RFD 065]**: If the typed resource model replaces
  `AttachmentContent`, `Attachment::byte_size()` will need to be updated to read
  byte lengths from the new representation.

- **Interaction with [RFD 015]**: [RFD 015] removes stateful
  `add`/`remove`/`list` methods from `Handler`.
  Because the size policy operates on resolved `Attachment` values rather than
  handler state, the two designs do not conflict.

## Implementation Plan

### Phase 1: `Attachment::byte_size()` and config types

Add `Attachment::byte_size()` in `jp_attachment`.
Add `AttachmentSizeConfig` (`size_threshold`, `size_policy`, `truncate_to`) and
the `SizePolicy` enum to `jp_config::conversation::attachment`, and wire the
fields into `PartialConversationConfig`.
This phase can merge independently with no behavioral change.

### Phase 2: Size policy function and truncation

Implement `apply_attachment_size_policy()` and `truncate_attachments()` in
`jp_cli::cmd::attachment::size_policy`, with unit tests covering each policy
mode, UTF-8 boundary alignment, binary passthrough, and the truncation marker.
This phase can merge independently before the function is wired into the CLI.

### Phase 3: Wire into `Query::run()` and add prompt

Call `apply_attachment_size_policy()` in `Query::run()` between attachment
resolution and `build_thread()`, add the `InlineSelect` prompt for `ask`, and
add integration tests using `MockPromptBackend`.
This phase depends on Phases 1 and 2.

## References

- [RFD 015: Simplified Attachment Handler Trait][RFD 015]
- [RFD 049: Non-Interactive Mode and Detached Prompt Policy][RFD 049]
- [RFD 065: Typed Resource Model for Attachments][RFD 065]
- `MAX_BINARY_SIZE` in `jp_attachment_file_content`: 10 MiB hard limit on binary
  file attachments

[RFD 015]: ../015-simplified-attachment-handler-trait.md
[RFD 049]: ../049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 065]: ../065-typed-resource-model-for-attachments.md
