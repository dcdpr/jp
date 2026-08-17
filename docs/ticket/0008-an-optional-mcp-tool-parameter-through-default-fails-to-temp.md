# T0008: An optional MCP tool parameter through default('') fails to template

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-08-11

A local MCP tool whose `command` renders an optional parameter through
`default('')` returns `Template error` on every call, whether the argument is
supplied or not.

Reproduced on `rfd_lint` while adding it:

```toml
command = "just rfd-lint {{tool.arguments.nnn}} {{tool.arguments.flag | default('')}}"
```

Every invocation failed.
Removing the `flag` parameter and its template expression fixed it:

```toml
command = "just rfd-lint {{tool.arguments.nnn}}"
```

The diagnosis took a while because two conversations disagreed.
One had cached the working, flag-less definition and kept succeeding; a freshly
created conversation loaded the current definition and failed on every id and
flag combination.
That difference is what isolated the parameter as the cause.
An earlier guess that an `enum` on the optional parameter was responsible was
wrong: the enum had already been removed when it still failed.

## Latent elsewhere

`.jp/mcp/tools/rfd/renumber.toml` uses the same construct:

```toml
command = "just rfd-renumber {{tool.arguments.nnn}} {{tool.arguments.mmm | default('')}}"
```

`mmm` is optional, so `rfd_renumber` is likely broken the same way and nobody
has noticed.
Worth calling it once to confirm before assuming the construct works anywhere.

## What to decide

Either fix the filter so an absent optional argument renders as an empty string,
or drop `default()` from the documented idiom and give tools that need optional
arguments a different shape.
The second wants a note in whatever documents local tool definitions, because
`renumber.toml` currently reads as the working example to copy.
