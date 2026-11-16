# RFD D62: Configurable Tool Parameter Visibility

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-18
- **Extends**: [RFD 042]
- **Summary**: Adds an `enabled` flag on tool parameters to hide disabled
  parameters from model schemas and reject model-supplied values.

## Summary

This RFD adds an `enabled` flag to tool parameter configuration.
A parameter with `enabled = false` is omitted from the JSON Schema sent to the
model and from `describe_tools`, and is rejected if the model supplies it, while
the host still injects any configured `default` value.
Configuration layers can toggle `enabled` to keep context-specific parameters
hidden by default and expose them only when needed.

## Motivation

Some tool parameters are useful to the model only in specific contexts.
However, parameter visibility in JP is binary and permanent: a parameter is
either included in the schema for every conversation or omitted entirely.

Consider a tool that usually operates on the workspace root, but occasionally
needs to target another directory, such as a second build workspace inside the
repository or a checkout mounted alongside it.
Two designs are available without per-layer visibility:

1. **Declare the parameter unconditionally.** Every conversation includes an
   argument the model must reason about and can misuse, even though most
   conversations do not need it.
2. **Move the setting into `options`.** Options are configured by the host,
   hidden from the model, and fixed for the entire conversation.
   Because the host sets the value, the model cannot target one directory and
   then another within the same conversation.

Moving the setting to `options` works when the user should always fix the value.
However, JP conflates *who chooses the value* with *when the model is allowed to
choose it*.
Because visibility is tied to the parameter's declaration, a parameter cannot be
hidden by default and exposed on demand: declaring it as a parameter exposes it
everywhere, while moving it to `options` puts it permanently out of the model's
reach.

## Design

### Configuration

`ToolParameterConfig` gains an `enabled` boolean field:

```toml
[conversation.tools.run_build.parameters.root]
type = "string"
enabled = false
summary = "Directory to build in. Defaults to the workspace root."
```

A later config layer can re-enable the parameter:

```toml
[conversation.tools.run_build.parameters.root]
enabled = true
default = "crates/some-project"
```

`enabled` defaults to `true`, leaving existing configurations unchanged.

### Scope: top-level parameters only

`ToolParameterConfig` doubles as a recursive JSON Schema node (`properties`
describes an object's fields, and `items` describes an array's elements), so
adding a field to `ToolParameterConfig` exposes that field at every level of the
schema tree.

`enabled` is honored only on entries directly under `parameters`.
Setting `enabled` on a nested `properties` entry or inside `items` is a **config
error** that reports the full path to the offending entry.

Rejecting nested `enabled` settings rather than ignoring them prevents silent
misconfiguration.
Because `ToolParameterConfig` is recursive, schema completion offers `enabled`
on nested nodes and the parser accepts it.
Ignoring the field there would let the config load cleanly while leaving the
nested field visible to the model.

Restricting `enabled` to top-level parameters covers the motivating use case and
avoids the edge cases of recursive visibility: whether disabling an object
property also removes it from the parent object's `required` list, whether a
nested `default` is injected when the parent object itself is omitted, and what
disabling `items` means for an array.

This load-time validation rule is a stopgap for the shared recursive type.
Splitting `ToolParameterConfig` into a top-level parameter type (with `enabled`)
and a nested schema-node type (without it) would make nested `enabled` fields
unrepresentable in the schema and eliminate the validation check, but that
refactoring is not required to ship this feature.

### Behavior of disabled parameters

A disabled parameter is completely hidden from the model:

- It is omitted from the JSON Schema sent to the provider.
- It is omitted from `describe_tools` output.
- It is rejected if the model supplies it anyway.

The first two rules are schema filters that apply to every tool source.
Rejecting model-supplied values requires an additional check before tool
dispatch because argument validation only runs for local tools.
While `validate_tool_arguments` (`crates/jp_llm/src/tool.rs`) rejects any
argument absent from `parameters`, only `execute_local` calls it.
`execute_mcp` forwards the argument object directly to `mcp_client.call_tool`,
and `execute_builtin` passes it directly to the builtin executor without
validation.
Without a pre-dispatch check, a model that guesses a disabled parameter's name
(from earlier conversation history or ordinary schema noncompliance) would still
pass that argument to MCP and builtin tools.

Leaving disabled parameters unreachable for local tools but merely unadvertised
for MCP and builtin tools would break the guarantee that disabled parameters
cannot be set by the model.
Moreover, `parameters` already applies to MCP tools (unlike `options`, which
[RFD 042] scoped to local tools), so `enabled` must work across all tool
sources.

**This RFD therefore adds a source-independent preprocessing step before tool
dispatch.** For every tool source, this step:

