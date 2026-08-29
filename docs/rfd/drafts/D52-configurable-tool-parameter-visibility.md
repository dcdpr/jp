# RFD D52: Configurable Tool Parameter Visibility

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-18
- **Extends**: [RFD 042]

## Summary

This RFD adds `enabled` to a tool parameter's configuration.
A parameter with `enabled = false` is absent from the JSON Schema sent to the
model, absent from `describe_tools`, and rejected if the model supplies it
anyway.
Config layers can flip the flag, so a parameter can be hidden by default and
exposed only in the context that needs it.

Host-side default injection already exists and is unchanged, so a disabled
parameter that declares a `default` still reaches the tool with that value —
"hide this and always pass this" needs no new mechanism.

## Motivation

A tool parameter is useful to the model in some contexts and pure noise in
others.
Today the choice is binary and permanent: a parameter is either in the schema
for every conversation, or it does not exist.

Take a tool that usually acts on the workspace root, but occasionally needs to
act somewhere else — a second build workspace inside the repository, or a
checkout mounted alongside it.
Two designs are available today:

1. **Declare the parameter unconditionally.** Every conversation now carries an
   argument the model must reason about and can misuse, to serve a case that
   arises in a minority of them.
2. **Move the choice into `options`.** Host-configured, invisible to the model,
   fixed for the whole conversation.
   The model cannot act on one location and then the other, because it is not
   the one choosing.

Neither is wrong in itself, and the second is right whenever the value genuinely
is the user's to fix.
What is missing is the axis that separates them: *who chooses*, and *in which
contexts is the model allowed to*.

Doing nothing keeps that decision welded to the parameter's declaration.
A parameter that should be invisible by default and available on demand has
nowhere to live: exposing it everywhere is noise, and moving it to `options`
puts it permanently out of the model's reach.

## Design

### Configuration

`ToolParameterConfig` gains one field:

```toml
[conversation.tools.run_build.parameters.root]
type = "string"
enabled = false
summary = "Directory to build in. Defaults to the workspace root."
```

A later config layer re-enables it:

```toml
[conversation.tools.run_build.parameters.root]
enabled = true
default = "crates/some-project"
```

`enabled` defaults to `true`, so every existing configuration is unaffected.

### Scope: top-level parameters only

`ToolParameterConfig` doubles as a recursive JSON Schema node — `properties`
describes an object's fields, `items` an array's elements — so adding a field
to it adds that field everywhere in the tree.

`enabled` is honoured only on entries directly under `parameters`.
Setting it on a nested `properties` entry or inside `items` is a **config
error**, reported with the full path to the offending entry.

Rejecting is not the same as ignoring, and the difference matters.
A nested `enabled` would be offered by schema completion, accepted by the
parser, and then do nothing: the config loads clean while the field stays
visible to the model.
That failure is silent and invisible at exactly the moment the user believes
they have hidden something.
An error at load time costs a moment; a silently-ignored one costs a debugging
session, or a parameter the user thinks is closed.

Top-level-only is the smaller design and it covers the motivating case.
Recursive visibility raises questions this RFD does not need to answer: whether
a disabled object property is stripped from `required` as well as `properties`,
whether a nested default is injected when the parent object is itself absent,
and what disabling `items` could mean for an array.

The validation rule is a stopgap for a type problem.
The cleaner expression is a type split — a top-level parameter type carrying
`enabled`, a nested schema-node type without it — so the config surface cannot
state something that has no meaning, and no runtime check is needed.
That refactor is worth doing and is not required to ship the flag.

### What "disabled" means

Disabled is total.
The parameter does not exist as far as the model is concerned:

- It is omitted from the JSON Schema sent to the provider.
- It is omitted from `describe_tools` output.
- It is rejected if the model supplies it anyway.

The first two are pure schema filtering and apply to every tool source.

The third needs work that this RFD has to include, because rejection is not
currently universal.
`validate_tool_arguments` (`crates/jp_llm/src/tool.rs`) does reject any argument
absent from `parameters`, but it is called only inside `execute_local`.
`execute_mcp` forwards the argument object straight to `mcp_client.call_tool`,
and `execute_builtin` hands it straight to the executor; neither validates.
A model that guesses a disabled parameter's name — from stale context, or
ordinary schema noncompliance, which models produce in normal use — would have
that argument delivered.

So a disabled parameter is unreachable for local tools today and merely
unadvertised for the other two.
That is not a defensible contract for a feature whose whole claim is that the
parameter does not exist, and `parameters` already applies to MCP tools (unlike
`options`, which [RFD 042] scoped to local tools), so narrowing this RFD to
local tools would contradict the field it extends.

**This RFD therefore adds one source-independent step, scoped to the visibility
contract alone.** For every tool source, before dispatch:

