# A read-write mount after a read-only one stays read-only

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-15
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: relevant-rfd=076
- **Label**: type=bug

`jp query --mount` adds an access rule per tool for each mount, and skips a tool
that has a rule for the mount's path already (`inject_mounts` in
`crates/jp_cli/src/cmd/query.rs`).
The skip ignores the mode, so a `TOOL:NAME=PATH:rw` mount listed after a
`NAME=PATH` mount of the same name leaves that tool read-only, without a
warning:

```sh
jp query --mount code=../code --mount wiki_edit:code=../code:rw ...
```

Listed the other way round, `wiki_edit` can write, as it should.
A later mount of the same path that asks for `rw` should widen the tool's rule
to read-write.
