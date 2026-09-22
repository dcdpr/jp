# Renaming or renumbering an RFD orphans its summary-cache entry

- **Status**: Done
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-09-07
- **Label**: domain=tooling
- **Label**: type=bug

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

## Comments

-----

- **From**: jp
- **Date**: 2026-09-22T10:56:16Z

Resolved by removing the cache, not by fixing either of the two ways out.

An RFD's summary is a `- **Summary**:` field in its metadata header, so it moves
with the file.
`rfd-rename` and `rfd-renumber` have nothing left to re-key, and both lost the
`Run \`just rfd-summaries\`` line they printed.

`rfd-summaries` is gone with it.
The author writes the sentence; the docs build rejects a published RFD that has
none (`checkSummaries` in `docs/.vitepress/loaders/rfd-shared.mjs`).
Whether a summary still describes its document is a question for whoever reviews
the diff, which now shows the prose and the sentence describing it in the same
file.

`docs/.vitepress/rfd-summaries.json` was the merge-conflict hotspot that
prompted the change: 113 commits touched it in twelve months, and every new RFD
appended at EOF, which conflicts between any two branches that add one.
