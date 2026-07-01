# RFD 008: Ordered Tool Directives

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-13
- **Tracking Issue**: [\#437]
- **Extended by**: [RFD 081]

## Summary

Make `--tool` and `--no-tools` CLI flags order-sensitive, so interleaved
invocations like `--no-tools --tool=write --no-tools=fs_modify_file` are
processed left-to-right.
This replaces the current fixed-order processing that ignores flag position.

## Motivation

Today, `--tool` values are collected into one `Vec` and `--no-tools` values into
another.
`apply_enable_tools` then processes them in a hardcoded sequence: disable-all \>
enable-all \> enable-named \> disable-named.
The position of flags on the command line has no effect.

This means `jp q --tool=write --no-tools --tool=read` and `jp q --no-tools
--tool=write --tool=read` produce the same result, even though a user would
reasonably expect left-to-right evaluation.
The current behavior is surprising and limits the expressiveness of tool
selection.

With ordered processing, users can compose tool sets precisely:

```sh
# Start from nothing, add only what you need
jp q --no-tools --tool=write --no-tools=fs_modify_file

# Start from everything, carve out exceptions
jp q --tool --no-tools=dangerous_tool
```

## Design

> [!TIP]
> Since RFD 081, a directive only mutates a tool's `state`; whether a given
> directive may flip that state is gated by the tool's `allow_toggle` policy
> (`any` / `never` / `if_named` / `if_named_or_group`).
> Bare `-t` / `-T` are bulk-scope directives and only affect freely-toggleable
> (`any`) tools, so they can no longer erase a tool's classification (the
> behavior the original `test_interleaved_disable_all_then_enable_all`
> documented).
> Named directives a policy forbids are errors.
> See RFD 081 for the scope/policy model.

Replace the two separate fields on `Query`:

```rust
tools: Vec<Option<String>>,
no_tools: Vec<Option<String>>,
```

with a single flattened struct:

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

`ToolDirectives` implements `clap::Args` (defining both `--tool` and
`--no-tools` as before) and `clap::FromArgMatches` manually.
The `FromArgMatches` implementation uses `ArgMatches::indices_of` to recover the
position of each value across both args, then merges and sorts them by index
into a single ordered list.

The `Query` struct uses `#[command(flatten)]`:

```rust
#[command(flatten)]
tool_directives: ToolDirectives,
```

The CLI surface is unchanged: same flags, same short forms (`-t`, `-T`), same
value syntax.

`apply_enable_tools` changes from four fixed steps to a sequential loop over
directives.
Upfront validation (unknown tool names, attempts to disable core tools) remains.

The existing restriction against combining bare `--no-tools` with bare `--tool`
is removed — ordered evaluation makes that sequence well-defined.

## Implementation Plan

Single PR scoped to `crates/jp_cli/src/cmd/query.rs` and `query_tests.rs`:

1. Add `ToolDirective` enum and `ToolDirectives` struct with manual `clap::Args`
   - `FromArgMatches`.
2. Replace `tools`/`no_tools` fields on `Query` with `#[command(flatten)]
   tool_directives`.
3. Rewrite `apply_enable_tools` to iterate directives sequentially.
4. Update existing tests; add new tests for interleaved ordering.

[RFD 081]: 081-decompose-tool-enable-into-state-and-allow_toggle.md
[\#437]: https://github.com/dcdpr/jp/issues/437
