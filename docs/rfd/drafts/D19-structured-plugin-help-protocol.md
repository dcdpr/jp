# RFD D19: Structured Plugin Help Protocol

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-07
- **Extends**: [RFD 072]
- **Required by**: [RFD D23]
- **Summary**: Replaces raw plugin help text with a structured `Help` protocol
  message rendered through `clap::Command` on the host.

## Summary

This RFD replaces the free-text `help` field in the plugin `Describe` response
([RFD 072]) with a structured `Help` protocol message.
The host constructs a `clap::Command` from the plugin's argument and subcommand
definitions so `-h` and `--help` render with the same formatting as built-in
commands.

## Motivation

In [RFD 072], the `Describe` response includes an optional `help` field
containing a raw text string that the host prints verbatim for `jp <plugin> -h`.
Printing pre-formatted text from the plugin has three drawbacks:

- **Inconsistent formatting**: Plugin help text does not match clap's column
  alignment, color, or line wrapping.
- **No `-h` vs `--help` support**: clap distinguishes between compact short help
  (`-h`) and expanded long help (`--help`), which a single `help` string cannot
  represent.
- **Duplicated layout logic**: Each plugin formats its own help output,
  duplicating layout logic that the host already handles through clap.

The host should own help rendering.
Plugins provide the command definition (command name, arguments, flags, and
subcommands), and the host renders it through clap's `Command` builder.

## Design

### `Help` Protocol Message

The plugin protocol adds a `Help` request/response pair alongside `Describe`.
While `Describe` returns lightweight metadata for `jp -h` listings, `Help`
returns the full CLI definition.

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

The host then calls `command.print_help()` for `-h` or
`command.print_long_help()` for `--help`.

### Fallback to `Describe`

`Describe` still provides lightweight metadata for plugin discovery and `jp -h`
listings, and the `help` field on `DescribeResponse` serves as a fallback for
plugins that do not implement the `Help` message.

For `jp <plugin> -h` or `jp <plugin> --help`, the host runs the following flow:

1. Spawn the plugin and send `help`.
2. If the plugin responds with a `Help` message, build the `clap::Command` and
   render it.
3. If the plugin responds with anything else (or does not recognize `help`),
   fall back to `describe` and print the `help` text field.

### Shell Script Plugins

Shell scripts can implement `Help` by echoing the JSON structure directly.
Simple plugins with only a few flags can emit `Help` inline, while complex
plugins can rely on the `describe.help` fallback.

## Drawbacks

- **Coupling to clap's model**: The protocol types mirror clap's API, so
  breaking changes in clap could require protocol updates.
  In practice, clap's core model (commands and arguments) is stable across major
  versions.

- **More work for plugin authors**: Defining arguments as structured JSON is
  more verbose than returning a single help string, though the `describe.help`
  fallback mitigates this for simple plugins.

## Alternatives

### Keep the free-text `help` field

Keeping the existing free-text `help` field is simpler for plugin authors, but
leaves plugin help output visually inconsistent with built-in commands.

### Plugin-side clap rendering

Plugins could format their own help text using clap (or another CLI framework)
and return the pre-rendered string.
This still fails to guarantee consistent formatting with the host and forces
every plugin to depend on a CLI framework.

## Non-Goals

- **Argument validation**: The host uses the structured data only to render
  help; each plugin still parses and validates its own arguments.
- **Completions**: Shell completions for plugin commands are out of scope for
  this RFD, though future work could build on the same structured data.

## Implementation Plan

### Phase 1: Protocol types

- Add `Help` request to `HostToPlugin` and `HelpResponse` to `PluginToHost`.
- Define `PluginCommand`, `PluginSubcommand`, and `PluginArg` in `jp_plugin`.
- Add fallback logic in the host: try `help` first, then fall back to
  `describe`.

### Phase 2: Host-side clap builder

- Implement `build_clap_command()` to construct a `clap::Command` from
  `PluginCommand`.
- Wire `build_clap_command()` into the `-h` and `--help` dispatch path.

### Phase 3: Migrate `jp-serve`

- Replace the static `HELP_TEXT` in `jp-serve` with a `Help` response that
  returns structured arg definitions.
- Remove the `help` field from `jp-serve`'s `Describe` response.

## References

- [RFD 072: Command Plugin System][RFD 072]

[RFD 072]: ../072-command-plugin-system.md
[RFD D23]: D23-json-input-and-schema-for-cli.md
