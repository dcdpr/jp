---
type: added
---
The `jp release` plugin records what each change means for the people using a
project, as changelog fragments and highlights for its release notes.
`jp release init` sets up the fragments directory, `changelog.d/` by default.
`jp release add` writes a fragment, or a highlight with `--highlight`, and `jp
release check` checks every file in the directory against the format.
`jp release tools init` writes the definition of the `changelog_add` tool,
through which the assistant writes fragments too.
