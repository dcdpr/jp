<!--
  This template is a starting point, not a constraint. Delete sections that
  don't apply, add sections that do, or restructure entirely. The only
  requirement is the metadata header (Status, Authors, Date).

  Use HTML comments like this one for draft-time notes and review markers.
  They do not appear in the rendered output and can be removed when the RFD
  advances to Discussion status.
-->

# RFD D43: Tool Access to External Paths via Workspace Symlinks

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-27
- **Extends**: [RFD 076], [RFD 075]

## Summary

This RFD lets the assistant read and modify files outside the workspace via
user-created symlinks.
It extends [RFD 076] in three places: tool call arguments must be
workspace-relative before canonicalisation; `FsRule` gains an `external` scope
field that acknowledges the rule's path is permitted to resolve outside the
workspace; and rule-path canonicalisation preserves a lexical workspace-relative
prefix plus an approved canonical target for these rules instead of rejecting
them during policy compilation.
The canonical target is approved host-side at policy compile time via
trust-on-first-use.
A `--mount` flag on `jp q` creates a symlink and writes the access grant in one
step.

## Motivation

JP's workspace model is workspace-rooted by intent, but current code does not
enforce confinement on local subprocess tools.
`jp_llm::run_tool_command` only sets `current_dir = workspace root`; a
subprocess can still open `/etc/passwd` or `~/.ssh/id_rsa`.
[RFD 076] introduces the cooperative `AccessPolicy` layer and [RFD 075]
introduces OS-level sandboxing — both planned, neither shipped.
This RFD extends those planned layers, not today's confined model.

The friction this RFD addresses shows up regularly.
A common case: working in JP workspace `W` on feature `F`, which depends on a
forked crate at `~/code/forks/X`.
For `F` to work end-to-end the assistant needs to (a) make a fix in
`~/code/forks/X` and push it, and (b) update `W`'s `Cargo.toml` to point at the
fork.
Today (a) is manual — the assistant can guide the user but can't read or edit
the fork.
Running `jp init` inside the fork creates a separate workspace divorced from
`F`'s conversation history, context, and tool configuration.

We want the assistant to reach the fork while continuing the conversation in
`W`, without (i) reintroducing absolute paths the LLM has to reason about, (ii)
breaking the workspace-relative invariant that confines the LLM's expressible
reach, or (iii) widening the access-policy or OS-sandbox layers for every tool
by default.

A workspace symlink solves this.
The user creates `<W>/fork -> ~/code/forks/X` and grants the assistant explicit
access.
The assistant calls `fs_modify_file fork/src/lib.rs`.
The path is workspace-relative; the canonical target is external; the explicit
grant permits the resolution.
Conversation events record `fork/src/lib.rs`, which is portable across machines
— a teammate cloning the workspace either has their own `fork` (works) or
doesn't (clean failure).

## Design

### Field grouping in `FsRule`

[RFD 076] defines an `FsRule` with two structurally distinct kinds of fields:

- **Scope fields** answer *which paths the rule applies to*.
  Today: `path`.
- **Capability fields** answer *what the tool may do within that scope*: `read`,
  `create`, `update`, `delete`, `execute` (plus the `write` alias).

This RFD adds a second scope field, `external`.
The conceptual distinction matters for explaining where the field belongs and
what it does.

### Pre-canonical workspace-relative invariant for tool calls

Tool call arguments must be workspace-relative before canonicalisation.
The check happens in `jp_tool::Context::check_*`:

> Reject the call if the input path is absolute, or if its lexical resolution
> (with `..` segments collapsed) escapes `ctx.root`.

Today this rejection is an implicit consequence of canonicalisation: absolute
paths and `..`-escapes happen to fail the post-canonical workspace check.
Promoting it to an explicit pre-canonical step produces clearer error messages
("absolute paths are not permitted" rather than "path escapes workspace") and
decouples the LLM's expressible reach from the filesystem layout under the
workspace.

`Context::check_*` accepts only the raw workspace-relative path the LLM
supplied.
Tools must not pre-resolve paths against `ctx.root` before calling `check_*`;
the checker does the join + canonicalisation itself.

The invariant scopes to **tool calls only**.
Attachments (`--attach`) operate under [RFD D03]'s permissive model — the user
typing the path is the consent action, and D03's `external:` URI scheme handles
privacy and dedup concerns.
A tool can never reach external content directly: it either operates on a
workspace-relative path (potentially via a symlink, governed by the next
section), or receives an `external:` URI as conversation context that has no
filesystem-path shape from the LLM's perspective.

### `FsRule.external: bool`

A new optional scope field on `FsRule` ([RFD 076]):

```toml
[[conversation.tools.fs_modify_file.access.fs]]
path = "fork"
external = true
read = true
write = true
```

