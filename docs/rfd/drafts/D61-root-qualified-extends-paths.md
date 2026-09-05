# RFD D61: Root-Qualified Extends Paths

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-18

## Summary

This RFD lets an `extends` entry name the config root its path is resolved
against:

```toml
extends = [{ root = "workspace", path = "crates/my-project/.jp/config.toml" }]
```

Today every `extends` path resolves relative to the directory of the declaring
file, so a config file in one root cannot reliably reach a file in another.

## Motivation

JP resolves the two load-time path mechanisms against different anchors:

| Mechanism           | Resolved relative to                |
| ------------------- | ----------------------------------- |
| `config_load_paths` | the workspace root                  |
| `extends`           | the directory of the declaring file |

That asymmetry is invisible until a user wants a higher-precedence root to pull
in a file that lives in the workspace.
The concrete case: a second project is developed inside another workspace, in a
directory excluded from the repository.
Its workspace config lives with it, and the user-workspace config makes it
reachable:

```toml
# user-workspace config.toml
config_load_paths = ["crates/my-project/.jp/config"]
```

`jp q -c my-project` then finds `my-project.toml` there, and the project's
config can be edited in place by the assistant, because it sits inside the
workspace.

What that cannot express is a fragment that must load on *every* invocation
rather than on `-c my-project`.
Deferred loading only happens when `--cfg` names an entry ([RFD 079]), so an
always-on fragment has to come from an implicitly loaded source, and the only
non-committed one is the user-workspace config.
The user therefore has to inline those settings into the user-workspace config
itself, which is:

- outside the workspace, so the assistant cannot read or edit it;
- outside version control of any kind, so the settings are unbacked;
- separated from the rest of the project's config, which lives together in the
  workspace.

The natural expression is for the user-workspace config to extend the
workspace-relative file, but `extends` cannot address it.
Relative paths from `$XDG_DATA_HOME/jp/<workspace_id>/` to a checkout are
neither stable nor knowable, and `ExtendingRelativePath` is typed as
`RelativePathBuf`, so an absolute path is not expressible either.

### This is an ergonomics gap, not a capability gap

The workspace file *can* be reached today, by symlinking it into the workspace
and granting an `external = true` access rule ([RFD 076], [RFD D43]).
Approvals are keyed on `(rule_path, canonical_target)` with no tool dimension,
so a single `--mount` seeds an approval that every tool's rules then compile
against, and a bare mount expands over the whole enabled-local tool set.
Once [RFD D43] Phase 3 lands (the deferred trust-on-first-use prompt), even that
bootstrap disappears: a hand-authored `external = true` rule prompts once and is
remembered.

So this RFD unlocks nothing.
It removes indirection: no symlink, no external grant, no approval record, and
no per-machine artifact, for what is only JP reading its own config file.
The cost of doing nothing is a config layout harder to read and explain than the
thing it expresses, not one that is impossible.

## Design

### User-facing configuration

`extends` accepts a third entry form: a table with `root` and `path`.

```toml
extends = [
    # Unchanged: relative to this file's directory.
    "fragments/tools.toml",

    # Unchanged: relative, with an explicit merge strategy.
    { path = "fragments/model.toml", strategy = "after" },

    # New: resolved against a named config root.
    { root = "workspace", path = "crates/my-project/.jp/config.toml" },
    { root = "workspace", path = "crates/my-project/.jp/config.toml", strategy = "after" },
]
```

`root` accepts the implicitly loaded sources named in [RFD 079]:

| `root`           | Anchor                                            |
| ---------------- | ------------------------------------------------- |
| `workspace`      | the workspace root (the directory holding `.jp/`) |
| `user-workspace` | `$XDG_DATA_HOME/jp/<workspace_id>/`               |
| `user-global`    | the platform user config dir for `jp`             |

`path` stays a relative path; `root` decides what it is relative to.
Omitting `root` preserves today's behavior exactly, so every existing config
file is unaffected.

### Type change

`ExtendingRelativePath` gains a variant alongside `Path` and `WithStrategy`:

```rust
pub enum ExtendingRelativePath {
    Path(RelativePathBuf),
    WithStrategy(RelativePathWithStrategy),
    WithRoot(RelativePathWithRoot),
}

pub struct RelativePathWithRoot {
    pub root: ConfigRoot,
    pub path: RelativePathBuf,
    pub strategy: ExtendingStrategy,
}
```

`ConfigRoot` is a `ConfigEnum` over the three roots above.
The enum is `#[serde(untagged)]` already, so `{ root, path }` is distinguished
from `{ path, strategy }` by the presence of `root`.

Resolution moves from "join onto the declaring file's directory" to "join onto
the anchor named by the entry, defaulting to the declaring file's directory".
`load_config_file_with_extends` in `jp_config::util` is the single place that
resolves these paths; it needs the anchor set passed in.

### Failure behavior

A `root` that cannot be resolved for the current invocation is an error naming
the root and the path, not a silent skip.
The workspace root is absent when JP runs outside a workspace, and
`user-workspace` requires a workspace id; both are knowable at resolution time.

A missing file at a resolved root-qualified path follows the existing `extends`
failure behavior ([RFD 079]) rather than inventing a second rule.

### Cycles and depth

Root-qualified entries join the same `ExtendsStack` used for relative entries,
so the existing cycle and depth checks cover them.
Cycles across roots are now expressible (a workspace file extending a
user-workspace file that extends it back) and are caught by the same mechanism,
because detection keys on the resolved absolute path.

