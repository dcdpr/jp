# RFD D12: Large Attachment Size Policy

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-02

## Summary

This RFD introduces a configurable size policy for attachments. When the total
resolved attachment content exceeds a configurable threshold, JP applies a
policy: prompt the user for confirmation (with options to attach as-is, truncate,
or cancel), auto-truncate, allow unconditionally, or reject. The policy
composes with non-interactive mode — when no TTY is available, the configured
policy replaces the interactive prompt.

## Motivation

Attachments are resolved lazily. The `Handler::add()` method records a URI; the
actual content is fetched later by `Handler::get()` at query time. For glob
patterns, a single `file://src/**/*.rs` attachment can expand to hundreds of
files. For HTTP attachments, the response body size is unknown until fetched.

Today, the only size guard is a 10 MiB hard limit on individual binary files in
the `file` handler (`MAX_BINARY_SIZE`). Text files have no size check at all. A
user can accidentally attach a large directory tree or a verbose command output
and send hundreds of thousands of tokens to the LLM without any warning.

This wastes tokens and money, can exceed provider context windows (causing
cryptic API errors), and degrades response quality when the context is dominated
by irrelevant content.

Users need a way to:

- Get warned before sending unexpectedly large attachments.
- Truncate oversized content without manually editing the attachment.
- Configure the behavior for scripted/non-interactive use.

## Design

### User Experience

When a user runs `jp query --attach src/` and the resolved content exceeds the
configured threshold, they see:

```
⚠ Attachments total 847 KB (threshold: 512 KB)

  src/**/*.rs  — 127 files, 623 KB
  Cargo.lock   — 224 KB

Attach large content? [y,t,n,?]?
```

- `y` — attach as-is, proceed with the query.
- `t` — truncate each text attachment to `truncate_to` bytes (default: half the
  threshold), then proceed.
- `n` — cancel the query.
- `?` — print help.

Binary attachments are not truncatable. If the total binary content alone
exceeds the threshold and the user picks `t`, only text content is truncated;
binary content is kept as-is. A future iteration could offer to drop specific
binary attachments, but that is out of scope here.

### Configuration

New fields under `conversation.attachment`:

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

| Policy     | TTY behavior            | Non-TTY behavior        |
|------------|-------------------------|-------------------------|
| `ask`      | Interactive prompt      | Auto-approve (attach as-is) |
| `allow`    | No prompt               | No prompt               |
| `truncate` | Auto-truncate silently  | Auto-truncate silently  |
| `reject`   | Error                   | Error                   |

The `ask` policy auto-approves when no TTY is present, matching the existing
convention for permission prompts (see [RFD 049]).

### Where the Check Happens

The check is a single function called in `Query::run()`, between attachment
resolution and thread construction:

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

Content is already in memory at this point. This is deliberate — reliable size
information is only available after fetch. The cost of fetching content that
gets discarded is acceptable: file I/O is local and fast, HTTP attachments are
uncommon and typically small, and command output is bounded by execution time.

### The `apply_attachment_size_policy` Function

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

A `byte_size()` method is added to `Attachment`:

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

Total size sums all attachments. The threshold comparison uses raw byte size,
not token estimates. Byte size is deterministic, fast to compute, and
handler-agnostic. Token estimation would require a tokenizer dependency and
varies by model.

### Truncation

Text attachments are truncated to `truncate_to` bytes, aligned to the nearest
UTF-8 character boundary. A marker is appended:

```
... [truncated, 623 KB → 256 KB]
```

Binary attachments are never truncated. Truncating an image or PDF produces
corrupt data.

When multiple text attachments are present, each is truncated independently to
`truncate_to` bytes. This is simpler than proportional allocation and avoids
penalizing small attachments. The total post-truncation size may still exceed
the threshold if there are many attachments; this is acceptable — the goal is
a reasonable reduction, not a hard cap.

### Prompt Rendering

The warning message groups attachments by source with size annotations. This
gives the user enough context to decide:

```
⚠ Attachments total 847 KB (threshold: 512 KB)

  src/**/*.rs  — 127 files, 623 KB
  Cargo.lock   — 224 KB

Attach large content? [y,t,n,?]?
```

For glob-expanded attachments, the display groups by the original pattern (the
`source` field on `Attachment`). Individual file names within a glob are not
listed — the count and aggregate size are sufficient.

The prompt uses the existing `InlineSelect` component from `jp_inquire`.

## Drawbacks

- **Post-fetch check**: Content is fully loaded into memory before the size
  check runs. For extremely large attachments (e.g. a multi-gigabyte directory
  tree), this means the memory is allocated and then potentially discarded. In
  practice this is unlikely — the `file` handler already skips binary files over
  10 MiB, and text files large enough to cause memory pressure are rare.

