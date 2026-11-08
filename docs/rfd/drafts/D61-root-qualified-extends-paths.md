# RFD D61: Root-Qualified Extends Paths

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-18
- **Summary**: Allows `extends` entries to specify a configuration root
  (`workspace`, `user_global`, or `user_workspace`) for cross-root path
  resolution.

## Summary

This RFD allows an `extends` entry to resolve its path against a named
configuration root (`workspace`, `user_global`, or `user_workspace`), rather
than resolving only relative to the declaring file's directory.

## Motivation

JP resolves its two load-time config path mechanisms against different anchors:

| Mechanism           | Resolved relative to                |
| ------------------- | ----------------------------------- |
| `config_load_paths` | the workspace root                  |
| `extends`           | the directory of the declaring file |

This asymmetry matters when a higher-precedence config root needs to include a
file that lives in the workspace.
For example, consider a second project developed inside a git-ignored directory
of an existing workspace.
The project keeps its config in its own directory, and the user-workspace config
adds that directory to `config_load_paths`:

```toml
# user-workspace config.toml
config_load_paths = ["crates/my-project/.jp/config"]
```

Running `jp q -c my-project` loads `my-project.toml` from that directory, and
the assistant can edit the project's config in place because it sits inside the
workspace.

However, `config_load_paths` only resolves files requested via `--cfg` (or
`-c`), as defined in [RFD 079].
A config fragment that must load on *every* invocation rather than only on `-c
my-project` has to come from an implicitly loaded source, and the only
non-committed implicit source is the user-workspace config.
Without a way to extend the workspace file from the user-workspace config, the
user must inline those settings directly into the user-workspace config, which
is:

- outside the workspace, where the assistant cannot read or edit it;
- outside version control, leaving the settings unbacked;
- separated from the rest of the project's config in the workspace.

Extending the workspace-relative file from the user-workspace config would keep
the settings in the workspace, but `extends` cannot address that file.
Relative paths from `$XDG_DATA_HOME/jp/<workspace_id>/` to a checkout are
neither stable nor predictable, and `ExtendingRelativePath` wraps
`RelativePathBuf`, which rejects absolute paths.

### Existing Workaround

Users can work around this limitation by symlinking the user-workspace config
into the workspace and granting an `external = true` access rule ([RFD 076],
[RFD D43]) so the assistant can edit it.
Because approvals are keyed on `(rule_path, canonical_target)` with no tool
dimension, a single `--mount` seeds an approval that every tool's rules compile
against, and a bare mount expands across all enabled local tools.
Once [RFD D43] Phase 3 lands (the deferred trust-on-first-use prompt), a
hand-authored `external = true` rule prompts once and is remembered without
`--mount`.

Root-qualified `extends` paths remove that indirection: JP can load the
in-workspace config file directly without a symlink, an external grant, an
approval record, or a per-machine artifact.

## Design

### User-facing configuration

`extends` accepts a third entry form: a table with `root`, `path`, and an
optional `strategy`.

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

`path` is always a relative path, and `root` selects the directory it resolves
against.
When `root` is omitted, `path` resolves relative to the declaring file's
directory, leaving existing config files unchanged.

### Type change

`ExtendingRelativePath` gains a `WithRoot` variant alongside `Path` and
`WithStrategy`:

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
Because `ExtendingRelativePath` is `#[serde(untagged)]`, serde distinguishes `{
root, path }` from `{ path, strategy }` by the required `root` field.

`load_config_file_with_extends` in `jp_config::util` resolves `extends` paths.
It takes the set of root anchors and joins each entry's `path` onto its
configured root anchor, or onto the declaring file's directory when no root is
specified.

### Failure behavior

If a configured `root` cannot be resolved for the current invocation, resolution
fails with an error naming both the root and the path rather than skipping the
entry.
For example, `workspace` is absent when JP runs outside a workspace, and
`user-workspace` requires a workspace ID; both are known at resolution time.

Once the root anchor is resolved, a missing target file follows the standard
`extends` failure behavior defined in [RFD 079].

### Cycles and depth

Root-qualified entries use the same `ExtendsStack` cycle and depth checks as
file-relative entries.
Because `ExtendsStack` keys on the resolved absolute path, it also catches
cross-root cycles (such as a workspace file extending a user-workspace file that
extends the workspace file back).

## Drawbacks

**Implicitly loaded configs can pull in arbitrary workspace files.** A
user-workspace config can extend any file in the workspace, including committed
files, rather than only files relative to its own directory.
A workspace file can therefore affect an invocation without appearing in the
workspace's own config chain.

**Merge precedence does not match the target's root.** A root-qualified entry
merges at the declaring file's position in the load order, not at the target
root's position.
Extending a `workspace`-rooted file from the user-workspace config merges that
file *after* the workspace config, even though its path lives in the workspace.
The `strategy` field already decouples merge order from file location for
relative entries, and cross-root `extends` makes this easier to hit.

**New config vocabulary and a fixed root list.** Users must learn three root
names for a niche use case, and `ConfigRoot` hardcodes the current implicit
sources; adding a fifth implicit source later requires extending the enum.

