# RFD 076: Tool Access Grants

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-14
- **Required by**: [RFD 075], [RFD 078]
- **Summary**: Adds typed access policy grants for tools to declare and enforce
  what workspace resources they can access.

## Summary

This RFD adds a typed `access` policy to tool configuration that declares which
filesystem, network, and environment resources a tool may access.
The resolved policy is passed to tools in their runtime context so they can
validate grants before performing operations.
OS-level enforcement of the same policy is defined in [RFD 075].

## Motivation

Individual tools implement ad-hoc safeguards today: `unix_utils` applies a
`sandbox-exec` profile on macOS and runs with `clean_env: true`, and the `fs_*`
tools join user-supplied paths against the workspace root.
These protections are hardcoded per tool rather than expressed in configuration.
The host-tool contract has no declarative access policy that lets a
configuration restrict a tool to writing only under `.config/jp/tools`.
The only configurable safeguard is `RunMode::Ask`, which controls *whether* a
tool runs, not *what it can do* while running.

This creates two problems:

1. **Tool configuration cannot scope access.** A configuration that enables
   `fs_create_file` for authoring tools may want to grant write access to
   `.config/jp/tools` and read-only access to the rest of the workspace.
   Without scoped grants, the tool gets full access to the workspace.

2. **Tools cannot explain out-of-scope errors.** When an OS-level sandbox denies
   a syscall, the tool receives a raw permission error with no context.
   A tool that knows its access policy can reject the operation early with an
   error message that names the configured grants and suggests how to update the
   configuration.

This RFD solves the first problem and provides the policy model for the second.
It defines the configuration surface and runtime policy types that [RFD 075]
consumes for OS-level enforcement, ensuring that cooperative tool checks and the
OS sandbox agree on what each rule permits.

## Design

### Configuration

