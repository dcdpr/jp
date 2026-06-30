# RFD 081: Decompose tool enable into state and allow\_toggle

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-11
- **Extends**: [RFD 008]
- **Required by**: [RFD 083]

## Summary

Replace the flat `Enable` enum on tool configuration with a struct that
separates the tool's current `state` (`bool`) from `allow_toggle` (an enum
describing which CLI directives may flip that state).
This eliminates two latent bugs in the existing model, removes the need for a
`Sticky` variant originally proposed in [RFD 083] (which now adopts this RFD's
shape directly), and absorbs [RFD 055]'s `ExplicitOrGroup` variant without
schema growth.

## Motivation

The current `Enable` enum carries two orthogonal concerns in a single mutable
field: the tool's enabled state, and the policy that controls how that state may
change.
Two bugs follow from this conflation, and a third variant proliferation problem
follows from the same root cause.

### Bug 1: `Enable::Always` is filtered out of the LLM-visible tool list

`ToolConfigWithDefaults::enable()` returns
`self.tool.enable.or(self.defaults.enable).is_none_or(Enable::is_on)`.
`Enable::is_on` matches only `Enable::On`, so `Enable::Always` resolves to
`false`.
`tool_definitions()` filters on this predicate, which means the only builtin
currently registered with `Enable::Always` — `describe_tools` — has never been
sent to the LLM in practice.
Its executor is registered but unreachable.
The same `Enable::is_on` assumption also drives
`Ctx::configure_active_mcp_servers` and the `--tool-use NAME` validation in
`apply_tool_use` — the latter applied directly on the raw partial config, which
compounds the problem.
Any future MCP tool or user config using `Enable::Always` would silently break
in the same way.

### Bug 2: bare directives erase tool classifications

[RFD 008] made CLI directives state-mutating: `apply_enable_tools` rewrites the
`enable` field to `On` / `Off` in place.
The filter in `EnableAll` skips `Explicit`; the filter in `DisableAll` skips
`Always`.
The asymmetry means `-t -T` on an `Always` tool runs `EnableAll` first, which
rewrites `Always` to `On` (`Always` is not `Explicit`), after which `DisableAll`
rewrites `On` to `Off` (the value is no longer `Always`).
Net result: a bare `-t -T` disables `describe_tools`, contradicting its
documented "cannot be disabled" contract.

The equivalent erasure for `Explicit` (`-T -t` flips it to `On`) is the behavior
the existing `test_interleaved_disable_all_then_enable_all` documents as
intentional.
Both arise from the same conflation of state and policy in a single mutable
field.

### Variant proliferation for future features

[RFD 083] originally proposed a `Sticky` variant: "the disable-side mirror of
`Explicit`" — on by default, requires a named directive to disable.
Under the flat-enum shape this would require a new variant, new `is_sticky()`
predicate, new filter arms in the directive engine, and another conditional
branch in every `match Enable { ... }` site.

[RFD 055] proposes `ExplicitOrGroup`: "off by default; enabled by named tool or
named group."
Same pattern — another variant, more predicates, more match arms.

A new variant per (default state × directive sensitivity) combination scales
poorly.
The orthogonal axes — "what state does the tool start in?" and "which
directives may flip that state?" — are not naturally expressible as a single
flat enum.

## Design

Replace `Enable` (currently a flat enum) with a pair of types that splits the
two concerns into two roles `Enable` plays today — stored configuration vs.
value consumers reason about:

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

Directives only ever mutate `state`.
`allow_toggle` is the user's persistent assertion about which directives may do
so, and is never rewritten by the directive engine.

