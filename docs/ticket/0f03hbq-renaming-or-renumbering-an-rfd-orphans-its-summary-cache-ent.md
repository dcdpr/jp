# Renaming or renumbering an RFD orphans its summary-cache entry

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-09-07

`docs/.vitepress/rfd-summaries.json` is keyed by RFD filename.
`just rfd-renumber` and `just rfd-rename` both move the file, so the old key is
left pointing at a document that no longer exists and the new filename has no
entry at all.

Both recipes handle this by printing `Run \`just rfd-summaries\` to refresh the
summary cache.` to stderr.
That regenerates the missing entry, at the cost of an LLM call for a document
whose content did not change, and it never removes the orphan.

Two ways out, and the second looks better:

- Have each recipe move the key alongside the file.
- Have `rfd-summaries` prune entries whose file is gone, and match an existing
  entry by its content hash before regenerating.
  The hash is already stored next to the summary, so a renamed file with
  unchanged content could re-key its own entry with no LLM call.

The second keeps the recipes ignorant of the cache and fixes the orphan left by
every rename already committed.