A new `access` field on tool configuration declares resource grants in two
scopes: on an individual tool, and on the `[conversation.tools.'*']` defaults
block (see [Scope inheritance](#scope-inheritance)).
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
across all resource types (`fs`, `net`, `env`), preserving backward
compatibility for existing tools and configurations.

Default-deny applies **per resource type**.
A resource type is restricted only when its rule list contains at least one rule
after merging; an absent or empty list leaves that resource type unrestricted.
Declaring a rule in `access.fs` switches filesystem access to default-deny while
leaving `access.net` and `access.env` unrestricted, so adding a network grant
does not silently block filesystem access.

`access` applies only to `local` subprocess tools: JP serializes the policy into
the `Context` JSON passed to the tool binary, which validates grants before
acting.
Builtin tools run in-process with their own configuration semantics, and MCP
tools run on external servers outside JP's control.
Setting `access` on a named tool whose finalized (post-merge) `source` is
`builtin` or `mcp` is a hard error at config load, avoiding false confidence in
a security-relevant setting.
Validation runs after all config layers merge so an override layer can add
`access` without restating `source` (which `ToolConfig` already requires once
finalized).
Host-side enforcement for builtin or MCP tools is left to a follow-up RFD.
For grants that reach a tool through the `*` defaults, see [Declared versus
inherited grants](#declared-versus-inherited-grants).

#### Cross-layer merging

`access.fs`, `access.net`, and `access.env` each use `MergeableVec` from
`jp_config`.
The default merge strategy is **append**: rules from later config layers are
appended to those from earlier layers, and longest-prefix match resolves which
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

The merged policy contains two `fs` rules: read on `.` and read+write on
`.config/jp/tools`.

To replace rather than append, a config layer uses the `Merged` form with an
explicit strategy:

```toml
[conversation.tools.fs_create_file.access.fs]
strategy = "replace"
value = [
    { path = ".config/jp/tools", read = true, write = true },
]
```

This reuses the standard `jp_config` `MergeableVec` semantics already used by
attachments, instructions, and sections.

#### Scope inheritance

`access` can be declared in two scopes:

- **Per-tool**: `[conversation.tools.<name>.access]`.
- **Defaults**: `[conversation.tools.'*'.access]`, which applies to every local
  tool that declares no `access` block of its own.

Scope resolution uses **replace** rather than field-by-field merge: a tool that
declares any `access` rules ignores the `*` block entirely.
A tool's `access` block is therefore a complete specification of its policy.

Appending `*` rules to per-tool rules was rejected because it leaves inherited
grants active without showing them in the tool's own configuration and makes
narrowing awkward: revoking a capability granted in `*` would require adding a
longer-prefix rule with capabilities set to `false` to shadow the broader grant
(the same hazard described in [Inheritance-based
evaluation](#inheritance-based-evaluation)).
Replacing the whole block requires a tool that extends the defaults to restate
them, keeping the full policy auditable in one place.

Cross-layer merging and scope resolution operate on independent axes:

| Axis                             | Rule                                                     |
| -------------------------------- | -------------------------------------------------------- |
| Between scopes (`*` vs per-tool) | Replace: the most specific non-empty scope wins in full  |
| Between layers within one scope  | Append (see [Cross-layer merging](#cross-layer-merging)) |

`*` and each named tool accumulate rules in separate pools across config layers
(where `strategy = "replace"` resets only that scope's pool).
After layer merging completes, a tool uses its own pool if non-empty, or falls
back to the `*` pool.

Replacement applies to the entire `access` block, not per resource type.
A scope is non-empty when any of `fs`, `net`, or `env` contains at least one
rule, and the winning scope supplies all three lists.
A tool that declares only `access.fs` rules therefore discards `access.env` and
`access.net` rules from `*`, reverting to unrestricted environment and network
access unless it restates them.

Conversely, adding a single rule under `[conversation.tools.'*'.access]`
switches that resource type to default-deny for every local tool without its own
`access` block.
For example, a `*` block that grants only `read = true` on `"."` revokes write
access for all such tools.

#### Declared versus inherited grants

Config validation only rejects `access` when it is **declared on a named**
`builtin` or `mcp` tool.
Grants inherited from `[conversation.tools.'*'.access]` are *skipped* for
`builtin` and `mcp` tools rather than rejected.

A user who writes `[conversation.tools.some_mcp_tool.access]` targets that tool
explicitly and would otherwise expect the policy to apply.
By contrast, `*` sets defaults for local tools without targeting individual
builtin or MCP tools; rejecting `*` whenever a builtin or MCP tool is configured
would make `[conversation.tools.'*'.access]` unusable in any workspace with
builtins or an MCP server.

The same distinction applies to any future scope broader than a single tool
(such as tool groups in RFDs 055-057): grants declared on a group apply to the
local tools in that group and skip the rest.

### Rule evaluation

Each resource type defines its own specificity metric.
When multiple rules match a target, the most specific rule wins, and its
capabilities apply in full without inheriting from less specific rules.
When matching rules have equal specificity, the **last rule in the vector
wins**.
Because `MergeableVec` appends rules from later config layers after earlier ones
by default, later layers take precedence on ties.

### Filesystem rules

Each filesystem rule grants capabilities at a path prefix relative to the
workspace root, where `"."` matches the entire workspace.
Rule paths are literal: glob characters such as `*` and `?` have no special
meaning.
Matching compares path components rather than byte prefixes, so `path = "src"`
matches `src/lib.rs` but not `src_generated/foo.rs`.

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

A filesystem rule supports five atomic capabilities and one alias:

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
Although `write` can be set in configuration and deserialized onto `FsRule`,
`FsRule` exposes no `write()` accessor; callers read the atomic `create()`,
`update()`, and `delete()` methods, which expand the `write` alias.

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

When the target is a directory, mutating checks apply to the entire subtree, and
when `fs_delete_file` or `fs_move_file` removes an empty parent directory, that
parent is checked separately (see [Operations that reach a
subtree](#operations-that-reach-a-subtree)).

#### Evaluation: longest prefix match

When multiple rules match a target path, the rule with the most path components
wins and applies in full without inheriting capabilities from shorter prefixes.

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

If no rule matches a target path, all capabilities are denied.
Ties at equal component count resolve as described in [Rule
evaluation](#rule-evaluation).

Because rules are self-contained, each rule can be read in isolation.
Rule B must restate `read = true`, but a broader rule can never silently grant
capabilities that a narrower rule intended to revoke.

#### Operations that reach a subtree

Longest-prefix match evaluates a single path, but mutating a directory affects
every entry beneath it.
Checking only the directory path when renaming or deleting a directory would
bypass more specific rules that deny access to paths inside that directory.

Mutating operations on a directory (`create`, `update`, and `delete`) therefore
perform a subtree check.
A subtree check permits the capability only when the winning rule for the target
directory grants it **and** no rule under that directory denies it.
Each descendant rule path is evaluated through full longest-prefix and
tie-breaking resolution rather than inspected in isolation, so a deny rule
overridden by a later config layer does not block the operation.

```toml
[[access.fs]]
path = "."
read = true
write = true

[[access.fs]]
path = "docs/ticket"
read = true
write = false
```

- `docs/notes.md` → `delete` granted: the root rule wins, and no descendant
  rule denies `delete`.
- `docs/ticket/T-02wt0kx-fix-the-header.md` → `delete` denied: the
  `docs/ticket` rule wins.
- `docs` (directory) → `delete` denied: the root rule grants `delete` on
  `docs`, but the `docs/ticket` rule denies `delete` on a path inside `docs`.

`read` on a directory only lists its immediate entries, and `execute` targets a
single file, so both use a single-path check.
A target path that does not exist yet is not a directory and also uses a
single-path check, with two additional cases in tool operations:

- **Moving a directory checks the destination subtree.** Moving a directory
  places all of its contents under the destination path, even when the
  destination does not exist yet.
  `fs_move_file` therefore performs a subtree check on the destination whenever
  the source is a directory.
- **Cleaning up an empty parent directory requires its own `delete` grant.**
  When `fs_delete_file` or `fs_move_file` leaves the source's parent directory
  empty, it removes that parent directory only if `delete` is also permitted on
  the parent.
  If a policy grants `delete` on files inside a directory but denies `delete` on
  the directory itself, the empty directory is left in place and the file
  operation still succeeds.

#### Rule path canonicalization

Rule paths are canonicalized when converting the merged `AccessConfig` into a
`jp_tool::AccessPolicy`, using the same algorithm as [Path
evaluation](#path-evaluation).
Before the policy is serialized to the tool, each rule's `path` is joined with
`ctx.root`, lexically normalized, resolved through symlinks on every existing
ancestor, and stripped back to a workspace-relative path.
Any rule path that escapes the workspace (via an absolute path outside
`ctx.root`, `..` segments, or a symlink pointing outside `ctx.root`) fails
config load with an error naming the offending rule.

Canonicalizing rule paths ensures that `path = "src"` still matches targets when
`src/` is a symlink to another directory inside the workspace, because both rule
and target paths are reduced to the same canonical form before comparison.
It also ensures that cooperative checks and the OS sandbox ([RFD 075]) enforce
against the same resolved paths.

#### Path evaluation

Filesystem rules are evaluated against a **canonical, workspace-relative**
target path, never against raw user input.
For any target path `input`, an implementation must:

1. If `input` is relative, join it with `ctx.root`.
   If it is absolute and not under `ctx.root`, reject it as outside the
   workspace.
2. Lexically normalize the joined path by removing redundant separators and
   resolving `..` segments.
   If normalization escapes `ctx.root`, reject it as a workspace escape.
3. Resolve symlinks on the path and each existing ancestor.
   If the resolved path leaves `ctx.root`, reject it as a workspace escape.
   When the target does not yet exist (for example, `create` on
   `new/dir/file.txt` where `new/` is also missing), resolve the nearest
   existing ancestor and append the remaining relative suffix; non-existent
   components cannot contain symlinks.
4. Strip the `ctx.root` prefix to produce the workspace-relative canonical path.
5. Evaluate the canonical path against `AccessPolicy` using longest-prefix
   match, or a subtree check for mutating directory operations (see [Operations
   that reach a subtree](#operations-that-reach-a-subtree)).

The host passes the policy regardless of `Context::action` (`Run` or
`FormatArguments`).
Tools choose when to call `check_*`; `FormatArguments` implementations typically
return before performing I/O and never run a check.
OS-level enforcement in [RFD 075] applies to every invocation regardless of
action, treating the tool as an untrusted black box while cooperative checks
provide actionable error messages.

This canonical form is normative.
[RFD 075] generates `sandbox-exec` profiles and Landlock rulesets from the
resolved absolute paths produced in step 3 (before stripping `ctx.root`), and
accounts for subtree denials so the kernel sandbox never permits a directory
mutation that a descendant rule denies.
In-tree Rust tools use `Context::check_read` and related helpers in `jp_tool`
(see [Runtime types](#runtime-types)); tools written in other languages must
implement the same steps so their self-checks match the OS sandbox.

### Network rules

Network rules match against parsed URIs rather than raw strings.
Each rule requires a `host` and accepts an optional `scheme`, `port`, and
`path_prefix`:

```toml
[[access.net]]
host = "api.github.com"
allow = true

[[access.net]]
host = "api.github.com"
path_prefix = "/admin"
allow = false
```

Rule and target hosts are normalized identically before comparison: parsed with
`url::Host`, converted to ASCII (Punycode), and lowercased.
A rule with `host = "münchen.de"` therefore matches target URIs in either
Unicode or Punycode form.
Rule `host` values that fail to parse are rejected at config load.

A rule matches a target URI when all four components match:

- `scheme`: exact match if specified; any scheme if absent.
- `host`: exact match after normalization, never a string prefix
  (`api.github.com` does not match `api.github.com.evil.com`).
- `port`: exact match if specified; the scheme's default port if absent.
- `path_prefix`: segment-aware prefix match if specified (`/admin` matches
  `/admin/users` but not `/administration`); any path if absent.

With the example rules above:

- `https://api.github.com/repos` → matches first rule → allowed
- `https://api.github.com/admin/users` → matches second rule → denied
- `https://api.github.com.evil.com/` → no host match → denied
- `https://example.com` → no match → denied

Rule specificity is the sum of matched components: `1` if `scheme` is set, plus
`1` if `port` is set, plus the number of path segments in `path_prefix`.
Because `host` is required on every rule, it does not affect relative
specificity.
The most specific matching rule wins, with ties broken as described in [Rule
evaluation](#rule-evaluation).

Matching parsed URI components avoids the pitfalls of string-prefix matching on
URLs, such as subdomain spoofing (`api.github.com.evil.com`), userinfo
smuggling, and port ambiguity, and gives [RFD 075] a structured model for
OS-level network enforcement.

### Environment variable rules

Environment variable rules match variable names either exactly or by prefix.
A trailing `*` in `name` marks a prefix rule (`name = "AWS_*"` matches any
variable starting with `AWS_`), while a `name` without `*` matches only that
exact variable name.
A `*` is allowed only as the final character of `name`, which is no practical
restriction because POSIX environment variable names cannot contain `*`.

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

Requiring an explicit trailing `*` rather than inferring prefixes from a
trailing underscore ensures that `name = "AWS_TOKEN"` matches only `AWS_TOKEN`,
never `AWS_TOKEN_LOG`.

Because environment variable names have no universal delimiter, specificity is
the byte length of the literal portion of `name` (excluding any trailing `*`).
When an exact rule and a prefix rule have the same literal length, the exact
rule wins: `AWS_TOKEN` (9 literal bytes, exact) beats `AWS_TOKEN*` (9 literal
bytes, prefix) for `AWS_TOKEN`.

Matching is case-sensitive so a checked-in `.jp/config.toml` evaluates
identically across platforms.
However, case-sensitive matching alone does not prevent bypasses on operating
systems with case-insensitive environment variables.
On Windows, if `*` grants read and `GITHUB_TOKEN` denies it, a lookup for
`github_token` misses the exact deny, matches `*`, and resolves to
`GITHUB_TOKEN` in the OS environment.
Canonicalizing only the requested variable name is also insufficient if the deny
rule itself was written in a different case.

> [!NOTE]
> **Open question.** Case-insensitive platforms need a deny-preserving rule
> before the first `env` consumer ships, either by canonicalizing both request
> and rule names to a single case or by matching deny rules case-insensitively
> while keeping grants case-sensitive.
> The first `env` consumer will settle this choice.

### Runtime types

The runtime access policy types live in `jp_tool`, which defines the JSON wire
format between JP and tool binaries.
Config-layer merging happens on the host before serialization (see [Cross-layer
merging](#cross-layer-merging)), so tools receive a finalized `AccessPolicy`
with plain `Vec` fields rather than `MergeableVec`, keeping `jp_tool`
independent of `jp_config`.

`Context` carries the policy in an optional `access` field:

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
    pub fn permits(&self, capability: Capability, canonical: &Utf8Path) -> bool { /* ... */ }

    /// The same question asked of the path and everything beneath it.
    pub fn permits_subtree(&self, capability: Capability, canonical: &Utf8Path) -> bool { /* ... */ }

    /// The decision every consumer goes through: a mutating capability on a
    /// directory is answered for the subtree, everything else for the path.
    pub fn permits_entry(
        &self,
        capability: Capability,
        canonical: &Utf8Path,
        is_dir: bool,
    ) -> bool { /* ... */ }
}
```

`FsAccessError` is specific to filesystem checks.
Future cooperative `net` and `env` consumers will define their own error types
(`NetAccessError`, `EnvAccessError`) with resource-specific fields (host and
port for `net`, variable name for `env`) rather than sharing a combined
`AccessError` enum.

For Rust tools that link `jp_tool`, `Context` implements the [Path
evaluation](#path-evaluation) steps directly.
Each `check_*` method canonicalizes `input`, delegates to `AccessPolicy`, and
returns the resolved absolute path so the caller can perform the operation
without resolving the path a second time.
When `self.access` is `None`, `check_*` still canonicalizes the path and rejects
workspace escapes, skipping only the capability check:

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

`FsAccessError::Denied` includes the configured `grants` so callers can report
which paths the policy permits alongside the denied target.
Tools in other languages deserialize `access` from the `Context` JSON and
implement the same language-neutral evaluation steps.

### Config-layer types

`jp_config` defines mergeable config types that mirror the serialized shape of
the `jp_tool` types, deriving `schematic::Config` and wrapping each rule list in
`MergeableVec`:

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

`ToolConfig` and `ToolsDefaultsConfig` each gain an `access:
Option<AccessConfig>` field with `AssignKeyValue`, `PartialConfigDelta`, and
`ToPartial` implementations.
Neither type merges `access` in `FillDefaults`: because scope resolution
replaces the whole block rather than filling individual fields like `style`,
`ToolConfigWithDefaults::access()` resolves the winning scope directly (matching
`run()` and `result()`).
In `jp_llm::execute_local`, the finalized `AccessConfig` is converted into a
`jp_tool::AccessPolicy`, canonicalizing rule paths (see [Rule path
canonicalization](#rule-path-canonicalization)) and normalizing hosts (see
[Network rules](#network-rules)) before serializing `Context`.

### Data flow

1. Tool configuration declares `access` on `[conversation.tools.<name>]` or on
   the `[conversation.tools.'*']` defaults.
   Across config layers, `access.fs`, `access.net`, and `access.env` merge
   independently within each scope as `MergeableVec` (appending by default, or
   replacing with `strategy = "replace"`; see [Cross-layer
   merging](#cross-layer-merging)).
2. After all layers merge into a finalized `AccessConfig` per scope, config
   validation rejects `access` declared on any named tool whose finalized
   `source` is `builtin` or `mcp`.
   Scope resolution then selects each local tool's policy: the tool's own
   `AccessConfig` if non-empty, or the `*` policy otherwise (see [Scope
   inheritance](#scope-inheritance)).
   Builtin and MCP tools ignore the `*` policy.
3. `jp_llm::execute_local()` converts the resolved `AccessConfig` to a
   `jp_tool::AccessPolicy`, canonicalizing rule paths against `ctx.root` and
   normalizing hosts, and serializes it into the `Context` JSON passed to the
   tool binary.
4. The tool binary deserializes `Context` with `access: Option<AccessPolicy>`.
5. Before operating on a path, the tool runs [Path evaluation](#path-evaluation)
   (`ctx.check_read(path)`, `check_create`, and so on in Rust tools).
   In V1, cooperative enforcement applies to the `fs_*` tool family;
   subprocess-spawning tools such as `unix_utils` rely on [RFD 075]'s OS-level
   sandbox for `access.fs` confinement.
6. On denial, the tool returns an error naming the denied capability and listing
   the configured grants.

### Relationship to RFD 075

[RFD 075] introduces OS-level sandboxing for subprocess tools and consumes
`AccessPolicy` to generate platform-native sandbox profiles (macOS
`sandbox-exec` and Linux Landlock).
RFD 075 extends `AccessPolicy` with subprocess `CommandRule` spawn restrictions
without changing the `fs`, `net`, or `env` rule semantics defined here.

This division has two consequences for the design:

1. **This RFD defines the authoritative policy model.** Every rule type needs
   semantics precise enough for RFD 075 to enforce at the kernel level.
   That is why `fs` rules specify canonical path evaluation (see [Path
   evaluation](#path-evaluation)) and `net` rules match parsed URI components
   (see [Network rules](#network-rules)) rather than raw string prefixes.
2. **Both enforcement layers agree.** This RFD provides cooperative self-checks
   with descriptive error messages, while RFD 075 enforces the same rules in the
   OS kernel.
   Any operation that passes a tool's self-check also passes the OS sandbox, up
   to platform-specific capability granularity (for example, Landlock
   distinguishes `create`, `update`, and `delete`, whereas `sandbox-exec`
   collapses them into `file-write*`).

| Concern       | This RFD (cooperative) | RFD 075 (OS-level)      |
| ------------- | ---------------------- | ----------------------- |
| Enforcement   | Tool self-checks       | OS sandbox              |
| Failure mode  | Helpful error message  | Raw permission denied   |
| Bypass        | Tool can ignore        | OS enforces             |
| Policy source | Defines `AccessPolicy` | Consumes `AccessPolicy` |

## Drawbacks

**Tools must opt into checking.** Cooperative enforcement is advisory: a tool
that does not call `ctx.check_update()` (or an equivalent check) ignores the
policy.
This is sufficient for JP's in-tree tools, which call these helpers, but
third-party tools receive no enforcement until [RFD 075]'s OS-level sandbox is
in place.

**Self-contained rule evaluation requires repetition.** A more specific rule
must restate every capability it grants, even those already granted by a broader
rule.
This keeps each rule readable in isolation at the cost of verbosity in complex
policies.
A future `inherit` flag could reduce this repetition if needed.

**Tightening a policy across layers requires an explicit strategy.** Because
`access.fs`, `access.net`, and `access.env` use `MergeableVec` (which defaults
to append and supports replace, prepend, and dedup), a later config layer cannot
drop broad grants from an earlier layer simply by adding narrower rules; it must
set `strategy = "replace"`.

**Accessor types collapse unset and `false`.** `FsRule::read()` and the other
capability accessors return `bool` rather than `Option<bool>`, because
self-contained evaluation does not distinguish an omitted capability from
`false`.
Supporting a future `inherit` flag would require changing those accessors to
expose tri-state values (see [Inheritance-based
evaluation](#inheritance-based-evaluation)).

## Alternatives

### Access policy via `options`

Passing access grants through the existing untyped `options` map
(`IndexMap<String, JsonValue>`) would avoid new config types, but it would lose
schema validation, force every tool to parse raw JSON values manually, and mix
behavioral tool options with security policy.

### Inheritance-based evaluation

More specific rules could inherit unset capabilities from less specific rules
instead of being evaluated in isolation.
That reduces repetition, but it requires `Option<bool>` at every evaluation
layer to distinguish "unset" from `false` and makes it easy for a parent rule to
silently grant capabilities that a child rule intended to restrict.

## Non-Goals

- **OS-level enforcement.** This RFD does not sandbox tool subprocesses; [RFD
  075] consumes `AccessPolicy` for OS-level enforcement.
- **MCP and builtin tool access control.** MCP tools run on external servers
  managed by the server operator, and builtin tools run in-process.
  Config validation rejects `access` declared on a named MCP or builtin tool,
  and `*` defaults skip them (see [Configuration](#configuration)).
- **Group-level access defaults and overrides.** `access` is configured per tool
  or on the `*` defaults; there is no per-group scope, so tools sharing the same
  grants repeat them in configuration.
  If RFDs 055-057 (tool group defaults and overrides) land, a group will form an
  intermediate scope between `*` and the named tool using the same whole-block
  replacement and declared-versus-inherited rules.
  Defining the group merge model is out of scope here.

## Risks and Open Questions

1. **Overlapping `env` prefix rules.** Requiring an explicit trailing `*` for
   prefix matching (see [Environment variable
   rules](#environment-variable-rules)) prevents `AWS_TOKEN` from matching
   `AWS_TOKEN_LOG`.
   Two prefix rules can still overlap: both `AWS_SEC*` (7 literal bytes) and
   `AWS_SECRET_*` (11 literal bytes) match `AWS_SECRET_KEY`, and the longer
   prefix wins.
   User documentation should call this out, and we can revisit if it causes
   confusion in practice.

2. **Symlink resolution overhead.** Every filesystem check resolves symlinks via
   `canonicalize`, which performs filesystem lookups.
   For tools that check many paths (such as `fs_grep_files`), this adds per-path
   syscalls.
   Lexical-only checks would diverge from [RFD 075]'s OS enforcement and open a
   symlink bypass.
   If symlink resolution becomes a bottleneck, we can add a cache keyed on
   `(dev, inode)`.

## Implementation Plan

V1 defines the shared `fs`, `net`, and `env` rule types in `jp_tool` and
enforces `fs` cooperatively across the `fs_*` tool family.
Cooperative `net` and `env` consumers (such as `web_fetch` network checks or
`unix_utils` environment forwarding) are omitted from V1 so their enforcement
can be designed alongside each consuming tool.
[RFD 075] reuses the same policy model for OS-level enforcement across all three
rule types.

### Phase 1: Types and evaluation in `jp_tool`

Add `AccessPolicy`, `FsRule`, `NetRule`, `EnvRule`, and `FsAccessError` to
`jp_tool`.
Implement the path-canonicalization helper (shared by tools for target paths and
by the host when converting `AccessConfig` to `AccessPolicy`) and the
`Context::check_*` methods.
Implement structured `net` matching (`scheme`, `host`, `port`, and
`path_prefix`) with `url::Host` normalization for both rule and target hosts,
and explicit-`*` `env` matching with literal-length specificity.
Add `access: Option<AccessPolicy>` to `Context` with `#[serde(default)]`, and
add unit tests covering workspace escapes, symlink resolution, host
normalization (including IDN/Punycode), port defaults, and exact-vs-prefix `env`
ties.

This phase has no dependencies and can merge independently.

### Phase 2: Config types in `jp_config`

Add `AccessConfig`, `FsRuleConfig`, `NetRuleConfig`, and `EnvRuleConfig` in
`jp_config` with `MergeableVec` wrappers and `Partial`, `PartialConfigDelta`,
and `ToPartial` implementations.
Add `access: Option<AccessConfig>` to `ToolConfig` and an accessor on
`ToolConfigWithDefaults`.
Implement the `AccessConfig` to `jp_tool::AccessPolicy` conversion,
canonicalizing rule paths against `ctx.root` (rejecting workspace escapes at
config load) and normalizing rule hosts.
Reject `access` in post-merge validation on tools whose finalized `source` is
`builtin` or `mcp`.

Depends on Phase 1.

### Phase 3: Plumbing in `jp_llm`

Serialize the resolved `AccessPolicy` into the `Context` JSON passed to tool
commands in `execute_local()` and on the `FormatArguments` path.

Depends on Phase 2.

### Phase 4: `fs_*` enforcement

Replace ad-hoc path joining in the `fs_*` tool family with `ctx.check_read()`,
`check_create()`, `check_update()`, `check_delete()`, and `check_execute()`,
returning error messages that name the denied capability and list the configured
grants.

`unix_utils` is **out of scope** for V1 `access.fs` grant enforcement because it
spawns subprocesses that open files directly (such as `sort /etc/passwd`), which
per-argument path checks cannot reliably intercept.
V1 only updates `unix_utils`'s existing argument scanner to use the shared
canonicalization helper for workspace-escape detection.
Enforcing `access.fs` on subprocess file access is handled by [RFD 075]'s
OS-level sandbox.

Depends on Phase 3.

### Phase 5: `*`-scope grants

Add `access: Option<AccessConfig>` to `ToolsDefaultsConfig` and resolve it in
`ToolConfigWithDefaults::access()` using whole-block replacement, skipping
`builtin` and `mcp` tools.
Treat a tool's `access` block as non-empty when any of `fs`, `net`, or `env`
contains a rule, so a tool with only `env` rules still replaces the `*` block.
Narrow post-merge validation to `access` declared on a named tool so grants
inherited from `*` do not trigger the `builtin`/`mcp` rejection.

Update `--mount`, which appends an `fs` rule to a tool's own `access` block.
Because whole-block replacement detaches a tool with its own `access` block from
`*`, and adding a single `fs` rule switches `access.fs` from unrestricted to
default-deny, `--mount` must preserve the tool's existing effective policy
before inserting the mount rule:

- If the tool declared no `access` block of its own, copy the `*` block into the
  tool's `access` block so mounting does not discard restrictive `*` rules and
  widen access.
- If the effective policy had no `access.fs` rules (for example, an unrestricted
  tool or an `env`-only policy), seed `access.fs` with a workspace-wide rule
  (`path = "."`) so the mounted path does not become the only permitted
  filesystem path.

Depends on Phase 2; independent of Phases 3 and 4.

## References

- [RFD 075]: OS-level sandboxing for subprocess tools.
  Consumes this RFD's `AccessPolicy` types to generate platform-native sandbox
  profiles and extends them with subprocess `CommandRule` spawn restrictions.
- [RFD 016]: WASM plugin architecture.
  Uses a capability-based model for filesystem, network, and commands with
  different field names and a simpler `allow`/`writable` filesystem model.
  A future RFD may migrate WASM plugin configuration to `AccessPolicy`.
- [RFD 042]: Tool options.
  Established the `options` map; this RFD makes access policy a first-class
  typed field instead.
- [Deno security model]: Inspiration for the grant-based, default-deny
  permission model.

[Deno security model]: https://docs.deno.com/runtime/fundamentals/security/
[RFD 016]: 016-wasm-plugin-architecture.md
[RFD 042]: 042-tool-options.md
[RFD 075]: 075-tool-sandbox-and-access-policy.md
[RFD 078]: 078-tool-config-mutation.md
