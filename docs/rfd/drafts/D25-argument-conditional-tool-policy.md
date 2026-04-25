# RFD D25: Argument-Conditional Tool Policy

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-24

## Summary

This RFD introduces argument-conditional tool policies. Today, `RunMode` and
`ResultMode` (`ask`, `unattended`, `edit`, `skip`) are per-tool-name only —
every invocation of a tool uses the same mode regardless of arguments. This RFD
makes them conditional on tool call arguments: a user can allow unattended file
modifications in `src/` while requiring approval for `.env`, or auto-deliver
results for safe operations while reviewing results that touch sensitive paths.
The `run` and `result` fields move into a new `policy` namespace and accept
either a backward-compatible string alias or an ordered array of conditional
rules evaluated with first-match-wins semantics.

## Motivation

JP's current permission model is binary: a tool either always asks for
confirmation or never does. The `run` field on tool configuration accepts a
single `RunMode` value that applies to every invocation regardless of what the
tool is asked to do.

This creates a tension between safety and flow:

1. **Overly permissive.** Setting `fs_modify_file.run = "unattended"` means
   the assistant can modify any file without approval — including `.env`,
   `Cargo.toml`, or files outside the intended working area.

2. **Overly restrictive.** Setting `fs_modify_file.run = "ask"` means every
   file modification requires a confirmation prompt, including routine changes
   to `src/` files that the user trusts completely. This interrupts flow and
   trains users to approve without reading.

3. **No middle ground.** There is no way to express "unattended for files in
   `src/`, ask for everything else." The granularity is per-tool, not
   per-argument.

The same problem applies to `result` (how tool results are delivered to the
assistant) and will apply to any future per-invocation policy.

This RFD solves the problem by making run policy conditional on the actual
arguments of each tool call.

### Relationship to RFD 076 and D04

[RFD 076] defines typed access grants (`AccessPolicy`) that declare what
resources a tool *can* access. [RFD 075] enforces those grants at the OS level.
This RFD addresses a different concern: not *what* a tool can do, but *whether
the user is prompted* before the tool runs.

| Concern | RFD 076 / D04 | This RFD |
|---------|---------------|----------|
| **Question** | "Can this tool access this resource?" | "Should JP prompt before running?" |
| **Enforcement** | Tool self-check + OS sandbox | Coordinator prompt logic |
| **Granularity** | Per-path capabilities | Per-argument run mode |
| **Config key** | `access` | `policy.run` |

The two systems are complementary. A tool may have broad access rights (D24)
but still require approval for sensitive arguments (this RFD). Or a tool may
have restricted access but run unattended because the access policy already
constrains it sufficiently.

Both systems share the `path` parameter type introduced in this RFD, and both
use path-prefix matching where applicable. The matching infrastructure (prefix
evaluation, path normalization) can be shared at the implementation level.

## Design

### The `policy` namespace

A new `policy` section in tool configuration groups execution policy settings:

```toml
[conversation.tools.fs_modify_file.policy]
run = "ask"
result = "unattended"
```

The existing top-level `run` and `result` fields continue to work as aliases
for backward compatibility:

```toml
# These are equivalent:
[conversation.tools.fs_modify_file]
run = "ask"

[conversation.tools.fs_modify_file.policy]
run = "ask"
```

If both are present, `policy.run` takes precedence and the top-level `run` is
ignored with a deprecation warning. The same applies to `result`.

The `policy` namespace also applies to `ToolsDefaultsConfig`:

```toml
[conversation.tools.*.policy]
run = "ask"
result = "unattended"
```

### Run policy: string or rules array

The `policy.run` field accepts either a string (backward-compatible alias) or
an ordered array of conditional rules:

```toml
# String alias (equivalent to a single catch-all rule):
run = "ask"

# Array of rules:
run = [
    { arg = "/path", prefix = "src/sensitive/", mode = "ask" },
    { arg = "/path", prefix = "src/", mode = "unattended" },
    { arg = "/path", prefix = ".env", mode = "ask" },
    { mode = "ask" },
]
```

