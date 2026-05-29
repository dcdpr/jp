# RFD 076: Tool Access Grants

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-14
- **Required by**: [RFD 075], [RFD 078]

## Summary

This RFD adds a typed access policy to tool configuration that declares what
resources a tool can access.
The policy flows to tools as part of their runtime context.
Tools self-enforce the policy by checking grants before performing operations.
OS-level enforcement is tackled separately in [RFD 075].

## Motivation

Individual tools implement ad-hoc safeguards today — `unix_utils` applies a
`sandbox-exec` profile on macOS and runs with `clean_env: true`; the `fs_*`
tools join user-supplied paths against the workspace root.
These protections are local to each tool and are not expressible in
configuration.
There is no typed, declarative access policy in the host/tool contract that a
user's tool config can use to say "this tool may only write to
`.config/jp/tools`."
The only configurable safeguard is `RunMode::Ask`, which controls *whether* a
tool runs — not *what it can do* while running.

This creates two problems:

1. **Tool config can't scope access.** A tool-writing config that enables
   `fs_create_file` wants to grant write access to `.config/jp/tools` and read
   access to the rest of the workspace.
   Today there is no way to express this.
   The tool gets full access to everything.

2. **Tools can't provide good error messages for out-of-scope operations.** When
   a future OS-level sandbox denies a syscall, the tool gets a raw permission
   error with no context.
   If the tool knows its access policy, it can reject the operation early with a
   helpful message that names the configured grants and suggests how to fix the
   config.

This RFD solves the first problem and lays the groundwork for the second.
It establishes the access policy types and configuration surface that [RFD 075]
consumes for OS-level enforcement.
Getting the policy semantics right here is a prerequisite for 075 — the two
layers must agree on what each rule means.

## Design

### Configuration

A new `access` field on per-tool configuration declares resource grants.
Three resource types are supported: filesystem (`fs`), network (`net`), and
environment variables (`env`).

```toml
# tool config (e.g. in .jp/config.toml)

[conversation.tools.fs_create_file]
enable = true
run = "unattended"

[[conversation.tools.fs_create_file.access.fs]]
path = "."
read = true

[[conversation.tools.fs_create_file.access.fs]]
path = ".config/jp/tools"
read = true
write = true

[[conversation.tools.fs_modify_file.access.fs]]
path = "."
read = true

[[conversation.tools.fs_modify_file.access.fs]]
path = ".config/jp/tools"
read = true
write = true
```

When no `access` field is present, the tool has unrestricted workspace access
across all resource types (fs, net, env).
This preserves backward compatibility — existing tools and configs work without
changes.

Default-deny applies **per resource type**.
Declaring at least one rule in `access.fs` shifts filesystem access to
default-deny; `access.net` and `access.env` remain unrestricted until they, too,
have at least one rule.
This avoids the footgun where adding a single network grant silently denies
filesystem access.
A resource type is considered "declared" when its list contains at least one
rule after merging; absent or empty lists mean "unrestricted" for that type.

Config validation rejects `access` on tools whose finalized (post-merge) source
is `builtin` or `mcp`.
`source` is a required field, so a ToolConfig with `access` and no source is
already rejected at load — this check runs after all layers are merged so that
a layer that only adds `access` (without restating source) is allowed, and the
builtin/mcp rejection observes the effective source.
`access` is designed for the local subprocess contract: the policy is serialized
into the `Context` JSON that JP passes to tool binaries, and those binaries
check it before acting.
Builtin tools are in-process Rust code with their own configuration semantics,
and MCP tools run on external servers JP does not control.
Neither participates in the `access` surface of this RFD.
Silently accepting `access` on those sources would create false confidence in a
security-relevant field, so it is a hard error at config load.
If host-side semantics for builtin or MCP tools are added later (e.g., JP-side
enforcement for builtins, MCP argument proxying), a follow-up RFD will define
them.

#### Cross-layer merging

`access.fs`, `access.net`, and `access.env` are each a `MergeableVec` (from the
`jp_config` types module).
Default merge strategy is **append**: rules from later config layers are added
to the pool defined by earlier layers, and longest-prefix-match resolves which
rule applies to a given target.

