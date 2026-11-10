# RFD D47: Divergent Display View for Tool Results

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-03
- **Summary**: Allows local tools to produce a user-facing display view of a
  tool result distinct from the assistant-facing payload.

## Summary

This RFD lets a local tool produce a **display view** of its result (rendered to
the user) distinct from the **assistant view** (recorded in the conversation and
sent to the model).
JP generates the display view once at turn time via a new tool `Action` and
stores it in tool-call event metadata.
Divergence is opt-in; by default, the display view matches the assistant view.

## Motivation

A tool returns a single result today.
The user sees a projection of the same bytes the assistant receives: JP can
truncate (`inline_results`), hide (`hidden`), or style the output, but a tool
cannot show the user different content than it sends to the model.

Two use cases need separate views:

- **`fs_read_file` line gutter.** Numbering each line helps the assistant pass
  line ranges directly to line-addressed tools (`fs_read_file`, `git_blame`,
  `git_diff_file`).
  However, terminal syntax highlighting relies on a leading ` ```lang ` fence,
  which a ` 1:  ` gutter breaks.
  The tool needs numbered lines for the assistant and clean, fenced code for the
  terminal.
- **Summary vs. full output.** A tool may want to show the user a one-line
  summary while sending the full payload to the assistant, or the reverse.

Returning a single result forces these tools to optimize for one audience at the
expense of the other.

## Design

### Vocabulary

A tool result has two views:

- **Assistant view**: Recorded in the conversation stream and replayed into the
  model's context.
- **Display view**: Rendered to the user.

By default, the display view equals the assistant view (`display view ==
assistant view`).
Divergence is opt-in.

> These views are orthogonal to the four output channels in [RFD 048], which
> separate JP's process streams (stdout, stderr, `/dev/tty`, and log) rather
> than the audiences of a single tool result.

### The `RewriteResult` action

`jp_tool::Action` currently defines `Run` and `FormatArguments`.
This RFD adds a third variant, `RewriteResult` (working name).
After `Run` completes, JP invokes the tool again during the turn with the
assistant view as input, and the tool returns the display view.
This mirrors custom argument formatting, where
`render::tool::format_args_custom` invokes a local tool with
`Action::FormatArguments` on the render path.

```
Run            ──▶ assistant view ──▶ recorded into the conversation (replayed to the model)
                        │
RewriteResult  ◀────────┘  (turn time, once, opt-in)
               ──▶ display view ──▶ stored in event metadata ──▶ rendered to the user
                                                              └─▶ replayed from storage at print time