1. Reject an argument naming a disabled parameter.
2. Inject a disabled parameter's configured `default`.

That is the whole of it.

### What this step deliberately does not do

Everything else stays exactly where it is.
Default injection for *enabled* parameters, and rejection of arguments that name
no parameter at all, remain inside `execute_local` and remain local-only.

The temptation is to hoist all of `apply_parameter_defaults` and
`validate_tool_arguments` above the dispatch and unify the three sources in one
go.
That would be a larger, unrelated behavioural change:

- **MCP servers would start receiving arguments they never received.** JSON
  Schema treats `default` as an annotation, explicitly *not* a value to populate
  on the caller's behalf, and a server may distinguish an absent field from an
  explicitly-supplied one even where the values coincide.
  Injecting into an MCP call changes the wire format for every MCP tool that has
  a configured default.
- **Builtin tools would start rejecting arguments** that reach their executors
  untouched today.

Neither has anything to do with parameter visibility.
Unifying host-side argument handling across sources is worth doing on its own
terms, with its own compatibility analysis; it is not this RFD's to smuggle in.

Because the step is scoped this way, a configuration with no disabled parameters
produces byte-identical behaviour on every path.

### Disabled and `required` are orthogonal

Hiding a parameter says nothing about whether the tool needs a value for it.
The two are separate axes and both combinations are meaningful:

|                    | `enabled = true`     | `enabled = false`                                          |
| ------------------ | -------------------- | ---------------------------------------------------------- |
| `required = false` | Model may supply it  | Tool falls back to its own default                         |
| `required = true`  | Model must supply it | `default` is mandatory; the config is rejected without one |

`required` describes the schema the model is held to; `enabled` describes
whether the model sees that schema at all.
Disabled plus required is coherent: the value is mandatory *and* not the model's
to choose.

Two rules make that concrete, and both are needed — without them the
combination is merely undefined.

**Validation runs against the enabled set.** A disabled parameter is not
required of the model, because the model cannot see it.
Validating against the full set instead would mark a disabled parameter
permanently missing and fail every single call.

**Disabled + required + no `default` is rejected at config resolution.** Given
the rule above, such a parameter would otherwise be silently absent: the model
cannot supply it, nothing injects it, validation no longer asks for it, and the
tool — which by design knows nothing about visibility — cannot tell it apart
from an omitted optional.
A value declared mandatory would simply never arrive.
That is a stable mistake in configuration, not a runtime condition, so it is
caught once at load rather than on every invocation.
`reject_access_on_non_local_tools` is the precedent: an incoherent combination
refused at config time rather than half-honoured later.

### Defaults, mostly already solved

`apply_parameter_defaults` already fills in a configured `default` for any
parameter the arguments omit, and already runs before validation.
It exists because models routinely omit defaulted parameters even when they are
marked required.

A disabled parameter is always omitted by definition, so it always takes its
default:

```toml
[conversation.tools.run_build.parameters.root]
enabled = false
default = "crates/some-project"
```

The tool receives `root = "crates/some-project"` and cannot tell the difference
between that and the model having passed it.
"Hide this parameter and always pass this value" therefore needs no new concept
— only the ability to hide the parameter.

For local tools this is entirely existing behaviour.
For MCP and builtin tools, where nothing injects today, the visibility check
carries the disabled parameter's default and nothing else; their handling of
*enabled* parameters is untouched.

**Injection behaviour is unchanged for enabled parameters, on every source.** An
earlier draft of this RFD proposed restricting injection to disabled parameters;
that would have been a breaking change.
An enabled, required parameter with a default that the model omits succeeds
today and must continue to — turning it into a failed tool call would persist
that failure into the conversation and feed it back to the model on the next
cycle.

### Two argument maps, and where each is used

The stored `ToolCallRequest` keeps the model's raw arguments.
That is deliberate and unchanged: stored requests are replayed to providers as
conversation history, so persisting an injected value would hand the model back
a parameter this feature exists to hide.

Preprocessing therefore produces a second, **effective** map, and everything
that acts on or reports the call uses that one:

| Map       | Contents                                 | Used for                                                               |
| --------- | ---------------------------------------- | ---------------------------------------------------------------------- |
| Stored    | Exactly what the model emitted           | The conversation stream; replay to providers as history                |
| Effective | Stored, plus disabled-parameter defaults | Permission prompts, argument formatting, rendering, tracing, execution |

**The ordering is the point.** Preprocessing happens when the executor is
created — before `permission_info` builds a prompt — not on the way into the
tool.

Today injection happens inside `execute_local`, well after the approval prompt
is rendered from `request.arguments`.
For an enabled parameter with a default that is an occasional cosmetic mismatch.
For a disabled one it is systematic: a disabled parameter is *always* absent
from the model's output, so its configured value would *always* be missing from
the prompt.