## Alternatives

**Allow absolute paths in `extends`.** Changing `RelativePathBuf` to accept
absolute paths would require a smaller diff and solve the motivating case
directly.
However, absolute paths make config files machine-specific and non-portable,
whereas `RelativePathBuf` keeps configs shareable across machines.
A `root` qualifier keeps paths portable within a role.

**Make `extends` workspace-relative everywhere.** Resolving all `extends` paths
against the workspace root would match `config_load_paths` and simplify the
mental model, but it would break every existing `extends` directive without
benefiting the common case where a fragment sits next to its parent file.

**Add an eager-loading flag to `config_load_paths`.** Marking a search path as
eager would also load an in-workspace fragment unconditionally.
However, this conflates two distinct mechanisms: `config_load_paths` specifies
where to search for named `--cfg` entries, whereas `extends` specifies which
files a config pulls in automatically.
Loading eagerly from a search path would force `--cfg` resolution and implicit
loading to share a list with per-entry semantics.

**Symlink the out-of-workspace file into the workspace.** As described in
Motivation, symlinking the user-workspace config into the workspace and granting
an `external = true` rule ([RFD 076], [RFD D43]) requires no changes to JP and
works as an interim workaround.
Because filesystem tools canonicalize paths and reject targets outside the
workspace root, the symlink requires an approved target (seeded for all tools by
a single `--mount`, or prompted once under [RFD D43] Phase 3).
However, reaching JP's own config file should not require a symlink, an external
grant, and a trust-on-first-use record, and the symlink path obscures which
config file it targets.

## Non-Goals

- **Changing default `extends` resolution.** Entries without `root` continue to
  resolve relative to the declaring file's directory.
- **Extending paths outside known roots.** Arbitrary absolute paths remain
  unsupported.
- **Changing the implicit source order or precedence rules from [RFD 079].**
  This RFD only changes how an `extends` entry anchors its path.
- **Cross-workspace config inheritance.** Inheriting another workspace's
  personas, skills, and tool config is a broader question about workspace
  relationships; this RFD only addresses resolving a file within the current
  workspace's config roots.

## Risks and Open Questions

- **Per-entry `root` vs. a list-level default.** A list-level default such as
  `extends_root = "workspace"` would reduce repetition when several entries
  share a root, but this RFD places `root` on each entry because mixed lists are
  the expected case.
- **Interaction with [RFD D35].** D35 renames `extends` to `loader.extends` and
  introduces `{ root = "workspace", path = ... }` as a *source identifier* for
  `loader.overrides.extends`, whereas this RFD uses the same shape as a
  *locator*.
  Both features should share one type and one `root` enum; whichever lands
  second must adopt the other's naming, or D35 should absorb this design so the
  two do not ship separate vocabularies.
- **Whether to include `user-global`.** Supporting `user-global` completes the
  set of implicit config roots, but has no known use case.
  Shipping only `workspace` and `user-workspace` would keep the surface smaller
  while still solving the motivating case.
- **Diagnostic for cross-root merge precedence.** Extending a `workspace`-rooted
  entry from a higher-precedence root merges that file at the higher-precedence
  position (see Drawbacks).
  We need to decide whether to emit a warning in this case or rely on
  documentation.

## Implementation Plan

**Phase 1: Type and resolution.** Add `ConfigRoot` and the `WithRoot` variant to
`ExtendingRelativePath`, pass the root anchor set into
`load_config_file_with_extends`, and resolve root-qualified entries against it
while keeping file-relative resolution for existing entries.
This phase includes unit tests for the resolver and can merge independently.

**Phase 2: Diagnostics.** Emit clear errors when a configured `root` cannot be
resolved, and update the cycle-detection error message to format root-qualified
entries as written by the user.
Depends on Phase 1.

**Phase 3: Documentation.** Update `docs/configuration.md` and the [RFD 079]
source table, including the merge-precedence note from Drawbacks.
Depends on Phase 1.

Path resolution runs once per config file per invocation and only performs a
path join, so it adds no measurable runtime cost.

## References

- [RFD 079]: Config sources and load order, defining the four implicit sources
  and the existing `extends` semantics.
- [RFD D35]: The `loader` namespace and entry-scoped `extends` overrides, which
  introduce the `{ root, path }` shape.
- [RFD D43]: Tool access to external paths via workspace symlinks, covering the
  `external` rule, the approval store, and the deferred Phase 3 prompt that
  makes the symlink route self-bootstrapping.
- [RFD 035]: Multi-root `config_load_paths` resolution.
- `crates/jp_config/src/types/extending_path.rs`: Defines
  `ExtendingRelativePath`.
- `crates/jp_config/src/util.rs`: Defines `load_config_file_with_extends`.

[RFD 035]: ../035-multi-root-config-load-path-resolution.md
[RFD 076]: ../076-tool-access-grants.md
[RFD 079]: ../079-config-sources-and-load-order.md
[RFD D35]: D35-extends-overrides-and-loader-namespace.md
[RFD D43]: D43-tool-access-to-external-paths-via-workspace-symlinks.md
