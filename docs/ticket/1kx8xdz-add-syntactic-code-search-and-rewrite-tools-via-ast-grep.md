# Add syntactic code search and rewrite tools via ast-grep

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-16
- **Label**: domain=tooling
- **Label**: type=feature

Assistants search and edit code with regex tools (`fs_grep_files`,
`fs_modify_file`).
A question about code shape, such as "every call to `foo` that passes a
closure", is awkward as a regex, and the same structural edit across many files
takes one pattern per variation.

Add two local tools that wrap [ast-grep]: one that searches by syntax pattern
and one that rewrites the matches.
Working names: `code_search` and `code_rewrite`.

ast-grep keeps no state and covers many languages through tree-sitter grammars.
It fits the one-process-per-call local-tool model, and serves the Swift and
TypeScript in this repository as well as the Rust.

## Scope

- [ ] A search tool: pattern, language, `paths`, and `context`, with matches
      formatted like `fs_grep_files`.
- [ ] A rewrite tool: pattern and replacement, applied through the same write
      pipeline as `fs_modify_file` (`access` check, dirty-file prompt,
      `apply_changes`).
- [ ] Config under `.jp/mcp/tools/` and a skill file that enables both for the
      dev persona.

## Not in scope

These tools match syntax, not symbols: `$X.iter()` matches every `.iter()` call
regardless of receiver type.
Semantic symbol lookup and renaming require a language server (`code_symbol` and
`code_rename`).

## To decide while implementing

- Shell out to the `ast-grep` CLI and read its JSON output, or link its crates
  into `jp-tools`.
- How a pattern that fails to parse is reported.
- Whether the rewrite tool reports the diff, as `fs_modify_file` does, or counts
  per file.

[ast-grep]: https://github.com/ast-grep/ast-grep
