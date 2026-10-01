# A later config layer replaces a command plugin's options wholesale

- **Status**: Done
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-10-01
- **Label**: domain=plugins
- **Label**: package=jp_config

`plugins.command.<name>.options` is an `Option<Value>` with no merge strategy,
so the derived merge replaces it whenever a later layer sets it
(`crates/contrib/schematic_macros/src/config/field_value.rs:366-370`).
The surrounding `plugins.command` map deep-merges per entry
(`map_with_strategy`), which makes the replacement easy to miss.

Input that hits it: a workspace sets

```toml
[plugins.command.ticket.options]
dir = "docs/ticket"
assistant = "jp"
```

and a contributor's user-workspace config sets `options.assistant = "me"`.
The resolved `options` holds only `assistant`; `dir` is gone, and the plugin
falls back to its default without saying so.

Found by reading the code, not reproduced in a test.

Fix: give `options` a deep-merge strategy, so object keys merge recursively and
a later layer only replaces the keys it names.
Add a test that layers two partials with disjoint `options` keys and asserts
both survive.
RFD 116 puts each plugin's configuration in `options`, so it depends on this
fix.
