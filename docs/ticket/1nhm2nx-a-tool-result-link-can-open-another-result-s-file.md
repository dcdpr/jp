# A tool result link can open another result's file

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-19
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: relevant-rfd=137
- **Label**: type=bug

`ToolRenderer::render_result` (`crates/jp_cli/src/render/tool.rs`) writes each
tool result it renders to a file in the OS temp directory, so that
`results_file_link` has a path to print.
Three things are wrong with that write, and none of them depends on where the
file is stored.

## Defects

1. **Names repeat.** The file is named `tool_call_<millis>.<ext>`, where
   `<millis>` is `subsec_millis()` of the current time: a number from 0 to 999.
   A later result that lands on the same number with the same extension
   overwrites the earlier file, and the earlier link then opens the later
   result.
   The temp directory is shared by every `jp` process, so the files of all runs
   count.
   By calculation, not measurement: with the milliseconds evenly spread, 38
   files of one extension give better than even odds that two of them share a
   name.
2. **A failed write still prints a link.** The result of `fs::write` is
   discarded (`let _err = ...`), and the link is printed either way.
   After a failed write the link names a file that was never written, or an
   older file with the same name.
3. **A file is written that nothing links to.** The write happens before
   `results_file_link` is consulted.
   With `results_file_link = "off"` and inline results on, every rendered result
   still writes a file.
   `jp conversation print` renders through the same function, so a replay writes
   one for every result it shows.

`write_summary_file` in `crates/jp_cli/src/cmd/conversation/compact.rs` has none
of the three: its name is fixed per conversation and turn range, and it prints
no path when the write fails.

## Fix

- Name the file by its content: `tool_call_<hash>.<ext>`, where `<hash>` is the
  first 16 hex characters of the SHA-256 of the bytes written.
  Different results get different names, and the same result rendered twice is
  one file.
  `sha2` is already a dependency of `jp_cli`.
- Write the file only in the two branches that print a link (`LinkStyle::Full`
  and `LinkStyle::Osc8`).
- When the write fails, log a warning and print no link.
  The inline result still renders.

The files stay in the OS temp directory.
RFD 137 moves them to a per-conversation directory and prunes them by age, and
its second phase includes these three fixes.
This ticket makes them where the files are today, so wrong and dead links stop
before the RFD is accepted.
The hash is the one RFD 137 specifies, so the later move changes the directory
and the `tool_call` label, not the naming scheme.

## Verifying

Tests in `crates/jp_cli/src/render/tool_tests.rs`, which has none for the file
today:

- Two results with different content, rendered with `results_file_link =
  "full"`, print two different paths, and each file holds its own content.
- The same result rendered twice prints the same path.
- A result rendered with `results_file_link = "off"` and inline results on
  leaves no file behind.
- A write that fails prints no `see:` line and still prints the inline result.

The last two need a target directory the test controls; `render_result` reads
`env::temp_dir()` directly today.

## Related

- RFD 137 (Stable Storage for Link Files): the location, the retention window,
  and percent-encoded link URLs, none of which are part of this ticket.