## Drawbacks

**It widens the reach of implicitly loaded config.** Today a user-workspace
config can only pull in files near itself.
After this change it can pull in arbitrary workspace files, including committed
ones.
That is the point, but it means a workspace file can become load-bearing for an
invocation without appearing in the workspace's own config chain.

**Precedence becomes harder to read.** A root-qualified entry is merged at the
declaring file's position in the order, not its target's.
A `workspace`-rooted file extended from the user-workspace config is therefore
merged *after* the real workspace config, which is the opposite of where its
path suggests it sits.
The `strategy` field already exposes this hazard for relative entries; this
change makes the surprise more available.

**Three roots is a vocabulary the user has to learn** for a feature most users
will never need.
It also hardcodes the current source list: adding a fifth implicit source later
means extending the enum.

## Alternatives

**Absolute paths in `extends`.** Changing `RelativePathBuf` to accept absolute
paths is a smaller diff, and would solve the motivating case immediately.
It was rejected because it makes config files machine-specific and non-portable,
and because the relative-path type is a deliberate constraint that keeps config
shareable.
A `root` qualifier keeps paths portable within a role.

**Make `extends` workspace-relative everywhere.** Consistent with
`config_load_paths`, and simpler to explain.
Rejected as a breaking change to every existing `extends` directive, for no gain
in the common case where a fragment sits next to its parent.

**A `config_load_paths` entry that always loads.** An "eager" flag on a search
path would also make an in-workspace fragment load unconditionally.
Rejected because it conflates two mechanisms: search paths answer "where do I
look for named entries", `extends` answers "what else does this file pull in".
Loading eagerly from a search path would make `--cfg` resolution and implicit
loading share a list with per-entry semantics.

**Symlink the out-of-workspace file into the workspace.** Requires no JP change,
and is the recommended interim answer.
The fs tools canonicalize paths and reject targets outside the workspace root,
so the symlink needs an `external = true` rule with an approved target ([RFD
076], [RFD D43]).
The approval is tool-agnostic, so one `--mount` covers every tool, and [RFD D43]
Phase 3 would remove that step as well.
Not adopted as the permanent shape because it spends a symlink, an external
grant and a trust-on-first-use record on reaching JP's own config file, and
because the resulting mount path says nothing about what it points at.

## Non-Goals

- **Not** changing the default resolution of existing `extends` entries.
- **Not** introducing a way to extend a path outside all known roots.
  Arbitrary absolute paths stay unexpressible.
- **Not** changing the implicit source order or precedence rules from [RFD 079].
  This RFD only changes how a path inside an `extends` entry is anchored.
- **Not** addressing cross-workspace config inheritance (a second workspace
  inheriting a first workspace's personas, skills and tool config).
  That is a larger question about workspace relationships; this RFD is only
  about addressing a file.

## Risks and Open Questions

- **Does `root` belong on the entry or the list?** A per-list default
  (`extends_root = "workspace"`) would cut repetition when several entries share
  a root.
  Per-entry is proposed because mixed lists are the expected case.
- **Interaction with [RFD D35].** D35 renames `extends` to `loader.extends` and
  introduces `{ root = "workspace", path = ... }` as a *source identifier* for
  `loader.overrides.extends`.
  This RFD uses the same shape as a *locator*.
  The two should share one type and one `root` enum.
  Whichever lands second must adopt the other's naming; ideally D35 absorbs this
  design rather than the two shipping separate vocabularies.
- **Is `user-global` worth including?** It completes the set, but there is no
  known use for it.
  Shipping only `workspace` and `user-workspace` would be narrower and still
  solve the motivating case.
- **Precedence surprise.** Whether the merge position described under Drawbacks
  needs a diagnostic (a warning when a `workspace`-rooted entry is extended from
  a higher-precedence root) or just documentation.

## Implementation Plan

**Phase 1: type and resolution.** Add `ConfigRoot` and the `WithRoot` variant to
`ExtendingRelativePath`; thread an anchor set into
`load_config_file_with_extends`; resolve root-qualified entries against it.
Existing entries keep the current anchor.
Independently mergeable, with unit tests over the resolver.

**Phase 2: diagnostics.** Errors for an unresolvable root, and the cycle message
updated to render root-qualified entries in the form the user wrote.
Depends on Phase 1.

**Phase 3: documentation.** `docs/configuration.md` and the [RFD 079] source
table, including the precedence note from Drawbacks.
Depends on Phase 1.

No measurable cost implications: resolution happens once per config file per
invocation, and the work is a path join.

## References

- [RFD 079] — config sources and load order; the four implicit sources and the
  existing `extends` semantics.
- [RFD D35] — `loader` namespace and entry-scoped extends overrides; origin of
  the `{ root, path }` shape.
- [RFD D43] — tool access to external paths via workspace symlinks; the
  `external` rule, the approval store, and the deferred Phase 3 prompt that
  makes the symlink route self-bootstrapping.
- [RFD 035] — multi-root config load path resolution.
- `crates/jp_config/src/types/extending_path.rs` — `ExtendingRelativePath`.
- `crates/jp_config/src/util.rs` — `load_config_file_with_extends`.

[RFD 035]: ../035-multi-root-config-load-path-resolution.md
[RFD 076]: ../076-tool-access-grants.md
[RFD 079]: ../079-config-sources-and-load-order.md
[RFD D35]: D35-loader-namespace-and-extends-overrides.md
[RFD D43]: D43-tool-access-to-external-paths-via-workspace-symlinks.md