- **Byte size ≠ token cost**: The threshold is in bytes, not tokens. A 512 KB
  file is roughly 128K–170K tokens depending on content and tokenizer. Users who
  think in tokens need to mentally convert. This is a deliberate trade-off:
  byte size is handler-agnostic and doesn't require a tokenizer dependency.

- **Per-attachment truncation**: Each text attachment is truncated independently
  to the same limit. If a query has 20 small files and one large file, all get
  the same `truncate_to` budget. Proportional allocation would be fairer but
  adds complexity for marginal benefit.

## Alternatives

### Inline post-fetch check without configuration

A hardcoded threshold and prompt in `Query::run()` with no config surface. This
is the minimal version of the proposal.

Rejected because it doesn't compose with non-interactive mode and gives users
no way to tune the threshold for their workflow. A user attaching large
codebases for code review has different needs than one attaching a single file.

### Two-phase handler protocol (preflight → get)

Add a `preflight()` method to the `Handler` trait that returns estimated content
size before fetching. The CLI prompts between preflight and fetch.

Rejected because:

- It requires a breaking change to the `Handler` trait (all five handler
  implementations need updating).
- Several handlers cannot estimate size (`cmd` output is unpredictable, MCP
  resources have no size metadata).
- Glob expansion in the `file` handler would need to run twice (once for
  estimation, once for content).
- The marginal benefit (avoiding fetch of discarded content) doesn't justify the
  complexity. File I/O is fast and HTTP attachments are uncommon.

### Token-based threshold

Use estimated token count instead of byte size. This would align the threshold
with the resource that actually matters (context window tokens).

Rejected because token estimation requires a tokenizer dependency, varies by
model, and is slow for large content. Byte size is a reasonable proxy — the
relationship between bytes and tokens is roughly linear for text content.

## Non-Goals

- **Per-attachment policies**: This RFD applies a single policy to the aggregate
  size of all attachments. Per-attachment thresholds or per-handler policies are
  future work.

- **Smart truncation**: Truncation is a simple byte cut. Content-aware
  truncation (e.g. keeping function signatures, removing method bodies) is out
  of scope.

- **Token budget management**: Fitting attachments within a model's context
  window is a broader problem that involves the system prompt, conversation
  history, and tool definitions. This RFD addresses only the "surprisingly
  large attachment" case.

- **Binary attachment truncation or dropping**: This RFD does not offer to drop
  or resize binary attachments. The prompt only truncates text content.

## Risks and Open Questions

- **Default threshold value**: 512 KB is a starting guess. It should be
  validated against real usage. Too low and users get prompted constantly; too
  high and the guard is ineffective. The value is configurable, so a wrong
  default is recoverable.

- **Glob grouping in the prompt**: The prompt groups files by `source`. For
  glob patterns, all expanded files share the same source pattern. This works
  for `file://src/**/*.rs` but may be confusing if multiple glob patterns
  overlap. Needs validation with real glob-heavy workflows.

- **Interaction with [RFD 065]**: The typed resource model may change how
  attachment content is represented. The size policy should work with whatever
  content type replaces `AttachmentContent`. The `byte_size()` method is simple
  enough to adapt.

- **Interaction with [RFD 015]**: The simplified handler trait removes stateful
  `add`/`remove`/`list`. The size policy is independent of handler state — it
  operates on resolved `Attachment` values — so the two proposals are
  compatible.

## Implementation Plan

### Phase 1: `Attachment::byte_size()` and config types

Add the `byte_size()` method to `Attachment` in `jp_attachment`. Add
`AttachmentSizeConfig` (threshold, policy, truncate_to) and `SizePolicy` enum
to `jp_config::conversation::attachment`. Wire the new config fields into
`PartialConversationConfig`.

Mergeable independently. No behavioral change.

### Phase 2: Size policy function and truncation

Implement `apply_attachment_size_policy()` and `truncate_attachments()` in a new
module `jp_cli::cmd::attachment::size_policy`. Unit tests for each policy mode
and for truncation (UTF-8 boundary handling, binary passthrough, marker text).

Mergeable independently. The function exists but is not called yet.

### Phase 3: Wire into `Query::run()` and add prompt

Call `apply_attachment_size_policy()` in `Query::run()` between attachment
resolution and `build_thread()`. Add the `InlineSelect` prompt for the `ask`
policy. Add integration-level tests with `MockPromptBackend`.

Depends on phases 1 and 2.

## References

- [RFD 015: Simplified Attachment Handler Trait][RFD 015]
- [RFD 049: Non-Interactive Mode and Detached Prompt Policy][RFD 049]
- [RFD 065: Typed Resource Model for Attachments][RFD 065]
- `MAX_BINARY_SIZE` in `jp_attachment_file_content` — existing 10 MiB hard limit
  on binary file attachments

[RFD 015]: 015-simplified-attachment-handler-trait.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 065]: 065-typed-resource-model-for-attachments.md
