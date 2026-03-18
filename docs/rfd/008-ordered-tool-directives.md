# RFD 008: Ordered Tool Directives

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-13
- **Tracking Issue**: [#437](https://github.com/dcdpr/jp/issues/437)

## Summary

Make `--tool` and `--no-tools` CLI flags order-sensitive, so interleaved
invocations like `--no-tools --tool=write --no-tools=fs_modify_file` are
processed left-to-right. This replaces the current fixed-order processing that
ignores flag position.

## Motivation

Today, `--tool` values are collected into one `Vec` and `--no-tools` values into
another. `apply_enable_tools` then processes them in a hardcoded sequence:
disable-all > enable-all > enable-named > disable-named. The position of flags
on the command line has no effect.

This means `jp q --tool=write --no-tools --tool=read` and `jp q --no-tools
--tool=write --tool=read` produce the same result, even though a user would
reasonably expect left-to-right evaluation. The current behavior is surprising
and limits the expressiveness of tool selection.

With ordered processing, users can compose tool sets precisely:

```sh
# Start from nothing, add only what you need
jp q --no-tools --tool=write --no-tools=fs_modify_file

# Start from everything, carve out exceptions
jp q --tool --no-tools=dangerous_tool
```

## Design

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
`--no-tools` as before) and `clap::FromArgMatches` manually. The
`FromArgMatches` implementation uses `ArgMatches::indices_of` to recover the
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
directives. Upfront validation (unknown tool names, attempts to disable core
tools) remains.

The existing restriction against combining bare `--no-tools` with bare `--tool`
is removed — ordered evaluation makes that sequence well-defined.

## Implementation Plan

Single PR scoped to `crates/jp_cli/src/cmd/query.rs` and `query_tests.rs`:

1. Add `ToolDirective` enum and `ToolDirectives` struct with manual `clap::Args`
   + `FromArgMatches`.
2. Replace `tools`/`no_tools` fields on `Query` with `#[command(flatten)]
   tool_directives`.
3. Rewrite `apply_enable_tools` to iterate directives sequentially.
4. Update existing tests; add new tests for interleaved ordering.