A tool with `run = "ask"` and a disabled `root` defaulting to a second build
directory would ask the user to approve a call showing no `root` at all —
implying the workspace root — and then run somewhere else.
The user's consent would not describe the operation performed, which defeats the
only thing `ask` mode exists to do.

After an `edit` permission changes the arguments, preprocessing and validation
run again on the edited map, so the final prompt and the executed call still
agree.

The residual cost is unchanged from today: a tool's observed arguments do not
match the stored request, which matters to anything auditing or replaying tool
calls from the stream.
What this RFD fixes is that the *human being asked to approve* now sees what the
tool will actually receive.

### Tools stay ignorant of visibility

Tools are not told which parameters are disabled.
A tool reads its arguments and applies its own fallback when one is missing,
exactly as today.

The alternative — exposing the disabled set so a tool can branch on it — is
deliberately not adopted.
It pushes visibility awareness into every tool that wants a context-dependent
default, when `default` injection already covers the motivating need.
It can be added later if a tool appears that genuinely cannot express itself
through a default value.

### Relationship to tool options

Options and parameters answer different questions, and this feature sharpens the
line between them.

An **option** is a setting the model has no business choosing: a credential, a
feature toggle, an environment detail.
It is host-configured and never appears in a schema.

A **parameter** is an input the model could sensibly choose.
Until now, a parameter that should be hidden in most contexts had to be demoted
to an option to get out of the way — which also removed the model's ability to
choose it in the contexts where it should.
`enabled` removes that forced trade.

The test for which to reach for is unchanged and simple: could the model ever
sensibly pick this value?
If yes, it is a parameter, and `enabled` decides where it is offered.

## Drawbacks

**A parameter's existence becomes layer-dependent, and nothing reports the
result.** Reading a tool's config no longer tells you what the model sees; you
need the merged view, and no command produces one.
`jp config show` prints a commented skeleton of available keys, or defaults with
`--defaults`; neither reflects merged configuration, and the command loads no
conversation, so conversation-layer state is out of reach entirely.
The failure mode is a user wondering why the model ignores a documented
argument, with no way to check.

[RFD 060] would close this — a focused
`--explain=conversation.tools.<tool>.parameters.<name>.enabled` is exactly the
diagnostic — but it is in Discussion, so this RFD ships without one.

**Two axes multiply.** `enabled` times `required` times `default` is eight
combinations, one of which is refused at config resolution.
That is a third axis to hold in mind when reading a tool's parameters, and the
one illegal combination is discoverable mainly by tripping over it.

## Alternatives

**Leave it in `options`.** Zero work, and correct whenever the value should be
fixed for a conversation.
Rejected as the general answer because it cannot express "the model may choose
this here", which is the actual gap.

**Reversing [RFD 042].** This RFD proposes what RFD 042 considered and rejected,
so the reversal needs stating rather than assuming.
RFD 042's Alternatives dismissed "hidden parameters that the LLM doesn't see but
the tool reads" on two grounds, and the intervening design answers both:

- *"The LLM might hallucinate values for these hidden parameters."* It may, and
  the call is now rejected.
  The source-independent check (see above) makes that true for every tool
  source, which is precisely what closes 042's objection.
- *"Validation might reject tool calls that omit them."* It does not.
  `apply_parameter_defaults` fills the value in before validation runs, so an
  omitted-but-defaulted parameter is not a failure.
  That machinery post-dates RFD 042.

What survives from 042 is its central distinction, and this RFD keeps it: a
setting the model should never choose is an option, not a hidden parameter.
What changes is narrower — a parameter that is model-meaningful *in some
contexts* no longer has to be demoted to an option to stay out of the way in the
others.

**Tool groups ([RFD 055], [RFD 056], [RFD 057]).** Swap whole tool definitions
per context, so a context supplies a variant with the parameter declared.
Heavier: it duplicates a tool definition to vary one field, and the group
machinery is still in Discussion.

**A `hidden` flag on the parameter instead of `enabled`.** Same mechanism,
different word.
`hidden` suggests presentation-only, which would be a mischaracterisation — the
parameter is rejected, not merely undisplayed.

**Follow the tool-level `state` / `allow_toggle` split ([RFD 081]).** Tools
distinguish "is it on" from "may a later layer change that".
Parameters could too.
Not adopted: no case yet needs to pin a parameter's visibility against later
layers, and a single boolean is the smaller step.
The split remains available if such a case appears.

## Non-Goals

- **Conditional visibility.** No expressions, no "enable when another parameter
  is set".
  Static per-layer configuration only.
- **Per-call visibility.** The set is fixed when tool definitions are resolved
  for an invocation.
- **Recursive visibility.** `enabled` on nested `properties` or `items` is a
  config error, not a feature; see the scope section.
