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
- **Summary**: Allows tools to access external paths through workspace symlinks
  with `external` access rules and a `--mount` CLI flag.

## Summary

This RFD lets the assistant read and modify files outside the workspace through
user-created workspace symlinks.
It extends [RFD 076] so tool arguments remain workspace-relative while `FsRule`
gains an `external` scope field that approves a symlink's canonical target via
trust-on-first-use (TOFU).
A `--mount` flag on `jp query` creates the workspace symlink and records its
access grant in one step.

## Motivation

JP's workspace model is workspace-rooted by intent, but current code does not
confine local subprocess tools.
`jp_llm::run_tool_command` only sets `current_dir = workspace root`, so a
subprocess can still open `/etc/passwd` or `~/.ssh/id_rsa`.
[RFD 076] introduces the cooperative `AccessPolicy` layer and [RFD 075]
introduces OS-level sandboxing.
Both are planned and unshipped; this RFD extends those planned layers.

Confining tools strictly to the workspace creates friction when a task spans
repositories.
Suppose a developer is working in workspace `W` on feature `F`, which depends on
a forked crate at `~/code/forks/X`.
To complete `F`, the assistant needs to fix `~/code/forks/X`, push the fix, and
point `W`'s `Cargo.toml` at the fork.
With strict workspace confinement, the assistant can guide the user through the
fix in `~/code/forks/X` but cannot read or edit the fork itself.
Running `jp init` inside the fork creates a separate workspace, splitting `F`'s
conversation history, context, and tool configuration.

The assistant should be able to reach the fork from `W`'s conversation without:

1. Exposing absolute paths for the LLM to reason about.
2. Breaking the workspace-relative invariant that bounds the paths the LLM can
   express.
3. Widening the access policy or OS sandbox for every tool by default.

A workspace symlink satisfies all three constraints.
The user creates `<W>/fork -> ~/code/forks/X` and grants the assistant explicit
access.
When the assistant calls `fs_modify_file fork/src/lib.rs`, the input path is
workspace-relative, the canonical target is external, and the explicit grant
permits the resolution.
Conversation events record `fork/src/lib.rs`, which stays portable across
machines: a teammate cloning the workspace either has their own `fork` symlink
or gets a clean failure.

## Design

### Field grouping in `FsRule`

[RFD 076] defines two kinds of fields on `FsRule`:

- **Scope fields** select which paths the rule applies to (`path`).
- **Capability fields** grant what the tool may do within that scope (`read`,
  `create`, `update`, `delete`, `execute`, and the `write` alias).

This RFD adds a second scope field, `external`.

### Pre-canonical workspace-relative invariant for tool calls

Tool call arguments must be workspace-relative before canonicalisation.
`jp_tool::Context::check_*` enforces this rule first:

> Reject the call if the input path is absolute, or if its lexical resolution
> (with `..` segments collapsed) escapes `ctx.root`.

