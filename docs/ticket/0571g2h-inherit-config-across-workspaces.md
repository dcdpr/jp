# Inherit config across workspaces

- **Status**: Todo
- **Kind**: Feature
- **Authors**: jp
- **Date**: 2026-08-19

A workspace cannot reuse another workspace's configuration.
This ticket records the problem and a rough direction; the design is worth
arguing properly, so it should be promoted to an RFD before implementation.

## Problem

The moment a directory gains its own `.jp/`, `find_root` stops there and it is a
standalone workspace.
It inherits nothing from the workspace it sits inside, or from any other
workspace.

So a second workspace needs its own copies of everything worth sharing: tool
configuration, personas, skills, knowledge sections, model definitions, render
style.
Copies drift, and the drift is silent — the second workspace simply behaves
like an older version of the first.

This is separate from [RFD 035] (multi-root *load path* resolution, which is
about where `--cfg` looks for named entries within one workspace) and from [RFD
D38] (which is about anchoring a single `extends` path at a named config root).
Neither addresses one workspace inheriting another's config as a whole.

## Rough direction

An `extends`-like mechanism that crosses the workspace boundary, so a
workspace's `.jp/config.toml` can declare "start from that workspace's config,
then override".
Open questions that make this RFD-shaped rather than ticket-shaped:

- What is the unit of inheritance?
  Whole config, or named entries only?
- How is the parent identified — path, workspace id, or something stable across
  machines?
- What happens when the parent is unavailable (moved, deleted, different
  machine)?
  Hard error, or degrade with a warning?
- Does inheritance compose transitively, and how are cycles handled?
- How does this interact with the four implicit sources and their precedence
  ([RFD 079])?

## Related

- [RFD D38] — root-qualified extends paths.
  Solves the narrow case of addressing one file in another root; this ticket is
  the general case.
- [RFD 046] — nested workspace projection.

## Next step

Promote to an RFD.
The problem is real and recurring, but the answer is a design argument, not an
implementation.