- **Changing `options`.** They keep their meaning; this only removes the cases
  where a parameter was forced into them.
- **Changing default injection for enabled parameters.** Existing behaviour is
  preserved exactly, on every tool source.
- **Unifying host-side argument handling across tool sources.** Worth doing, but
  it is a separate change with its own compatibility analysis; see "What this
  step deliberately does not do".
- **Telling tools which parameters are disabled.** See above.

## Risks and Open Questions

**May the user supply a disabled parameter when editing a call?** `run = "edit"`
lets the user rewrite arguments before approval.
`enabled = false` means "not the model's to choose", and the user is the host,
not the model — so a user-supplied value is arguably legitimate where a
model-supplied one is not.
Validation as described cannot tell the two apart: it sees one argument map.
Rejecting the edit blocks a defensible host override; accepting it reopens the
channel for anything that can write to that map.
This needs deciding during implementation rather than falling out of it.

**MCP tools whose server-side schema is richer than JP's config.** JP rejects an
argument naming a parameter it has been told is disabled, which presumes the
configured `parameters` map describes the same parameter the server means.
Worth confirming against a server that declares parameters JP's config does not
mirror.

**Sequencing against [PR 998].** That open pull request validates resolved tool
schemas and touches the same resolution path.
Whichever lands second inherits the merge; worth agreeing an order rather than
discovering it.

**[RFD D10] moves in the opposite direction.** It relocates parameter defaults
and argument validation *into* `StdioRuntime`, one runtime among several, while
this RFD places the visibility check above the source dispatch precisely because
it is source-independent.
D10 is a Draft, so it does not block this; but if this lands first, D10's
migration section needs revising to keep the visibility contract out of the
per-source runtimes.

## Implementation Plan

**Phase 1: The flag and the schema filter.** Add `enabled` to
`ToolParameterConfig`, defaulting to `true`.
Filter disabled parameters out of the schema sent to providers and out of
`describe_tools`.
Validate against the enabled set, reject `enabled` on nested schema nodes, and
reject disabled + required without a `default` at config resolution.
Mergeable on its own; with nothing configured, behaviour is unchanged.

**Phase 2: The visibility check, above the dispatch.** Reject arguments naming a
disabled parameter and inject disabled-parameter defaults, for every tool
source, producing the effective argument map.
Run it at executor creation so the map reaches the approval prompt, and re-run
it after an `edit`.
Depends on Phase 1.
This is the phase that makes "disabled is total" true and keeps approval honest;
until it lands, Phase 1 hides parameters without closing the guess path.

Injection for enabled parameters needs no phase: `apply_parameter_defaults`
already does the right thing, and is left alone.

With no parameter configured as disabled, both phases are no-ops: schemas,
arguments, and the wire format for every tool source are identical to today.

## References

- `crates/jp_config/src/conversation/tool.rs` — `ToolParameterConfig`,
  including its recursive `properties` and `items`.
- `crates/jp_llm/src/tool.rs` — `apply_parameter_defaults`,
  `validate_tool_arguments`, and the three `execute_*` paths that do not
  currently share them.
- `crates/jp_cli/src/cmd/query/tool/executor.rs` — `permission_info` and
  `arguments`, which read the stored request; where the effective map has to be
  in place by.
- `crates/jp_cli/src/cmd/config/show.rs` — what `jp config show` actually
  prints.
- [RFD D10] — unified tool execution model; moves defaults and validation into
  per-source runtimes, which this RFD's visibility check must sit above.
- [RFD 042] — tool options, and the rejected "hidden parameters" alternative
  this RFD revisits.
- [RFD 055], [RFD 056], [RFD 057] — tool groups.
- [RFD 060] — config explain, the natural home for a visibility diagnostic.
- [RFD 081] — the tool-level `state` / `allow_toggle` split.
- [Issue 208] — tool templates with parameter constraints (`fixed`, `default`,
  `enum`, `remove`) and host-side call validation.
  This RFD is the narrow first slice of that design: one boolean on the existing
  `parameters` map, no templating, no multi-exposure.
  The vocabularies should converge rather than diverge — `enabled = false` is
  208's `remove`, and `enabled = false` with a `default` is its `fixed`.
- [PR 998] — resolved tool schema validation, in flight against the same path.

[Issue 208]: https://github.com/dcdpr/jp/issues/208
[PR 998]: https://github.com/dcdpr/jp/pull/998
[RFD 042]: ../042-tool-options.md
[RFD 055]: ../055-tool-groups.md
[RFD 056]: ../056-group-configuration-defaults.md
[RFD 057]: ../057-group-configuration-overrides.md
[RFD 060]: ../060-config-explain.md
[RFD 081]: ../081-decompose-tool-enable-into-state-and-allow_toggle.md
[RFD D10]: D10-unified-tool-execution-model.md
