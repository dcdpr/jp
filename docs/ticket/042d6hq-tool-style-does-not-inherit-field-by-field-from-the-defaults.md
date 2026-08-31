# Tool style does not inherit field-by-field from the '\*' defaults

- **Status**: Done
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-08-11

`[conversation.tools.'*'.style]` is silently ignored for any tool that sets a
single style field of its own.

`ToolConfigWithDefaults::style` resolves the whole struct at once
(`crates/jp_config/src/conversation/tool.rs:1215`):

```rust
pub fn style(&self) -> &DisplayStyleConfig {
    self.tool.style.as_ref().unwrap_or(&self.defaults.style)
}
```

Every neighbouring accessor fills field-by-field from the defaults.
`enable` goes through `Enable::effective`, and `run`, `result`, `format`, and
`cancellation_response` each `unwrap_or` a scalar.
Only `style` falls back as a unit.

## Why it bites

Setting a single style default across every tool looks like this and does
nothing:

```toml
[conversation.tools.'*'.style]
hidden = true
```

Every tool the skills enable already sets `style.inline_results`, so every one
of them takes its own style wholesale and drops the default.
The failure is silent: no error, no warning, the setting is simply absent from
the resolved config.

Found while hiding read-only tool calls for the RFD pipeline personas, which
ended up enumerating twenty-two tools by name because the one-line version does
not work.
That list will drift as tools are added.

## Fix

`DisplayStyleConfig` implements `FillDefaults`, and
`PartialToolsDefaultsConfig::fill_from` already resolves the `'*'` block's own
style against the schematic defaults.
There is no `PartialToolConfig::fill_from`; nothing fills a tool's style from
the `'*'` block.

Adding that cross-key step is the fix.
It cannot go in `ToolConfigWithDefaults::style`, because `ToolConfig::style` is
an `Option<DisplayStyleConfig>`: once a tool declares one key the option is
`Some` and every other field reads as deliberately set.
By the time the config is resolved, which keys the tool asked for is no longer
recoverable.

So the fill happens at partial-merge time, in `PartialToolsConfig::fill_from`,
where the partial still records the tool's own keys.
The alternative is to follow `enable` and keep the per-field optionality in the
resolved type, splitting `DisplayStyleConfig` (stored, optional fields) from a
concrete style returned at read time, the way `EnableConfig` splits from
`Enable`.
That changes the type every caller of `style()` reads.

Filling at merge time needs a second half.
A resolved config goes back to a partial in two places that matter: the
conversation config layer (`ConversationStream::config`) and the diff a query
records into the stream (`get_config_delta_from_cli`).
Both run through `ToolsConfig::to_partial`, and a filled-in style field is
indistinguishable there from one the tool wrote itself, so a plain round-trip
republishes it as a key of the tool's own, pinning it against every later `'*'`
change.
So `to_partial` subtracts the resolved `'*'` style from each tool's style,
keeping only the fields that differ, the same way `AppConfig::to_partial`
subtracts the assistant from `conversation.inquiry.assistant`.

## Watch for

Whether anything currently relies on the whole-struct behaviour, for instance a
tool that sets one style field expecting the others to fall back to the
hardcoded defaults rather than to a `'*'` block.

Equality is the only signal the subtraction has.
A tool that deliberately sets a style field to the value the `'*'` block
already holds is indistinguishable from one inheriting it, and will follow a
later `'*'`-only change instead of holding its own value.
