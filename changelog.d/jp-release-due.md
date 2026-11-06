---
type: added
---
`jp release due` says whether the next release should be cut, and exits 0 when
it should and 1 when it shouldn't.
A release is due once `plugins.command.release.options.cadence` has passed since
the last minor or major release was cut, that release has been finalized, and
`main` has a fragment no release lists yet.
The cadence is a duration such as `2w` or `10d`, and defaults to two weeks.
With `jp --format json`, it prints `{"due": …, "reason": …}`, so a script can
tell a release that isn't due from a command that failed.