The string aliases `"ask"`, `"unattended"`, `"edit"`, and `"skip"` desugar to
`[{ mode = "<value>" }]` — a single catch-all rule with no conditions. This
preserves the existing behavior: a plain `run = "ask"` or `run = "unattended"`
requires no argument inspection and can be decided before any arguments arrive.

### Rule structure

Each rule in the array has:

- **Zero or one condition** (`arg` + matcher). A rule with no condition is a
  catch-all.
- **A mode** (`mode`). The `RunMode` to use when this rule matches.

```toml
# Condition on a top-level parameter:
{ arg = "/path", prefix = "src/", mode = "unattended" }

# Condition on a nested parameter (see "Argument paths" below):
{ arg = "/patterns/paths", prefix = ".env", mode = "ask" }

# Catch-all (no condition):
{ mode = "ask" }
```

Compound conditions (matching multiple parameters simultaneously) are out of
scope for this RFD. See [Non-Goals](#non-goals).

### Evaluation: first-match-wins

Rules are evaluated top to bottom. The first rule whose condition is satisfied
determines the run mode. If no rule matches, an implicit `{ mode = "ask" }` is
appended as a safety fallback.

```toml
run = [
    { arg = "/path", prefix = "src/sensitive/", mode = "ask" },
    { arg = "/path", prefix = "src/", mode = "unattended" },
    { mode = "ask" },
]
```

For `path = "src/sensitive/secret.rs"`:
- Rule 1: prefix `"src/sensitive/"` matches → **`ask`**

For `path = "src/lib.rs"`:
- Rule 1: prefix `"src/sensitive/"` does not match → skip
- Rule 2: prefix `"src/"` matches → **`unattended`**

For `path = "README.md"`:
- Rule 1: no match → skip
- Rule 2: no match → skip
- Rule 3: catch-all → **`ask`**

First-match-wins means the user controls priority through declaration order.
More specific rules go first; broader rules and catch-alls go last. This is the
same model used by firewalls, nginx location blocks, and route tables.

### Argument paths (JSON Pointer)

The `arg` field uses [JSON Pointer (RFC 6901)][rfc-6901] syntax to identify
which parameter value to match against:

| `arg` value | Resolves to |
|-------------|-------------|
| `/path` | Top-level `path` parameter |
| `/patterns/paths` | Nested: each pattern's `paths` array |
| `/patterns/old` | Nested: each pattern's `old` field |
| `/source` | Top-level `source` parameter |

JSON Pointer parsing uses the [`jsonptr`][jsonptr] crate, which implements
RFC 6901 with `serde_json` integration.

**Array traversal.** Standard JSON Pointer uses numeric indices to address
specific array elements (`/patterns/0/paths/0`). For run policy evaluation, the
pointer navigates the tool's **parameter schema** rather than a specific
document instance. When a pointer segment resolves to an array type in the
schema, it implicitly means "any element" — the matcher is evaluated against
every element at that position in the actual arguments.

For example, given `arg = "/patterns/paths"` and arguments:

```json
{
  "patterns": [
    {
      "old": "foo",
      "new": "bar",
      "paths": [
        "src/a.rs"
      ]
    },
    {
      "old": "x",
      "new": "y",
      "paths": [
        ".env"
      ]
    }
  ]
}
```

The evaluator walks: `patterns[0].paths[0]` → `"src/a.rs"`, `patterns[0].paths[1]` (if any), `patterns[1].paths[0]` → `".env"`, etc. If **any** resolved value satisfies the matcher, the condition is met.

This existential ("any element matches") semantics is the secure default: since
tool calls are atomic and cannot be partially executed, a single sensitive value
anywhere in the arguments should trigger the restrictive policy.

**Validation at config load time.** JP walks the tool's `ToolParameterConfig`
tree following the pointer segments. Each segment must resolve to a valid
property or array item type. Mismatches (e.g., treating a string field as an
object, referencing a nonexistent property) are config errors.

### The `path` parameter type

This RFD introduces `path` as a recognized parameter type alongside the
existing `string`, `number`, `integer`, `boolean`, `array`, and `object` types:

```toml
[conversation.tools.fs_modify_file.parameters.path]
type = "path"
summary = "The path to the file to modify."
```

The `path` type affects two things:

1. **JSON Schema generation.** `to_json_schema()` emits `"type": "string"` for
   `path` parameters. LLMs see a standard string field. The `path` type is a
   JP-internal semantic annotation, not a JSON Schema type.

2. **Matcher behavior.** The `prefix` matcher on a `path`-typed parameter uses
   path-component-count for matching instead of byte-length string prefix.
   `prefix = "src/"` matches `"src/lib.rs"` (component match) but not
   `"src-old/lib.rs"` (byte prefix but not component prefix). Path values are
   normalized before matching (resolve `.`, `..`, strip trailing separators).

Tool definitions should migrate path-like `string` parameters to `path` where
the value represents a filesystem path. This is backward-compatible for LLMs
(they still see `"type": "string"`) and enables more accurate policy matching.

### Type-constrained matchers

Each rule condition consists of an `arg` (JSON Pointer to the parameter) and
exactly one matcher keyword. The available matchers depend on the resolved
parameter type. This follows [JSON Schema validation vocabulary][json-schema-validation]
where applicable, with a JP-specific `prefix` extension for path matching.

#### Matchers for any type

| Keyword | JSON Schema | Description |
|---------|:-----------:|-------------|
| `const` | ✓ | Exact value match. Accepts any JSON value. |
| `enum` | ✓ | Set membership. Value must equal one of the listed values. |

```toml
{ arg = "/util", const = "jq", mode = "ask" }
{ arg = "/util", enum = ["date", "wc", "head", "tail"], mode = "unattended" }
{ arg = "/replace_using_regex", const = true, mode = "ask" }
```

#### Matchers for `string` and `path` types

| Keyword | JSON Schema | Description |
|---------|:-----------:|-------------|
| `pattern` | ✓ | Regular expression match (ECMA-262 dialect). |
| `prefix` | ✗ (JP) | Path-prefix match. For `path`-typed params: component-aware. For `string`-typed params: byte-length prefix. |

```toml
{ arg = "/path", prefix = "src/", mode = "unattended" }
{ arg = "/patterns/old", pattern = "rm\\s+-rf", mode = "ask" }
```

The `prefix` matcher is not part of JSON Schema. It exists because path-prefix
matching is the primary use case for run policies and regex (`pattern = "^src/"`)
does not provide path-component-aware matching or normalization.

#### Matchers for `number` and `integer` types

| Keyword | JSON Schema | Description |
|---------|:-----------:|-------------|
| `minimum` | ✓ | Inclusive lower bound. |
| `maximum` | ✓ | Inclusive upper bound. |
| `exclusive_minimum` | ✓ | Exclusive lower bound. |
| `exclusive_maximum` | ✓ | Exclusive upper bound. |

```toml
{ arg = "/start_line", minimum = 1000, mode = "ask" }
```

Each rule supports exactly one matcher keyword. Range constraints (min AND max
on the same parameter) require compound conditions, which are out of scope for
this RFD.

#### Matchers for `boolean` type

Only `const` is available for boolean parameters. `enum` is technically valid
but redundant (booleans have only two values).

#### Validation

At config load time, JP validates:

1. The `arg` pointer resolves to a valid parameter in the tool's schema.
2. The matcher keyword is valid for the resolved parameter's type.
3. The matcher value's type is compatible (e.g., `const = true` on a `number`
   parameter is an error).