```toml
# layer A (e.g. project default)
[[conversation.tools.fs_create_file.access.fs]]
path = "."
read = true

# layer B (e.g. user override)
[[conversation.tools.fs_create_file.access.fs]]
path = ".config/jp/tools"
read = true
write = true
```

Result: two `fs` rules, read on `.` and read+write on `.config/jp/tools`.

To replace rather than append, a config layer uses the `Merged` form with an
explicit strategy:

```toml
[conversation.tools.fs_create_file.access.fs]
strategy = "replace"
value = [
    { path = ".config/jp/tools", read = true, write = true },
]
```

This applies the standard jp\_config merge primitives uniformly — users already
familiar with `MergeableVec` semantics elsewhere (attachments, instructions,
sections) don't learn new rules for `access`.

### Rule evaluation

Each resource type defines its own specificity metric (see per-type sections
below).
When multiple rules match a target, the most specific rule wins — its
capabilities apply in full, without inheritance from less specific rules.
When multiple rules share the same specificity, the **last rule in the vector
wins**.
`MergeableVec`'s default append semantics mean later config layers append to
earlier ones, so the most recently declared rule takes precedence on ties.

### Filesystem rules

Each filesystem rule grants capabilities at a path prefix relative to the
workspace root.
The path `"."` matches the entire workspace.
Rule paths are literal: glob characters like `*` and `?` have no special meaning
and are treated as literal path segments.
Matching is component-aware, not byte-based — a rule `path = "src"` matches
`src/lib.rs` but not `src_generated/foo.rs`.

```toml
[[access.fs]]
path = "."
read = true

[[access.fs]]
path = ".config/jp/tools"
read = true
write = true

[[access.fs]]
path = ".env"
# all capabilities default to false — denies all access to .env
```

#### Capabilities

Five atomic capabilities and one alias:

| Field     | Description                                      |
| --------- | ------------------------------------------------ |
| `read`    | Read file contents, list directory entries       |
| `create`  | Create new files and directories                 |
| `update`  | Modify existing files (content changes, renames) |
| `delete`  | Remove files and directories                     |
| `execute` | Execute files as programs                        |
| `write`   | Alias for `create + update + delete`             |

The `write` alias sets the default for `create`, `update`, and `delete`.
Explicit atomic values override the alias:

```toml
# Full write access
write = true
# → create=true, update=true, delete=true

# Write without delete
write = true
delete = false
# → create=true, update=true, delete=false

# Create only, no update or delete
create = true
# → create=true, update=false, delete=false
```

All capabilities default to `false`.
The `write` field is settable in config and on the struct (via deserialization),
but there is no `write()` accessor on `FsRule` — consumers read the atomic
capabilities via `create()`, `update()`, and `delete()`, which apply the alias
expansion.

#### How tools map to capabilities

| Tool             | Checks                                         |
| ---------------- | ---------------------------------------------- |
| `fs_read_file`   | `read` on target                               |
| `fs_list_files`  | `read` on listed directories                   |
| `fs_grep_files`  | `read` on searched paths                       |
| `fs_create_file` | `create` (new) or `update` (exists)            |
| `fs_modify_file` | `update` on target(s)                          |
| `fs_delete_file` | `delete` on target                             |
| `fs_move_file`   | `delete` on source; `create` (new) or `update` |
|                  | (existing) on target                           |

#### Evaluation: longest prefix match

When multiple rules match a target path, the most specific rule wins.
Specificity is determined by path component count — more components means more
specific.
The winning rule applies in full; capabilities are not inherited from less
specific rules.

```toml
# Rule A: workspace root (0 components after normalization)
path = "."
read = true
write = true

# Rule B: src directory (1 component)
path = "src"
read = true

# Rule C: generated code (2 components)
path = "src/generated"
read = true
write = true
```

- `README.md` → matches Rule A → read + write
- `src/lib.rs` → matches Rule B → read only
- `src/generated/schema.rs` → matches Rule C → read + write
- `tests/main.rs` → matches Rule A → read + write

