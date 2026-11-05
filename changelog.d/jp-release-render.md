---
type: added
---
`jp release render` prints a release's notes, its upgrade guide with `--page
upgrade`, or the changes no release has taken yet, as Markdown or, with
`--format json`, as JSON for other tools to render.
Each entry credits the pull requests that wrote it, read from git history.
`plugins.command.release.options.forge` names where those are reviewed
(`github`, `gitlab`, or `none`, and otherwise read from git's `origin` remote),
and `plugins.command.release.options.headings` renames a type's section of the
release notes.
