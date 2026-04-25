# RFD D19: Structured Plugin Help Protocol

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-07

## Summary

This RFD proposes replacing the free-text `help` field in the plugin `Describe`
response ([RFD 072]) with a structured `Help` protocol message. The host builds
a `clap::Command` from the structured data and renders help text, ensuring
consistent formatting between built-in and plugin commands.

## Motivation

[RFD 072] introduced the `Describe` protocol message, which includes an optional
`help` field containing a raw text string that the host prints verbatim for
`jp <plugin> -h`. This works but has drawbacks:

- **Inconsistent formatting**: Plugin help text doesn't match clap's column
  alignment, color, wrapping, or short/long help distinction.
- **No `-h` vs `--help` support**: clap distinguishes between short help
  (`-h`, compact) and long help (`--help`, expanded). With a single `help`
  string, plugins can't participate in this convention.
- **Duplicated layout logic**: Each plugin must format its own help text,
  duplicating layout decisions that the host already handles through clap.

The host should own help rendering. Plugins provide the semantic content
(command name, arguments, flags, subcommands), and the host feeds it into
clap's `Command` builder for rendering.

## Design

### New Protocol Message: `Help`

A new request/response pair is added to the plugin protocol alongside the
existing `Describe` message. `Describe` remains for lightweight metadata
(used in `jp -h` listings). `Help` returns the full CLI definition.

**Host → Plugin:**

```json
{
  "type": "help"
}
```

**Plugin → Host:**

```json
{
  "type": "help",
  "command": {
    "about": "Start the read-only web interface for browsing conversations.",
    "long_about": "Start the read-only web interface for browsing JP conversations.\n\nThe server renders conversation history as HTML...",
    "args": [
      {
        "long": "web",
        "short": "w",
        "help": "Start the web server",
        "long_help": "Start the web server. Required for now; future modes may be added.",
        "required": false
      },
      {
        "long": "bind",
        "short": "b",
        "help": "Address to bind to",
        "value_name": "ADDR",
        "default_value": "127.0.0.1"
      },
      {
        "long": "port",
        "short": "p",
        "help": "Port to listen on",
        "value_name": "PORT",
        "default_value": "3000"
      }
    ],
    "subcommands": []
  }
}
```

### Structured Types

The `command` object maps closely to clap's `Command` and `Arg` builders:

```rust
struct PluginCommand {
    /// Short help line (clap `about`).
    about: String,

    /// Long help text (clap `long_about`). Optional.
    long_about: Option<String>,

    /// Arguments and flags.
    args: Vec<PluginArg>,

    /// Subcommands (recursive).
    subcommands: Vec<PluginSubcommand>,
}

struct PluginSubcommand {
    /// Subcommand name.
    name: String,

    /// Short help line.
    about: String,

    /// Long help text. Optional.
    long_about: Option<String>,

    /// Visible aliases for this subcommand.
    visible_aliases: Vec<String>,

    /// Arguments and flags.
    args: Vec<PluginArg>,
}

struct PluginArg {
    /// Long flag name without dashes (e.g. "web").
    long: String,

    /// Single-char short flag (e.g. "w"). Optional.
    short: Option<char>,

    /// Short help text shown with `-h`.
    help: String,

    /// Long help text shown with `--help`. Optional.
    long_help: Option<String>,

    /// Value placeholder (e.g. "PORT"). None means boolean flag.
    value_name: Option<String>,

    /// Default value shown in help. Optional.
    default_value: Option<String>,

    /// Whether the argument is required.
    required: bool,

    /// Restrict to specific values. Optional.
    possible_values: Vec<String>,
}
```

### Host-Side Rendering

When the host receives a `Help` response, it builds a `clap::Command`
dynamically:

```rust
fn build_clap_command(name: &str, cmd: &PluginCommand) -> clap::Command {
    let mut command = clap::Command::new(name).about(&cmd.about);

    if let Some(long) = &cmd.long_about {
        command = command.long_about(long);
    }

    for arg in &cmd.args {
        let mut clap_arg = clap::Arg::new(&arg.long).long(&arg.long);
        if let Some(short) = arg.short {
            clap_arg = clap_arg.short(short);
        }
        clap_arg = clap_arg.help(&arg.help);
        // ... map remaining fields
        command = command.arg(clap_arg);
    }

    for sub in &cmd.subcommands {
        // recursively build subcommands
    }

    command
}
```

Then calls `command.print_help()` or `command.print_long_help()` depending on
whether the user passed `-h` or `--help`.

### Interaction with `Describe`

`Describe` continues to serve its current role: lightweight metadata for plugin
discovery and `jp -h` listings. The `help` field in `DescribeResponse` becomes
a fallback for plugins that don't implement the `Help` message.

The host's flow for `jp <plugin> -h`:

1. Spawn plugin, send `help`.
2. If the plugin responds with a `Help` message, build and render via clap.
3. If the plugin responds with anything else (or doesn't understand `help`),
   fall back to `describe` and print the `help` text field.

### Shell Script Plugins

Shell scripts can implement `Help` by echoing the JSON structure. For simple
plugins with a few flags, this is straightforward. For complex plugins, the
`describe.help` fallback provides a low-effort alternative.

## Drawbacks

- **Coupling to clap's model**: The structured types mirror clap's API. If
  clap changes, the protocol types might need updating. In practice, clap's
  core model (commands + args) is stable across major versions.

- **More work for plugin authors**: Defining args as structured JSON is more
  verbose than a help string. The fallback to `describe.help` mitigates this
  for simple plugins.

## Alternatives

### Keep the free-text `help` field

The current approach. Simple but inconsistent. Plugin help looks different from
built-in command help.

### Plugin-side clap rendering

Have plugins use clap (or their own framework) and return pre-rendered text.
Rejected because it doesn't achieve formatting consistency and requires every
plugin to depend on a CLI framework.

## Non-Goals

- **Argument validation**: The host renders help from the structured data but
  does not validate plugin arguments. The plugin is responsible for its own
  argument parsing and validation.
- **Completions**: Shell completions for plugin commands are out of scope for
  this RFD but could build on the same structured data in the future.

## Implementation Plan

### Phase 1: Protocol types

- Add `Help` request to `HostToPlugin` and `HelpResponse` to `PluginToHost`.
- Define `PluginCommand`, `PluginSubcommand`, `PluginArg` in `jp_plugin`.
- Add fallback logic in the host: try `help` first, fall back to `describe`.

### Phase 2: Host-side clap builder

- Implement `build_clap_command()` that constructs a `clap::Command` from
  `PluginCommand`.
- Wire into the `-h` / `--help` dispatch path.

### Phase 3: Migrate `jp-serve`

- Replace the static `HELP_TEXT` in `jp-serve` with a `Help` response that
  returns structured arg definitions.
- Remove the `help` field from `jp-serve`'s `Describe` response.

## References

- [RFD 072: Command Plugin System][RFD 072]

[RFD 072]: 072-command-plugin-system.md