If no rule matches a target path, all capabilities are denied (default-deny).
See [Rule evaluation](#rule-evaluation) for tie-breaking.

Rules are self-contained — each rule is readable in isolation.
The cost is some repetition (Rule B must re-state `read = true`), but this
avoids subtle bugs from implicit inheritance where a less specific rule silently
grants capabilities that a more specific rule intended to restrict.

#### Rule path canonicalization

Rule paths themselves are canonicalized at **policy compilation time** using the
same algorithm as [Path evaluation](#path-evaluation) below.
Before the merged `AccessConfig` is converted to a `jp_tool::AccessPolicy` (and
before it crosses the wire to the tool), each rule's `path` is joined with
`ctx.root`, normalized, resolved through symlinks on each existing ancestor, and
stripped back to workspace-relative form.
Rules rejected during canonicalization (workspace escape, absolute path outside
`ctx.root`, symlink target outside workspace) fail config load with a clear
error naming the offending rule.

This ensures a rule `path = "src"` still matches target paths when `src/` is a
symlink to another directory inside the workspace — both rule and target are
reduced to the same canonical form before comparison.
It also means the cooperative layer and the OS sandbox ([RFD 075]) agree on the
resolved paths they enforce against.

#### Path evaluation

Filesystem rule evaluation operates on a **canonical, workspace-relative form**
of the target path.
Raw user input is never evaluated directly.
For any target path `input`, an implementation must:

1. If `input` is relative, join it with `ctx.root`.
   If it is absolute and not under `ctx.root`, reject as out-of-workspace.
2. Normalize the joined path, removing redundant separators and resolving `..`
   segments lexically.
   If normalization escapes `ctx.root`, reject as workspace-escape.
3. Resolve symlinks on the path and on each existing ancestor.
   If the resolved path is outside `ctx.root`, reject as workspace-escape.
   When the target does not yet exist (e.g., `create` on `new/dir/file.txt`
   where `new/` is also missing), resolve the nearest existing ancestor, then
   append the remaining relative suffix.
   The missing components are not resolved — they cannot contain symlinks that
   haven't been created yet.
4. Strip the `ctx.root` prefix to produce the workspace-relative canonical form.
5. Evaluate against `AccessPolicy` using longest-prefix match on that form.

The `action` field on `Context` (`Run` vs `FormatArguments`) does not gate these
checks at the host layer.
Whether and when a tool calls `check_*` is the tool's choice —
`FormatArguments` implementations typically return early before any I/O and
therefore never reach a check.
OS-level enforcement via [RFD 075] always applies regardless of action, so the
tool is treated as a potentially hostile black box: cooperative checks improve
error quality but are not the security boundary.

This canonical form is the **authoritative shape** of a filesystem rule.
[RFD 075] generates OS-level enforcement (sandbox-exec profiles, Landlock
rulesets) against resolved absolute paths — the same paths step 3 produces
before stripping the workspace prefix.
The cooperative layer (this RFD) and the OS layer see the same rule mean the
same thing.

JP's in-tree Rust tools link against the `jp_tool` crate, which provides
`Context::check_read` and friends (see [Runtime types](#runtime-types)) as a
reference implementation of the steps above.
Tools written in other languages receive `Context` as a JSON object and must
implement the same algorithm, or rely on [RFD 075]'s OS-level enforcement.
Diverging from the canonical form means the tool's self-check disagrees with the
OS sandbox — which is why the steps here are normative, not a convenience.

### Network rules

Network rules match against parsed URIs, not raw strings.
Each rule specifies a host (required), with optional scheme, port, and path
prefix:

```toml
[[access.net]]
host = "api.github.com"
allow = true

[[access.net]]
host = "api.github.com"
path_prefix = "/admin"
allow = false
```

Both rule and target hosts are normalized identically before matching: parsed
via `url::Host`, converted to ASCII (Punycode) form, then lowercased.
This means `host = "münchen.de"` in config matches target URIs whether they
arrive in Unicode or Punycode form.
Rule `host` values that fail to parse as a host are rejected at config load.

Evaluation then matches:

- `scheme`: exact match if specified; any scheme if absent.
- `host`: exact match, case-insensitive, after normalization.
  No string prefix matching — `api.github.com` does not match
  `api.github.com.evil.com`.
- `port`: exact match if specified; default-port-for-scheme if absent.
- `path_prefix`: segment-aware prefix match if specified; any path if absent.
  `/admin` matches `/admin/users` but not `/administration`.

Examples with the rules above:

- `https://api.github.com/repos` → matches first rule → allowed
- `https://api.github.com/admin/users` → matches second rule → denied
- `https://api.github.com.evil.com/` → no host match → denied
- `https://example.com` → no match → denied

Specificity for longest match is the sum of matched-component counts: presence
of `scheme` + presence of `port` + path segment count.
`host` is required and therefore constant across matching rules, so it does not
contribute to tie-breaking.
When two rules match a target, the more specific wins; ties resolve per [Rule
evaluation](#rule-evaluation).

Raw string prefix matching on URIs would accept `api.github.com.evil.com` as a
match for `api.github.com` and fail similarly on userinfo smuggling and port
variants.
Structured matching is the only defensible primitive here, and it must be
defined at the policy layer so [RFD 075]'s OS-level enforcement consumes the
same model without re-interpreting string rules.

### Environment variable rules

Environment variable rules match variable names either exactly or as a prefix,
distinguished by a trailing `*` in the `name` field.
A `name` without `*` is an exact match; `name = "AWS_*"` is a prefix match where
`*` is a sentinel (not part of the matched prefix).

```toml
[[access.env]]
name = "GITHUB_TOKEN"
read = true

[[access.env]]
name = "AWS_*"
read = true

[[access.env]]
name = "AWS_SECRET_ACCESS_KEY"
read = false
```

- `GITHUB_TOKEN` → exact match on first rule → allowed
- `GITHUB_TOKEN_LOG` → no match (first rule is exact, not prefix) → denied
- `AWS_REGION` → matches `AWS_*` prefix → allowed
- `AWS_SECRET_ACCESS_KEY` → matches exact deny → denied
- `HOME` → no match → denied

Choosing explicit `*` over a trailing-underscore convention makes
exact-vs-prefix a syntactic distinction rather than a convention.
A rule `name = "AWS_TOKEN"` unambiguously matches only `AWS_TOKEN`, never
`AWS_TOKEN_LOG`, regardless of the target variable's naming style.

Specificity is the byte length of the literal portion of `name` (excluding the
trailing `*` if present).
On ties at the same literal length, exact rules beat prefix rules — exact
`AWS_TOKEN` (9 bytes, exact) beats `AWS_TOKEN*` (9 bytes, prefix) on variable
`AWS_TOKEN`.
Component count is not used; environment variable names have no universal
separator.

A literal `*` cannot appear anywhere in a `name` value except as the trailing
sentinel.
This keeps the matching algorithm simple and is not a practical restriction:
POSIX env var names cannot contain `*`.

### Runtime types

The access policy types live in `jp_tool`, which defines the wire format between
the host (JP) and tool binaries.
`Context` is serialized to JSON and passed to each tool invocation.
The types here are the **finalized** policy — merging across config layers
happens host-side (see [Cross-layer merging](#cross-layer-merging)) before
serialization, so the tool sees a resolved `Vec<FsRule>`, not a `MergeableVec`.
`MergeableVec` lives in `jp_config` and is never exposed on the wire; this keeps
`jp_tool` free of config-layer dependencies.

A new `access` field carries the policy:

```rust
/// Contextual information available to a tool.
pub struct Context {
    pub root: Utf8PathBuf,
    pub action: Action,

    /// Access grants for this tool invocation.
    ///
    /// When `None`, the tool has unrestricted workspace access (backward
    /// compatibility). When `Some`, only explicitly granted capabilities
    /// are available — unmatched paths/resources are denied.
    #[serde(default)]
    pub access: Option<AccessPolicy>,
}

pub struct AccessPolicy {
    #[serde(default)]
    pub fs: Vec<FsRule>,
    #[serde(default)]
    pub net: Vec<NetRule>,
    #[serde(default)]
    pub env: Vec<EnvRule>,
}

pub struct NetRule {
    pub host: String,
    #[serde(default)]
    pub scheme: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub path_prefix: Option<String>,
    #[serde(default)]
    pub allow: bool,
}

pub struct EnvRule {
    pub name: String,
    #[serde(default)]
    pub read: bool,
}
```

`FsRule` uses private fields with accessor methods to encapsulate the `write`
alias expansion:

```rust
pub struct FsRule {
    path: Utf8PathBuf,
    #[serde(default)]
    read: Option<bool>,
    #[serde(default)]
    write: Option<bool>,
    #[serde(default)]
    create: Option<bool>,
    #[serde(default)]
    update: Option<bool>,
    #[serde(default)]
    delete: Option<bool>,
    #[serde(default)]
    execute: Option<bool>,
}

impl FsRule {
    pub fn path(&self) -> &Utf8Path { &self.path }
    pub fn read(&self) -> bool { self.read.unwrap_or(false) }
    pub fn create(&self) -> bool {
        self.create.unwrap_or_else(|| self.write.unwrap_or(false))
    }
    pub fn update(&self) -> bool {
        self.update.unwrap_or_else(|| self.write.unwrap_or(false))
    }
    pub fn delete(&self) -> bool {
        self.delete.unwrap_or_else(|| self.write.unwrap_or(false))
    }
    pub fn execute(&self) -> bool { self.execute.unwrap_or(false) }
}
```

`AccessPolicy` provides low-level evaluation on already-canonicalized paths:

```rust
#[derive(Debug, thiserror::Error)]
pub enum FsAccessError {
    /// Target is absolute and not under the workspace root.
    #[error("path is outside the workspace: {0}")]
    Outside(Utf8PathBuf),
    /// Target escapes the workspace via `..` or a symlink.
    #[error("path escapes the workspace: {0}")]
    Escape(Utf8PathBuf),
    /// No rule grants the requested capability.
    #[error("access denied: {capability} on {target}")]
    Denied {
        capability: &'static str,
        target: Utf8PathBuf,
        grants: Vec<Utf8PathBuf>,
    },
}

impl AccessPolicy {
    /// Evaluate a capability on a canonicalized, workspace-relative path.
    /// Callers must canonicalize first; see `Context::check_*`.
    pub fn can_read(&self, canonical: &Utf8Path) -> bool { /* ... */ }
    pub fn can_create(&self, canonical: &Utf8Path) -> bool { /* ... */ }
    pub fn can_update(&self, canonical: &Utf8Path) -> bool { /* ... */ }
    pub fn can_delete(&self, canonical: &Utf8Path) -> bool { /* ... */ }
    pub fn can_execute(&self, canonical: &Utf8Path) -> bool { /* ... */ }
}
```

`FsAccessError` is scoped to filesystem checks.
When net and env cooperative consumers land, they define their own error types
(`NetAccessError`, `EnvAccessError`) with shape-appropriate context — host/port
for net, variable name for env.
There is no unified `AccessError` union; each resource type carries the fields
that make sense for it.

For tools written in Rust that link `jp_tool` as a library, `Context` exposes a
reference implementation of the [Path evaluation](#path-evaluation) steps.
Each method canonicalizes `input`, then delegates to `AccessPolicy`.
On success it returns the resolved absolute path so the caller can perform the
operation without re-resolving.
When `self.access` is `None` (unrestricted), canonicalization still runs and
workspace-escape is still rejected — only the capability check is skipped.

```rust
impl Context {
    pub fn check_read(&self, input: &Utf8Path)
        -> Result<Utf8PathBuf, FsAccessError>;
    pub fn check_create(&self, input: &Utf8Path)
        -> Result<Utf8PathBuf, FsAccessError>;
    pub fn check_update(&self, input: &Utf8Path)
        -> Result<Utf8PathBuf, FsAccessError>;
    pub fn check_delete(&self, input: &Utf8Path)
        -> Result<Utf8PathBuf, FsAccessError>;
    pub fn check_execute(&self, input: &Utf8Path)
        -> Result<Utf8PathBuf, FsAccessError>;
}
```

`FsAccessError::Denied` carries the configured grants so callers can produce
helpful error messages that name what the tool is allowed to do, not just what
it was denied.

Tools written in other languages deserialize `Context` from JSON and must
implement the same algorithm to participate in cooperative enforcement.
They receive `access` as a JSON object matching the shape of `AccessPolicy`.
The path-evaluation steps are language-neutral; only the Rust convenience
methods are specific to `jp_tool`.

### Config-layer types

`jp_config` defines partial/mergeable counterparts for each rule type.
They share the on-wire shape of `jp_tool`'s types but derive `schematic::Config`
for the partial/merge/delta machinery, and use `MergeableVec` to participate in
the standard cross-layer merge model:

```rust
// crates/jp_config/src/conversation/tool/access.rs

#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct AccessConfig {
    #[setting(
        nested,
        partial_via = MergeableVec::<FsRuleConfig>,
        merge = vec_with_strategy,
    )]
    pub fs: Vec<FsRuleConfig>,

    #[setting(
        nested,
        partial_via = MergeableVec::<NetRuleConfig>,
        merge = vec_with_strategy,
    )]
    pub net: Vec<NetRuleConfig>,

    #[setting(
        nested,
        partial_via = MergeableVec::<EnvRuleConfig>,
        merge = vec_with_strategy,
    )]
    pub env: Vec<EnvRuleConfig>,
}

#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct FsRuleConfig {
    pub path: Utf8PathBuf,
    pub read: Option<bool>,
    pub write: Option<bool>,
    pub create: Option<bool>,
    pub update: Option<bool>,
    pub delete: Option<bool>,
    pub execute: Option<bool>,
}

// NetRuleConfig and EnvRuleConfig follow the same pattern:
// plain fields that deserialize from the TOML shape shown earlier, with
// Config-derived partials so they participate in layered merging.
```

`ToolConfig` gains an `access: Option<AccessConfig>` field with standard
`AssignKeyValue`, `PartialConfigDelta`, and `ToPartial` impls alongside the
existing fields like `options`.
After merging, the finalized `AccessConfig` is converted to
`jp_tool::AccessPolicy` at the boundary in `jp_llm::execute_local` — rule paths
are canonicalized (see [Rule path
canonicalization](#rule-path-canonicalization)) and hosts are normalized (see
[Network rules](#network-rules)) during conversion.
Only the finalized `AccessPolicy` is serialized into the `Context` JSON the tool
receives; `MergeableVec` and the partial types never cross the wire.

### Data flow

1. Tool config declares `access` on `[conversation.tools.*]` entries.
   After all config layers are merged, config load rejects `access` on tools
   whose finalized source is `builtin` or `mcp`.
2. Config layers merge per-subfield: `access.fs`, `access.net`, and `access.env`
   each merge independently as `MergeableVec`.
   Default strategy is append; replace requires explicit `strategy = "replace"`
   (see [Cross-layer merging](#cross-layer-merging)).
   The merged result is an `AccessConfig` with plain `Vec<_>` fields.
3. `jp_llm::execute_local()` converts the merged `AccessConfig` to a
   `jp_tool::AccessPolicy`: rule paths are canonicalized against `ctx.root`,
   hosts are normalized, and the resulting policy is serialized into the context
   JSON passed to the tool binary.
4. The tool binary deserializes `Context` with `access: Option<AccessPolicy>`.
5. Before performing an operation on a path, the tool runs it through the [Path
   evaluation](#path-evaluation) steps and checks the result against the policy.
   Rust tools using `jp_tool` call `ctx.check_read(path)` (or `check_create`,
   etc.); tools in other languages implement the algorithm directly.
   V1 cooperative enforcement is applied only to the `fs_*` tool family —
   subprocess-style tools like `unix_utils` rely on [RFD 075]'s OS-level sandbox
   for `access.fs` confinement.
6. On denial, the tool returns an error naming the denied capability and listing
   configured grants so the user knows what to change.

### Relationship to RFD 075

[RFD 075] introduces OS-level sandboxing for subprocess tools and consumes this
RFD's `AccessPolicy` types to generate platform-native sandbox profiles (macOS
`sandbox-exec`, Linux Landlock). 075 extends `AccessPolicy` with subprocess
`CommandRule` for spawn restrictions but does not modify the fs, net, or env
rule semantics defined here.

Two consequences for this design:

1. **This RFD defines the authoritative policy model.** Every rule type must
   have semantics precise enough that 075 can enforce them at the kernel level.
   This is why fs rules specify a canonical evaluation form (see [Path
   evaluation](#path-evaluation)) and why net rules use structured matching (see
   [Network rules](#network-rules)) instead of raw string prefix.
   Getting the semantics wrong here propagates into 075.
2. **The two layers always agree.** This RFD provides cooperative self-checks
   with helpful error messages; 075 provides mandatory OS enforcement of the
   same rules.
   A tool that self-checks and passes must also pass the OS sandbox, modulo
   platform-specific capability mapping — for example, Landlock distinguishes
   `create`/`update`/ `delete`, while `sandbox-exec` collapses them to
   `file-write*`.

| Concern       | This RFD (cooperative) | RFD 075 (OS-level)      |
| ------------- | ---------------------- | ----------------------- |
| Enforcement   | Tool self-checks       | OS sandbox              |
| Failure mode  | Helpful error message  | Raw permission denied   |
| Bypass        | Tool can ignore        | OS enforces             |
| Policy source | Defines `AccessPolicy` | Consumes `AccessPolicy` |

## Drawbacks

**Tools must opt into checking.** The policy is informational — a tool that
doesn't call `ctx.check_update()` (or equivalent) silently ignores the policy.
This is acceptable for JP's own tools (we'll add the checks) but means
third-party tools get no protection until [RFD 075]'s OS-level enforcement is in
place.

**Self-contained rule evaluation requires repetition.** A more specific rule
must re-state all capabilities, even those that a less specific rule already
grants.
This is a deliberate trade-off for clarity — every rule is readable in
isolation — but adds verbosity for complex policies.
A future `inherit` flag could reduce this if it becomes painful in practice.

**Merge strategy is configurable, not implicit.** `access.fs`, `access.net`, and
`access.env` use `MergeableVec`, which defaults to append and lets users opt
into replace, prepend, or dedup.
This avoids the "last-wins only" trap but means users writing strict policies
must understand the merge model — an append from an upstream layer cannot be
overridden by adding rules; it requires an explicit `strategy = "replace"`.

**Accessor types collapse unset and `false`.** `FsRule::read()` and the other
capability accessors return `bool`, not `Option<bool>`.
Combined with self-contained rule evaluation, there is no meaningful distinction
between "unset" and "explicitly false" at evaluation time.
The cost is that a future `inherit` flag, if added, would require an API change
to expose tri-state values to consumers.
See [Inheritance-based evaluation](#inheritance-based-evaluation) for the
variant this design rules out.

## Alternatives

### Access policy via `options`

Use the existing `options` mechanism (`IndexMap<String, JsonValue>`) to pass
access policy.
This avoids new config types but loses schema validation, forces tools to parse
`Value` manually, and conflates behavioral configuration with security policy.

### Inheritance-based evaluation

Instead of self-contained rule evaluation, more specific rules could inherit
unset capabilities from less specific rules.
This reduces repetition but requires `Option<bool>` to distinguish "not set"
from "explicitly false" at every level, and creates subtle bugs where a parent
rule silently grants capabilities the child intended to restrict.
Self-contained evaluation is simpler and safer.

## Non-Goals

- **OS-level enforcement.** This RFD does not sandbox tool subprocesses.
  [RFD 075] defines OS-level enforcement that consumes the types this RFD
  establishes.
- **MCP tool access control.** MCP tools run on external servers.
  The server's security is the server operator's responsibility.
  Config load rejects `access` on MCP and builtin tools (see Configuration).
- **Group-level access defaults and overrides.** `access` is per-tool in V1.
  The example in Motivation is intentionally repetitive.
  If RFDs 055–057 (group defaults/overrides) land, access grants should
  participate in the same group merge model as other tool config; defining that
  is out of scope here.

## Risks and Open Questions

1. **Env prefix overlap between explicit prefixes.** The largest class of
   over-grant surprises is eliminated by requiring explicit `*` for prefix
   matching (see [Environment variable rules](#environment-variable-rules)) —
   `AWS_TOKEN` never silently matches `AWS_TOKEN_LOG`.
   Residual overlaps between two prefix rules remain possible: `AWS_SEC*` (7
   bytes) and `AWS_SECRET_*` (11 bytes) both match `AWS_SECRET_KEY`; the longer
   wins.
   Documented in guidance; revisit if it proves painful in practice.

2. **Symlink resolution cost.** Every fs check resolves symlinks via
   `canonicalize`, which performs filesystem lookups.
   For tools that check many paths (e.g., `fs_grep_files`), this adds per-path
   syscalls.
   The alternative — lexical-only checks — would diverge from [RFD 075]'s OS
   enforcement and create a policy gap, which is worse.
   If measurable overhead appears, a cache keyed on `(dev, inode)` can be added.

## Implementation Plan

V1 defines the shared `fs`, `net`, and `env` rule types in `jp_tool` and
enforces `fs` cooperatively across the `fs_*` tool family.
`net` and `env` consumers (e.g., `web_fetch` network checks, `unix_utils` env
forwarding) are deliberately not part of V1 — enforcing them well requires the
consuming tool in scope for design, not after.
[RFD 075] reuses the same policy model for OS-level enforcement across all three
rule types.

### Phase 1: Types and evaluation in `jp_tool`

Add `AccessPolicy`, `FsRule`, `NetRule`, `EnvRule`, and `FsAccessError` to
`jp_tool`.
Implement the path-canonicalization helper (reused by tools for target paths and
by the host for rule paths at `AccessConfig` → `AccessPolicy` conversion) and
`Context::check_*` methods.
Implement structured net matching (scheme/host/port/path\_prefix) with
`url::Host` normalization for both rule and target hosts, and explicit-`*` env
matching with literal-length specificity.
Add `access: Option<AccessPolicy>` to `Context` with `#[serde(default)]`.
Unit tests for evaluation logic covering workspace escape, symlink resolution,
host normalization (including IDN/Punycode), port defaulting, and
exact-vs-prefix env ties.

No dependency.
Can merge independently.

### Phase 2: Config types in `jp_config`

Add `AccessConfig`, `FsRuleConfig`, `NetRuleConfig`, `EnvRuleConfig` in
`jp_config` with `MergeableVec` wrappers and the standard partial/delta/
`ToPartial` impls.
Add `access: Option<AccessConfig>` field to `ToolConfig` and an accessor on
`ToolConfigWithDefaults`.
Implement the `AccessConfig` → `jp_tool::AccessPolicy` conversion, which
canonicalizes rule paths against `ctx.root` (rejecting workspace-escape at
config load) and normalizes rule hosts.
Post-merge config validation rejects `access` on tools whose finalized source is
`builtin` or `mcp` with a clear error.

Depends on Phase 1.

### Phase 3: Plumbing in `jp_llm`

Include access policy in the JSON context passed to tool commands in
`execute_local()` and the `FormatArguments` path.

Depends on Phase 2.

### Phase 4: `fs_*` enforcement

Replace ad-hoc path joining in the `fs_*` tool family with `ctx.check_read()`,
`check_create()`, `check_update()`, `check_delete()`, and `check_execute()`.
Return clear error messages naming the denied capability and listing configured
grants.

`unix_utils` is deliberately **out of scope** for V1 `access.fs` grant
enforcement.
It invokes subprocesses that read files opaquely (e.g., `sort /etc/passwd`), and
per-argument path analysis fights the subprocess model.
V1 only updates `unix_utils`'s existing argument scanner to delegate to the
canonicalization helper for workspace-escape detection — matching today's
behavior, just with shared code.
Full enforcement of `access.fs` on subprocess-spawned reads is [RFD 075]'s
responsibility via OS-level sandboxing.

Depends on Phase 3.

## References

- [RFD 075] — OS-level sandboxing for subprocess tools.
  Consumes this RFD's `AccessPolicy` types to generate platform-native sandbox
  profiles and extends them with subprocess `CommandRule` spawn restrictions.
- [RFD 016] — WASM plugin architecture.
  Shares the same general capability-based model (filesystem, network, commands)
  but uses different field names and a simpler `allow`/`writable` filesystem
  model.
  A future RFD may migrate RFD 016's WASM plugin config to consume
  `AccessPolicy` directly.
- [RFD 042] — Tool options.
  Established the `options` mechanism; this RFD explains why access policy is a
  first-class field instead.
- [Deno security model] — Inspiration for the grant-based, default-deny
  permission model.

[Deno security model]: https://docs.deno.com/runtime/fundamentals/security/
[RFD 016]: 016-wasm-plugin-architecture.md
[RFD 042]: 042-tool-options.md
[RFD 075]: 075-tool-sandbox-and-access-policy.md
[RFD 078]: 078-tool-config-mutation.md
