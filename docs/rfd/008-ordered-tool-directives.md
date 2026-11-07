# RFD 008: Ordered Tool Directives

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-13
- **Tracking Issue**: [\#437]
- **Extended by**: [RFD 081]
- **Summary**: Processes `--tool` and `--no-tools` CLI flags left-to-right so
  interleaved flags compose in the order given.

## Summary

This RFD makes the `--tool` and `--no-tools` CLI flags order-sensitive,
evaluating interleaved invocations like `--no-tools --tool=write
--no-tools=fs_modify_file` from left to right rather than in a fixed precedence
order.

## Motivation

`Query` collects `--tool` values into one `Vec` and `--no-tools` values into
another, and `apply_enable_tools` processes them in a hardcoded sequence:
disable-all > enable-all > enable-named > disable-named.
Flag position on the command line is ignored, so `jp q --tool=write --no-tools
--tool=read` and `jp q --no-tools --tool=write --tool=read` produce the same
result.

Evaluating flags from left to right lets users compose tool sets in order:

```sh
# Start from nothing, add only what you need
jp q --no-tools --tool=write --no-tools=fs_modify_file

# Start from everything, carve out exceptions
jp q --tool --no-tools=dangerous_tool
```

## Design

> [!TIP]
> [RFD 081] gates each directive on the tool's `allow_toggle` policy (`any`,
> `never`, `if_named`, `if_named_or_group`): a directive mutates the tool's
> `state` only when that policy permits it, and named directives forbidden by a
> policy return an error.
> Bare `-t` and `-T` are bulk-scope directives that only affect freely
> toggleable (`any`) tools, so they do not erase a tool's classification as the
> original `test_interleaved_disable_all_then_enable_all` test documented.

Replace the separate `tools` and `no_tools` fields on `Query`:

```rust
tools: Vec<Option<String>>,
no_tools: Vec<Option<String>>,
```

with an ordered `ToolDirectives` list:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
enum ToolDirective {
    EnableAll,
    DisableAll,
    Enable(String),
    Disable(String),
}

#[derive(Debug, Clone, Default)]
struct ToolDirectives(Vec<ToolDirective>);
```

`ToolDirectives` manually implements `clap::Args` and `clap::FromArgMatches`.
`Args` defines `--tool` (`-t`) and `--no-tools` (`-T`) with the same value
syntax as before, so the CLI surface is unchanged.
`FromArgMatches` uses `ArgMatches::indices_of` to read the command-line index of
each value across both flags, then merges and sorts the directives by index into
a single ordered list.

`Query` flattens `ToolDirectives`:

```rust
#[command(flatten)]
tool_directives: ToolDirectives,
```

`apply_enable_tools` replaces its four fixed steps with a sequential loop over
`ToolDirectives`.
Upfront validation for unknown tool names and attempts to disable core tools is
unchanged, while the restriction against combining bare `--no-tools` with bare
`--tool` is removed because left-to-right evaluation makes that sequence
well-defined.

## Implementation Plan

A single PR scoped to `crates/jp_cli/src/cmd/query.rs` and `query_tests.rs`:

1. Add the `ToolDirective` enum and `ToolDirectives` struct with manual
   `clap::Args` and `clap::FromArgMatches` implementations.
2. Replace the `tools` and `no_tools` fields on `Query` with
   `#[command(flatten)] tool_directives`.
3. Rewrite `apply_enable_tools` to apply directives sequentially.
4. Update existing tests and add tests for interleaved ordering.

[RFD 081]: 081-decompose-tool-enable-into-state-and-allow_toggle.md
[\#437]: https://github.com/dcdpr/jp/issues/437
