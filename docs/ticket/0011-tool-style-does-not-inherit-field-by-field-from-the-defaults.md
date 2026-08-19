# T0011: Tool style does not inherit field-by-field from the '\*' defaults

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

`DisplayStyleConfig` already implements `FillDefaults`, and
`PartialToolConfig::fill_from` already calls `style.fill_from(defaults.style)`
at partial-merge time.
The resolved-read path wants the same treatment: resolve the per-tool style
against the defaults per field rather than choosing one struct.

## Watch for

Whether anything currently relies on the whole-struct behaviour, for instance a
tool that sets one style field expecting the others to fall back to the
hardcoded defaults rather than to a `'*'` block.
