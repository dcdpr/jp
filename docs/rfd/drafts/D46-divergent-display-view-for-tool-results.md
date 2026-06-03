# RFD D46: Divergent Display View for Tool Results

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-03

## Summary

This RFD lets a local tool author a separate **display view** of its result —
the bytes shown to the user — distinct from the **assistant view** that is
recorded into the conversation and replayed to the model.
The display view is produced once at turn time by a new tool `Action`, stored in
tool-call event metadata, and replayed from storage.
It is opt-in per tool and defaults to being identical to the assistant view, so
existing behavior is unchanged.

## Motivation

A tool returns a single result today.
The user sees a *projection* of the same bytes the assistant sees: JP can
truncate it (`inline_results`), hide it (`hidden`), or style it, but the human
and the model always look at the same underlying content.
There is no way for a tool to show the user *different* content than it feeds
the assistant.

Two cases want exactly that:

- **`fs_read_file` line gutter.** Numbering each line helps the assistant feed a
  range straight back into the line-addressed tools (`fs_read_file`,
  `git_blame`, `git_diff_file`).
  But the terminal renderer keys syntax highlighting off a leading ` ```lang `
  fence, and a ` 1:  ` gutter defeats that detection, degrading the human's
  view.
  The tool wants numbered output for the assistant and clean, fenced output for
  the terminal.
- **Summary vs. full dump.** A tool may want to show the human a terse one-line
  summary while handing the assistant the full payload, or the reverse.

Doing nothing forces every such tool to pick one audience and degrade the other.

## Design

### Vocabulary

A tool result has two views:

- **Assistant view** — recorded into the conversation stream, replayed into the
  model's context.
- **Display view** — rendered to the user.

The default is `display view == assistant view`.
Divergence is opt-in.

> This is a different axis from the four output channels in [RFD 048], which
> separate JP's *own* streams (stdout / stderr / `/dev/tty` / log).
> This RFD is about a single tool result having two audiences.

### The third action

`jp_tool::Action` today has `Run` and `FormatArguments`.
This RFD adds a third variant (working name `RewriteResult`).
After a tool runs, JP invokes it again at turn time with the assistant view as
input; the tool returns the display view.
This mirrors the existing custom-argument-formatter path, which already invokes
a local tool with `Action::FormatArguments` and awaits it on the render path
(`render::tool::format_args_custom`).

```
Run            ──▶ assistant view ──▶ recorded into the conversation (replayed to the model)
                        │
RewriteResult  ◀────────┘  (turn time, once, opt-in)
               ──▶ display view ──▶ stored in event metadata ──▶ rendered to the user
                                                              └─▶ replayed from storage at print time