1. Rejects any argument that names a disabled parameter.
2. Injects the configured `default` value for any disabled parameter.

### Scope of pre-dispatch preprocessing

Default injection for *enabled* parameters and rejection of unknown arguments
(arguments not declared in `parameters` at all) remain inside `execute_local`
and apply only to local tools.

Hoisting all of `apply_parameter_defaults` and `validate_tool_arguments` above
tool dispatch would unify argument handling across all three tool sources, but
it would also introduce unrelated behavioral changes:

- **MCP servers would receive default arguments they previously did not.** JSON
  Schema treats `default` as an annotation rather than a value for the caller to
  populate, and an MCP server may distinguish an omitted field from an
  explicitly supplied one even when the values match.
  Injecting defaults into enabled parameters would change the wire payload for
  every MCP tool with a configured default.
- **Builtin tools would reject undeclared arguments** that currently reach their
  executors untouched.

Unifying host-side argument handling across tool sources requires its own
compatibility analysis and is out of scope for this RFD.
Scoping the pre-dispatch step to disabled parameters ensures that configurations
without disabled parameters behave identically to existing releases.

### Disabled and `required` are orthogonal

`required` specifies whether a parameter must have a value, while `enabled`
specifies whether the model sees and can supply that parameter.
Combining `enabled = false` with `required = true` means the parameter is
mandatory for the tool, but the host supplies its value rather than the model:

|                    | `enabled = true`     | `enabled = false`                                          |
| ------------------ | -------------------- | ---------------------------------------------------------- |
| `required = false` | Model may supply it  | Tool falls back to its own default                         |
| `required = true`  | Model must supply it | `default` is mandatory; the config is rejected without one |

Two validation rules enforce this behavior:

**Argument validation checks only enabled parameters.** Because the model cannot
see a disabled parameter, tool call validation does not require the model to
supply it.
Validating against all configured parameters would fail every call when a
disabled parameter is marked `required = true`.

**Config resolution rejects `enabled = false` + `required = true` without a
`default`.** Without a `default`, the model cannot supply the parameter, the
host injects nothing, argument validation skips it, and the tool (which has no
visibility metadata) cannot distinguish it from an omitted optional parameter.
A mandatory value would never reach the tool.
Because this is a static configuration error rather than a runtime condition, JP
rejects it at config load time, following the precedent of
`reject_access_on_non_local_tools`.

### Default injection

For local tools, `apply_parameter_defaults` already populates any configured
`default` for omitted parameters before validation runs, because models
routinely omit defaulted parameters even when marked `required`.

Because a valid tool call always omits disabled parameters, a disabled parameter
with a `default` always receives that default value:

```toml
[conversation.tools.run_build.parameters.root]
enabled = false
default = "crates/some-project"
```

The tool receives `root = "crates/some-project"` just as if the model had passed
it.
Pinning a parameter to a fixed value therefore requires no separate mechanism
beyond setting `enabled = false` and `default`.

For local tools, `apply_parameter_defaults` already provides this behavior.
For MCP and builtin tools, which do not otherwise inject defaults, the
pre-dispatch visibility check injects defaults only for disabled parameters and
leaves enabled parameters untouched.

**Default injection for enabled parameters remains unchanged across all tool
sources.** Restricting default injection to disabled parameters would break
existing local tool configurations: when a model omits an enabled, required
parameter that has a `default`, the call succeeds rather than recording a failed
tool call in the conversation stream and forcing a retry.

### Stored vs. effective argument maps

The stored `ToolCallRequest` keeps the model's raw arguments.
Because stored requests are replayed to providers as conversation history,
persisting an injected default would expose a disabled parameter to the model on
subsequent turns.

Preprocessing therefore produces a separate **effective** argument map used by
every stage that inspects or executes the call:

| Map       | Contents                                 | Used for                                                               |
| --------- | ---------------------------------------- | ---------------------------------------------------------------------- |
| Stored    | Exactly what the model emitted           | The conversation stream; replay to providers as history                |
| Effective | Stored, plus disabled-parameter defaults | Permission prompts, argument formatting, rendering, tracing, execution |

**Preprocessing runs when the executor is created, before `permission_info`
builds the approval prompt.** Currently, `execute_local` injects defaults only
during execution, after the approval prompt has already been rendered from
`request.arguments`.
For an enabled parameter with a default, this causes an occasional mismatch when
the model omits the argument.
For a disabled parameter, the mismatch would happen on every call: because the
model never emits the parameter, its injected default would never appear in the
prompt.

For example, a tool configured with `run = "ask"` and a disabled `root`
parameter defaulting to a second build directory would prompt the user to
approve a call with no `root` argument (implying the workspace root) and then
execute in the second directory.
Running preprocessing before `permission_info` ensures that the approval prompt
displays the exact arguments the tool will receive.