```

### Storage and replay

JP computes the display view once at turn time and stores it in the tool-call
event metadata, matching how the turn loop drains custom-formatted arguments
into `rendered_arguments`.
`jp conversation print` reads the stored view directly without re-invoking the
tool, keeping replay synchronous.
JP already persists large rendered outputs in metadata (for example, the
`fs_modify_file` argument formatter stores full diffs of hundreds of lines) and
replays them from disk.

JP persists the display view only when it differs from the assistant view.
Identical views store nothing extra, so only opted-in tools that produce
divergent output incur the storage cost.

### Configuration

Tools opt into divergence via a boolean field on the `conversation::tool`
configuration (supporting the `*` wildcard), which defaults to off.
The setting belongs on `conversation::tool` rather than `access` because it
controls representation, whereas `access` scopes `fs`, `env`, and `net`
capabilities.

Enabling this setting also declares that the tool implements `RewriteResult`.
JP sends `RewriteResult` only to opted-in tools, so older or third-party tools
never receive an unrecognized action.
Tools handle the action under the same contract as `FormatArguments`: branch on
the action before performing any I/O.

### Fallback and verification

When no display view is stored (because the tool did not opt in, failed, was
cancelled, or is absent during replay), JP falls back to rendering the assistant
view.
The `result = "ask"` prompt uses the same path, letting users inspect the exact
payload the assistant received.

### Scope

Only local command tools support `RewriteResult`.
MCP tools have no local command for JP to re-invoke, which matches custom
argument formatters (`FormatArguments` also applies only to command tools).

## Drawbacks

- **Terminal output can differ from model input.** Today, the terminal shows a
  faithful (if truncated) view of what the assistant received.
  With tool-authored divergence, the user's screen is no longer guaranteed to
  match the assistant's context, even though divergence is gated and opt-in.
- **A third `Action` variant for tool authors.** Local tool authors must handle
  or ignore one more `Action` variant.
- **Additional metadata storage.** Divergent results store a second payload
  alongside the assistant view.
  Storing the display view only when it differs restricts this cost to opted-in
  tools, matching existing `rendered_arguments` storage.

## Alternatives

- **Re-derive the display view at render time.** Re-computing the view on every
  render would require the tool to still exist and behave deterministically when
  viewing history, and would spawn a subprocess per tool result during `jp
  conversation print`.
  As with `FormatArguments`, computing once and replaying from storage keeps
  output reproducible and replay synchronous.
- **JP-side projection only.** A built-in projection cannot generate
  content-level summaries or strip tool-specific line gutters generically.
- **Configure divergence under `access`.** `access` grants capability scope
  (`fs`, `env`, `net`), whereas result rewriting controls output representation
  (see [RFD 076]).

## Non-Goals

- **Per-frontend rendering.** Event metadata stores a single display view rather
  than distinct renderings for web, native, and TUI frontends.
  If future frontends need separate renderings, JP can migrate the stored
  metadata or re-derive views then.
- **Caching beyond the stored view.** Event metadata is the only cache.
- **MCP tool divergence.** `RewriteResult` applies only to local command tools.

## Risks and Open Questions

- **Prompt-injection blind spot.** A malicious or compromised tool could display
  benign output to the user while sending a different payload to the assistant.
  Three constraints limit this risk: `RewriteResult` is restricted to local
  tools, defaults to off, and can be bypassed with `result = "ask"` to inspect
  the assistant view.
  Because third-party local command plugins are less trusted than built-in
  tools, the opt-in configuration serves as the trust gate; documentation alone
  only warns honest tool authors against confusing divergence (such as breaking
  copy-paste).
- **Action naming.** `RewriteResult` conveys that the content itself may change,
  whereas `FormatResult` parallels `FormatArguments` but implies cosmetic
  formatting.
  The final variant name remains open.
- **Input payload for `RewriteResult`.** Determine whether the rewrite
  invocation receives only the assistant view, the original tool arguments, or
  both.
- **Pipeline reuse.** Verify that `RewriteResult` can reuse `run_tool_command`
  and the existing `RenderOutcome::Rendered { content }` drain into event
  metadata without introducing a parallel execution path.

## Implementation Plan

### Phase 1: Action variant, turn-time invocation, and storage

Add the `RewriteResult` variant to `jp_tool::Action`.
Invoke it once at turn time after `Run` via the custom-formatter pipeline
(`format_args_custom` → `run_tool_command`), and store the resulting display
view in tool-call event metadata alongside `rendered_arguments`.
Update conversation replay to read the stored display view from metadata.

This phase can merge independently and does not change behavior until a tool
opts in.

### Phase 2: Configuration and gating

Add the default-off opt-in field (including `*` wildcard support) to
`conversation::tool`, and gate `RewriteResult` invocation on that setting.

Depends on Phase 1.

### Phase 3: Fallback, verification, and author documentation

Wire `result = "ask"` to render the assistant view directly, and document the
divergence contract and copy-paste caveats in doc comments for tool authors.

Depends on Phases 1 and 2; the two tasks within this phase are independent.

## References

- [RFD 048: Four-Channel Output Model][RFD 048]: JP's process output channels,
  which are orthogonal to tool result views.
- [RFD 036: Conversation Compaction][RFD 036]: Precedent for extending the
  `Action` enum with a new variant.
- [RFD 058: Typed Content Blocks for Tool Responses][RFD 058]: Potential future
  home for an audience tag if the string metadata field proves too narrow.
- [RFD 076: Tool Access Grants][RFD 076]: Defines `access` as capability scope
  rather than output representation.
- `crates/jp_cli/src/render/tool.rs`: `format_args_custom`, `render_approved`,
  and `render_formatted_arguments`.
- `crates/jp_cli/src/cmd/query/tool/coordinator.rs`: Drains `rendered_arguments`
  into event metadata.
- `crates/jp_tool/src/lib.rs`: `Action` enum definition.

[RFD 036]: ../036-conversation-compaction.md
[RFD 048]: ../048-four-channel-output-model.md
[RFD 058]: ../058-typed-content-blocks-for-tool-responses.md
[RFD 076]: ../076-tool-access-grants.md
