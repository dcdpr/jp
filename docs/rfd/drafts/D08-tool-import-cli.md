# RFD D08: Tool Import CLI

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-12

## Summary

This RFD introduces `jp tool` subcommands that streamline adding new tools to a
JP workspace: `jp tool new` for scaffolding empty configs, `jp tool import` for
LLM-assisted tool registration from arbitrary commands, and `jp tool import
--mcp` for importing tools from MCP servers.

## Motivation

Adding a tool to JP today requires manually creating a TOML config file with the
correct structure, parameter types, descriptions, and source configuration. For
local tools, this means understanding the command's flags and translating them
into JP's parameter schema. For MCP tools, it means knowing the server name,
tool name, and manually referencing them in config.

This friction is the single biggest barrier to tool adoption. Users who want to
expose a CLI command to the assistant face a multi-step manual process before
they can even test whether the tool works.

The goal is to reduce "I want to use ripgrep as a tool" to a single command that
produces a working tool config.

## Design

### `jp tool new <name>`

Scaffolds an empty TOML tool config file.

```
jp tool new my_tool
```

Creates `.jp/mcp/tools/my_tool.toml` with a minimal template:

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

This is the lowest-friction starting point. The user fills in the command and
parameters manually. The template includes commented-out parameter examples as
a guide.

### `jp tool import <command>`

LLM-assisted tool registration from an arbitrary CLI command.

```
jp tool import rg
jp tool import -- kubectl get pods
```

**Flow:**

1. JP runs `<command> --help` (and optionally `man <command>` if available) to
   collect help text.
2. JP starts a conversation (or uses the current one) with the help text
   attached.
3. Using structured output, the assistant generates a `ToolConfig` TOML based
   on the help text and any additional user instructions.
4. JP validates the generated TOML against the config schema.
5. JP writes the result to `.jp/mcp/tools/<name>.toml`.
6. The user can review, edit, and iterate.

This dogfoods JP's own capabilities — the assistant reads `--help` output and
produces structured config, the same way a human would but faster.

**Safety:** The assistant generates *config*, not *executions*. It proposes
example invocations in its output that the user can manually test, but it does
not run the tool during the import process. This keeps the human in the loop for
state-mutating commands.

### `jp tool import --mcp <server>`

Import tools from a configured MCP server.

```
jp tool import --mcp my_mcp_server
jp tool import --mcp my_mcp_server --tool search_files
```

**Flow:**

1. JP connects to the named MCP server (must already be configured in
   `providers.mcp`).
2. Lists available tools from the server.
3. If `--tool` is specified, imports that specific tool. Otherwise, presents a
   selection UI (or imports all).
4. For each selected tool, generates a TOML config entry with:
   - `source = "mcp.<server>.<tool>"`
   - Summary and description from the MCP tool definition
   - Parameter overrides pre-filled where useful
   - Default run mode, style settings
5. Writes to `.jp/mcp/tools/<tool>.toml`.

This reduces MCP tool setup from manual TOML authoring to a single command.

### Output Location

All commands write to `.jp/mcp/tools/<name>.toml` by default. This is the
project-level tool config directory. A `--global` flag could write to the
user-level config directory instead (future consideration).

## Drawbacks

- **`jp tool import <command>` depends on `--help` quality.** Some commands have
  poor, nonstandard, or nonexistent help text. The LLM can only work with what
  it gets. The fallback is always `jp tool new` + manual editing.
- **LLM-generated config may need editing.** The structured output won't be
  perfect for complex tools. This is a starting point, not a final product. The
  user should expect to review and adjust.
- **New CLI surface area.** Adding `jp tool` subcommands increases the number of
  commands to document and maintain. However, these are simple scaffolding
  commands with minimal logic.

## Alternatives

**Parse `--help` output programmatically.** Instead of using the LLM, write a
parser for common help text formats (argparse, clap, getopt). Rejected because
help text formats vary wildly, and maintaining parsers for each is more work than
the LLM approach. The LLM handles variety naturally.

**Parse shell completion scripts.** Some tools ship structured completions for
bash/zsh/fish that are more parseable than `--help`. Coverage is spotty and
formats differ. Could be a future enhancement as an additional signal for the
LLM.

**No import command — just documentation.** Users follow a guide to write TOML
by hand. This is the status quo. It works but doesn't scale to the "make it
easy" goal.

## Non-Goals

- **Automatic tool discovery from PATH.** This RFD is about explicit import, not
  ambient discovery. The user decides what to import.
- **Running imported tools during the import process.** The import flow generates
  config only. Testing the tool is a separate step.
- **Tool registries or package managers.** No central repository of tool
  definitions. Each user imports tools from their own environment.

## Risks and Open Questions

- **Structured output maturity.** The `jp tool import <command>` flow relies on
  structured output (RFD 029) to generate valid TOML. If structured output isn't
  reliable enough for TOML generation, the fallback is the assistant producing
  the TOML in a code block for the user to copy. Need to assess structured
  output readiness.
- **MCP server availability during import.** `jp tool import --mcp` requires the
  MCP server to be running and reachable. If the server is down, the import
  fails. Error messages should be clear about this.
- **Config file conflicts.** If a tool config file already exists, the import
  commands need a strategy: overwrite, merge, or error. Likely: error with a
  `--force` flag, or prompt the user.

## Implementation Plan

### Phase 1: `jp tool new`

Add the `jp tool new` subcommand. Scaffolds a TOML template. No LLM
interaction. This is a straightforward CLI addition.

### Phase 2: `jp tool import <command>`

Add the LLM-assisted import flow. Depends on structured output (RFD 029) being
functional. Start with `--help` as the only input signal.

### Phase 3: `jp tool import --mcp`

Add MCP server import. Depends on the MCP client being available in the CLI
context. Uses the existing `jp_mcp::Client` to list tools.

## References

- RFD D06: Self-Describing Local Tools (defines the protocol that imported
  tools can implement)
- RFD 029: Scriptable Structured Output (prerequisite for LLM-assisted import)
- `.jp/mcp/tools/` — default tool config directory
- `crates/jp_config/src/conversation/tool.rs` — `ToolConfig` type