If the user modifies arguments via `run = "edit"`, preprocessing and validation
re-run on the edited map so the final prompt and the executed call remain
consistent.
As with existing default injection, the arguments passed to the tool differ from
the raw `ToolCallRequest` stored in the conversation stream, which affects
stream auditing and replay, but the user approving the call always sees the
effective arguments.

### Tools stay ignorant of visibility

Tools do not receive metadata about which parameters are disabled; a tool simply
reads its arguments and applies its own fallback when an optional argument is
omitted.

Exposing the set of disabled parameters to tools would push visibility logic
into every tool that needs a context-dependent default, even though host-side
`default` injection already handles that case.
If a future tool cannot express its behavior through a default value, visibility
metadata can be exposed then.

### Relationship to tool options

Options and parameters serve distinct roles:

- An **option** is a host-configured setting that the model should never choose,
  such as a credential, a feature toggle, or an environment setting.
  Options never appear in the tool schema.
- A **parameter** is an input that the model can choose when appropriate.
  Without `enabled`, hiding a parameter in most contexts required converting it
  into an option, which also prevented the model from choosing it in contexts
  where it should be exposed.

The rule for choosing between them is simple: if the model should ever choose
the value, declare it as a parameter and use `enabled` to control which contexts
expose it.

## Drawbacks

**Effective parameter visibility depends on merged config layers, with no CLI
command to inspect it.** Reading a single tool configuration file does not show
what the model sees across merged layers.
`jp config show` prints only a commented template of available keys (or default
values with `--defaults`) rather than the merged configuration, and it does not
load conversation state.
A user investigating why the model ignores a documented parameter has no command
to check whether a layer disabled it.
[RFD 060] proposes
`--explain=conversation.tools.<tool>.parameters.<name>.enabled`, which would
provide this diagnostic, but [RFD 060] is still in Discussion.

**Three interacting parameter settings.** Combining `enabled`, `required`, and
`default` creates eight configuration states, one of which (`enabled = false`
with `required = true` and no `default`) is rejected at config resolution.
Authors must track a third setting when reading tool parameter definitions, and
the invalid combination is only surfaced when config loading fails.

## Alternatives

**Use `options` instead.** Keeping context-specific settings in `options`
requires no changes and works when the value is fixed for the entire
conversation, but it cannot expose the parameter to the model in contexts where
the model should choose the value.

**Keep [RFD 042]'s rejection of hidden parameters.** [RFD 042] considered and
rejected "hidden parameters that the LLM doesn't see but the tool reads" on two
grounds, both of which JP's current design addresses:

- *"The LLM might hallucinate values for these hidden parameters."* The
  source-independent pre-dispatch check rejects any call that supplies a
  disabled parameter across all tool sources.
- *"Validation might reject tool calls that omit them."*
  `apply_parameter_defaults` (added after [RFD 042]) populates defaults before
  validation runs, and validation only checks enabled parameters.

This RFD preserves [RFD 042]'s core distinction: a setting the model should
never choose belongs in `options`, whereas a parameter that is relevant to the
model only in specific contexts stays in `parameters` and uses `enabled` to
control its visibility.

**Tool groups ([RFD 055], [RFD 056], [RFD 057]).** Swapping entire tool
definitions per context so that specific contexts supply a variant with the
extra parameter would duplicate the tool definition just to toggle a single
field, and tool groups are still in Discussion.

**Name the flag `hidden` instead of `enabled`.** `hidden` implies a display-only
setting, whereas a disabled parameter is also rejected at call time.

**Split visibility into `state` and `allow_toggle` ([RFD 081]).** Tools separate
whether a tool is enabled from whether later config layers may toggle it.
No current use case requires locking a parameter's visibility against later
layers, so a single `enabled` boolean is simpler and can be split later if
needed.

## Non-Goals

- **Conditional visibility.** Visibility is configured statically per layer;
  parameters cannot be enabled dynamically based on expressions or other
  parameter values.
- **Per-call visibility.** Parameter visibility is fixed when tool definitions
  are resolved for an invocation.
- **Recursive visibility.** Setting `enabled` on nested `properties` or `items`
  is a configuration error.
- **Changing `options`.** Tool options keep their existing semantics.
- **Changing default injection for enabled parameters.** Existing default
  injection behavior for enabled parameters is preserved across all tool
  sources.
- **Unifying host-side argument handling across tool sources.** Generalizing
  default injection and unknown-argument rejection across local, MCP, and
  builtin tools is a separate change with its own compatibility requirements.
- **Exposing disabled-parameter metadata to tools.** Tools receive only the
  effective argument map.