For most `(state, allow_toggle)` combinations `allow_toggle` governs
*config-time directive behavior only* — once `state` is resolved, the runtime
does not consult `allow_toggle` again.
The one exception is the locked-off case (`state = false, allow_toggle =
Never`), which is enforced at runtime so the terminology stays honest; see
[Locked-off means hidden](#locked-off-means-hidden).

### TOML surface

The common case keeps a bool shorthand.
Tools that need a non-default toggle policy use the explicit struct form:

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

`"if_named_or_group"` is accepted today and behaves identically to `"if_named"`
until [RFD 055] lands the `-t GROUP` / `-T GROUP` parser.
The schema accepts the value now to avoid a later additive change.

### Serde

`Deserialize` is implemented on `EnableConfig` and `PartialEnableConfig`, not on
resolved `Enable` — `Enable` is produced by the resolver, not deserialized
directly.
Both deserializers accept a bool, a string, or a map.
Within the map form, the `allow_toggle` field accepts the strings `"any"` (=
`Always`), `"never"` (= `Never`), `"if_named"`, or `"if_named_or_group"`.
Omitted map fields stay `None` so they participate in per-field merging — see
[Defaults and merge](#defaults-and-merge).

The table below describes the form a TOML input produces when deserialized into
`EnableConfig` and then passed through the resolver with no defaults layer (so
any `None` field falls through to the hardcoded fallback):

| Input                               | Resolver output, no defaults layer        |
| ----------------------------------- | ----------------------------------------- |
| `true`                              | `{ state: true, allow_toggle: Always }`   |
| `false`                             | `{ state: false, allow_toggle: Always }`  |
| `"on"`                              | `{ state: true, allow_toggle: Always }`   |
| `"off"`                             | `{ state: false, allow_toggle: Always }`  |
| `"always"`                          | `{ state: true, allow_toggle: Never }`    |
| `"explicit"`                        | `{ state: false, allow_toggle: IfNamed }` |
| `{ state, allow_toggle }`           | as written                                |
| `{ state }` (allow\_toggle omitted) | `allow_toggle` fills to `Always`          |

In `PartialEnableConfig`, the same `{ state }` map leaves `allow_toggle` as
`None`, preserving any value inherited from a defaults layer.

The string forms (`"on"`, `"off"`, `"always"`, `"explicit"`) are the legacy
flat-enum variants.
They are preserved for backward compatibility — see [Backward
compatibility](#backward-compatibility).

`Serialize` follows the same per-field rules for stored `EnableConfig` and
`PartialEnableConfig`; the serialization table below applies to both.
The bool shorthand is emitted only when both fields are set and `allow_toggle`
is `Always`; otherwise the map form is emitted, carrying only the fields that
are set.
A stored `{ state: Some(true), allow_toggle: None }` therefore serializes as `{
state = true }`, never `true` — `true` would erase inheritance from a defaults
layer.
Serialization always operates on the stored optional fields, never on
`effective_enable()`, when writing config files, `base_config.json`, or
`config_delta` events.
Round-trip is exact for inputs already in canonical form; legacy strings and
explicit-`Always` maps are one-way-normalized to canonical on the first
write-back.

`PartialEnableConfig` serializes only the fields that are `Some`.
This matters because `jp config set` and `config_delta` events write partial
deltas that may set just one half — e.g. `jp config set
conversation.tools.foo.enable.allow_toggle if_named` produces a partial with
`state: None`, which the bool shorthand cannot express.

| Partial input                                        | Serialized output           |
| ---------------------------------------------------- | --------------------------- |
| `{ state: Some(true), allow_toggle: Some(Always) }`  | `true` (bool shorthand)     |
| `{ state: Some(false), allow_toggle: Some(Always) }` | `false` (bool shorthand)    |
| `{ state: Some(_), allow_toggle: Some(non-Always) }` | `{ state, allow_toggle }`   |
| `{ state: Some(_), allow_toggle: None }`             | `{ state }`                 |
| `{ state: None, allow_toggle: Some(_) }`             | `{ allow_toggle }`          |
| `{ state: None, allow_toggle: None }`                | omitted from the parent map |

On the deserialize side, bool and legacy-string inputs to a
`PartialEnableConfig` fill *both* fields (so `enable = true` in a delta
overrides both halves of any underlying value).
The map form preserves omission: `{ state = true }` deserializes to
`PartialEnableConfig { state: Some(true), allow_toggle: None }`, which is what
lets per-field merge (see [Defaults and merge](#defaults-and-merge)) inherit
`allow_toggle` from a lower layer.

For partial overrides at the layered-config level (set `state` and inherit
`allow_toggle` from a defaults layer), use the explicit map form with only the
field you want to set: `enable = { state = true }`.
The bool and string shorthand forms fully specify both fields.
See [Defaults and merge](#defaults-and-merge).

If no layer sets `enable` at all, the implicit default is `{ state: true,
allow_toggle: Always }` — enabled, freely toggleable.
This matches today's "absence means On" behavior.

### Defaults and merge

`ToolsDefaultsConfig.enable` is the same `Enable` field as a per-tool entry —
same TOML shape (bool / string / map), same compat deserializer, same
serialization rules.
There is no separate defaults schema; the same value type appears at every
layer.

```toml
[conversation.tools.'*']
enable = { state = false, allow_toggle = "if_named" } # Defaults are Explicit.

[conversation.tools.foo]
enable = { state = true } # foo overrides only state, inherits allow_toggle.
# Effective: state=true, allow_toggle=if_named.
# (Same shape as today's Sticky.)
```

Two distinct merges produce this result, at different layers of the config
pipeline:

1. **Cross-layer merge** (between config files setting the same path — e.g.
   user-level config layered onto project-level config, both writing
   `conversation.tools.foo.enable`).
   Happens in the partial layer.
   `PartialEnableConfig` exposes `state: Option<bool>` and `allow_toggle:
   Option<AllowToggle>`, so a partial that mentions only `state` does not erase
   `allow_toggle` set in a lower layer.
   This is the standard `PartialConfig::merge` path that composes layered config
   in `load_partial` (`crates/jp_config/src/fs.rs`); see [RFD 079] for the
   source/precedence model.
   `PartialEnableConfig` must therefore be a nested partial (derived through
   schematic so `merge` recurses into its fields), not a leaf
   `Option<EnableConfig>` — otherwise the higher-priority layer would replace
   the entire value and erase `allow_toggle` from below.
   `FillDefaults` is unrelated to this path; it only seeds schema defaults at
   finalization.

2. **Cross-key merge** (from `conversation.tools.*.enable` defaults into
   per-tool entries like `conversation.tools.foo.enable`).
   Happens at runtime, against the *final* config —
   `PartialToolsConfig::fill_from` does not field-merge defaults into individual
   tool entries, and there is no plan to add that.
   This is why final `ToolConfig.enable` and `ToolsDefaultsConfig.enable` store
   `Option<EnableConfig>` (the optional-field form) rather than the filled
   `Enable`: if `enable` were filled at finalization time, "the user wrote only
   `state`" would be indistinguishable from "the user wrote both fields with
   `allow_toggle = Always`", and the cross-key merge would silently shadow the
   defaults' `allow_toggle`.
   The runtime resolver in `ToolConfigWithDefaults` (see [Effective enable
   resolution](#effective-enable-resolution)) reads the stored `EnableConfig`
   field by field and falls through to defaults exactly when a field is `None`.

Users never write `state` / `allow_toggle` as top-level fields — the
field-optional split is an internal mechanism for cross-layer overrides and
cross-key defaults inheritance.

### Effective enable resolution

`apply_tool_use`, `apply_enable_tools`, `tool_definitions`, and
`Ctx::configure_active_mcp_servers` all consume the same *effective* enable
value, resolved per-field from per-tool config, then defaults, then the
hardcoded fallback:

```text
effective.state        = tool.enable.state        ?? defaults.enable.state        ?? true
effective.allow_toggle = tool.enable.allow_toggle ?? defaults.enable.allow_toggle ?? Always
```

Two seams expose this fallback, one per consumer type.
Both produce the same filled `Enable` for the same `(tool, defaults)` pair.

1. **Final-config seam** — for consumers operating on a built `AppConfig`
   (`tool_definitions()`, `Ctx::configure_active_mcp_servers`):

   ```rust
   impl ToolConfigWithDefaults {
       pub fn effective_enable(&self) -> Enable;
       pub fn is_enabled(&self) -> bool;
   }
   ```

   Both run the per-field fallback above against the stored `EnableConfig`s in
   `self.tool` and `self.defaults`.
   `is_enabled()` is the convenience wrapper for `effective_enable().state` and
   replaces today's `enable()` at every call site.

2. **Partial-config seam** — for CLI directive consumers operating on
   `PartialAppConfig` *before* `from_partial_with_defaults` runs
   (`apply_enable_tools`, `apply_tool_use`):

   ```rust
   impl PartialEnableConfig {
       pub fn effective(&self, defaults: &PartialEnableConfig) -> Enable;
   }
   ```

   The CLI path reads the per-tool partial at
   `partial.conversation.tools.tools.<name>.enable` and the defaults partial at
   `partial.conversation.tools.defaults.enable`, then calls `effective` to
   obtain the same filled `Enable` the final-config seam would produce.
   Resolving at the partial layer avoids building a temporary final config
   purely for the directive check.

Today this is inconsistent: `tool_definitions()` goes through
`ToolConfigWithDefaults::enable()` and sees defaults-merged values, but
`apply_tool_use` filters partial config directly with
`cfg.enable.is_some_and(Enable::is_on)`, which fails for tools that rely on the
default-on fallback.
Under this RFD every consumer routes through the same resolver, so `--tool-use
NAME` works for builtins (e.g.
`describe_tools`) and for user tools that leave `enable` unset.

If [RFD 056] / [RFD 057] land, the same per-field resolution extends across
their group-default and group-override layers in the order those RFDs define —
this RFD does not constrain that ordering.
[RFD 057] also separately commits to "CLI flags always win over group
overrides."
A group override that sets `allow_toggle = Never` would, under this RFD's
directive engine, block a named CLI directive — which conflicts with that
commitment.
This RFD takes no position on the resolution; [RFD 057] must decide whether
group-sourced `allow_toggle` blocks CLI directives or whether CLI directives
bypass it.

### Directive engine

Directives are classified by *scope*:

```rust
pub enum ToggleScope {
    Bulk,        // -t / -T with no argument
    Named,       // -t NAME / -T NAME
    NamedGroup,  // -t GROUP / -T GROUP — reserved for RFD 055
}
```

The directive engine asks two questions per (directive, tool) pair:

1. Does `allow_toggle` permit this directive scope?
   (`Enable::accepts(scope)`)
2. Would applying the directive flip `state`?

| `accepts(scope)` | `state` already matches intent | `state` would flip                 |
| ---------------- | ------------------------------ | ---------------------------------- |
| `true`           | trivially OK — no work         | apply: flip `state`                |
| `false`          | trivially OK — no work         | error (named) or skip (bulk/group) |

The `accepts` predicate:

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

Under this rule, `-t -T` and `-T -t` preserve the policy of any tool with
`allow_toggle ≠ Always`.
The two bugs in [Motivation](#motivation) become unrepresentable.

### `--tool-use NAME` validation

`apply_tool_use` validates that the named target is configured and effectively
enabled (`state == true`).
The filter switches from "only `Enable::On`" to "`state == true`," regardless of
`allow_toggle`.
`jp -u describe_tools` becomes valid because `describe_tools.state` is `true`.

This is a config-eligibility check, not a delivery guarantee: it runs on the
partial config before MCP servers start, so it cannot promise the tool reaches
the LLM.
A tool backed by an optional MCP server that fails to start is still dropped
later by `tool_definitions()`.
Reconciling that runtime mismatch — a forced tool absent from the resolved list
— is a pre-existing concern orthogonal to this RFD and is left to the runtime
access-control track.

### Locked-off means hidden

A tool with `state = false, allow_toggle = Never` (the canonical locked-off
case) is treated as **truly off**, not just immune to CLI directives.
Three rules implement this:

1. **`tool_definitions()` always drops locked-off tools**, regardless of the
   `forced_tool` exemption that today protects an `assistant.tool_choice` match.
   The current short-circuit in `crates/jp_llm/src/tool.rs` includes a forced
   tool even when its enable check returns `false`; under this RFD that
   exemption no longer applies when the tool is locked-off.
2. **`assistant.tool_choice = "foo"` is rejected during final `AppConfig`
   validation** when `foo` resolves to a locked-off tool.
   The check lives at the `AppConfig` level (the only validator that sees both
   `assistant.tool_choice` and `conversation.tools`), not in
   `ToolsConfig::validate`.
   The error names both config paths — `assistant.tool_choice` and
   `conversation.tools.<name>.enable` — rather than silently coercing, or
   passing through to a provider that will reject the request anyway
   (Google/Gemini does).
   This mirrors the existing `--tool-use NAME` validation against the enabled
   set.
3. **`Ctx::configure_active_mcp_servers` already drops locked-off MCP tools**,
   since it filters on `is_enabled()` before consulting any forced name.
   No additional change is needed there beyond the `is_enabled()` rewrite
   covered in Phase 2.

The asymmetry is intentional: only `(state = false, allow_toggle = Never)` gets
the stronger treatment.
`(state = true, allow_toggle = Never)` is locked-on — the tool is always
present, no semantic conflict.
The other locked combinations don't exist: `Never` only pairs meaningfully with
these two `state` values.

This pulls one specific case out of the runtime access-control track ([RFD 075]
/ [RFD 076] / [RFD 077]).
The justification is honesty: if the terminology says "locked off" and the
schema lets users declare it, the runtime must back it up.
Broader runtime enforcement — argument-level policy, tool-call sandboxing,
plugin trust — remains out of scope and stays with the access-policy track.

### Behavior matrix

Behavior under bulk and named directives (group parsing lands in [RFD 055]).
Until then, `IfNamedOrGroup` behaves identically to `IfNamed` for both `-t` and
`-T`:

| Tool config            | `-t name` | `-T name` | `-t` bulk | `-T` bulk |
| ---------------------- | --------- | --------- | --------- | --------- |
| `state=true, Always`   | no-op     | flips off | no-op     | flips off |
| `state=true, Never`    | no-op     | **error** | skip      | skip      |
| `state=true, IfNamed`  | no-op     | flips off | skip      | skip      |
| `state=false, Always`  | flips on  | no-op     | flips on  | no-op     |
| `state=false, Never`   | **error** | no-op     | skip      | skip      |
| `state=false, IfNamed` | flips on  | no-op     | skip      | skip      |

`IfNamedOrGroup` rows are intentionally omitted: until [RFD 055] introduces `-t
GROUP` parsing, they collapse to the `IfNamed` rows above.

Errors carry an `allow_toggle`-aware message ("cannot disable `describe_tools`:
this tool is configured as locked-on") rather than the legacy "system tool
cannot be disabled" framing.

### Predicates and helpers

```rust
impl EnableConfig {
    // Stored-form constants used by builtin registrations and other
    // code that needs a compile-time-known configuration.
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
Some(EnableConfig::LOCKED_ON)`.

## Backward compatibility

The deserializer accepts the legacy string forms — `"on"`, `"off"`, `"always"`,
`"explicit"` — and rewrites each to its canonical struct form at parse time:

| Legacy input          | Canonical form                            |
| --------------------- | ----------------------------------------- |
| `enable = "on"`       | `{ state: true, allow_toggle: Always }`   |
| `enable = "off"`      | `{ state: false, allow_toggle: Always }`  |
| `enable = "always"`   | `{ state: true, allow_toggle: Never }`    |
| `enable = "explicit"` | `{ state: false, allow_toggle: IfNamed }` |

The new `allow_toggle` field is a string enum: `"any"` (freely toggleable, the
default), `"never"` (locked), `"if_named"`, or `"if_named_or_group"`.
The freely-toggleable variant is deliberately spelled `"any"` rather than
`"always"`, so it cannot be confused with the legacy `enable = "always"`
shorthand — which means the opposite (locked-on).
The legacy string forms stay on the outer `enable` value only; `allow_toggle`
never accepts them.

This is required for **conversation persistence**.
The compat deserializer (`jp_conversation::compat::deserialize_partial_config`)
is consumed by both the base config snapshot (`base_config.json`) and
`config_delta` events in the event stream, so any conversation created before
this RFD landed may carry legacy `enable` values on either surface.
Without compat-aware parsing, the affected stored config layer (base snapshot or
`config_delta` event) would fail typed deserialization and be replaced with an
empty partial after a warning (see
`jp_conversation::compat::deserialize_partial_config`).
The conversation would still open, but legacy `enable` values would be silently
lost, changing tool availability for old conversations.

Output is always canonical.
The compat path is read-only — no new code emits legacy strings.
On the first re-serialization after this RFD lands (e.g. a subsequent
`config_delta` write, or a `jp config set` against the user's config file),
legacy strings normalize to bool shorthand or the map form.

In-code builtin registrations (currently `Enable::Always` for `describe_tools`)
are updated to `EnableConfig::LOCKED_ON` as part of the same change.

RFD updates accompany the code change:

- [RFD 008] gets a TIP under "Design" noting that directive state-mutation is
  now gated by `allow_toggle`.
  The existing `test_interleaved_disable_all_then_enable_all` is replaced by
  tests asserting policy preservation.
- [RFD 055] is amended to remove `Enable::ExplicitOrGroup` from its
  Implementation Plan, the `explicit_or_group` row from the "Interaction with
  Tool `enable` Field" table, the related Drawbacks entry ("New `Enable`
  variant"), and the Risks/Open Questions entry on `Enable` enum growth.
  Tool groups land with `AllowToggle::IfNamedOrGroup` already in the schema and
  just need the `ToggleScope::NamedGroup` parser.
- [RFD 056] is updated to refer to the stored `EnableConfig` shape and per-field
  resolution, replacing the `Enable` field type and the `enable()` /
  `enable_mode()` accessor names with `is_enabled()` / `effective_enable()`.
- [RFD 057]'s `apply_enable_tools` wording is updated to route through the
  partial-config resolver seam (`PartialEnableConfig::effective`).
  The existing note that [RFD 057] must decide whether group-sourced
  `allow_toggle = Never` blocks CLI directives is preserved.
- [RFD 083] is updated to fix stale `allow_toggle` wording: the bool-shorthand
  note that "resets `allow_toggle` to `always`" becomes `any`, and the builtin
  registration examples change from the resolved `Enable { ... }` to the stored
  `EnableConfig { ... }` form.
  The `Requires: [RFD 081]` relationship is unchanged.

## Drawbacks

**One-way serde normalization.** Legacy string inputs (`"on"`, `"off"`,
`"always"`, `"explicit"`) serialize back as their canonical form.
The shape is stable from the first write-back onward.
Round-trip is exact only for inputs already in canonical form — consistent with
existing `Enable` behavior (today's `enable = "on"` already collapses to `enable
= true`).

**Slightly more TOML for non-default cases.** Non-default `allow_toggle` values
require the map form: `enable = { state = true, allow_toggle = "never" }` in new
configs is more verbose than the legacy `enable = "always"`.
The map form is more discoverable in exchange — each field states exactly one
fact.

**`AllowToggle::IfNamedOrGroup` is unreachable until [RFD 055] lands.** The
variant is in the schema and the `accepts` predicate handles it, but no
directive parser produces `ToggleScope::NamedGroup` until tool groups ship.
A user who writes `allow_toggle = "if_named_or_group"` today gets `if_named`
behavior until group parsing arrives, matching the user-facing note in [TOML
surface](#toml-surface).
The unreachable code is a deliberate forward-compat investment to avoid a schema
change when [RFD 055] lands.

## Alternatives

Four alternatives were considered before this shape was chosen.
Each is rejected for the reason given.

**Patch the predicate, leave the directive engine alone.** Update `enable()` to
recognize `Enable::Always` as enabled.
Fixes Bug 1, leaves Bug 2 and the variant proliferation problem in place.
Rejected as a half-fix.

**Two top-level fields, `enabled: Option<bool>` + `policy: Option<Policy>`.**
Clean per-field separation.
Rejected for two reasons.
First, two top-level fields flatten the (state, policy) pair into the tool's
namespace where they mingle with unrelated fields like `run`, `result`, and
`command`, and lose the visual grouping that signals these two values jointly
define activation behavior.
Bool shorthand is also harder to preserve when the policy field is a sibling
rather than a sub-field — `enable = true` has no natural counterpart for the
policy half.
Second, the field name `policy` clashes with [RFD 075] / [RFD 076] / [RFD 077]
(`AccessPolicy`, `RunPolicy`, `TrustPolicy`).

**Single field with nested `Enable::State | Enable::Policy` variants.** Same
architectural properties as the chosen shape, but introduces a deep nested type
(`Enable::State(EnableState::On)`) that propagates through every match site.
The chosen `Enable { state, allow_toggle }` struct expresses the same
distinction with less syntactic weight.

**Flat enum with exhaustive `is_enabled()` / `is_policy()` methods.** The
minimal-invasive alternative: keep `Enable` as a flat enum, fix the predicates
to enumerate variants exhaustively.
Smallest diff, but each new policy concept (`Sticky`, `ExplicitOrGroup`) still
requires a new variant, new match arm, and new test surface.
Doesn't address the variant proliferation problem.

## Non-Goals

**Tool group directive parsing.** [RFD 055] introduces `-t GROUP` / `-T GROUP`
directive parsing.
This RFD reserves `ToggleScope::NamedGroup` and includes
`AllowToggle::IfNamedOrGroup` so that group parsing becomes a parser-only
change, but does not implement group parsing itself.

**Runtime access control.** [RFD 075] / [RFD 076] / [RFD 077] govern what a tool
may *do* at runtime (filesystem, network, subprocess).
`allow_toggle` governs what may *change the tool's enable state at config time*.
The two are orthogonal concepts that share no fields.

**Runtime tool-choice enforcement beyond the locked-off case.** `allow_toggle`
does not validate `assistant.tool_choice` in general.
A persisted `tool_choice = "foo"` where `foo` has `state = true, allow_toggle =
Always` is still forced into the request payload even after a CLI `-T foo`
directive flipped `state` to `false`, because that flip happens on a
freely-toggleable tool.
The narrow case of a locked-off tool (`state = false, allow_toggle = Never`)
**is** enforced — see [Locked-off means hidden](#locked-off-means-hidden) — to
keep the terminology honest.
Broader runtime enforcement (argument-level policy, tool-call sandboxing, plugin
trust) remains with the access-policy track ([RFD 075] / [RFD 076] / [RFD 077]).

## Risks and Open Questions

**Persona-layer composition.** Per-field defaults inheritance is the right
composition rule for layered config, but the existing persona system has not
been exercised against this kind of split field.
Verify during implementation that the partial-config delta machinery handles
`PartialEnableConfig` correctly when only one of `state` / `allow_toggle` is set
in a layer.

**Round-trip fidelity in `jp config set`.** Verify that `jp config set` against
a TOML containing `enable = { state = true, allow_toggle = "never" }`
round-trips without mutation.
Verify that `jp config set` against a TOML containing legacy `enable = "always"`
produces `enable = { state = true, allow_toggle = "never" }` on the first
write-back and then stays stable.
This depends on the inline-table merge fix in Phase 1: the format-preserving
TOML merge (`deep_merge_toml` in `crates/jp_config/src/fs.rs`) recurses only
through standard tables today, so without the fix a nested `jp config set
conversation.tools.foo.enable.state ...` against an inline-table `enable` would
replace the whole value and drop `allow_toggle`.

**Legacy strings in persisted conversation data.** Conversations created before
this RFD landed may contain legacy `enable = "..."` strings in both their
`base_config.json` snapshot and their stored `config_delta` events.
The compat deserializer must accept these on load on both surfaces;
re-serialization (e.g. via a subsequent `config_delta` write) normalizes them.
Add regression tests that load a conversation with legacy `enable` values in its
base config and in its event stream, and assert the merged config exposes the
correct `state` / `allow_toggle`.

**Impact on tool config mutation grants.** [RFD 078] (Accepted, not yet
implemented) lets tools write to paths under `conversation.tools.*` via the
`access.config` grant model.
Existing payloads that write a bool or legacy string (`true`, `"on"`,
`"always"`, …) to `conversation.tools.*.enable` remain valid — the
deserializer accepts them unchanged, so no migration is forced.
A tool only needs the map form (`{ state, allow_toggle }`) when it wants to set
one subfield while preserving the other from a lower layer.
When [RFD 078] is implemented, confirm grant payloads and any built-in
config-mutating tools handle the map form; whole-value writes need no grant-path
change.

## Implementation Plan

### Phase 1: type and serde

- Replace `Enable` (currently a flat enum) with the stored/resolved pair in
  `jp_config/src/conversation/tool.rs`: `EnableConfig` for the stored form (with
  `Option<bool>` and `Option<AllowToggle>` fields; lives in `ToolConfig.enable`
  and `ToolsDefaultsConfig.enable` as `Option<EnableConfig>`), and `Enable` for
  the resolved form returned by the effective-enable resolver.
- Add `AllowToggle` enum and `ToggleScope` enum.
  `ToggleScope` lives in `jp_config` alongside `Enable` because
  `Enable::accepts` consumes it; the CLI directive parser in `jp_cli` produces
  values of this type rather than defining its own.
- Implement `Serialize` / `Deserialize` with the bool-or-map shape for
  `EnableConfig` and `PartialEnableConfig`.
  `Enable` only needs `Serialize` — it is produced by the resolver, not
  deserialized directly.
- Implement `Schematic`.
- Add the `EnableConfig::ON` / `OFF` / `LOCKED_ON` / `LOCKED_OFF` constants and
  the `Enable::is_enabled`, `Enable::is_locked`, and `Enable::accepts`
  predicates.
- Update the partial-config infrastructure (`PartialEnableConfig`, `ToPartial`,
  `PartialConfigDelta`, `AssignKeyValue`) for the new struct shape.
- Implement compat-aware deserialization: accept `true` / `false` / `"on"` /
  `"off"` / `"always"` / `"explicit"` as input and rewrite each to the canonical
  struct form at parse time.
  Output is always canonical — the compat path is read-only.
- Make the format-preserving TOML merge recurse into inline tables.
  `deep_merge_toml` in `crates/jp_config/src/fs.rs` recurses only through
  standard tables today (`as_table_mut` / `as_table`), so a nested `jp config
  set conversation.tools.foo.enable.state ...` against an `enable = { state =
  true, allow_toggle = "never" }` inline table would replace the whole value and
  drop `allow_toggle`.
  Switch the recursion to table-likes (`as_table_like_mut` / `as_table_like`) so
  inline and standard tables both deep-merge.

### Phase 2: predicates and ctx

- Add `ToolConfigWithDefaults::effective_enable() -> Enable` and
  `ToolConfigWithDefaults::is_enabled() -> bool`, both performing the per-field
  fallback against the stored `EnableConfig`s.
  Replace today's `ToolConfigWithDefaults::enable()` with `is_enabled()` at
  consumer sites.
- Add `PartialEnableConfig::effective(&self, defaults: &PartialEnableConfig) ->
  Enable` for the partial-config consumers exercised in Phase 3.
- Verify `Ctx::configure_active_mcp_servers` works against `is_enabled()`.
- Update `tool_definitions()` to use `is_enabled()`, and remove the
  `forced_tool` exemption for locked-off tools (`state == false && allow_toggle
  == Never`).
  Locked-off tools are filtered out regardless of `assistant.tool_choice`.
  See [Locked-off means hidden](#locked-off-means-hidden).
- Update the builtin registration of `describe_tools` to
  `EnableConfig::LOCKED_ON`.

### Phase 3: directive engine

- Rewrite `apply_enable_tools` to use the scope-vs-policy model.
  The bulk-only filters (`is_explicit`, `is_always`) are removed.
  The named-disable guard is generalized to the named directive case.
- `ToggleScope::NamedGroup` is added but no parser produces it (parking spot for
  [RFD 055]).
- Route `apply_tool_use` and `apply_enable_tools` through the partial-config
  resolver seam (`PartialEnableConfig::effective`; see [Effective enable
  resolution](#effective-enable-resolution)) instead of inspecting raw partial
  fields.
- Add an `AppConfig`-level validation check (in `AppConfig::validate`, which
  sees both `assistant` and `conversation.tools`) that rejects
  `assistant.tool_choice = Function(name)` when `name` resolves to a locked-off
  tool.
  The error names both `assistant.tool_choice` and
  `conversation.tools.<name>.enable`, matching how `--tool-use NAME` validates
  against the enabled set today.

### Phase 4: tests and RFD updates

- Replace `test_interleaved_disable_all_then_enable_all` with tests asserting
  (a) `allow_toggle` is preserved across all interleaved directive sequences for
  every policy, and (b) `state` changes only when `accepts(scope)` is true and
  the directive intent differs from the current state.
- Add tests for the named-directive error paths and silent no-op paths.
- Add tests for `tool_definitions()` and `Ctx::configure_active_mcp_servers`
  against `EnableConfig::LOCKED_ON`.
- Add layered-merge tests:
  - `enable = { state = true }` on a tool inherits `allow_toggle` from
    `[conversation.tools.'*']`.
  - `enable = true` (bool shorthand) fully specifies both fields, overriding any
    inherited `allow_toggle`.
  - omitted `enable` resolves to the default-on, freely-toggleable pair.
- Add `--tool-use NAME` tests:
  - accepts a tool whose effective `state` is true via defaults only (no
    per-tool `enable` set).
  - accepts `describe_tools` (locked-on builtin).
- Add compat tests for legacy `"always"` / `"explicit"` strings appearing in
  both `base_config.json` snapshots and `config_delta` events.
- Add `jp config set` round-trip tests for `conversation.tools.foo.enable.state`
  and `conversation.tools.foo.enable.allow_toggle`.
- Add locked-off enforcement tests:
  - `tool_definitions()` excludes a locked-off tool even when
    `assistant.tool_choice` names it.
  - `assistant.tool_choice = "foo"` is rejected at config resolution when `foo`
    is locked-off.
  - `Ctx::configure_active_mcp_servers` does not start the MCP server backing a
    locked-off MCP tool.
- Update [RFD 008] with a TIP describing the new gating.
- Update [RFD 055] to drop `Enable::ExplicitOrGroup` and the surrounding
  interaction table row, drawbacks, and risks entries.
- Update [RFD 056] to refer to the stored `EnableConfig` shape and per-field
  resolution, replacing the `Enable` field type and `enable()` / `enable_mode()`
  accessor names with `is_enabled()` / `effective_enable()`.
- Update [RFD 057]'s `apply_enable_tools` wording to route through
  `PartialEnableConfig::effective`, preserving the existing CLI-vs-group
  `allow_toggle = Never` open question.
- Update [RFD 083] to fix the stale "resets `allow_toggle` to `always`" wording
  (now `any`) and convert its builtin registration examples from `Enable { ...
  }` to the stored `EnableConfig { ... }` form.

Phases 1–3 are interdependent and ship as one change.
Phase 4 (tests and RFD updates) lands in the same PR.

## References

- [RFD 008] — Ordered tool directives (directive evaluation semantics).
- [RFD 055] — Tool groups (consumer of `AllowToggle::IfNamedOrGroup`).
- [RFD 056] — Group configuration defaults (group-level `enable` inheritance).
- [RFD 057] — Group configuration overrides (group-level `enable` enforcement).
- [RFD 075] — Tool sandbox and access policy (`AccessPolicy`, naming context).
- [RFD 076] — Tool access grants (`AccessPolicy`).
- [RFD 077] — Plugin configuration and trust policy (`RunPolicy`,
  `TrustPolicy`).
- [RFD 078] — Tool config mutation (writes to `conversation.tools.*.enable`).
- [RFD 079] — Config sources and load order (the source/precedence model this
  RFD's cross-layer merge participates in).
- [RFD 083] — `ask_user` tool (originally introduced `Enable::Sticky`).
- `crates/jp_config/src/conversation/tool.rs` — `Enable`, `ToolConfig`,
  `ToolConfigWithDefaults`.
- `crates/jp_cli/src/cmd/query.rs` — `apply_enable_tools`, `apply_tool_use`.
- `crates/jp_cli/src/cmd/query/tool/builtins.rs` — builtin tool registration.
- `crates/jp_llm/src/tool.rs` — `tool_definitions()`.

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
