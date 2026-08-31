# Prompt to approve external access rules on first use

- **Status**: Todo
- **Kind**: Feature
- **Authors**: jp
- **Date**: 2026-08-18

Finish the deferred half of the approval lifecycle in RFD D43 Phase 3: a
hand-authored `external = true` access rule should prompt for approval on first
use instead of being dropped.

## Current behaviour

`compile_tool_policy` consults the approval store for every external rule and
maps both outcomes to a refusal:

```rust
ApprovalLookup::Approved => ApprovalDecision::Approved,
ApprovalLookup::Retargeted { .. } | ApprovalLookup::Unknown => ApprovalDecision::Rejected,
```

An unapproved rule is dropped with a warning, and `compile_policy` inserts a
deny-all sentinel so the tool stays default-deny rather than degrading to
unrestricted workspace access.
Nothing is silently granted, so this is safe — just unusable without a
bootstrap step.

The only way to seed an approval today is `--mount`, which creates a symlink and
records the binding as a side effect.
So a config-declared external rule cannot work until the user has run a
`--mount` invocation for that same rule path.

## Why it matters

It inverts the intended authoring model.
The config is meant to declare intent and JP is meant to ask about the risky
part once.
Instead the CLI flag is load-bearing, and the config-only path silently does
nothing (bar a warning).

The concrete case: a second project developed inside a JP workspace keeps its
own JP config in an ignored directory, and wants a rule granting one tool access
to a file outside the workspace.
Declaring it is natural; having to remember an unrelated `--mount` invocation
first is not.

## What D43 already specifies

The design is settled, so this is implementation rather than a new decision:

- Approval is **target-only**.
  The user approves that a workspace-relative rule path may resolve to a
  specific canonical absolute target.
  Capability edits to the same rule (adding write, say) do **not** re-prompt
  while the target is unchanged; those are visible in git diff and `jp config
  show`.
  Silent retargeting is the threat trust-on-first-use exists to catch.
- The prompt is **host-side** and uses the terminal prompting UI, but is
  deliberately **not** recorded as `InquiryRequest` / `InquiryResponse` events
  in the conversation stream: the prompt text contains a canonical host path,
  which must not enter shared conversation state.
  The durable record is the user-local approval store only.
- Retargeting is a distinct case from unknown, and the store already
  distinguishes them (`ApprovalLookup::Retargeted { previous }`), so the prompt
  can say what the binding used to point at.

## Scope

- `crates/jp_cli/src/access/compile.rs` — `compile_tool_policy` needs an
  approver that can prompt, rather than the current pure store lookup.
  Note `compile_fs` already takes `approve: impl FnMut(&str, &Utf8Path) ->
  ApprovalDecision`, so the seam exists.
- `crates/jp_cli/src/access/approvals.rs` — `record` and `save` already exist;
  the prompt path needs to persist on approval.
- Decide the non-interactive behaviour: a prompt is impossible under
  `--no-interactive` or when detached, and the answer should be the current one
  (drop with a warning) rather than blocking or auto-approving.

## Out of scope

The other unimplemented D43 phases: OS-level sandboxing (Phase 4, RFD 075),
Windows junction fallback (Phase 6), `--no-mount` cleanup, and the broad-mount
tool-scope confirmation prompt.

## Note on RFD status

D43 is still a Draft, so `implements` is deliberately unset — that field is for
phases of an accepted RFD's plan, and setting it would put a draft in the In
Progress column.
Phases 1, 2 and 5 of D43 shipped in PR \#727 while it was a draft, so filing
this against the draft matches how the RFD has actually been built out.
