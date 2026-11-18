# Validate conversations read-only and report failures

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-18
- **Label**: client=cli
- **Label**: domain=storage
- **Label**: package=jp_cli
- **Label**: package=jp_storage
- **Label**: package=jp_workspace
- **Label**: type=enhancement
- **Parent**: T-1n2tnwv

Startup validation moves invalid conversations to `.trash/`.
It should leave the filesystem untouched and report failures instead.

- [ ] `FsStorageBackend::sanitize` no longer calls `trash_invalid_conversation`;
      the report lists invalid conversations instead of trashed ones
- [ ] Invalid conversations never enter the conversation index or reach core
      code
- [ ] Directory walkers that reconcile or remove by id tolerate invalid
      directories without touching them (see T-0sr3zez, T-0vmwqfq)
- [ ] After the command's output, on stderr: up to 3 failures each with id,
      path, and error; more than 3 summarized as "N conversations failed to
      load"; both point at `jp doctor`
- [ ] Works the same under `--no-persist` and for read-only (`Inspect`) loads
- [ ] Glossary entry updated (W-00dxybd says sanitization moves broken
      conversations to the trash)