If validation fails, the configuration is rejected with a specific error
identifying the rule, the parameter, and the type mismatch.

### Unreachable rule detection

JP detects rules that can never match because an earlier rule on the same `arg`
always matches first. These are **config errors**, not warnings, because they
indicate the user's intent does not match their configuration.

Detected cases:

| Earlier rule | Later rule | Why it's unreachable |
|-------------|------------|----------------------|
| `prefix = "src/"` | `prefix = "src/sensitive/"` | Every value matching the later prefix also matches the earlier, shorter prefix. |
| `prefix = "src/"` | `const = "src/lib.rs"` | The const value starts with the earlier prefix. |
| `enum = ["jq", "wc"]` | `const = "jq"` | The const value is in the earlier enum set. |
| `enum = ["jq", "wc", "date"]` | `enum = ["jq", "wc"]` | The later set is a subset of the earlier set. |
| Catch-all `{ mode = "ask" }` | Any rule after it | A catch-all matches everything; nothing after it can fire. |

The detection is mechanical and has zero false positives. Every detected case is
provably unreachable under first-match-wins evaluation.

JP also warns (not errors) when no catch-all rule is present at the end of the
array. An implicit `{ mode = "ask" }` is appended for safety, but the user
should make the fallback explicit.

### Interaction with argument streaming