Default `false`.
`external = true` is an *acknowledgement* by the user that the rule's `path` is
permitted to resolve outside the workspace via symlink.
It is not a capability grant — capabilities are still expressed by the
capability fields below it.

To eliminate ambiguity, the policy compiler **rejects `external = true` during
policy compilation if the rule's lexical path canonicalises inside the
workspace**.
The flag only makes sense on rules whose `path` is (or resolves through) a
symlink to an external target.
A configuration like:

```toml
path = "."
external = true
```

is rejected because `.` canonicalises to the workspace root; the flag has no
useful effect on it.
The clear error tells the user to declare a specific rule pointing at the
symlink instead.
This forecloses the failure mode where `external = true` on a workspace-anchored
rule could be misimplemented as "follow any symlink anywhere."

With the field, [RFD 076]'s post-canonical rule becomes:

> If the canonical path is outside `ctx.root`, reject as workspace-escape
> **unless the matching rule has `external = true`**, in which case the target
> is permitted subject to the approved-target boundary (see [Nested-escape
> boundary](#nested-escape-boundary)).

The capability check is orthogonal: `external = true` permits *resolution*; the
capability fields decide what may be *done* at the resolved target.

Existing rules without `external` behave exactly as today.
The change is purely additive at the policy schema.

### Rule-path canonicalisation (amendment to RFD 076)

[RFD 076] canonicalises rule paths at policy compilation time and rejects rules
whose path canonicalises outside the workspace.
Under that rule, a rule with `path = "fork"` where `fork` is a symlink to an
external target would be rejected during policy compilation.
This RFD amends that behaviour for rules with `external = true`.

The compiled form of `FsRule` carries:

- `lexical_path`: the workspace-relative path the LLM sees and matches against,
  normalised lexically (`..` collapsed) but with the rule's own symlink **not**
  resolved.
- `approved_target: Option<Utf8PathBuf>`: the canonical absolute path that
  external resolution is permitted to land in.
  `Some` only for rules with `external = true` whose target has been approved;
  `None` for ordinary workspace-anchored rules.

Policy compilation:

1. For each rule, canonicalise the path against `ctx.root`.
2. If canonicalisation lands inside the workspace **and** the rule has `external
   = true`, reject during policy compilation (see
   [`FsRule.external`](#fsruleexternal-bool)).
3. If canonicalisation lands inside the workspace and the rule has no `external`
   field, the result is an ordinary rule: `lexical_path = canonicalised
   workspace-relative form`, `approved_target = None`.
   Behaviour unchanged from [RFD 076].
4. If canonicalisation lands outside the workspace and the rule has `external =
   false` (or absent), reject during policy compilation with the existing error.
   Behaviour unchanged from [RFD 076].
5. If canonicalisation lands outside the workspace and the rule has `external =
   true`, the host runs the approval lifecycle (see below) before activation:
   - Approved: `lexical_path = lexical workspace-relative form of the rule's
     path`, `approved_target = the canonical absolute path`.
   - Not approved (and no interactive approval possible): the rule is dropped
     from the compiled policy.
     Tool calls matching its lexical prefix fall through to default-deny.
6. Broken symlinks (target does not exist on this machine) cause the rule to be
   dropped during policy compilation with a warning.
   Tool calls matching the lexical prefix surface the broken-link error at I/O
   time.

Matching at tool-call time is unchanged in shape: longest lexical-prefix match
on the workspace-relative target path.
The post-canonicalisation verification adds the [Nested-escape
boundary](#nested-escape-boundary) check for `external` rules.

### Approval lifecycle (host-side)

Approval lives in the host (JP), not in the tool subprocess.
`jp_tool` is the wire boundary: tools deserialise an `AccessPolicy` and call
`Context::check_*` against it, but they have no terminal, no user-local storage
access, and no way to inquire.
JP also cannot intercept tool calls — only the tool knows which of its
arguments are paths.
The host must therefore approve and bake external targets into the compiled
`AccessPolicy` before the tool spawns; the tool's cooperative check is a pure
lookup against pre-approved data.

Mount approval prompts are user-local trust prompts owned by the host.
They use the terminal prompting UI (e.g. `TerminalPromptBackend`) but are
**not** recorded as `InquiryRequest` / `InquiryResponse` events in the
conversation stream — the prompt contains the canonical external target path,
which by design does not enter shared conversation state.
The durable record is the user-local approval store only.

Approval is **target-only**: the user approves that a specific
workspace-relative rule path is permitted to resolve to a specific canonical
absolute target.
Capability changes to the same rule (e.g. adding write access through a config
edit) do not re-prompt while the target is unchanged.
Capability edits are config decisions visible in normal review channels (git
diff, `jp config show`); silent retargeting is the threat TOFU exists to catch.

The approval check runs during `AccessConfig -> AccessPolicy` conversion, in the
`jp_cli` host's policy-compilation step.
For each rule with `external = true`:

1. Canonicalise the rule's path.
   Capture the resolved target.

2. Consult the user-local approval store (see [Storage](#storage)).

3. If a matching `(rule_path, canonical_target)` entry exists, proceed.

4. If no entry exists and a terminal is available, inquire with the full effect
   visible:

   ```
   Approve this target binding?
   
     fork → /Users/jean/code/forks/serde-yaml
   
   Current grants under this mount:
     fs_read_file:    read
     fs_modify_file:  read, create, update, delete
   
   The target binding will be remembered. Capability changes through
   config edits do not re-prompt while the target is unchanged.
   
   Approve? [y/N]
   ```

   On approval, store the entry.
   On rejection, drop the rule from the compiled policy.

5. If an entry exists with a different `canonical_target`, re-prompt with both
   old and new targets visible:

   ```
   Symlink `fork` retargeted:
     was: /Users/jean/code/forks/serde-yaml
     now: /etc/passwd
   Allow new target? [y/N]
   ```

   On approval, replace the stored target.
   On rejection, drop the rule.

6. If no terminal is available and no matching approval exists, the behaviour
   depends on rule origin:

   - **Pre-existing rules** (hand-authored config, persisted from a prior
     session) are dropped silently with a warning.
     Users running JP non-interactively pre-seed the approval store by editing
     the file directly.
   - **Rules created by an explicit `--mount` in this invocation** bypass this
     path: the symlink-creation step in `Query::run` seeds the approval store
     directly (see [Effects of `--mount`](#effects-of---mount)), so the prompt
     does not need to fire.
     The CLI invocation typing the target is the consent action.

The compiled `AccessPolicy` reaching the tool contains only approved rules.
From the tool's perspective, a missing approval looks identical to a missing
rule — default-deny applies, with the existing helpful-error message naming the
configured grants.

Approval feeds the OS sandbox identically: post-[RFD 075] profile generation
emits allow-entries for approved canonical targets.
The cooperative policy and the OS sandbox see the same approved set.

#### Storage

Approvals live alongside the existing per-user, per-workspace storage entries
(`config/`, `conversations/`, `locks/`, `sessions/`, `storage`):

```
<user-workspace-storage>/approvals.json
```

The physical resolution of `<user-workspace-storage>` follows the existing
storage backend convention (`FsStorageBackend::user_storage_with_path`); this
RFD does not commit to a specific directory layout, which is governed by [RFD
079] and any future durable-storage RFD.

File shape:

```json
{
  "mounts": [
    {
      "rule_path": "fork",
      "canonical_target": "/Users/jean/code/forks/serde-yaml",
      "approved_at": "2026-05-26T13:00:00Z"
    },
    {
      "rule_path": "vendor/openssl",
      "canonical_target": "/Users/jean/code/vendor/openssl-fixed",
      "approved_at": "2026-05-12T09:14:00Z"
    }
  ]
}
```

JSON matches the format used by `conversations/` and `sessions/`.
The `mounts` key is the only approval category in v1; future categories (e.g.
plugins, MCP servers) can join the same file as sibling fields.

**Ownership and write semantics.** The approval store is owned by `jp_cli`'s
policy compiler.
Writes go through atomic temp-file-and-rename to avoid corruption from
interrupted writes.
Corruption on read (malformed JSON) is treated as an empty store with a warning.
Concurrent writes from two `jp q` processes resolve last-writer-wins; the file
is rarely written, so contention is theoretical.

#### Nested-escape boundary

Approving `fork -> /code/forks/x` does not implicitly authorise `fork/secrets ->
/etc`.
Without a boundary check, a nested symlink inside the approved target could
exfiltrate arbitrary paths through the same rule.

The `approved_target` on the compiled rule is the boundary.
At tool-call time, after canonicalising the requested path, the post-canonical
step verifies that the canonical target remains under the rule's
`approved_target`:

- Tool call: `fork/src/lib.rs`.
  Canonicalises to `/code/forks/x/src/lib.rs`.
  Under `/code/forks/x` (the approved target).
- Tool call: `fork/secrets/passwd`.
  Canonicalises to `/etc/passwd` (via the nested symlink).
  Not under `/code/forks/x`. ✗ — reject.

The OS sandbox enforces the same boundary by only allowlisting the approved
target.
A nested escape that the cooperative checker missed would still be denied at the
syscall layer.

Nested escapes do not trigger a separate inquiry in v1.
They reject with a clear error.
If the user genuinely needs the nested target, they create a second explicit
symlink at the workspace level with its own rule and approval.

### `--mount` and `--no-mount`

A CLI shortcut on `jp q` creates the symlink and writes the access grant in one
step.
The `--mount` value has the form `[TOOL:]NAME=PATH[:MODE]`:

```sh
jp q --mount fork=~/code/forks/serde-yaml          # all enabled local tools, :ro default
jp q --mount fork=~/code/forks/serde-yaml:ro       # same, explicit
jp q --mount fs_modify_file:fork=~/code/forks/x:rw # one tool, read-write
jp q --mount a=/p1 --mount b=/p2:ro                # repeated for multiple mounts
jp q --mount fs_read_file:fork=C:\code\forks\x:ro  # Windows
```

Parsing rules:

- Split on the first `=`.
  The left side is `[TOOL:]NAME`; the right side is `PATH[:MODE]`.
- On the left, if a `:` is present, the part before it is the tool name; the
  part after is the mount name.
  Without `:`, the entire left side is the mount name and the grant applies to
  all enabled local tools (see [Tool-scope expansion](#tool-scope-expansion)).
- On the right, peel a trailing `:ro` or `:rw`.
  The remainder is the path.

Tool names are identifiers (`[a-z_][a-z0-9_]*`), so the optional `TOOL:` prefix
is unambiguous.
Windows drive letters (`C:`) appear only on the right side, after the first `=`,
where the mode is peeled from the tail rather than split from the head.

#### Mode rules

Least-privilege defaults at every step:

| Form                        | Mode  | Behaviour                                                                                  |
| --------------------------- | ----- | ------------------------------------------------------------------------------------------ |
| `--mount NAME=PATH`         | `:ro` | All enabled local tools; user prompted with the affected tool list before grant is applied |
| `--mount NAME=PATH:ro`      | `:ro` | Same as above, explicit                                                                    |
| `--mount NAME=PATH:rw`      | ERROR | `:rw` requires `TOOL:` prefix                                                              |
| `--mount TOOL:NAME=PATH`    | `:ro` | One tool, no prompt for tool scope (single scope is its own confirmation)                  |
| `--mount TOOL:NAME=PATH:rw` | `:rw` | One tool, read-write                                                                       |

The user opts into write access *twice*: by typing `:rw` explicitly and by
naming the specific tool that may write.

#### NAME grammar

`NAME` is interpreted as a path relative to the current working directory,
normalised lexically, and must resolve to a location under the workspace.
Absolute paths and `..`-escapes that exit the workspace are rejected at parse
time.

Examples (workspace at `.`, with subdirectories `foo/bar/` and `qux/`):

```sh
cd . && jp q --mount foo/bar/baz=~/code/forks/x       # symlink at <ws>/foo/bar/baz
cd ./foo && jp q --mount baz=~/code/forks/x           # symlink at <ws>/foo/baz
cd ./foo && jp q --mount ../qux/baz=~/code/forks/x    # symlink at <ws>/qux/baz
cd ./foo && jp q --mount ../baz=~/code/forks/x        # symlink at <ws>/baz
cd . && jp q --mount ../baz=~/code/forks/x            # error: escapes workspace
```

`NAME` may not target a path under `.jp/` or other JP-managed storage.
If the resolved path already exists as a non-symlink (regular file or
directory), `--mount` errors before doing anything.
Intermediate directories are created as needed.

#### Tool-scope expansion

The per-tool `access.fs` model in [RFD 076] does not currently support wildcard
grants — `conversation.tools.*` is `ToolsDefaultsConfig`, which has no `access`
field.
A `--mount` invocation without a `TOOL:` prefix expands at CLI time: the CLI
enumerates enabled local tools in the current conversation's resolved config and
writes one rule per tool.
MCP and builtin tools are excluded (consistent with [RFD 076]'s validation that
rejects `access` on those sources).

Tools added to the configuration after the `--mount` invocation do not inherit
the grant.
Users who add new tools and want them to share the mount re-run `--mount`
(idempotent on the symlink; appends the new tool's rule).
A follow-up RFD can add group-level access defaults if this becomes painful.

#### Pipeline stages

Three stages happen at different points in the CLI pipeline:

1. **Config build / schema validation.** `apply_cli_config` (before final
   `AppConfig` is built) mutates the partial to include the new `access.fs`
   rule.
   This is pure data manipulation — `external = true` is accepted syntactically
   without checking whether the symlink exists.
   The mutation is idempotent; `apply_cli_config` runs twice in the current
   pipeline (default conversation resolution, then final config), and both runs
   must produce the same partial.
2. **Mount side effects.** `Query::run` (after the conversation lock is held)
   creates the symlink at `<resolved-name-path> -> <PATH>`, seeds the approval
   store with the canonical target, and records the mount delta into the
   conversation stream so subsequent `jp q` invocations inherit the grant.
3. **Host policy compilation.** Runs after the side-effect stage, also in
   `Query::run`.
   Canonicalises rule paths against the now-existing symlinks, runs the approval
   lifecycle (which finds the seeded approval for the just-created mount), and
   emits the compiled `AccessPolicy` for tool dispatch and (post-[RFD 075])
   sandbox profile generation.
   All "rejected" and "dropped" outcomes for `external` rules happen at this
   stage, not at stage 1.

For hand-authored rules without a `--mount` flag, stage 2's symlink-creation
step is a no-op — the symlink either already exists on disk (placed there by
the user) or doesn't (broken-link handling in stage 3).

#### Default-deny preservation

[RFD 076] specifies that absent `access.fs` means unrestricted workspace access;
declaring at least one rule shifts to default-deny.
A naive `--mount` that appends only the mount rule would unintentionally
restrict the affected tool's access to *only* the mounted path.
The CLI prevents this by checking the post-merge state of `access.fs` for each
tool before writing:

| Initial state for tool `T`                             | What `--mount` writes                                                                                     |
| ------------------------------------------------------ | --------------------------------------------------------------------------------------------------------- |
| `T`'s `access.fs` is empty (no rule from any layer)    | The mount rule **plus** `path = "."` with read/write to preserve `T`'s previous implicit workspace access |
| `T`'s `access.fs` is non-empty (any layer declared it) | Just the mount rule; the user has already opted into default-deny                                         |

The user does not need to think about this; the CLI handles it.

#### Effects of `--mount`

1. Resolve `NAME` to a workspace-relative path.
   Reject if outside.

2. Create a symlink at `<resolved-name-path> -> <PATH>`.
   If the symlink already exists with the same target, no-op.
   If it exists with a different target, error before doing anything else.

3. **Seed the approval store** with `(NAME, canonical_target)`, where
   `canonical_target` is the resolved absolute path of the just-created symlink.
   The CLI invocation typing the target is the consent action, so no separate
   approval prompt fires for this mount on the current or any subsequent
   invocation (until the target changes, at which point the standard re-prompt
   logic applies).

4. For each in-scope tool `T` (post-mode-rules and tool-scope expansion), inject
   the mount rule into the conversation's config layer:

   ```toml
   [[conversation.tools.<T>.access.fs]]
   path = "<NAME>"
   external = true
   read = true
   write = true                       # only when :rw
   ```

   And, if `T`'s `access.fs` was previously empty, also inject the
   workspace-default rule (see [Default-deny
   preservation](#default-deny-preservation)).

5. Persist in the conversation's stored config so subsequent `jp q` invocations
   on the same conversation inherit the grant.

6. The next host-side policy compilation finds the seeded approval and compiles
   the rule with `approved_target` set.

#### `--no-mount`

`--no-mount` removes mounts symmetrically:

```sh
jp q --no-mount             # remove all mounts (symlinks + access rules)
jp q --no-mount fork        # remove only `fork`
```

A mount is identified by the marker `external = true` on its `access.fs` rule.
`--no-mount` finds all such rules referencing the named path (or all such rules,
with no name), unlinks the corresponding symlink at `<resolved-name-path>`, and
writes a new `strategy = "replace"` config layer on `access.fs` for each
affected tool, snapshotting the previous effective rule list minus the mount
rules.
The operation is positive-only and requires no negative-delta infrastructure
([RFD 070]).

Two known limitations of this approach:

- A user who hand-authored an `access.fs` rule with `external = true` for
  purposes other than `--mount` would see it removed by `--no-mount`.
- The replace-snapshot disconnects the affected tools' `access.fs` from future
  workspace-config changes; later additions to workspace `access.fs` do not
  propagate into the conversation until the replace layer is rewritten or
  removed.

Both are accepted v1 trade-offs.
[RFD 070]'s negative-delta model would replace the snapshot with a targeted
removal that preserves inheritance.

### OS-sandbox integration

[RFD 075] generates `sandbox-exec` profiles (macOS) and Landlock rulesets
(Linux) from `AccessPolicy`.
With approved external rules, profile generation emits allow-entries for both
the workspace-relative path (the symlink itself, harmless) and the rule's
`approved_target` as a directory boundary.

No change to profile shape, only to the set of paths a profile contains.

This section applies once [RFD 075] is implemented.
The cooperative layer in this RFD ships independently and degrades cleanly on
platforms or releases where OS-level enforcement is not yet available.

### Platform notes

`std::fs::canonicalize`, `std::os::unix::fs::symlink`, and
`std::os::windows::fs::{symlink_dir, symlink_file}` are all
cross-platform-stable.

On Windows, creating a symlink requires either Developer Mode (Windows 10 1703+)
or administrator privileges.
The `--mount` shortcut falls back to a junction point (via the `junction` crate)
for directory targets when symlink creation fails with
`ERROR_PRIVILEGE_NOT_HELD`.
Junction points are directory-only and require absolute target paths, so file
targets on Windows without Developer Mode produce a clear error from the
`--mount` flow.

Reading and resolving symlinks works on Windows without any privilege.
The `external` field itself is platform-independent.

OS-level enforcement remains platform-conditional, unchanged from [RFD 075]'s
existing matrix: macOS via `sandbox-exec`, Linux via Landlock, Windows currently
has no enforced layer and relies on the cooperative checker alone.

## Drawbacks

**Cross-conversation visibility.** Symlinks live in the workspace tree.
Every conversation in the workspace sees them in directory listings, regardless
of whether it has a matching `external` grant.
A conversation without the grant that tries to access the path gets a clear
denial, so there's no security exposure, but the noise grows linearly with the
number of active mounts in the workspace.

**Windows symlink creation friction.** Without Developer Mode or admin, users
can't create file symlinks via `--mount`.
Junction points cover the directory case (which is the bulk of the use case —
mounting a forked repo), but file mounts on default Windows configurations are
an explicit, documented limitation.

**Initial approval prompts at session start.** A workspace with several
hand-authored `external` rules prompts the user once per rule on first use.
After approval the prompts disappear.
Mounts created via `--mount` auto-seed the approval store at symlink-creation
time and do not contribute to this.

**`--no-mount` marker collisions and inheritance freeze.** A user who
hand-authored an `external = true` rule outside `--mount` would see it removed
by `--no-mount`.
The replace-snapshot also disconnects the affected tools from future
workspace-config changes to `access.fs`.
Both refine cleanly once [RFD 070] lands.

**Symlinks tracked by VCS expose absolute targets.** If a user commits a
workspace symlink to git (or any VCS), the target string is in the commit.
JP is VCS-agnostic and does not interfere with VCS state; how the user manages
this is their decision.

## Alternatives

### Named-mount table in conversation state

Store mounts in conversation-level config (a `[[conversation.mounts]]` table),
entirely separate from `access.fs`.
Pure data, no filesystem manifestation; tools receive a separate `mounts` field
in their context.

Rejected because it duplicates the access-policy mechanism with a parallel one,
requires a plugin-protocol extension to expose mounts to plugins, and stores
absolute paths in conversation state (privacy and cross-machine portability
problems).
The symlink approach reuses existing infrastructure end to end.

### Bounded targets via `follow_to = ["..."]`

Instead of TOFU, let each rule declare allowed canonical targets explicitly.

Rejected because it reintroduces absolute paths into the configuration surface
— the privacy and portability problem the symlink design eliminates.
TOFU provides equivalent protection against silent retargeting without
committing target paths to shared config.

### Approval inside `Context::check_*`

Run the approval flow inside the tool's `Context::check_*` call rather than
host-side.

Structurally impossible.
`jp_tool::Context` runs inside the tool subprocess, which has no terminal, no
access to JP's user-local approval store, and no way to inquire.
The OS sandbox is also built before the tool spawns, so in-tool approval cannot
inform the sandbox profile.

### Per-resolution TOFU at tool-call time

Defer approval until the tool resolves a path that escapes the workspace,
prompting via `Outcome::NeedsInput`.

Rejected because the OS sandbox is built at tool spawn from a fixed set of
allowed paths.
Per-resolution prompts cannot add paths to a running sandbox.
Compile-time per-rule approval gives both the cooperative checker and the
sandbox the same approved set to operate against.

### Mount overlay separate from `access.fs`

Store mounts in a parallel namespace, compiled into `AccessPolicy` alongside
user-declared `access.fs` rules.
Adding a mount would not change whether `access.fs` was declared.

Rejected for UX reasons: a mount is, in user terms, a filesystem access grant.
Putting mounts and other filesystem grants in different sections asks the user
to internalise an implementation distinction.
The case-based default-deny preservation (see [Default-deny
preservation](#default-deny-preservation)) solves the same problem inside
`access.fs` without splitting the user-facing namespace.

### Unified attachment + tool access policy

Have attachments go through `access.fs` too, so a single rule governs both
LLM-driven tool access and user-initiated attachments.

Rejected because attachments are user-initiated (the `--attach` argument is the
consent action) and one-shot (snapshot, not ongoing privilege).
[RFD D03] covers the attachment case with the right semantics.

### Absolute-path `access.fs` rules

Allow rules to declare absolute paths directly, bypassing the workspace anchor.

Rejected.
Absolute paths in shared workspace config don't round-trip across machines, and
they widen the LLM's expressible reach beyond the workspace.

## Non-Goals

- **External attachments.** [RFD D03] is the design for `--attach` outside the
  workspace.

- **Conversation-scoped mounts.** Symlinks live in the workspace and are shared
  across all conversations.
  Per-conversation isolation breaks the workspace-relative invariant.
  Deferred until cross-conversation noise proves painful.

- **Unattended-mode pre-approval CLI flag.** Users running JP non-interactively
  pre-seed the approval store by editing the JSON file.
  A flag is small but unnecessary in v1.

- **Symlink creation for file targets on Windows without Developer Mode.**
  Junction points cover directories; file targets either require Developer Mode
  or fail clearly.

- **VCS handling.** JP is VCS-agnostic.
  `--mount` creates a symlink; whether the user commits, ignores, or otherwise
  manages it through their version control system is their decision.

- **Group-level access defaults.** `--mount` without a `TOOL:` prefix expands at
  CLI time to per-tool rules over the currently-enabled local tools.
  True group defaults belong in a follow-up that extends [RFD 076].

- **`external = true` with workspace-anchored rules.** Configurations like `path
  = "." + external = true` are rejected during policy compilation.
  External permission applies only to rules whose own path is (or resolves
  through) a symlink with an external target.

## Risks and Open Questions

**Symlink-retargeting attack surface.** A teammate committing a one-line symlink
change (`fork -> /etc/passwd`) is easy to miss in a `git pull` diff.
TOFU at policy-compile time is the mitigation: the next policy compilation
re-prompts because the canonical target differs from the stored approval.

**Target-only approval scope.** Approvals bind only the lexical mount path to a
canonical target.
A teammate widening capabilities (e.g. adding `write = true`) on the same mount
through a config edit does not re-prompt.
This is a deliberate trust-model choice — capability changes are config edits
visible in normal review channels (git diff, `jp config show`), while target
retargeting is the silent vector TOFU is designed to catch.

**Cross-conversation noise.** Workspaces with many mounts give every
conversation visibility into all of them.
Conversation-scoped mounts are the follow-up if denials become a token-cost or
UX problem.

**Marker collisions on `--no-mount`.** The v1 convention treats any rule with
`external = true` as a mount.
Refinement via stable mount-identity marker is straightforward if needed.

**Approval-store schema migration.** The JSON schema may need to grow
(expiration timestamps, per-tool scope, additional approval categories).
Plain JSON behind a single read site keeps migration cost bounded.

## Implementation Plan

> [!NOTE]
> **Current status (PR 727).** This first slice ships cooperative enforcement
> for local filesystem tools.
> What it does and does not cover:
>
> - Phase 1 (pre-canonical invariant): done.
> - Phase 2 (`FsRule.external`, rule-path canonicalisation, approved-target
>   boundary): done.
>   In-workspace rules are matched on the canonical workspace-relative form, so
>   an in-workspace symlink cannot dodge a more specific rule; external rules
>   keep their lexical mount prefix plus the approved-target boundary.
> - Phase 3 (approval lifecycle): partial.
>   The store, lookup, and `--mount`-seeded approvals are in place.
>   A hand-authored `external = true` rule does not yet get a trust-on-first-use
>   prompt: an unapproved or retargeted external rule is dropped with a warning,
>   never silently granted.
>   The interactive prompt is deferred.
> - Phase 5 (`--mount`): done (parsing, symlink creation, approval seeding,
>   case-based default-deny preservation, expansion over the resolved
>   enabled-local tool set).
>   `--no-mount` is not implemented; cleanup of the symlink and the persisted
>   config is manual until it lands.
>   The broad-mount tool-scope confirmation prompt is also deferred: in this
>   slice the `--mount` invocation is the consent action, and `:rw` still
>   requires an explicit `TOOL:` scope.
> - Phase 4 (OS sandbox, [RFD 075]) and Phase 6 (Windows junction fallback) are
>   not started; enforcement is cooperative only, and `net` / `env` rules are
>   not yet modelled in config.
>
> Correctness guarantees in this slice: config validation rejects `access` on
> tools whose finalised source is `builtin` or `mcp` ([RFD 076]); an `access`
> config that fails to compile fails the tool invocation, and a declared policy
> whose rules all drop (unapproved or broken external targets) stays
> default-deny rather than degrading to unrestricted workspace access; and the
> in-tree fs tools enforce capabilities against the resolved canonical path, so
> an in-workspace symlink cannot reach a denied location.

### Phase 1: Pre-canonical invariant in `jp_tool`

Add the explicit pre-canonicalisation check to `Context::check_*`.
Reject absolute paths and `..`-escapes with a typed error before any filesystem
I/O.
Update `FsAccessError` to distinguish "out-of-workspace input" from
"out-of-workspace canonical target."
Document the contract that `check_*` accepts only raw workspace-relative paths.

Independent.
Mergeable on its own.

### Phase 2: `FsRule.external` field and amended rule-path canonicalisation

Add the field to `jp_tool::FsRule` and `jp_config::FsRuleConfig`.
Modify [RFD 076]'s rule-path canonicalisation: rules with `external = true`
whose path canonicalises outside the workspace are preserved with `lexical_path`
and `approved_target`.
Reject `external = true` on rules whose path canonicalises inside the workspace.
Wire the `approved_target` boundary into the matching algorithm.
Treat broken symlinks as policy-compilation drop+warn.

Depends on Phase 1.
Includes the [RFD 076] modification.

### Phase 3: Host-side approval lifecycle

Define the approval store format under `<user-workspace-storage>/approvals.json`
with a `mounts` field.
Implement read/write helpers in `jp_cli` with atomic temp-file-and-rename
writes.
Wire the approval check into `AccessConfig -> AccessPolicy` conversion at the
host boundary.
Use the terminal prompting UI from the existing inquiry infrastructure ([RFD
005], [RFD 028]) for interactive prompts, but do not persist approval as inquiry
events (the canonical target path stays out of conversation state).
Implement the prompts with the affected tools and capabilities visible.
Distinguish behaviour on missing terminal: pre-existing rules drop+warn; rules
created by an explicit `--mount` in this invocation are seeded by the
symlink-creation step instead.

Depends on Phase 2.

### Phase 4: OS-sandbox profile generator

Update [RFD 075]'s profile generator to emit allow-entries for approved
`approved_target` paths.
Skip rules whose approval failed.
Add tests for macOS and Linux profile output.

Depends on Phase 3.
Coordinates with [RFD 075]'s implementation phases.

### Phase 5: `--mount` and `--no-mount` CLI

Add the flags to `jp q`.
Implement parsing for `[TOOL:]NAME=PATH[:MODE]`, including the `--attach`-style
`NAME` resolution and the mode rules table.
Split config mutation (in `apply_cli_config`, idempotent) from symlink creation
and conversation persistence (in `Query::run`).
Implement the case-based default-deny preservation.
Implement the marker-based `--no-mount` cleanup using `strategy = "replace"`
config layers.

Depends on Phase 3.

### Phase 6: Platform-specific symlink creation

Implement the Windows fallback path (symlink → junction for directories).
Add the `junction` crate as an optional `[target.'cfg(windows)'.dependencies]`
entry in `jp_cli`.
Surface a clear error for file targets on Windows when symlink creation fails.

Depends on Phase 5.
Mergeable in parallel with Phase 4.

## References

- [RFD 005] — First-class inquiry events.
  Provides the terminal prompting UI used by host-side approval.
  Approval prompts use the UI but are not persisted as `InquiryRequest` /
  `InquiryResponse` events (see [Approval
  lifecycle](#approval-lifecycle-host-side)).
- [RFD 028] — Structured inquiry system for tool questions.
  Same UI-yes/event-no distinction as RFD 005.
- [RFD 070] — Negative config deltas.
  Not required by this RFD; `--no-mount` uses positive `strategy = "replace"`
  writes instead.
- [RFD 075] — Tool sandbox and access policy.
  OS-level enforcement consumes approved external rules; this RFD does not
  modify 075's profile shape, only the set of paths a profile contains.
- [RFD 076] — Tool access grants.
  This RFD extends and amends 076's `FsRule`, its rule-path canonicalisation,
  and its path-evaluation steps.
- [RFD 079] — Config sources and load order.
  Governs the physical layout of user-workspace storage; this RFD references the
  logical location only.
- [RFD D03] — External attachment URI scheme.
  Covers the attachment counterpart; the two compose without overlapping.
- Plugin path-and-hash approval (`crates/jp_cli/src/cmd/plugin/dispatch.rs`,
  `crates/jp_cli/src/cmd/plugin/registry.rs`) — existing precedent for the
  TOFU-with-re-approval pattern used here.

[RFD 005]: ../005-first-class-inquiry-events.md
[RFD 028]: ../028-structured-inquiry-system-for-tool-questions.md
[RFD 070]: ../070-negative-config-deltas.md
[RFD 075]: ../075-tool-sandbox-and-access-policy.md
[RFD 076]: ../076-tool-access-grants.md
[RFD 079]: ../079-config-sources-and-load-order.md
[RFD D03]: D03-external-attachment-uri-scheme.md