```

### Compute once, store, replay from storage

The display view is computed **once at turn time** and stored in the tool-call
event metadata, exactly as the custom argument formatter already stores its
output (`rendered_arguments`, drained by the turn loop into event metadata).
`jp conversation print` reads the stored view and renders it without re-invoking
the tool — the same reason the replay path is synchronous today.

This is the established pattern: JP already persists large rendered output (the
`fs_modify_file` formatter stores a full diff, often hundreds of lines) and
replays it from disk.
Storing a divergent result rendering is the same thing, not a new burden.

**Store only on divergence.** A display view is persisted only when it differs
from the assistant view.
The default case (identical) stores nothing extra; only opted-in, genuinely
divergent tools pay the storage cost.

### Configuration

Divergence is opt-in via a field on the `conversation::tool` config, with a `*`
default, defaulting to off.
It lives there because it concerns *representation*, not capability — `access`
is for `fs`/`env`/`net` scope and is the wrong home.

The opt-in flag does double duty: it is also the declaration that a tool
implements `RewriteResult`.
JP only sends the new action to tools that opted in, so an older or third-party
tool never receives an action it does not recognize.
This reuses the existing "branch on the action before any I/O" contract that
`FormatArguments` already relies on.

### Fallback and the vigilant user

When no display view is stored — the tool did not opt in, errored, was
cancelled, or is absent at replay — JP renders the assistant view.
This is also the `result = "ask"` escape hatch: a user who wants to verify can
always see exactly what the assistant received.
The fallback and the vigilance path are the same mechanism, so the worst case is
always today's behavior.

### Scope

Local tools only.
MCP tools have no command to re-invoke for rewriting, matching how custom
argument formatters already work (they exist only for command tools).

## Drawbacks

- **It breaks an invariant.** Today the terminal is a faithful (if abbreviated)
  view of what the assistant received.
  Tool-authored divergence means the human's screen is no longer guaranteed to
  equal the assistant's input.
  This is deliberate, gated, and opt-in, but it is a real loss.
- **A third action for tool authors.** Every local tool author now has one more
  `Action` variant to be aware of, even if only to ignore it.
- **Storage of a second large blob** for divergent results.
  Mitigated by store-only-on-divergence, and not novel given existing
  rendered-argument storage.

## Alternatives

- **Re-derive the display view at render time (don't store).** Rejected.
  JP already faced this fork for `FormatArguments` and chose store-once,
  replay-from-storage.
  Re-deriving makes historical display depend on the tool still existing and
  behaving deterministically, and forces a tool spawn per result at `jp
  conversation print` time.
  Storing is reproducible and keeps replay synchronous.
- **JP-side projection only (no tool-authored divergence).** Rejected.
  A deterministic JP projection cannot express content-level divergence (a terse
  summary) and cannot strip a tool-specific gutter generically.
- **Put the opt-in under `access`.** Rejected.
  `access` is capability scope; this is representation.
  Forcing representation into a capability-grant model would be the wrong
  abstraction (see [RFD 076]).

## Non-Goals

- **Per-frontend rendering.** Serving distinct renderings to web / native / TUI
  frontends is deferred.
  The stored model bakes in one rendering; when multiple frontends land, the
  stored data is migrated and non-terminal frontends can re-derive at that
  point.
  This is the one case that would argue for re-derivation, and it is not needed
  yet.
- **Caching beyond the stored view.** Out of scope.
- **MCP tool divergence.** Out of scope; the rewrite action is local-only.

## Risks and Open Questions

- **Security: divergence can blind the auditor.** A divergent display lets a
  tool show the user benign content while feeding the assistant something else
  — a prompt-injection blind spot.
  The threat is contained by three properties: local-tools-only, default-off
  opt-in, and the assistant-view escape hatch (`result = "ask"`).
  Third-party local command plugins are still less-trusted code, so the opt-in
  flag is the gate.
  A doc comment warns honest authors against gratuitous divergence (paste-back
  confusion); the gate, not the comment, handles dishonest or compromised tools.
- **Action naming.** `RewriteResult` captures that the content may differ;
  `FormatResult` reads more parallel to `FormatArguments` but understates the
  divergence.
  Naming is open.
- **Context shape for the new action.** What the rewrite invocation receives —
  the assistant view, the original arguments, or both — needs to be pinned
  down.
- **Pipeline reuse.** Confirm the new action reuses `run_tool_command` and the
  `RenderOutcome::Rendered { content }` → drain → event-metadata path verbatim
  rather than growing a parallel one.

## Implementation Plan

### Phase 1: Action variant, turn-time invocation, storage

Add the `RewriteResult` variant to `jp_tool::Action`.
Invoke it once at turn time after `Run`, reusing the custom-formatter machinery
(`format_args_custom` → `run_tool_command`).
Store the display view in tool-call event metadata, mirroring
`rendered_arguments`.
Replay reads it from metadata.

Mergeable independently.
No behavioral change until a tool opts in.

### Phase 2: Configuration and gating

Add the default-off `*` config field on `conversation::tool`.
Gate invocation on it, so the action is only sent to opted-in tools (capability
declaration).

Depends on Phase 1.

### Phase 3: Fallback, vigilance, and the author contract

Wire `result = "ask"` to render the assistant view directly, and document the
divergence contract for tool authors (the doc-comment warning).

Depends on Phases 1-2.
Independent of each other within the phase.

## References

- [RFD 048: Four-Channel Output Model][RFD 048] — JP's process output channels;
  a distinct axis from this RFD.
- [RFD 036: Conversation Compaction][RFD 036] — precedent for extending the
  `Action` enum with a new variant.
- [RFD 058: Typed Content Blocks for Tool Responses][RFD 058] — a possible
  future home for an audience tag if the string-field model proves too narrow.
- [RFD 076: Tool Access Grants][RFD 076] — why the opt-in belongs on tool
  representation config, not `access` scope.
- `crates/jp_cli/src/render/tool.rs` — `format_args_custom`, `render_approved`,
  `render_formatted_arguments`.
- `crates/jp_cli/src/cmd/query/tool/coordinator.rs` — `rendered_arguments`
  drain into event metadata.
- `crates/jp_tool/src/lib.rs` — `Action` enum.

[RFD 036]: ../036-conversation-compaction.md
[RFD 048]: ../048-four-channel-output-model.md
[RFD 058]: ../058-typed-content-blocks-for-tool-responses.md
[RFD 076]: ../076-tool-access-grants.md
