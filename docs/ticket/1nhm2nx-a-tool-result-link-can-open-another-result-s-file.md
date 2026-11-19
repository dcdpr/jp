# A tool result link can open another result's file

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-19
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: relevant-rfd=137
- **Label**: type=bug

`ToolRenderer::render_result` (`crates/jp_cli/src/render/tool.rs`) writes every
rendered tool result to a file in the OS temp directory so `results_file_link`
has a path to print.
That write has three bugs independent of where the file is stored.

## Defects

1. **Filenames collide.** The file is named `tool_call_<millis>.<ext>`, where
   `<millis>` is `subsec_millis()` of the current system time (`0..=999`).
   Any later tool result that lands on the same sub-second millisecond with the
   same file extension overwrites the earlier file, causing the earlier link to
   open the newer result.
   Because all `jp` processes share the OS temp directory, collisions can also
   occur across concurrent runs.
   Assuming uniform millisecond distribution, 38 results with the same extension
   exceed a 50% collision probability.
2. **Failed writes still print a link.** `render_result` discards the result of
   `fs::write` (`let _err = ...`) and prints the link unconditionally, leaving a
   link that points to a missing file or to an older file with the same name.
3. **Files are written even when no link is printed.** `render_result` writes
   the temp file before checking `results_file_link`.
   With `results_file_link = "off"` and inline results enabled, every rendered
   tool result still writes an unreferenced file, and replaying a conversation
   with `jp conversation print` rewrites a file for every result.

`write_summary_file` in `crates/jp_cli/src/cmd/conversation/compact.rs` has a
narrower form of the first defect: its filename is fixed per conversation ID and
turn range (avoiding clock collisions, and printing no path on write failure),
but re-compacting the same turn range after `jp conversation compact --reset`
overwrites the file an earlier path points to.
This ticket leaves `write_summary_file` unchanged; RFD 137 switches summary
files to content-addressed names as well.

## Fix

- Name the file by content hash: `tool_call_<hash>.<ext>`, where `<hash>` is the
  first 16 lowercase hex characters of the SHA-256 digest of the written bytes
  (`sha2` is already a dependency of `jp_cli`).
  Distinct results get distinct filenames, and identical results share a single
  file.
- Write the file only in the branches that emit a link (`LinkStyle::Full` and
  `LinkStyle::Osc8`).
- If the write fails, log a warning, omit the link, and still render any inline
  result.

This ticket keeps the files in the OS temp directory so collisions and dead
links are fixed without waiting for RFD 137.
RFD 137 subsequently moves link files to a per-conversation directory with
age-based pruning and reuses the same 16-character SHA-256 naming scheme.

## Verifying

Add unit tests in `crates/jp_cli/src/render/tool_tests.rs` (configuring a
test-controlled target directory rather than reading `env::temp_dir()`
directly):

- Rendering two tool results with different content under `results_file_link =
  "full"` prints two distinct paths, and each file contains its respective
  content.
- Rendering the same result twice prints the same path.
- Rendering a result with `results_file_link = "off"` and inline results enabled
  writes no file.
- When writing the file fails, `render_result` omits the `see:` line and still
  renders the inline result.

## Related

- RFD 137 (Stable Storage for Link Files): Moves link files to the
  User-Workspace Directory, adds age-based pruning, and percent-encodes link
  URLs.
