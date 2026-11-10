# RFD D08: Tool Import CLI

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-12
- **Requires**: [RFD 029]
- **Summary**: Adds `jp tool new` and `jp tool import` subcommands to scaffold
  and generate local and MCP tool configurations.

## Summary

This RFD introduces `jp tool` subcommands for adding tools to a JP workspace:
`jp tool new` scaffolds an empty config, `jp tool import` generates a tool
config from a CLI command with LLM assistance, and `jp tool import --mcp`
imports tool definitions from an MCP server.

## Motivation

Adding a tool to JP requires manually writing a TOML config file with the source
configuration, parameter types, and descriptions.
For local tools, the user must read the command's flags and translate them into
JP's parameter schema.
For MCP tools, the user must look up the server and tool names and reference
them in the config by hand.

Users who want to expose a CLI command to the assistant face this manual setup
before they can even test whether the tool works.
Turning "I want to use ripgrep as a tool" into a working tool config should take
a single command.

## Design

### `jp tool new <name>`

`jp tool new` scaffolds an empty TOML tool config file:

```
jp tool new my_tool
```

Running this command creates `.jp/mcp/tools/my_tool.toml` with a minimal
template:

```toml
[conversation.tools.my_tool]
source = "local"
command = ""
summary = ""

# [conversation.tools.my_tool.parameters.example]
# type = "string"
# required = true
# summary = "Description of the parameter."
```

The template includes a commented-out parameter example as a guide while the
user fills in the command and parameters manually.

### `jp tool import <command>`

`jp tool import` registers a tool from an arbitrary CLI command with LLM
assistance:

```
jp tool import rg
jp tool import -- kubectl get pods
```

1. JP runs `<command> --help` (and `man <command>` if available) to collect help
   text.
2. JP starts a conversation (or uses the current one) with the help text
   attached.
3. Using structured output, the assistant generates a `ToolConfig` TOML from the
   help text and any additional user instructions.
4. JP validates the generated TOML against the config schema and writes it to
   `.jp/mcp/tools/<name>.toml` for the user to review and edit.

The import process generates configuration without executing the tool itself,
keeping a human in the loop for state-mutating commands.
The assistant can still propose example invocations in its output for the user
to test manually.

### `jp tool import --mcp <server>`

`jp tool import --mcp` imports tools from a configured MCP server:

```
jp tool import --mcp my_mcp_server
jp tool import --mcp my_mcp_server --tool search_files
```

1. JP connects to the named MCP server, which must already be configured in
   `providers.mcp`.
2. JP lists the tools available on the server.
3. If `--tool` is specified, JP imports that tool; otherwise, it presents a
   selection UI (or imports all tools).
4. For each selected tool, JP writes `.jp/mcp/tools/<tool>.toml` with:
   - `source = "mcp.<server>.<tool>"`
   - Summary and description from the MCP tool definition
   - Pre-filled parameter overrides where useful
   - Default run mode and style settings

### Output Location

All commands write to the project-level tool config directory,
`.jp/mcp/tools/<name>.toml`, by default.
A future `--global` flag could write to the user-level config directory instead.

## Drawbacks

- **`jp tool import <command>` depends on `--help` quality.** Commands with
  poor, nonstandard, or missing help text give the LLM little to work with.
  In those cases, users can fall back to `jp tool new` and manual editing.
- **LLM-generated config may need editing.** Generated configs for complex tools
  will not always be complete or accurate, so users should expect to review and
  adjust the output.
- **New CLI surface area.** The `jp tool` subcommands add CLI surface to
  document and maintain, though they are thin scaffolding commands with minimal
  logic.

## Alternatives

**Parse `--help` output programmatically.** Writing parsers for common help text
formats (argparse, clap, getopt) would avoid an LLM call, but help output varies
too widely across tools to maintain dedicated parsers.
The LLM handles arbitrary formats directly.

**Parse shell completion scripts.** Some tools ship structured bash, zsh, or
fish completions that are easier to parse than `--help` output, but coverage is
inconsistent and each shell uses a different format.
Completion scripts could still serve as an additional input signal for the LLM
in the future.

**Document manual TOML authoring only.** Relying on documentation alone
preserves the status quo and keeps the manual barrier to adding tools.

## Non-Goals

- **Automatic tool discovery from PATH.** The user explicitly chooses which
  commands to import; JP does not scan PATH for tools.
- **Running imported tools during the import process.** The import flow only
  generates configuration; testing the tool is a separate step.
- **Tool registries or package managers.** There is no central repository of
  tool definitions; users import tools from their own environment.

## Risks and Open Questions

- **Structured output maturity.** `jp tool import <command>` relies on
  structured output (RFD 029) to generate valid TOML.
  If structured output is not reliable enough for TOML generation, the assistant
  can fall back to emitting the TOML in a code block for the user to copy.
- **MCP server availability during import.** `jp tool import --mcp` requires a
  running, reachable MCP server and should fail with a clear error message when
  the server cannot be reached.
- **Config file conflicts.** When a tool config file already exists, the import
  commands must either overwrite, merge, or error.
  Failing by default with a `--force` flag to overwrite, or prompting the user
  interactively, are the main options.

## Implementation Plan

### Phase 1: `jp tool new`

Add the `jp tool new` subcommand to scaffold a TOML tool config template without
LLM interaction.

### Phase 2: `jp tool import <command>`

Add the LLM-assisted import flow, starting with `--help` output as the only
input signal.
This phase depends on structured output (RFD 029).

### Phase 3: `jp tool import --mcp`

Add MCP server import using the existing `jp_mcp::Client` to list tools.
This phase depends on the MCP client being available in the CLI context.

## References

- RFD D06: Self-Describing Local Tools (defines the protocol that imported tools
  can implement)
- RFD 029: Scriptable Structured Output (prerequisite for LLM-assisted import)
- `.jp/mcp/tools/`: default tool config directory
- `crates/jp_config/src/conversation/tool.rs`: `ToolConfig` type

[RFD 029]: ../029-scriptable-structured-output.md