Under [RFD 076], absolute paths and `..`-escapes fail only after
canonicalisation, when the resolved path falls outside the workspace.
Checking the lexical path before canonicalisation produces specific error
messages ("absolute paths are not permitted" rather than "path escapes
workspace") and keeps the paths the LLM can express independent of the symlinks
present in the workspace.

`Context::check_*` accepts only the raw workspace-relative path supplied by the
LLM.
Tools must not join paths against `ctx.root` before calling `check_*`; the
checker performs the join and canonicalisation itself.

This invariant applies only to tool calls.
Attachments (`--attach`) follow [RFD D03]'s permissive model: passing the path
on the CLI is the user's consent action, and D03's `external:` URI scheme
handles privacy and deduplication.
A tool never addresses external paths directly; it either operates on a
workspace-relative symlink governed by `FsRule.external`, or reads an
`external:` URI from conversation context.

### `FsRule.external: bool`

`FsRule` ([RFD 076]) gains an optional boolean scope field:

```toml
[[conversation.tools.fs_modify_file.access.fs]]
path = "fork"
external = true
read = true
write = true
```

`external` defaults to `false`.
Setting `external = true` permits the rule's `path` to resolve outside the
workspace through a symlink; the capability fields still determine what the tool
may do at the resolved target.

The policy compiler rejects `external = true` if the rule's lexical path
canonicalises inside the workspace.
The flag is valid only on rules whose `path` is, or resolves through, a symlink
to an external target.
For example, the compiler rejects:

```toml
path = "."
external = true
```

Because `.` canonicalises to the workspace root, the compiler instructs the user
to point the rule at a specific symlink instead.
This check prevents `external = true` on a workspace-anchored rule from allowing
arbitrary symlinks inside that directory to escape the workspace.

With `external`, [RFD 076]'s post-canonical check becomes:

> If the canonical path is outside `ctx.root`, reject as workspace-escape
> **unless the matching rule has `external = true`**, in which case the target
> is permitted subject to the approved-target boundary (see [Nested-escape
> boundary](#nested-escape-boundary)).

Rules that omit `external` behave as defined in [RFD 076].

### Rule-path canonicalisation (amendment to RFD 076)

[RFD 076] canonicalises rule paths at policy compilation time and rejects rules
whose path canonicalises outside the workspace.
Under that check, a rule with `path = "fork"` where `fork` is a symlink to an
external target would fail compilation.
This RFD amends compilation for rules with `external = true`.

A compiled `FsRule` carries two paths:

- `lexical_path`: the workspace-relative path the LLM sees and matches against,
  normalised lexically (`..` collapsed) without resolving the rule's own
  symlink.
- `approved_target: Option<Utf8PathBuf>`: the canonical absolute path that
  external resolution may land in.
  This is `Some` for approved `external = true` rules and `None` for
  workspace-anchored rules.

Policy compilation processes each rule as follows:

1. Canonicalise the rule's path against `ctx.root`.
2. If the canonical path is inside the workspace and `external = true`, reject
   the policy (see [`FsRule.external`](#fsrule-external-bool)).
3. If the canonical path is inside the workspace and `external` is `false` or
   omitted, compile an ordinary rule with `lexical_path` set to the canonical
   workspace-relative path and `approved_target = None`, matching [RFD 076].
4. If the canonical path is outside the workspace and `external` is `false` or
   omitted, reject the policy with the workspace-escape error from [RFD 076].
5. If the canonical path is outside the workspace and `external = true`, run the
   host approval lifecycle (see below):
   - If approved, set `lexical_path` to the rule's lexical workspace-relative
     path and `approved_target` to the canonical absolute path.
     If the target is an ancestor of the workspace, also insert a deny-all rule
     where the workspace reappears inside the mount (see [Targets that contain
     the workspace](#targets-that-contain-the-workspace)).
   - If rejected or unapproved without an interactive terminal, drop the rule
     from the compiled policy so matching tool calls fall through to
     default-deny.
6. If the rule's symlink is broken (its target does not exist on this machine),
   drop the rule with a warning.
   Tool calls matching its lexical prefix then surface the broken-symlink error
   at I/O time.

Tool-call matching still uses longest lexical-prefix match on the
workspace-relative input path, followed by the [Nested-escape
boundary](#nested-escape-boundary) check after canonicalisation for `external`
rules.

### Approval lifecycle (host-side)

Target approval runs in the JP host process, not in the tool subprocess.
`jp_tool` is the wire boundary: tools deserialise an `AccessPolicy` and call
`Context::check_*`, but they have no terminal, no access to user-local storage,
and no way to prompt the user.
The host also cannot intercept filesystem checks mid-call because only the tool
knows which of its arguments are paths.
The host therefore approves external targets and bakes them into the compiled
`AccessPolicy` before spawning the tool, leaving `Context::check_*` as a pure
in-memory lookup.

Mount approval prompts are user-local trust prompts owned by the host.
They use the terminal prompting UI (such as `TerminalPromptBackend`) but are not
recorded as `InquiryRequest` or `InquiryResponse` events in the conversation
stream, keeping the canonical external target path out of shared conversation
state.
The user-local approval store is the only persistent record.

Approval is **target-only**: the user approves that a specific
workspace-relative `rule_path` may resolve to a specific `canonical_target`.
Changing capabilities on the same rule (for example, adding `write = true` in
config) does not re-prompt as long as the canonical target is unchanged.
Capability changes are config edits visible in `git diff` and `jp config show`,
whereas TOFU guards against silent symlink retargeting.

The approval check runs in `jp_cli` when converting `AccessConfig` into
`AccessPolicy`.
For each rule with `external = true`:

1. Canonicalise the rule's path to obtain `canonical_target`.

2. Look up `rule_path` in the user-local approval store (see
   [Storage](#storage)).

3. If a matching `(rule_path, canonical_target)` entry exists, keep the rule.

4. If no entry exists and a terminal is available, prompt the user with the
   target and the current grants under the mount:

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

5. If an entry exists for `rule_path` with a different `canonical_target`,
   re-prompt with both targets visible:

   ```
   Symlink `fork` retargeted:
     was: /Users/jean/code/forks/serde-yaml
     now: /etc/passwd
   Allow new target? [y/N]
   ```

   On approval, replace the stored target.
   On rejection, drop the rule.

6. If no terminal is available and no matching approval exists, handling depends
   on how the rule was created:

   - **Pre-existing rules** (from config files or a prior session) are dropped
     with a warning.
     For unattended runs, users can pre-seed the approval store by editing
     `approvals.json` directly.
   - **Rules created by `--mount` in the current invocation** are already
     approved: the symlink-creation step in `Query::run` seeds the approval
     store directly (see [Effects of `--mount`](#effects-of-mount)) because
     passing the target on the CLI is the user's consent action.

The compiled `AccessPolicy` passed to the tool contains only approved rules.
To the tool, an unapproved rule is indistinguishable from an omitted rule:
default-deny applies and the error message lists the active grants.

The OS sandbox consumes the same approvals: post-[RFD 075] profile generation
emits allow-entries for each rule's `approved_target`.

#### Storage

Approvals are stored alongside the existing per-user, per-workspace files
(`config/`, `conversations/`, `locks/`, `sessions/`, `storage`):

```
<user-workspace-storage>/approvals.json
```

`<user-workspace-storage>` resolves through
`FsStorageBackend::user_storage_with_path`, whose physical layout is governed by
[RFD 079].

The file is a JSON object with a `mounts` array:

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

`mounts` is the only approval category in v1; future categories (such as plugins
or MCP servers) can be added as sibling keys.

**Ownership and write semantics.** `jp_cli`'s policy compiler owns the approval
store.
Writes use atomic temp-file-and-rename to prevent partial files on interruption.
If `approvals.json` contains malformed JSON on read, the compiler logs a warning
and treats the store as empty.
Concurrent writes from multiple `jp q` processes resolve last-writer-wins.

#### Nested-escape boundary

Approving `fork -> /code/forks/x` does not authorise a nested symlink inside
`/code/forks/x` (such as `fork/secrets -> /etc`) to escape that directory tree.

The compiled rule's `approved_target` defines the allowed boundary.
After canonicalising the tool call's target path, `Context::check_*` verifies
that the canonical path remains under `approved_target`:

- `fork/src/lib.rs` canonicalises to `/code/forks/x/src/lib.rs`, which is under
  `/code/forks/x` and is permitted.
- `fork/secrets/passwd` canonicalises to `/etc/passwd` through the nested
  symlink, which falls outside `/code/forks/x` and is rejected.

The OS sandbox enforces the same boundary by allowlisting only
`approved_target`.

In v1, a nested escape is rejected with an error rather than triggering an
approval prompt.
To grant access to the nested target, the user creates a separate workspace
symlink with its own rule and approval.

#### Targets that contain the workspace

An approved target may be an ancestor of the workspace.
For example, from a workspace at `~/w/a/b`, running `jp q --mount up=..`
approves `up -> ~/w/a` so the assistant can access sibling directories in the
enclosing repository.

An ancestor mount grants access to the target tree excluding the workspace
itself.
During policy compilation, the compiler inserts a deny-all rule at the lexical
path where the workspace reappears inside the mount (`up/b`):

- `up/c/lib.rs` canonicalises to `~/w/a/c/lib.rs` (outside the workspace) and
  matches the mount rule `up`.
- `up/b/src/lib.rs` canonicalises back into the workspace through the mount.
  The cooperative checker rejects the call and directs the tool to use the
  workspace-relative path `src/lib.rs` instead.
- Moving, deleting, or overwriting `up/b` (or `top/a` for a mount `top=../..`)
  is rejected: a tool cannot move, delete, or replace any ancestor directory of
  the workspace through a mount.
  This check runs against the canonical path, so accessing the ancestor through
  an internal symlink (such as `up/current/...` where `current -> releases/v1`)
  is also rejected.
  The symlink `up/current` itself can still be moved or deleted, and sibling
  files inside those ancestor directories remain accessible through the mount.

The deny-all rule at `up/b` protects tools that evaluate rules against lexical
paths, such as a directory walker traversing `up/` that would otherwise match
files under `up/b` against `up`.
Because directory walkers do not follow symlinks, `up/b` is the only path under
`up/` that reaches the workspace.
A tool that canonicalises paths before matching resolves `up/b/src/lib.rs` to
`src/lib.rs` and evaluates it against the workspace's own rules.
The OS sandbox enforces the same workspace exclusion (see [OS-sandbox
integration](#os-sandbox-integration)).

Mount targets that equal the workspace root or lie inside the workspace are
rejected at policy compilation time (see [Rule-path
canonicalisation](#rule-path-canonicalisation-amendment-to-rfd-076)).

### `--mount` and `--no-mount`

The `--mount` flag on `jp q` creates the workspace symlink and records the
access grant in a single command.
Its value has the form `[TOOL:][NAME=]PATH[:MODE]`:

```sh
jp q --mount ~/code/forks/serde-yaml               # NAME defaults to serde-yaml
jp q --mount fs_modify_file:~/code/forks/x:rw      # NAME defaults to x, one tool
jp q --mount fork=~/code/forks/serde-yaml          # all enabled local tools, :ro default
jp q --mount fork=~/code/forks/serde-yaml:ro       # same, explicit
jp q --mount fs_modify_file:fork=~/code/forks/x:rw # one tool, read-write
jp q --mount a=/p1 --mount b=/p2:ro                # repeated for multiple mounts
jp q --mount fs_read_file:fork=C:\code\forks\x:ro  # Windows
```

Parsing follows these rules:

- Split on the first `=`.
  The left side is `[TOOL:]NAME` and the right side is `PATH[:MODE]`.
  When `=` is omitted, the value is `[TOOL:]PATH[:MODE]` and `PATH` must exist
  on disk.
  `NAME` defaults to the final segment of `PATH` after lexical normalisation
  against the current working directory (without resolving symlinks), so `.` and
  `..` use the name of the directory they resolve to while a symlinked `PATH`
  keeps its symlink name.
  A `PATH` with no final segment (such as `/`) is rejected.
- Without `=`, a leading `TOOL:` prefix can collide with a path containing `:`.
  The prefix is parsed as `TOOL:` only if the remainder of the path exists on
  disk and the unsplit string does not.
  If both exist on disk, `--mount` rejects the ambiguous argument and asks for
  an explicit `NAME=PATH` or `TOOL:NAME=PATH`.
  A single-letter prefix is always parsed as a Windows drive letter (`C:\...`),
  never as a tool name.
- On the left side of `=`, if `:` is present, the prefix before `:` is the tool
  name and the remainder is `NAME`.
  Without `:`, the entire left side is `NAME` and the grant applies to all
  enabled local tools (see [Tool-scope expansion](#tool-scope-expansion)).
- On the right side, strip a trailing `:ro` or `:rw` mode suffix; the remainder
  is `PATH`.

When `=` is present, `TOOL:` is unambiguous: tool names match
`[a-z_][a-z0-9_]*`, and Windows drive letters (`C:`) appear only on the right
side of `=`, where the mode suffix is stripped from the end.

#### Mode rules

Mode defaults to read-only (`:ro`), and write access (`:rw`) requires an
explicit tool scope:

| Form                        | Mode  | Behaviour                                                                                  |
| --------------------------- | ----- | ------------------------------------------------------------------------------------------ |
| `--mount NAME=PATH`         | `:ro` | All enabled local tools; user prompted with the affected tool list before grant is applied |
| `--mount NAME=PATH:ro`      | `:ro` | Same as above, explicit                                                                    |
| `--mount NAME=PATH:rw`      | ERROR | `:rw` requires `TOOL:` prefix                                                              |
| `--mount TOOL:NAME=PATH`    | `:ro` | One tool, no prompt for tool scope (single scope is its own confirmation)                  |
| `--mount TOOL:NAME=PATH:rw` | `:rw` | One tool, read-write                                                                       |

Granting write access requires two explicit choices: appending `:rw` and naming
a single `TOOL`.

Every form in the table also accepts a bare `PATH` without `NAME=` (`--mount
PATH`, `--mount PATH:rw` as an error, and `--mount TOOL:PATH:rw`), deriving
`NAME` from `PATH`.

#### NAME grammar

`NAME` is resolved relative to the current working directory, normalised
lexically, and must remain inside the workspace.
Absolute paths and `..` segments that escape the workspace are rejected at parse
time.

For a workspace at `.` with subdirectories `foo/bar/` and `qux/`:

```sh
cd . && jp q --mount foo/bar/baz=~/code/forks/x       # symlink at <ws>/foo/bar/baz
cd ./foo && jp q --mount baz=~/code/forks/x           # symlink at <ws>/foo/baz
cd ./foo && jp q --mount ../qux/baz=~/code/forks/x    # symlink at <ws>/qux/baz
cd ./foo && jp q --mount ../baz=~/code/forks/x        # symlink at <ws>/baz
cd . && jp q --mount ../baz=~/code/forks/x            # error: escapes workspace
```

`NAME` cannot target `.jp/` or any other JP-managed storage directory.
If the resolved `NAME` path already exists as a regular file or directory rather
than a symlink, `--mount` fails before modifying the filesystem or config.
Missing parent directories under the workspace are created automatically.

#### Tool-scope expansion

When `--mount` is passed without a `TOOL:` prefix, the CLI expands the grant
across all enabled local tools in the conversation's resolved config, writing
one rule per tool.
MCP and builtin tools are excluded, matching [RFD 076]'s rule that rejects
`access` blocks on non-local tools.

`--mount` writes per-tool rules rather than a wildcard rule under
`[conversation.tools.'*'.access]` because a `'*'` rule would also apply to tools
enabled later, and per-tool `access` blocks replace rather than merge with `'*'`
([RFD 076]).

If a tool has no `access.fs` rules of its own, `--mount` seeds its rule list
before appending the mount rule (copying the `'*'` rules if present, or
inserting a workspace-wide rule otherwise) so adding a mount does not revoke the
tool's existing workspace access (see [Default-deny
preservation](#default-deny-preservation)).

Tools enabled after the `--mount` invocation do not inherit the grant.
To extend an existing mount to a newly added tool, the user re-runs `--mount`,
which leaves the symlink intact and appends the rule for the new tool.

#### Pipeline stages

`--mount` executes across three stages in the CLI pipeline:

1. **Config mutation and validation (`apply_cli_config`).** Before building the
   resolved `AppConfig`, `apply_cli_config` inserts the new `access.fs` rules
   into the partial config.
   This stage does not check whether the workspace symlink exists yet.
   It touches the filesystem only when `=` is omitted, checking whether `PATH`
   exists on disk to disambiguate a leading `TOOL:` prefix.
   Because `apply_cli_config` runs twice (once for default conversation
   resolution and once for the final config) before stage 2 creates the symlink,
   creating or deleting `PATH` on disk between those runs could cause stage 1
   and stage 2 to derive different mount names.
   That race fails closed: the config rule then references a path with no
   symlink, and stage 3 drops the rule.
   The config mutation itself is idempotent across both runs of
   `apply_cli_config`.
2. **Filesystem and store side effects (`Query::run`).** After acquiring the
   conversation lock, `Query::run` creates the symlink at `<resolved-name-path>
   -> <PATH>`, seeds the approval store with the canonical target, and records
   the config delta in the conversation stream so subsequent `jp q` invocations
   inherit the grant.
3. **Host policy compilation (`Query::run`).** Immediately after stage 2, the
   policy compiler canonicalises rule paths against the workspace symlinks, runs
   the approval lifecycle (finding the seeded approval from stage 2), and
   produces the compiled `AccessPolicy` for tool execution and post-[RFD 075]
   sandbox profile generation.
   Invalid or unapproved `external` rules are rejected or dropped at this stage.

For hand-authored `external = true` rules without `--mount`, stage 2 is a no-op
and stage 3 evaluates whatever symlink is on disk.

#### Default-deny preservation

In [RFD 076], an omitted `access.fs` grants unrestricted workspace access,
whereas declaring any rule switches the tool to default-deny.
Appending only the mount rule to a tool with no prior `access.fs` rules would
therefore revoke its access to the rest of the workspace.
To preserve the tool's existing workspace access, `--mount` inspects each tool's
effective `access.fs` state before writing:

| Initial state for tool `T`                                         | What `--mount` writes                                                                            |
| ------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------ |
| `T` declared rules of its own (any layer)                          | Just the mount rule; the user has already opted into default-deny                                |
| `T` declared none, but `[conversation.tools.'*'.access]` has rules | The mount rule **plus** a copy of the `'*'` rules, which `T` was inheriting until now            |
| No rule anywhere                                                   | The mount rule **plus** `path = "."` with read/write, preserving `T`'s implicit workspace access |

The second row is required because per-tool `access.fs` replaces `'*'` rather
than merging with it: writing a rule onto `T` detaches `T` from
`[conversation.tools.'*'.access]`, so the inherited `'*'` rules must be copied
into `T`'s block first.

#### Effects of `--mount`

Running `jp q --mount` performs six steps:

1. Resolve `NAME` to a workspace-relative path and reject it if it escapes the
   workspace.

2. Create the symlink `<resolved-name-path> -> <PATH>`.
   If a symlink already exists with the same target, do nothing.
   If a symlink with a different target or a non-symlink file exists at that
   path, abort with an error.

3. **Seed the approval store** with `(NAME, canonical_target)`, where
   `canonical_target` is the canonical absolute path of `<PATH>`.
   Because passing `--mount` on the CLI is the user's consent action, no
   interactive approval prompt fires unless the symlink is later retargeted.

4. For each target tool `T`, append the mount rule to the conversation's config
   layer:

   ```toml
   [[conversation.tools.<T>.access.fs]]
   path = "<NAME>"
   external = true
   read = true
   write = true                       # only when :rw
   ```

   If `T` had no explicit `access.fs` rules, prepend the preserved workspace or
   `'*'` rules first (see [Default-deny
   preservation](#default-deny-preservation)).

5. Record the config delta in the conversation stream so subsequent `jp q` turns
   on the same conversation keep the grant.

6. Compile `AccessPolicy` on the host, matching the seeded approval and setting
   `approved_target` on the compiled rule.

#### `--no-mount`

`--no-mount` removes mounts and their symlinks:

```sh
jp q --no-mount             # remove all mounts (symlinks + access rules)
jp q --no-mount fork        # remove only `fork`
jp q --no-mount ~/code/x    # remove the mount `--mount ~/code/x` created
```

`--no-mount` accepts either a mount `NAME` or the target `PATH` passed to
`--mount`.
If the argument matches an existing mount's `path` directly, that mount is
selected first so nested names such as `vendor/openssl` are not reduced to their
final segment (`openssl`); otherwise `--no-mount` derives `NAME` from `PATH`
using the same rules as `--mount`.

Mounts are identified by `external = true` on their `access.fs` rules.
`--no-mount` finds all `external = true` rules matching the target mount (or all
`external = true` rules when no argument is given), removes the corresponding
symlink at `<resolved-name-path>`, and writes a `strategy = "replace"` config
layer on `access.fs` for each affected tool containing the previous effective
rules minus the removed mount rules.
Using `strategy = "replace"` avoids needing negative config deltas ([RFD 070]).

This approach has two v1 limitations:

- Hand-authored `access.fs` rules with `external = true` are indistinguishable
  from `--mount` rules and are also removed by `--no-mount`.
- Writing a `strategy = "replace"` snapshot detaches the tool's `access.fs` list
  from workspace-level config inheritance, so subsequent additions to workspace
  `access.fs` rules do not take effect in the conversation until the replace
  layer is updated or cleared.

Once [RFD 070] lands, `--no-mount` can emit a negative delta instead of a
replace snapshot, preserving config inheritance.

### OS-sandbox integration

[RFD 075] generates `sandbox-exec` profiles on macOS and Landlock rulesets on
Linux from `AccessPolicy`.
For each approved external rule, profile generation emits allow-entries for both
the workspace-relative symlink path and the rule's `approved_target` directory
tree.

When `approved_target` is an ancestor of the workspace, the sandbox profile must
exclude the workspace subtree; otherwise a read-write mount of `..` would grant
write access to the workspace at the OS layer regardless of the workspace's own
rules:

- **macOS (`sandbox-exec`).** After the allow rule for `approved_target`, the
  profile emits a deny rule for the workspace root followed by the workspace's
  own allow rules.
  This ordering relies on later `sandbox-exec` rules overriding earlier ones,
  which must be verified when implementing [RFD 075].
- **Linux (Landlock).** Landlock rulesets are strictly additive and cannot carve
  out a denied subtree from an allowed parent directory.
  Instead, at each ancestor directory from `approved_target` down to the
  workspace, the ruleset grants access to every sibling entry except the child
  directory containing the workspace.
  For `up -> ~/w/a` and a workspace at `~/w/a/b`, Landlock grants every entry in
  `~/w/a` except `b`.
  This enumeration has two fail-closed limitations: new top-level entries
  created directly inside `~/w/a` after the tool starts are unreachable, and
  tools cannot be granted permission to create new top-level entries directly in
  `~/w/a` because Landlock's directory-creation right applies to the entire
  directory subtree, including `b`.

Non-ancestor external mounts require no structural change to [RFD 075]'s sandbox
profiles beyond adding `approved_target` to the allowed path set.

The cooperative checks in this RFD ship independently of [RFD 075] and apply on
all platforms regardless of whether OS sandboxing is available.

### Platform notes

`std::fs::canonicalize`, `std::os::unix::fs::symlink`, and
`std::os::windows::fs::{symlink_dir, symlink_file}` are stable across supported
platforms.

On Windows, creating a symlink requires Developer Mode (Windows 10 1703+) or
administrator privileges.
When directory symlink creation in `--mount` fails with
`ERROR_PRIVILEGE_NOT_HELD`, JP falls back to creating an NTFS junction point via
the `junction` crate.
Because junction points support only directories with absolute target paths,
mounting a single file on Windows without Developer Mode fails with an explicit
error.

Reading and canonicalising existing symlinks on Windows requires no special
privileges, and `FsRule.external` itself is platform-independent.

OS-level sandbox enforcement follows [RFD 075]'s platform matrix: `sandbox-exec`
on macOS, Landlock on Linux, and cooperative enforcement only on Windows.

## Drawbacks

**Cross-conversation visibility.** Workspace symlinks sit in the workspace
directory tree, so every conversation in the workspace sees them in directory
listings even without a matching `external` grant.
A conversation without the grant receives a clean access denial if it tries to
read the symlink, so there is no security leak, but directory listings
accumulate entries as mounts are added.

**Windows file-symlink privileges.** Without Developer Mode or administrator
privileges, Windows users cannot mount individual files via `--mount`.
NTFS junction points cover directory mounts (such as mounting a forked
repository).

**Initial approval prompts for hand-authored rules.** A workspace with several
hand-authored `external = true` rules prompts once per rule on first use.
Subsequent runs reuse the stored approvals, and mounts created via `--mount`
seed the approval store without prompting.

**`--no-mount` marker collisions and inheritance freeze.** `--no-mount` removes
any `external = true` rule matching the target name, including hand-authored
rules, and its `strategy = "replace"` snapshot freezes `access.fs` inheritance
for affected tools until [RFD 070] replaces snapshot writes with negative
deltas.

**Ancestor mounts introduce a symlink cycle.** When a mount target contains the
workspace, the workspace is reachable again at `up/b`, `up/b/up/b`, and so on.
Access checks reject any path that re-enters the workspace through the mount,
and JP's own file walkers do not follow symlinks.
Third-party tools that follow symlinks during directory traversal will hit the
cycle; most stop on loop detection or the kernel symlink limit (`ELOOP`), though
naive walkers that resolve one level at a time may run slowly.
JP logs a warning trace when creating an ancestor mount.

**Version control exposure.** If a user commits a workspace symlink to Git or
another VCS, the symlink's target path is recorded in the commit history.
JP does not manage VCS state or `.gitignore` entries for mounts.

## Alternatives

### Named-mount table in conversation state

Store mounts in conversation config (`[[conversation.mounts]]`) separately from
`access.fs`, without creating workspace symlinks, and pass the mount table to
tools in `jp_tool::Context`.

Rejected because it duplicates `access.fs` with a second path-mapping mechanism,
requires extending the tool plugin protocol, and stores absolute host paths in
shared conversation state.
Workspace symlinks reuse normal filesystem paths and existing `FsRule` checks.

### Bounded targets via `follow_to = ["..."]`

Allow each `FsRule` to list permitted canonical target paths directly in config
instead of using TOFU approval.

Rejected because it puts machine-specific absolute paths into shared workspace
config, reintroducing the privacy and portability issues that symlinks avoid.
TOFU catches silent retargeting while keeping canonical paths in user-local
storage.

### Approval inside `Context::check_*`

Run the approval prompt inside `jp_tool::Context::check_*` rather than on the
host during policy compilation.

Infeasible because `jp_tool::Context` executes inside the tool subprocess, which
has no terminal access, no access to user-local storage, and no channel to emit
an inquiry.
In addition, the OS sandbox profile must be finalised before the tool subprocess
spawns.

### Per-resolution TOFU at tool-call time

Defer approval until a tool call resolves a symlink outside the workspace, then
pause the tool with `Outcome::NeedsInput` to prompt the user.

Rejected because the OS sandbox is configured at process spawn from a fixed list
of allowed paths and cannot be widened mid-execution.
Approving rules at policy compilation time gives both the cooperative checker
and the OS sandbox the same approved target set before the tool starts.

### Mount overlay separate from `access.fs`

Store `--mount` grants in a separate config namespace that compiles into
`AccessPolicy` alongside `access.fs`, so adding a mount never switches a tool's
`access.fs` from unset (unrestricted) to default-deny.

Rejected because a mount is a filesystem access grant, and splitting filesystem
grants across two config sections forces users to reason about two overlapping
models.
Seeding the tool's existing workspace or `'*'` rules when `--mount` writes to an
empty `access.fs` list (see [Default-deny
preservation](#default-deny-preservation)) preserves workspace access within a
single `access.fs` list.

### Unified attachment and tool access policy

Govern `--attach` through `access.fs` so one policy controls both LLM tool calls
and user attachments.

Rejected because `--attach` is a one-shot, user-initiated snapshot where typing
the CLI argument is itself the consent action, whereas `access.fs` governs
ongoing, LLM-chosen tool operations.
[RFD D03] handles external attachments directly.

### Absolute-path `access.fs` rules

Allow `access.fs` rules to specify absolute paths directly without a workspace
symlink.

Rejected because absolute paths in shared workspace config are not portable
across machines and require allowing the LLM to pass absolute paths in tool
arguments.

## Non-Goals

- **External attachments.** [RFD D03] defines `--attach` for paths outside the
  workspace.

- **Conversation-scoped mounts.** Mount symlinks live in the workspace tree and
  are visible in directory listings across all conversations in that workspace.
  Per-conversation filesystem isolation is deferred unless cross-conversation
  symlink clutter proves problematic.

- **CLI flag for unattended pre-approval.** Users running JP non-interactively
  can pre-seed `<user-workspace-storage>/approvals.json` directly.

- **File symlinks on Windows without Developer Mode.** Directory mounts fall
  back to NTFS junction points; file mounts on Windows require Developer Mode or
  administrator privileges.

- **VCS ignore management.** `--mount` creates the workspace symlink; whether to
  commit or ignore it in version control is left to the user.

- **Group-level tool access defaults.** An unscoped `--mount` expands at CLI
  time into individual rules on the currently enabled local tools.
  First-class tool-group access defaults belong in an extension to [RFD 076].

- **`external = true` on workspace-anchored rules.** Setting `external = true`
  on a path that canonicalises inside the workspace (such as `path = "."`) is
  rejected at policy compilation time.

## Risks and Open Questions

**Symlink-retargeting attack surface.** A collaborator could commit a symlink
change pointing an existing mount at a sensitive path (such as `fork ->
/etc/passwd`), which is easy to miss in a `git pull` diff.
TOFU mitigates this: policy compilation compares the symlink's current canonical
target against `approvals.json` and prompts before allowing the new target.

**Target-only approval scope.** Approvals bind `rule_path` to `canonical_target`
without recording capabilities.
If a config change widens capabilities on an already-approved mount (for
example, adding `write = true`), JP does not re-prompt.
This trade-off is deliberate: capability changes are visible in config diffs and
`jp config show`, whereas TOFU is designed to catch filesystem retargeting
outside config.

**Cross-conversation symlink clutter.** In workspaces with many mounts, every
conversation sees all mount symlinks when listing the workspace root.
If denied access attempts become noisy or waste tokens, conversation-scoped
mounts can be revisited.

**`--no-mount` marker collisions.** Identifying mounts solely by `external =
true` means `--no-mount` also deletes hand-authored external rules.
If users commonly mix hand-authored external rules with `--mount`, rules can
gain an explicit mount-origin field.

**Approval-store schema evolution.** `approvals.json` may later need expiration
timestamps, capability scopes, or new approval categories.
Keeping a single reader/writer module in `jp_cli` bounds migration cost.

## Implementation Plan

> [!NOTE]
> **Current status (PR 727).** This first slice ships cooperative enforcement
> for local filesystem tools:
>
> - **Phase 1 (pre-canonical invariant):** Done.
> - **Phase 2 (`FsRule.external`, rule-path canonicalisation, approved-target
>   boundary):** Done.
>   In-workspace rules match on their canonical workspace-relative path so an
>   in-workspace symlink cannot bypass a more specific rule; external rules keep
>   their lexical mount prefix and enforce `approved_target`.
> - **Phase 3 (approval lifecycle):** Partial.
>   The approval store, lookup, and `--mount` approval seeding are implemented.
>   Interactive TOFU prompts for hand-authored `external = true` rules are
>   deferred: unapproved or retargeted rules are dropped with a warning and
>   never granted silently.
> - **Phase 5 (`--mount`):** Done for `--mount` (parsing, symlink creation,
>   approval seeding, default-deny preservation, and expansion across enabled
>   local tools).
>   `--no-mount` is not yet implemented; removing a symlink and its persisted
>   config rules is manual until it lands.
>   The confirmation prompt for unscoped `--mount` across all tools is also
>   deferred: passing `--mount` on the CLI serves as consent, and `:rw` still
>   requires an explicit `TOOL:` prefix.
> - **Phase 4 (OS sandbox, [RFD 075]) and Phase 6 (Windows junction fallback):**
>   Not started.
>   Enforcement is cooperative only, and `net` and `env` rules are not yet
>   modelled in config.
>
> **Guarantees in this slice:** Config validation rejects `access` on `builtin`
> and `mcp` tools ([RFD 076]).
> An `access` config that fails compilation fails the tool invocation, and a
> policy whose rules are all dropped (due to unapproved or broken symlinks)
> remains default-deny rather than falling back to unrestricted workspace
> access.
> The in-tree filesystem tools check capabilities against the canonical target
> path, so an in-workspace symlink cannot bypass a deny rule.

### Phase 1: Pre-canonical invariant in `jp_tool`

Add the pre-canonicalisation check to `Context::check_*` to reject absolute
paths and `..`-escapes with a typed error before any filesystem I/O.
Update `FsAccessError` to distinguish out-of-workspace input paths from
out-of-workspace canonical targets, and document that `check_*` accepts only raw
workspace-relative paths.
This phase is independent and can merge on its own.

### Phase 2: `FsRule.external` field and amended rule-path canonicalisation

Add `external` to `jp_tool::FsRule` and `jp_config::FsRuleConfig`.
Amend [RFD 076]'s rule-path canonicalisation so rules with `external = true`
whose path canonicalises outside the workspace retain their `lexical_path` and
record `approved_target`.
Reject `external = true` on rules whose path canonicalises inside the workspace,
enforce the `approved_target` boundary during path matching, and drop broken
symlinks with a warning during policy compilation.

Depends on Phase 1.

### Phase 3: Host-side approval lifecycle

Define `<user-workspace-storage>/approvals.json` with its `mounts` array and
implement atomic temp-file-and-rename read/write helpers in `jp_cli`.
Run the approval check during `AccessConfig` to `AccessPolicy` conversion on the
host.
Reuse the terminal prompting UI from [RFD 005] and [RFD 028] to display affected
tools and capabilities without recording `InquiryRequest` or `InquiryResponse`
events in the conversation stream.
When no terminal is available, drop pre-existing unapproved rules with a warning
while allowing mounts seeded by `--mount` in the current invocation.

Depends on Phase 2.

### Phase 4: OS-sandbox profile generator

Update [RFD 075]'s sandbox profile generator to emit allow-entries for each
approved rule's `approved_target`, skipping unapproved rules, and add tests for
macOS and Linux profile generation.

Depends on Phase 3 and coordinates with [RFD 075].

### Phase 5: `--mount` and `--no-mount` CLI

Add `--mount` and `--no-mount` to `jp q`.
Implement `[TOOL:][NAME=]PATH[:MODE]` parsing, workspace-relative `NAME`
resolution, `NAME` derivation from bare `PATH` arguments, and the mode
restrictions.
Keep config mutation in `apply_cli_config` idempotent and separate from symlink
creation, approval seeding, and conversation persistence in `Query::run`.
Implement default-deny preservation when seeding rules and `--no-mount` cleanup
using `strategy = "replace"` config layers.

Depends on Phase 3.

### Phase 6: Platform-specific symlink creation

On Windows, fall back from directory symlink creation to NTFS junctions using an
optional `[target.'cfg(windows)'.dependencies]` dependency on `junction` in
`jp_cli`, and return a clear error when mounting a file without symlink
privileges.

Depends on Phase 5 and can merge in parallel with Phase 4.

## References

- [RFD 005]: First-class inquiry events.
  Provides the terminal prompting UI used for host-side mount approval without
  persisting `InquiryRequest` or `InquiryResponse` events (see [Approval
  lifecycle](#approval-lifecycle-host-side)).
- [RFD 028]: Structured inquiry system for tool questions.
  Shares the terminal prompting infrastructure with [RFD 005].
- [RFD 070]: Negative config deltas.
  Not required for v1; `--no-mount` uses `strategy = "replace"` snapshots
  instead.
- [RFD 075]: Tool sandbox and access policy.
  Consumes approved external mount rules when generating OS sandbox profiles.
- [RFD 076]: Tool access grants.
  Defines `FsRule`, rule-path canonicalisation, and path evaluation, which this
  RFD extends.
- [RFD 079]: Config sources and load order.
  Governs the physical layout of `<user-workspace-storage>`.
- [RFD D03]: External attachment URI scheme.
  Covers user-initiated external attachments (`--attach`), complementing tool
  mounts.
- Plugin path-and-hash approval (`crates/jp_cli/src/cmd/plugin/dispatch.rs`,
  `crates/jp_cli/src/cmd/plugin/registry.rs`): Existing precedent for the
  TOFU-with-re-approval pattern used here.

[RFD 005]: ../005-first-class-inquiry-events.md
[RFD 028]: ../028-structured-inquiry-system-for-tool-questions.md
[RFD 070]: ../070-negative-config-deltas.md
[RFD 075]: ../075-tool-sandbox-and-access-policy.md
[RFD 076]: ../076-tool-access-grants.md
[RFD 079]: ../079-config-sources-and-load-order.md
[RFD D03]: D03-external-attachment-uri-scheme.md