## Risks and Open Questions

**Allowing users to supply disabled parameters during `run = "edit"`.** `run =
"edit"` lets the user modify tool call arguments before approval.
Because `enabled = false` restricts the model rather than the user, a
user-supplied value for a disabled parameter is a valid host override.
However, re-running validation on the single edited argument map cannot
distinguish user-added arguments from model-supplied ones: rejecting the edit
blocks a legitimate user override, while skipping the check after an edit allows
any disabled parameter in the edited map through.
Implementation must decide whether to track user edits separately or reject
disabled parameters in edited calls.

**MCP tools whose server-side schema differs from JP's config.** Rejecting
arguments that match a disabled entry in JP's `parameters` map assumes that JP's
config mirrors the MCP server's parameter names.
We should verify how this behaves when an MCP server declares parameters that
are only partially mirrored in JP's config.

**Merge ordering with [PR 998].** [PR 998] validates resolved tool schemas and
modifies the same schema resolution path, so we should coordinate the merge
order between the two changes.

**Interaction with [RFD D10].** [RFD D10] moves parameter defaults and argument
validation into `StdioRuntime` (one of several per-source runtimes), whereas
this RFD places the disabled-parameter check above tool dispatch so it applies
to all tool sources.
Because [RFD D10] is still a Draft, it does not block this RFD, but its
migration plan will need updating to keep the visibility check above the
per-source runtimes.

## Implementation Plan

**Phase 1: Config field and schema filtering.** Add `enabled` (defaulting to
`true`) to `ToolParameterConfig`.
Filter disabled parameters out of provider tool schemas and `describe_tools`
output.
Update argument validation to check only enabled parameters, reject `enabled` on
nested schema nodes (`properties` and `items`), and reject `enabled = false`
with `required = true` and no `default` during config resolution.
This phase can merge independently and leaves existing behavior unchanged when
no parameters are disabled.

**Phase 2: Pre-dispatch visibility check and effective argument map.** Before
tool dispatch for any tool source, reject arguments that name a disabled
parameter and inject configured `default` values for disabled parameters to
produce the effective argument map.
Run this preprocessing at executor creation (before `permission_info` renders
the approval prompt) and re-run it after `run = "edit"`.
Depends on Phase 1.
While Phase 1 hides disabled parameters from schemas, Phase 2 prevents the model
from guessing disabled parameter names and ensures approval prompts show
injected defaults.

Default injection for enabled parameters in `apply_parameter_defaults` requires
no changes, and when no parameters are disabled, schemas, arguments, and wire
payloads across all tool sources remain unchanged.

## References

- `crates/jp_config/src/conversation/tool.rs`: Defines `ToolParameterConfig`,
  including its recursive `properties` and `items` fields.
- `crates/jp_llm/src/tool.rs`: Defines `apply_parameter_defaults`,
  `validate_tool_arguments`, and the `execute_local`, `execute_mcp`, and
  `execute_builtin` dispatch paths.
- `crates/jp_cli/src/cmd/query/tool/executor.rs`: Defines `permission_info` and
  `arguments`, which must read the effective argument map rather than the raw
  stored request.
- `crates/jp_cli/src/cmd/config/show.rs`: Implements `jp config show`.
- [RFD D10]: Unified tool execution model, which moves parameter defaults and
  validation into per-source runtimes below this RFD's pre-dispatch visibility
  check.
- [RFD 042]: Tool options and the rejected "hidden parameters" alternative that
  this RFD revisits.
- [RFD 055], [RFD 056], [RFD 057]: Tool groups.
- [RFD 060]: Config explain (`--explain`), which would report effective
  parameter visibility across layers.
- [RFD 081]: The tool-level `state` and `allow_toggle` split.
- [Issue 208]: Tool templates with parameter constraints (`fixed`, `default`,
  `enum`, `remove`) and host-side call validation.
  This RFD implements the first slice of that design with a single boolean on
  the existing `parameters` map, without templating or multiple exposures:
  `enabled = false` corresponds to `remove`, and `enabled = false` with a
  `default` corresponds to `fixed`.
- [PR 998]: Resolved tool schema validation touching the same resolution path.

[Issue 208]: https://github.com/dcdpr/jp/issues/208
[PR 998]: https://github.com/dcdpr/jp/pull/998
[RFD 042]: ../042-tool-options.md
[RFD 055]: ../055-tool-groups.md
[RFD 056]: ../056-group-configuration-defaults.md
[RFD 057]: ../057-group-configuration-overrides.md
[RFD 060]: ../060-config-explain.md
[RFD 081]: ../081-decompose-tool-enable-into-state-and-allow_toggle.md
[RFD D10]: D10-unified-tool-execution-model.md
