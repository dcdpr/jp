# A piped query opens the editor under --no-interactive

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-18
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: type=bug

`jp query` reads a query from piped stdin and then opens the editor to compose
it, even when `--no-interactive` is set.
In an environment with no terminal the editor blocks forever, and the run hangs
until something kills it.

## Reproduction

```sh
printf 'hello\n' | jp --no-interactive query --new -l > /tmp/out.log 2>&1
```

Expected: the turn runs with `hello` as the query.

Observed: `$EDITOR` is spawned on the conversation's `QUERY_MESSAGE.md` and the
process sits until killed.
`/tmp/out.log` stays empty, so nothing indicates what the run is waiting for.

Seen while driving `jp` from an unattended shell loop with stdout redirected to
a file.
The symptom at the call site is a `jp` process with no output and essentially no
CPU time, plus an editor process nobody asked for.

## Cause

`Query::edit_message` skips the editor when any of three conditions hold
(`crates/jp_cli/src/cmd/query.rs`, around line 1003):

```rust
if (self.input.query.as_ref().is_some_and(|v| !v.is_empty())
    || !piped
    || self.force_no_edit())
    && !self.force_edit()
    && !request.is_empty()
{
    return Ok((source, PartialAppConfig::empty()));
}
```

With the query on stdin and no positional argument, the first clause is false
and `!piped` is false.
The third is `force_no_edit()`, which is:

```rust
fn force_no_edit(&self) -> bool {
    self.no_edit
}
```

It reads the `--no-edit` flag and nothing else, so a caller that declared the
run non-interactive still lands in the editor branch.
Seeding the editor from piped stdin is the intended compose flow; doing it when
nobody is present to close the editor is not.

## Suggested fix

Have `force_no_edit()` also return `true` when the invocation is
non-interactive, so `--no-interactive` and `JP_NONINTERACTIVE=1` suppress the
editor the way `--no-edit` does.
`Term::interactive` already carries the signal.

`--edit` passed explicitly alongside `--no-interactive` is a contradiction worth
rejecting with a diagnostic rather than resolving silently either way.

## Test

An integration test that pipes a query with `--no-interactive` and an
`editor.cmd` pointing at a command that would fail or hang if invoked, asserting
the turn completes and the editor command never ran.
Asserting only that the run succeeds would pass if the editor were spawned and
happened to exit zero.

## Note

`--no-interactive`'s own help text names "a script or a CI job" as the case it
exists for, which is exactly the case that hangs.
