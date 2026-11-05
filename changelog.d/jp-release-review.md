---
type: added
---
`jp release review [VERSION]` asks the assistant to review a release's notes and
upgrade guide, or the unreleased fragments without a version.
It reports entries that read like commit messages or don't say what changed for
users, upgrade sections that don't say what to do, and typos, each with the file
to fix.
It edits nothing and exits 0 whatever it finds; with `jp --format json`, it
prints the findings as JSON.
