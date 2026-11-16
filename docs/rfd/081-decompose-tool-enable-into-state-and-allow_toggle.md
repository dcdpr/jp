# RFD 081: Decompose Tool Enable into state and allow_toggle

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-11
- **Extends**: [RFD 008]
- **Extended by**: [RFD 108]
- **Summary**: Splits tool `enable` configuration into a boolean `state` and an
  `allow_toggle` policy governing CLI directive overrides.

## Summary

This RFD replaces the flat `Enable` enum on tool configuration with a struct
that separates a tool's active `state` (`bool`) from its `allow_toggle` policy,
which governs which CLI directives can flip that state.
Separating state from toggle policy fixes directive-erasure bugs in the flat
enum and supports the toggle rules required by [RFD 055] and [RFD 083] without
adding combinatorial enum variants.

## Motivation

The `Enable` enum combines two orthogonal concerns in a single mutable field:
whether the tool is enabled, and which CLI directives can change that state.
Conflating the two causes two bugs and forces new enum variants for every
state-and-policy combination.

### Bug 1: `Enable::Always` is excluded from the LLM tool list

`ToolConfigWithDefaults::enable()` returns
`self.tool.enable.or(self.defaults.enable).is_none_or(Enable::is_on)`.
Because `Enable::is_on` matches only `Enable::On`, `Enable::Always` resolves to
`false`.
`tool_definitions()` filters on this predicate, so `describe_tools`, the only
built-in tool registered with `Enable::Always`, is never sent to the LLM.
Its executor is registered, but the model never sees the tool definition.
`Ctx::configure_active_mcp_servers` and the `--tool-use NAME` check in
`apply_tool_use` rely on the same `Enable::is_on` predicate, and
`apply_tool_use` runs directly against the raw partial config before defaults
are applied.
Any MCP tool or user config using `Enable::Always` fails the same way.

### Bug 2: Bare directives erase tool policies

[RFD 008] made CLI directives mutate configuration state in place:
`apply_enable_tools` rewrites the `enable` field to `On` or `Off`.
The filter in `EnableAll` skips `Explicit`, while the filter in `DisableAll`
skips `Always`.
Because of this asymmetry, running `-t -T` on an `Always` tool runs `EnableAll`
first, rewriting `Always` to `On` (since `Always` is not `Explicit`), and then
runs `DisableAll`, rewriting `On` to `Off` (since the value is no longer
`Always`).
A bare `-t -T` therefore disables `describe_tools`, violating its contract that
it cannot be disabled.

The reverse sequence (`-T -t`) erases `Explicit` by flipping it to `Off` and
then `On`, which `test_interleaved_disable_all_then_enable_all` codifies as
expected behavior.
Both erasures happen because a directive cannot flip a tool's state without
overwriting its toggle policy.

### Variant proliferation

[RFD 083] originally proposed a `Sticky` variant as the disable-side mirror of
`Explicit`: enabled by default, and disabled only by a named directive.
[RFD 055] proposed `ExplicitOrGroup`: disabled by default, and enabled only by a
named tool or named group directive.

In a flat enum, every combination of initial state and directive sensitivity
requires a new variant, a new predicate method, new filter arms in the directive
engine, and another branch in every `match Enable` expression.
A tool's initial state and the directives permitted to flip it are independent
axes and should be represented as separate fields.

## Design

This RFD replaces the flat `Enable` enum with two types that separate stored
configuration from the resolved value that consumers inspect:

```rust
/// Stored form. Lives in `ToolConfig.enable` and
/// `ToolsDefaultsConfig.enable` as `Option<EnableConfig>`. Per-field
/// `Option`s preserve which subfield(s) the user actually wrote, which is
/// what lets the cross-key defaults-into-tool merge (see
/// [Defaults and merge](#defaults-and-merge)) compose per-field rather
/// than overwriting whole.
pub struct EnableConfig {
    pub state: Option<bool>,
    pub allow_toggle: Option<AllowToggle>,
}

/// Resolved form. What consumers see after the effective-enable resolver
/// fills `EnableConfig` from per-tool config, then defaults, then the
/// hardcoded fallback (see
/// [Effective enable resolution](#effective-enable-resolution)). Not
/// stored directly — produced on demand.
pub struct Enable {
    pub state: bool,
    pub allow_toggle: AllowToggle,
}

pub enum AllowToggle {
    /// Any directive may flip `state`. Serialized as `"any"`.
    #[default]
    Always,
    /// No directive may flip `state`. Serialized as `"never"`.
    Never,
    /// Only named-tool directives may flip `state`. Serialized as `"if_named"`.
    IfNamed,
    /// Named-tool or named-group directives may flip `state`. Serialized as
    /// `"if_named_or_group"`.
    IfNamedOrGroup,
}
```