String aliases and catch-all-only policies (no conditions) are decided before
any arguments arrive — identical to the current behavior where `run = "ask"` or
`run = "unattended"` requires no argument inspection.

Rules with conditions are evaluated after all arguments arrive. Each rule's
`arg` pointer identifies exactly which parameter it depends on, which enables
a future optimization: evaluating rules incrementally as individual parameters
finish streaming, rather than waiting for the complete argument object.

> [!TIP]
> [RFD D26] defines this streaming evaluation model.

### Applying `policy.run` to `result`

The same rule structure applies to `policy.result`. The `mode` values correspond
to `ResultMode` instead of `RunMode`:

```toml
[conversation.tools.fs_modify_file.policy]
result = [
    { arg = "/path", prefix = ".env", mode = "ask" },
    { mode = "unattended" },
]
```

The evaluation model, argument paths, matchers, and validation are identical.

## Drawbacks

- **Ordering burden.** First-match-wins requires users to order rules from most
  specific to least specific. Users familiar with specificity-based systems
  (CSS, D24's longest-prefix-match) may find this counterintuitive. Config
  error detection for shadowed rules mitigates this, but the user must still
  understand the ordering model.

- **No compound conditions.** Cross-parameter conditions ("ask when path is in
  src/ AND regex is enabled") cannot be expressed. The user must approximate
  with single-parameter rules, which may be overly broad. See
  [Non-Goals](#non-goals).

- **New dependency.** The `jsonptr` crate is added for JSON Pointer parsing.
  This is a well-maintained crate (37M downloads) with minimal transitive
  dependencies, but it is a new dependency nonetheless.

- **Migration cost.** Moving `run` and `result` into `policy` requires a
  backward-compatibility shim and eventual deprecation of the top-level fields.
  During the transition, both forms coexist, which adds surface area to the
  config system.

- **Nested argument paths are complex.** Matching against
  `/patterns/paths` requires walking the parameter schema tree and iterating
  over array elements. This adds implementation complexity beyond simple
  top-level parameter matching.

## Alternatives

### Specificity-based matching

Instead of first-match-wins, evaluate all rules and pick the most specific
match (longest prefix wins, exact match beats prefix, etc.). This is the model
used by [RFD 076] for access grants.

Rejected because the specificity model introduces hidden ordering that is
difficult to reason about. Within a single matcher type (prefix vs prefix),
specificity is intuitive. Across types (does `const` beat `prefix`? does a
longer prefix beat an `enum`?), the ordering is arbitrary and must be memorized.
First-match-wins makes the priority explicit and visible in the configuration.

The cost is that users must order rules correctly, but config error detection
catches the most common mistake (shorter prefix before longer prefix on the same
parameter).

### Embed run mode in D24's `FsRule`

Attach a `run` field to `access.fs` rules, sharing D24's configuration surface
entirely.

Rejected because access grants and run policy are different concerns. `access`
controls what a tool *can* do (capability grant). `policy.run` controls whether
the user is *prompted* (UX decision). Coupling them means you must configure
access rules to get run-mode overrides, even when the default access policy is
sufficient.

### Generic predicate system with AND/OR combinators

A fully expressive predicate language supporting nested `and`/`or`/`not`
combinators across multiple parameters.

Rejected as over-engineered for current needs. Single-parameter conditions cover
the vast majority of use cases (path-based policies, command-name-based
policies). Compound conditions can be added later via the `all` key without
breaking existing configurations.

### Object-keyed `run` (grouped by parameter)

Structure `run` as an object keyed by parameter names rather than an array of
rules:

```toml
[conversation.tools.fs_modify_file.policy.run]
mode = "ask"
[conversation.tools.fs_modify_file.policy.run.path]
"src/" = "unattended"
```

Rejected because: compound conditions don't fit the shape, matcher type is
implicit (the key is a prefix? a const?), and cross-parameter rules have no
natural location.

## Non-Goals

- **Compound conditions.** Matching on multiple parameters simultaneously (e.g.,
  "path in src/ AND regex enabled") is not supported. A future RFD can add this
  via an `all` key on rules without breaking existing configurations.

- **Streaming argument evaluation.** Evaluating rules as individual parameters
  stream in (before the full argument object is available) is a future
  optimization. This RFD evaluates conditional rules after all arguments arrive.

> [!TIP]
> [RFD D26] extends D25 with streaming policy evaluation, consuming [RFD 043]'s
> per-parameter completion signals to resolve conditional rules before the full
> argument object arrives.

- **Access policy enforcement.** This RFD does not restrict what a tool can do.
  It only controls whether the user is prompted. Access restriction is handled
  by [RFD 076] (cooperative) and [RFD 075] (OS-level).

- **Result content inspection.** Conditioning `policy.result` on the tool's
  output (e.g., "ask before delivering results containing error messages") is
  out of scope. `policy.result` conditions match on the tool call's input
  arguments, not its output.

- **MCP tool arguments.** MCP tools receive arguments through the MCP protocol.
  Run policy evaluation applies to MCP tool calls the same way — the arguments
  are available as JSON before execution. No MCP-specific handling is needed.

## Risks and Open Questions

1. **JSON Pointer array traversal semantics.** Standard JSON Pointer uses
   numeric indices; this RFD uses implicit "any element" traversal for array
   types in the schema. This is a non-standard extension. Should the config
   syntax make array traversal explicit (e.g., requiring `/-` or `/*` at array
   positions) rather than inferring it from the schema?

2. **Parameter ordering for future streaming.** When conditional rules are
   evaluated during argument streaming (future RFD), the order in which
   parameters arrive matters. LLMs typically respect JSON schema property
   ordering, which comes from `IndexMap` iteration order in the tool TOML. This
   is not guaranteed by all providers. Should the future streaming RFD validate
   provider behavior, or treat out-of-order arrival as a graceful degradation
   (wait for all arguments)?

> [!TIP]
> [RFD D26] addresses this: out-of-order arrival is graceful degradation. The
> evaluator returns `Waiting` until the needed parameter arrives. No incorrect
> decisions, just lost optimization.

3. **`prefix` on `string`-typed parameters.** For `string`-typed parameters
   (not `path`), `prefix` uses byte-length string prefix matching. Should
   `prefix` be restricted to `path`-typed parameters only, forcing `string`
   parameters to use `pattern = "^..."` instead?

4. **Interaction with `RunMode::Edit`.** When a conditional rule resolves to
   `mode = "edit"`, the user edits the arguments. If the edit changes the
   matched parameter (e.g., changes the path from `src/` to `.env`), should
   JP re-evaluate the rules with the edited arguments? The current design
   does not re-evaluate — the mode is determined before the edit. Documenting
   this behavior may be sufficient.

5. **Global defaults with conditions.** Can `conversation.tools.*.policy.run`
   accept an array of rules? This would apply conditional rules as defaults for
   all tools, not just specific ones. The main question is whether the `arg`
   pointers make sense across tools with different parameter schemas. An `arg`
   that doesn't exist in a tool's schema would cause that rule to be skipped
   (not an error), since it's a default, not a tool-specific config.

## Implementation Plan

### Phase 1: `path` parameter type

- Recognize `"path"` in `ToolParameterConfig::kind`.
- Map `"path"` → `"string"` in `to_json_schema()`.
- Update JP's filesystem tool definitions to use `type = "path"` for path
  parameters.
- Unit tests for schema generation and type recognition.

No external dependencies. Can merge independently.

### Phase 2: `policy` namespace

- Add `PolicyConfig` struct with `run` and `result` fields to
  `jp_config::conversation::tool`.
- Add `policy` field to `ToolConfig` and `ToolsDefaultsConfig`.
- Implement backward compatibility: top-level `run`/`result` fields are read
  as `policy.run`/`policy.result` if `policy` is absent.
- Deprecation warning when both forms are present.
- `ToolConfigWithDefaults::run()` reads from `policy.run` first, falling
  back to the top-level field, then global defaults.
- Implement `AssignKeyValue`, `PartialConfigDelta`, `FillDefaults`, and
  `ToPartial` for the new types.

Depends on Phase 1 (for `path` type awareness in validation).

### Phase 3: Run policy types and evaluation

- Define `RunPolicy` enum (string alias or rules vec) and `RunRule` struct.
- Define `ParamCondition` and `TypedMatcher` types.
- Add `jsonptr` dependency to `jp_config` for JSON Pointer parsing.
- Implement rule evaluation: iterate rules, first match wins.
- Implement schema-walking validation: verify `arg` pointers resolve to valid
  parameters and matcher keywords are valid for the resolved type.
- Implement array traversal: when a pointer segment hits an array type,
  iterate all elements.
- Unit tests for evaluation logic, type validation, and array traversal.

Depends on Phase 2.

### Phase 4: Unreachable rule detection

- Implement shadowing detection for same-`arg` rules: prefix-shadows-prefix,
  prefix-shadows-const, enum-shadows-const, enum-shadows-enum,
  catch-all-shadows-any.
- Surface as config errors with specific messages naming the shadowing and
  shadowed rules.
- Warn when no catch-all is present.
- Unit tests for each detection case.

Depends on Phase 3.

### Phase 5: Integration with tool coordinator

- Modify `ToolExecutor::permission_info()` to return the full `RunPolicy`
  instead of a single `RunMode`.
- Modify `ToolCoordinator::decide_permission()` to evaluate the run policy
  against tool call arguments.
- For string aliases and catch-all-only policies, preserve the current
  fast path (no argument inspection needed).
- For conditional policies, evaluate after arguments are available.
- Apply the same changes for `policy.result` in result delivery.
- Integration tests verifying end-to-end behavior with conditional rules.

Depends on Phase 4.

### Phase 6: Migrate built-in tool definitions

- Update `.jp/mcp/tools/fs/*.toml` to use `type = "path"` for path parameters.
- Update `.jp/mcp/tools/git/*.toml` similarly.
- Optionally add example `policy.run` configurations to tool documentation.

Depends on Phase 1. Can proceed in parallel with Phases 2–5.

## References

- [RFD 076] — Tool access grants. Defines `AccessPolicy` types and cooperative
  enforcement. This RFD shares the `path` parameter type and path-prefix
  matching infrastructure.
- [RFD 075] — Tool sandbox and access policy. OS-level enforcement of access
  grants. Complements this RFD's prompt-level control.
- [RFD 042] — Tool options. Established per-tool `options` configuration.
- [RFC 6901] — JSON Pointer. Defines the syntax used for `arg` paths.
- [JSON Schema Validation] — Validation vocabulary. This RFD reuses `const`,
  `enum`, `pattern`, `minimum`, `maximum`, `exclusive_minimum`, and
  `exclusive_maximum` keywords.
- [`jsonptr` crate] — Rust implementation of RFC 6901 used for pointer parsing.

[RFD 076]: 076-tool-access-grants.md
[RFD 075]: 075-tool-sandbox-and-access-policy.md
[RFD D26]: D26-streaming-policy-evaluation-for-tool-call-arguments.md
[RFD 043]: 043-incremental-tool-call-argument-streaming.md
[RFD 042]: 042-tool-options.md
[RFC 6901]: https://www.rfc-editor.org/rfc/rfc6901.html
[rfc-6901]: https://www.rfc-editor.org/rfc/rfc6901.html
[JSON Schema Validation]: https://www.ietf.org/archive/id/draft-bhutton-json-schema-validation-01.html
[json-schema-validation]: https://www.ietf.org/archive/id/draft-bhutton-json-schema-validation-01.html
[`jsonptr` crate]: https://crates.io/crates/jsonptr
[jsonptr]: https://crates.io/crates/jsonptr