CLI directives mutate only `state`.
`allow_toggle` declares which directives can change `state`, and the directive
engine never rewrites it.

For most `(state, allow_toggle)` combinations, `allow_toggle` governs only
config-time directive evaluation; once `state` is resolved, the runtime ignores
`allow_toggle`.
The sole exception is a locked-off tool (`state = false, allow_toggle = Never`),
which the runtime also enforces (see [Locked-off means
hidden](#locked-off-means-hidden)).

### TOML surface

A boolean shorthand covers the common case where `allow_toggle` defaults to
`Always`.
Tools that need a non-default toggle policy use the table form:

```toml
# Bool shorthand — common case (allow_toggle defaults to Always).
[conversation.tools.fs_read_file]
enable = true

# Today's Enable::Always (describe_tools): on, can never be toggled off.
[conversation.tools.describe_tools]
enable = { state = true, allow_toggle = "never" }

# Today's Enable::Explicit: off, only enabled when named.
[conversation.tools.dangerous_tool]
enable = { state = false, allow_toggle = "if_named" }

# RFD 083's Sticky (ask_user): on, only disabled when named.
[conversation.tools.ask_user]
enable = { state = true, allow_toggle = "if_named" }

# RFD 055's ExplicitOrGroup: off, enabled by name or group.
[conversation.tools.write_tool]
enable = { state = false, allow_toggle = "if_named_or_group" }

# New capability: locked off (no directive can toggle this on at config time).
[conversation.tools.network_tool]
enable = { state = false, allow_toggle = "never" }
```

`"if_named_or_group"` is accepted by the schema and behaves identically to
`"if_named"` until [RFD 055] adds the `-t GROUP` and `-T GROUP` parser.
Including the variant now avoids a later schema change.

### Serde

`EnableConfig` is a `#[derive(Config)]` struct with
`#[config(no_deserialize_derive)]`, following the same pattern as
`ModelIdConfig`.
Only `Deserialize` is hand-written on the generated `PartialEnableConfig` (the
type that config files, `base_config.json`, and `config_delta` events load
into).
`Serialize`, `Schematic`, per-field merge, and delta generation use the standard
macro-derived implementations, keeping `enable` consistent with other nested
config fields and requiring no `schematic` macro changes.
`Enable` is the resolved form produced by the resolver and is not deserialized
directly.

The deserializer accepts a boolean, a legacy string (`"on"`, `"off"`,
`"always"`, `"explicit"`, preserved for [backward
compatibility](#backward-compatibility)), or a table.
Within a table, `allow_toggle` accepts `"any"` (`Always`), `"never"` (`Never`),
`"if_named"`, or `"if_named_or_group"`.

Boolean and legacy-string inputs populate both `state` and `allow_toggle` on
`PartialEnableConfig`, so `enable = true` in a higher layer or delta overrides
both fields from lower layers.
Table inputs preserve omitted fields as `None`: `{ state = true }` deserializes
to `PartialEnableConfig { state: Some(true), allow_toggle: None }`, allowing
`allow_toggle` to inherit from a lower layer or defaults (see [Defaults and
merge](#defaults-and-merge)).
If no layer sets `enable`, the resolver falls back to `{ state: true,
allow_toggle: Always }`.

The table below shows how each TOML input resolves when no defaults layer is
present:

| Input                              | Resolver output, no defaults layer        |
| ---------------------------------- | ----------------------------------------- |
| `true`                             | `{ state: true, allow_toggle: Always }`   |
| `false`                            | `{ state: false, allow_toggle: Always }`  |
| `"on"`                             | `{ state: true, allow_toggle: Always }`   |
| `"off"`                            | `{ state: false, allow_toggle: Always }`  |
| `"always"`                         | `{ state: true, allow_toggle: Never }`    |
| `"explicit"`                       | `{ state: false, allow_toggle: IfNamed }` |
| `{ state, allow_toggle }`          | as written                                |
| `{ state }` (allow_toggle omitted) | `allow_toggle` fills to `Always`          |

`Serialize` uses the macro-generated implementation and **always writes the
table form**, never the boolean shorthand, matching how `ModelIdConfig` accepts
a `provider/name` string on input but serializes a `{ provider, name }` table.
Each field on `PartialEnableConfig` carries `skip_serializing_if =
"Option::is_none"`, so a partial that sets only one field serializes only that
field.
For example, `jp config set conversation.tools.foo.enable.allow_toggle if_named`
writes `{ allow_toggle = "if_named" }`, leaving `state` to inherit from lower
layers.
Serialization reads the stored optional fields directly, never
`effective_enable()`.

| Partial input                                        | Serialized output                         |
| ---------------------------------------------------- | ----------------------------------------- |
| `{ state: Some(true), allow_toggle: Some(Always) }`  | `{ state = true, allow_toggle = "any" }`  |
| `{ state: Some(false), allow_toggle: Some(Always) }` | `{ state = false, allow_toggle = "any" }` |
| `{ state: Some(_), allow_toggle: Some(_) }`          | `{ state, allow_toggle }`                 |
| `{ state: Some(_), allow_toggle: None }`             | `{ state }`                               |
| `{ state: None, allow_toggle: Some(_) }`             | `{ allow_toggle }`                        |
| `{ state: None, allow_toggle: None }`                | `{}` (empty; omitted by the parent map)   |

The table form round-trips unchanged.
Boolean and legacy-string shorthands normalize to the table form on the first
write-back (for example, `enable = true` becomes `enable = { state = true,
allow_toggle = "any" }`), just as `model.id = "anthropic/claude"` normalizes to
a table.

### Defaults and merge

`ToolsDefaultsConfig.enable` uses the same `Option<EnableConfig>` type as a
per-tool entry, sharing the same TOML syntax, compatibility deserializer, and
serialization rules:

```toml
[conversation.tools.'*']
enable = { state = false, allow_toggle = "if_named" } # Defaults are Explicit.

[conversation.tools.foo]
enable = { state = true } # foo overrides only state, inherits allow_toggle.
# Effective: state=true, allow_toggle=if_named.
# (Same shape as today's Sticky.)
```

Two distinct merges produce this result at different stages of the config
pipeline:

1. **Cross-layer merge** combines config files that set the same path (for
   example, a user config layered over a workspace config, both setting
   `conversation.tools.foo.enable`).
   This merge runs on partials via `PartialConfig::merge` in `load_partial`
   (`crates/jp_config/src/fs.rs`; see [RFD 079] for precedence rules).
   Because `PartialEnableConfig` is a schematic-derived nested partial with
   `state: Option<bool>` and `allow_toggle: Option<AllowToggle>`, `merge`
   recurses into its fields instead of replacing the struct as a leaf
   `Option<EnableConfig>`.
   A layer that sets only `state` preserves `allow_toggle` from lower layers.
   (`FillDefaults` is unrelated; it only seeds schema defaults during
   finalization.)

2. **Cross-key merge** combines `conversation.tools.*.enable` defaults into
   per-tool entries such as `conversation.tools.foo.enable`.
   Because `PartialToolsConfig::fill_from` does not merge defaults into
   individual tool entries, this merge runs at runtime on the resolved config.
   `ToolConfig.enable` and `ToolsDefaultsConfig.enable` therefore store
   `Option<EnableConfig>` (with optional fields) rather than a resolved
   `Enable`.
   Filling defaults during finalization would make `{ state = true }`
   indistinguishable from `{ state = true, allow_toggle = "any" }`, causing the
   per-tool entry to shadow `defaults.enable.allow_toggle`.
   Instead, the resolver on `ToolConfigWithDefaults` (see [Effective enable
   resolution](#effective-enable-resolution)) inspects each field of
   `EnableConfig` and falls back to `defaults.enable` only when a field is
   `None`.

`state` and `allow_toggle` are always nested under `enable`, never top-level
fields on a tool configuration.

### Effective enable resolution

`apply_tool_use`, `apply_enable_tools`, `tool_definitions`, and
`Ctx::configure_active_mcp_servers` all consume the same effective `Enable`
value, resolved per field from the per-tool config, the defaults config, and the
hardcoded fallback:

```text
effective.state        = tool.enable.state        ?? defaults.enable.state        ?? true
effective.allow_toggle = tool.enable.allow_toggle ?? defaults.enable.allow_toggle ?? Always
```

Two entry points expose this resolution, one for resolved configs and one for
partials.
Both return the same `Enable` value for a given `(tool, defaults)` pair:

1. **Resolved config** (`tool_definitions()`,
   `Ctx::configure_active_mcp_servers`):

   ```rust
   impl ToolConfigWithDefaults {
       pub fn effective_enable(&self) -> Enable;
       pub fn is_enabled(&self) -> bool;
   }
   ```

   Both methods resolve `self.tool` and `self.defaults` field by field.
   `is_enabled()` returns `effective_enable().state` and replaces `enable()` at
   every call site.

2. **Partial config** (`apply_enable_tools`, `apply_tool_use`, which run on
   `PartialAppConfig` before `from_partial_with_defaults`):

   ```rust
   impl PartialEnableConfig {
       pub fn effective(&self, defaults: &PartialEnableConfig) -> Enable;
   }
   ```

   The CLI path reads `partial.conversation.tools.tools.<name>.enable` and
   `partial.conversation.tools.defaults.enable`, then calls `effective` to
   produce the same `Enable` value as the resolved-config path without building
   a temporary `AppConfig`.

Without this shared resolver, `tool_definitions()` reads defaults-merged values
through `ToolConfigWithDefaults::enable()`, whereas `apply_tool_use` checks the
raw partial directly with `cfg.enable.is_some_and(Enable::is_on)` and rejects
tools that rely on the default-on fallback.
Routing every consumer through the same resolver lets `--tool-use NAME` accept
built-in tools such as `describe_tools` as well as user tools that omit
`enable`.

When [RFD 056] and [RFD 057] land, per-field resolution will extend across their
group-default and group-override layers in the order those RFDs define.
[RFD 057] states that CLI flags always win over group overrides, whereas under
this RFD a group override setting `allow_toggle = Never` would block a named CLI
directive.
[RFD 057] must decide whether group-sourced `allow_toggle` values block CLI
directives or CLI directives bypass them.

### Directive engine

Directives are classified by scope:

```rust
pub enum ToggleScope {
    Bulk,        // -t / -T with no argument
    Named,       // -t NAME / -T NAME
    NamedGroup,  // -t GROUP / -T GROUP — reserved for RFD 055
}
```

For each `(directive, tool)` pair, the directive engine checks two conditions:

1. Does `allow_toggle` permit the directive's scope (`Enable::accepts(scope)`)?
2. Would the directive change `state`?

| `accepts(scope)` | `state` already matches intent | `state` would flip                 |
| ---------------- | ------------------------------ | ---------------------------------- |
| `true`           | no-op                          | flip `state`                       |
| `false`          | no-op                          | error (named) or skip (bulk/group) |

`Enable::accepts` matches the policy against the directive scope:

```rust
impl Enable {
    pub fn accepts(&self, scope: ToggleScope) -> bool {
        match (self.allow_toggle, scope) {
            (AllowToggle::Always, _) => true,
            (AllowToggle::Never, _) => false,
            (AllowToggle::IfNamed, ToggleScope::Named) => true,
            (AllowToggle::IfNamedOrGroup,
             ToggleScope::Named | ToggleScope::NamedGroup) => true,
            _ => false,
        }
    }
}
```

Under this rule, `-t -T` and `-T -t` preserve `allow_toggle` on every tool and
never flip a tool whose policy rejects bulk directives, eliminating both bugs in
[Motivation](#motivation).

### `--tool-use NAME` validation

`apply_tool_use` validates that the target tool is configured and its effective
`state` is `true`, regardless of `allow_toggle`.
Replacing the `Enable::is_on` check with `state == true` makes `jp -u
describe_tools` valid because `describe_tools` has `state = true`.

Because `apply_tool_use` runs on `PartialAppConfig` before MCP servers start, it
checks config eligibility rather than runtime availability.
A tool backed by an optional MCP server that fails to start is still dropped
later by `tool_definitions()`.
Handling a forced tool that is absent from the resolved tool list is a separate
runtime concern outside the scope of this RFD.

### Locked-off means hidden

A locked-off tool (`state = false, allow_toggle = Never`) is completely excluded
at runtime, not merely immune to CLI directives:

1. **`tool_definitions()` always excludes locked-off tools.** The `forced_tool`
   exemption in `crates/jp_llm/src/tool.rs` normally includes the tool named by
   `assistant.tool_choice` even when disabled, but does not apply when the tool
   is locked-off.
2. **`AppConfig::validate` rejects `assistant.tool_choice = "foo"`** when `foo`
   resolves to a locked-off tool.
   This check lives on `AppConfig` (the only validator with access to both
   `assistant.tool_choice` and `conversation.tools`), not in
   `ToolsConfig::validate`.
   The error names both `assistant.tool_choice` and
   `conversation.tools.<name>.enable` rather than silently ignoring the setting
   or sending an invalid tool choice to a provider such as Google/Gemini.
3. **`Ctx::configure_active_mcp_servers` skips locked-off MCP tools** because it
   filters on `is_enabled()` before checking any forced tool name.

Only `(state = false, allow_toggle = Never)` needs this runtime check; a
locked-on tool (`state = true, allow_toggle = Never`) is already enabled and
creates no conflict with `assistant.tool_choice`.

Enforcing locked-off tools at runtime keeps the configuration contract accurate
when a user sets `state = false, allow_toggle = "never"`.
Broader runtime enforcement (argument-level policy, tool-call sandboxing, and
plugin trust) belongs to [RFD 075], [RFD 076], and [RFD 077].

### Behavior matrix

The table below shows how each `(state, allow_toggle)` combination responds to
named and bulk directives.
`IfNamedOrGroup` is omitted because it behaves identically to `IfNamed` until
[RFD 055] adds `-t GROUP` and `-T GROUP` parsing:

| Tool config            | `-t name` | `-T name` | `-t` bulk | `-T` bulk |
| ---------------------- | --------- | --------- | --------- | --------- |
| `state=true, Always`   | no-op     | flips off | no-op     | flips off |
| `state=true, Never`    | no-op     | **error** | skip      | skip      |
| `state=true, IfNamed`  | no-op     | flips off | skip      | skip      |
| `state=false, Always`  | flips on  | no-op     | flips on  | no-op     |
| `state=false, Never`   | **error** | no-op     | skip      | skip      |
| `state=false, IfNamed` | flips on  | no-op     | skip      | skip      |

Directive errors report the tool's `allow_toggle` policy (for example, "cannot
disable `describe_tools`: this tool is configured as locked-on") instead of the
legacy "system tool cannot be disabled" message.

### Predicates and helpers

```rust
impl PartialEnableConfig {
    // Partial-form constants used by builtin registrations and the directive
    // engine, which work on partials.
    pub const ON: Self = Self {
        state: Some(true),
        allow_toggle: Some(AllowToggle::Always),
    };
    pub const OFF: Self = Self {
        state: Some(false),
        allow_toggle: Some(AllowToggle::Always),
    };
    pub const LOCKED_ON: Self = Self {
        state: Some(true),
        allow_toggle: Some(AllowToggle::Never),
    };
    pub const LOCKED_OFF: Self = Self {
        state: Some(false),
        allow_toggle: Some(AllowToggle::Never),
    };
}

impl Enable {
    // Predicates on the resolved form, used by directive and consumer code.
    pub const fn is_enabled(&self) -> bool { self.state }
    pub const fn is_locked(&self) -> bool {
        matches!(self.allow_toggle, AllowToggle::Never)
    }
    pub fn accepts(&self, scope: ToggleScope) -> bool { /* as above */ }
}
```

`describe_tools`'s builtin registration becomes `enable:
Some(PartialEnableConfig::LOCKED_ON)`.

## Backward compatibility

The deserializer accepts the legacy string values (`"on"`, `"off"`, `"always"`,
`"explicit"`) and converts each to its canonical struct form at parse time:

| Legacy input          | Canonical form                            |
| --------------------- | ----------------------------------------- |
| `enable = "on"`       | `{ state: true, allow_toggle: Always }`   |
| `enable = "off"`      | `{ state: false, allow_toggle: Always }`  |
| `enable = "always"`   | `{ state: true, allow_toggle: Never }`    |
| `enable = "explicit"` | `{ state: false, allow_toggle: IfNamed }` |

The `allow_toggle` field accepts `"any"` (freely toggleable, the default),
`"never"` (locked), `"if_named"`, or `"if_named_or_group"`.
The freely toggleable variant is named `"any"` rather than `"always"` to avoid
confusion with the legacy `enable = "always"` shorthand, which means locked-on.
Legacy strings are accepted only on `enable`, never on `allow_toggle`.

Compatibility parsing is required for persisted conversations.
Both the base config snapshot (`base_config.json`) and `config_delta` events use
`jp_conversation::compat::deserialize_partial_config`, so conversations created
before this change may contain legacy `enable` strings in either place.
Without compatibility parsing, `deserialize_partial_config` would log a warning
and replace the failing layer with an empty partial, silently dropping legacy
`enable` settings in existing conversations.

The compatibility path is read-only; serialization always emits the canonical
table form.
On the first write-back (such as a `config_delta` event or `jp config set`),
boolean and legacy-string shorthands normalize to the table form.

Built-in tool registrations (such as `describe_tools`, previously registered
with `Enable::Always`) switch to `PartialEnableConfig::LOCKED_ON`.

Related RFDs are updated alongside the code change:

- [RFD 008] gains a `TIP` note under "Design" explaining that directive state
  mutation is gated by `allow_toggle`, and
  `test_interleaved_disable_all_then_enable_all` is replaced by policy
  preservation tests.
- [RFD 055] drops `Enable::ExplicitOrGroup` from its Implementation Plan, the
  `explicit_or_group` row from the "Interaction with Tool `enable` Field" table,
  the "New `Enable` variant" drawback, and the `Enable` enum growth risk.
  Tool groups use `AllowToggle::IfNamedOrGroup` and only need the
  `ToggleScope::NamedGroup` parser.
- [RFD 056] refers to `EnableConfig` and per-field resolution, replacing the
  `Enable` field type and the `enable()` / `enable_mode()` accessors with
  `is_enabled()` and `effective_enable()`.
- [RFD 057] routes `apply_enable_tools` through `PartialEnableConfig::effective`
  while keeping the open question on whether group-sourced `allow_toggle =
  Never` blocks CLI directives.
- [RFD 083] updates the bool-shorthand note from "resets `allow_toggle` to
  `always`" to `any`, and changes its built-in registration examples from
  `Enable { ... }` to `EnableConfig { ... }`.
  Its `Requires: [RFD 081]` metadata is unchanged.

## Drawbacks

**One-way serde normalization.** Boolean and legacy-string inputs (`true`,
`false`, `"on"`, `"off"`, `"always"`, `"explicit"`) serialize as tables (for
example, `enable = true` writes back as `enable = { state = true, allow_toggle =
"any" }`) and remain stable on subsequent writes.
This matches the shorthand-to-table normalization on `ModelIdConfig` (`model.id
= "anthropic/claude"` writes back as `{ provider, name }`), keeping `enable`
consistent with the rest of `jp_config` without a custom serializer.

**More verbose TOML for non-default policies.** Non-default `allow_toggle`
policies require a table: `enable = { state = true, allow_toggle = "never" }` is
longer than `enable = "always"`, though each field states its role explicitly.

**`AllowToggle::IfNamedOrGroup` is equivalent to `IfNamed` until [RFD 055]
lands.** The variant is in the schema and handled by `Enable::accepts`, but no
parser produces `ToggleScope::NamedGroup` until tool groups ship.
Until then, `allow_toggle = "if_named_or_group"` behaves identically to
`"if_named"`, as noted in [TOML surface](#toml-surface).
Adding the variant now avoids a schema change when [RFD 055] lands.

## Alternatives

**Fix `enable()` without changing the directive engine.** Updating `enable()` to
treat `Enable::Always` as enabled fixes Bug 1, but leaves Bug 2 and variant
proliferation unresolved.

**Split into two top-level fields (`enabled: Option<bool>` and `policy:
Option<Policy>`).** Rejected for two reasons.
First, placing both fields directly on `ToolConfig` mixes them with unrelated
settings (`run`, `result`, `command`), obscures that they jointly control tool
activation, and makes the `enable = true` boolean shorthand awkward to preserve.
Second, the name `policy` collides with `AccessPolicy`, `RunPolicy`, and
`TrustPolicy` in [RFD 075], [RFD 076], and [RFD 077].

**Use a single enum with nested `Enable::State | Enable::Policy` variants.**
This has the same semantics as the struct design, but requires nested pattern
matching (`Enable::State(EnableState::On)`) at every call site.
`Enable { state, allow_toggle }` expresses the same model with flatter types.

**Keep the flat enum and make `is_enabled()` / `is_policy()` exhaustive.** This
minimizes the initial diff, but every new policy (`Sticky`, `ExplicitOrGroup`)
still requires a new enum variant, new match arms, and additional tests.

## Non-Goals

**Tool group directive parsing.** [RFD 055] introduces `-t GROUP` and `-T GROUP`
directive parsing.
This RFD defines `ToggleScope::NamedGroup` and `AllowToggle::IfNamedOrGroup` so
that [RFD 055] only needs to update the parser.

**Runtime access control.** [RFD 075], [RFD 076], and [RFD 077] govern what a
tool may do at runtime (filesystem, network, subprocess).
`allow_toggle` governs which CLI directives may change a tool's `state` at
config time.

**General runtime `tool_choice` enforcement.** Beyond the locked-off case (see
[Locked-off means hidden](#locked-off-means-hidden)), `allow_toggle` does not
constrain `assistant.tool_choice`.
A persisted `tool_choice = "foo"` on a tool with `state = true, allow_toggle =
Always` is still forced into the request payload even after `-T foo` flips
`state` to `false`.
Broader runtime enforcement belongs to [RFD 075], [RFD 076], and [RFD 077].

## Risks and Open Questions

**Persona-layer composition.** Per-field inheritance is the standard composition
rule for layered config, but existing personas have not been tested with a split
`enable` table.
Implementation must verify that partial-config deltas handle
`PartialEnableConfig` when a layer sets only `state` or only `allow_toggle`.

**Round-trip behavior in `jp config set`.** `jp config set` on a file containing
`enable = { state = true, allow_toggle = "never" }` must round-trip without
losing fields, and `jp config set` on legacy `enable = "always"` must normalize
to `enable = { state = true, allow_toggle = "never" }` on the first write-back.
This requires `deep_merge_toml` (`crates/jp_config/src/fs.rs`) to recurse into
inline tables; because it currently recurses only into standard tables, running
`jp config set conversation.tools.foo.enable.state ...` against an inline table
would otherwise overwrite the whole table and drop `allow_toggle`.

**Legacy strings in persisted conversations.** Conversations created before this
RFD may store legacy `enable = "..."` strings in `base_config.json` or
`config_delta` events.
The compatibility deserializer must accept them on both surfaces, and regression
tests must verify that loading a conversation with legacy values produces the
expected `state` and `allow_toggle`.

**Tool config mutation grants.** [RFD 078] lets tools write to
`conversation.tools.*` paths via `access.config` grants.
Existing payloads that write a boolean or legacy string (`true`, `"on"`,
`"always"`) to `conversation.tools.*.enable` remain valid because the
deserializer accepts them unchanged.
A tool needs the table form (`{ state, allow_toggle }`) only to set one subfield
while preserving the other from a lower layer.
When [RFD 078] is implemented, grant validation and built-in config-mutating
tools must support the table form.

## Implementation Plan

### Phase 1: Types and serde

- In `jp_config/src/conversation/tool.rs`, replace the flat `Enable` enum with
  `EnableConfig` (`state: Option<bool>`, `allow_toggle: Option<AllowToggle>`,
  stored as `Option<EnableConfig>` on `ToolConfig.enable` and
  `ToolsDefaultsConfig.enable`) and `Enable` (the resolved struct returned by
  the effective-enable resolver).
- Add `AllowToggle` and `ToggleScope` in `jp_config` alongside `Enable` so
  `Enable::accepts` and the CLI directive parser in `jp_cli` share the same
  scope type.
- Derive `Config` on `EnableConfig` with `#[config(no_deserialize_derive)]` so
  `Serialize` and `Schematic` are macro-generated, and implement `Deserialize`
  on `PartialEnableConfig` for booleans, legacy strings (`"on"`, `"off"`,
  `"always"`, `"explicit"`), and tables.
- Add `PartialEnableConfig::{ON, OFF, LOCKED_ON, LOCKED_OFF}` and
  `Enable::{is_enabled, is_locked, accepts}`.
- Update `ToPartial`, `PartialConfigDelta`, and `AssignKeyValue` for
  `EnableConfig` and `PartialEnableConfig`.
- Update `deep_merge_toml` in `crates/jp_config/src/fs.rs` to use
  `as_table_like_mut` and `as_table_like` instead of `as_table_mut` and
  `as_table`, so `jp config set conversation.tools.foo.enable.state ...` merges
  into inline tables without dropping `allow_toggle`.

### Phase 2: Predicates and `Ctx`

- Add `ToolConfigWithDefaults::effective_enable() -> Enable` and
  `ToolConfigWithDefaults::is_enabled() -> bool`, and replace calls to
  `ToolConfigWithDefaults::enable()` with `is_enabled()`.
- Add `PartialEnableConfig::effective(&self, defaults: &PartialEnableConfig) ->
  Enable` for partial-config consumers.
- Verify that `Ctx::configure_active_mcp_servers` uses `is_enabled()`.
- Update `tool_definitions()` to use `is_enabled()` and drop the `forced_tool`
  exemption for locked-off tools (`state == false && allow_toggle == Never`; see
  [Locked-off means hidden](#locked-off-means-hidden)).
- Register `describe_tools` with `PartialEnableConfig::LOCKED_ON`.

### Phase 3: Directive engine

- Rewrite `apply_enable_tools` around `ToggleScope` and `Enable::accepts`,
  removing `is_explicit` and `is_always` and generalizing the named-disable
  guard to all named directives.
- Reserve `ToggleScope::NamedGroup` for [RFD 055].
- Route `apply_tool_use` and `apply_enable_tools` through
  `PartialEnableConfig::effective` (see [Effective enable
  resolution](#effective-enable-resolution)) instead of inspecting raw partial
  fields.
- In `AppConfig::validate`, reject `assistant.tool_choice = Function(name)` when
  `name` resolves to a locked-off tool, naming both `assistant.tool_choice` and
  `conversation.tools.<name>.enable` in the error.

### Phase 4: Tests and RFD updates

- Replace `test_interleaved_disable_all_then_enable_all` with tests verifying
  that `allow_toggle` is preserved across interleaved directive sequences and
  that `state` flips only when `accepts(scope)` is `true` and the target state
  differs.
- Add tests for named-directive errors and no-op cases.
- Add tests for `tool_definitions()` and `Ctx::configure_active_mcp_servers`
  with `PartialEnableConfig::LOCKED_ON`.
- Add layered-merge tests:
  - `enable = { state = true }` on a tool inherits `allow_toggle` from
    `[conversation.tools.'*']`.
  - `enable = true` sets both fields and overrides any inherited `allow_toggle`.
  - An omitted `enable` resolves to `{ state: true, allow_toggle: Always }`.
- Add `--tool-use NAME` tests for a tool enabled via defaults only and for
  `describe_tools`.
- Add compatibility tests for legacy `"always"` and `"explicit"` strings in
  `base_config.json` snapshots and `config_delta` events.
- Add `jp config set` round-trip tests for `conversation.tools.foo.enable.state`
  and `conversation.tools.foo.enable.allow_toggle`.
- Add locked-off enforcement tests:
  - `tool_definitions()` excludes a locked-off tool even when named by
    `assistant.tool_choice`.
  - `AppConfig::validate` rejects `assistant.tool_choice = "foo"` when `foo` is
    locked-off.
  - `Ctx::configure_active_mcp_servers` does not start an MCP server whose only
    configured tool is locked-off.
- Update [RFD 008], [RFD 055], [RFD 056], [RFD 057], and [RFD 083] as listed in
  [Backward compatibility](#backward-compatibility).

Phases 1 through 3 are interdependent and ship together with Phase 4 in a single
PR.

## References

- [RFD 008]: Ordered tool directives (directive evaluation semantics).
- [RFD 055]: Tool groups (consumer of `AllowToggle::IfNamedOrGroup`).
- [RFD 056]: Group configuration defaults (group-level `enable` inheritance).
- [RFD 057]: Group configuration overrides (group-level `enable` enforcement).
- [RFD 075]: Tool sandbox and access policy (`AccessPolicy`, naming context).
- [RFD 076]: Tool access grants (`AccessPolicy`).
- [RFD 077]: Plugin configuration and trust policy (`RunPolicy`, `TrustPolicy`).
- [RFD 078]: Tool config mutation (writes to `conversation.tools.*.enable`).
- [RFD 079]: Config sources and load order (the source/precedence model this
  RFD's cross-layer merge participates in).
- [RFD 083]: `ask_user` tool (originally introduced `Enable::Sticky`).
- `crates/jp_config/src/conversation/tool.rs`: `Enable`, `ToolConfig`,
  `ToolConfigWithDefaults`.
- `crates/jp_cli/src/cmd/query.rs`: `apply_enable_tools`, `apply_tool_use`.
- `crates/jp_cli/src/cmd/query/tool/builtins.rs`: built-in tool registration.
- `crates/jp_llm/src/tool.rs`: `tool_definitions()`.

[RFD 008]: 008-ordered-tool-directives.md
[RFD 055]: 055-tool-groups.md
[RFD 056]: 056-group-configuration-defaults.md
[RFD 057]: 057-group-configuration-overrides.md
[RFD 075]: 075-tool-sandbox-and-access-policy.md
[RFD 076]: 076-tool-access-grants.md
[RFD 077]: 077-plugin-configuration-and-trust-policy.md
[RFD 078]: 078-tool-config-mutation.md
[RFD 079]: 079-config-sources-and-load-order.md
[RFD 083]: 083-built-in-ask_user-tool-for-assistant-initiated-inquiries.md
[RFD 108]: 108-discoverable-tools.md
