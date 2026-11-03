# Changelog fragments

Each pull request that changes something users notice adds a fragment to this
directory: a short entry that tells them what changed.
A feature worth more than a line also gets a highlight, a few paragraphs of the
release notes that tell its story.
Release notes are built from these files rather than from commit messages, so
write for someone using JP, not for someone reading its code.
The formats are specified in [RFD 121].

## Writing a fragment

A fragment is a Markdown file directly in this directory, named after the
change, such as `enable-decomposition.md`.
Its name only has to be unique in the directory.
Two names are reserved in any letter case: `overview`, for a release's
introduction, and `readme`, for this README.

The frontmatter holds the change's `type`, and the entry follows it:

```markdown
---
type: added
---
`tools.<name>.enable` splits into `state` and `allow_toggle`, so you can pin a
tool on or off while still letting the assistant request the other state.
```

The `type` says what the change means for users, and is one of:

| `type`        | Used for                                        |
| ------------- | ----------------------------------------------- |
| `added`       | A new capability the user can now reach.        |
| `changed`     | A change to existing behavior.                  |
| `deprecated`  | Something still working but slated for removal. |
| `removed`     | Something taken away.                           |
| `fixed`       | A bug fix the user would notice.                |
| `security`    | A fix with security relevance.                  |
| `performance` | A user-visible performance improvement.         |

It is not the conventional-commit type of the pull request's title: an internal
`feat` changes nothing users see, and a `refactor` can change what they see.

A pull request that makes several user-visible changes adds a fragment for each.
A follow-up to an earlier change adds a fragment of its own, saying what it
adds, whether or not the earlier one has been released.
Edit an existing fragment only to correct its wording.

## Breaking changes

A change that requires users to act carries its section of the release's upgrade
guide, which starts at a paragraph opening with `BREAKING CHANGE:`.

````markdown
---
type: removed
---
`tools.<name>.enable` no longer accepts the legacy string `"explicit"`.

BREAKING CHANGE: `enable = "explicit"` is no longer accepted

`"explicit"` was the last of the string forms `enable` accepted before 0.2.0.
Replace it with the table form, which means the same thing:

```toml
# 0.2
enable = "explicit"

# 0.3
enable = { state = false, allow_toggle = "if_named" }
```
````

Such a fragment has three parts:

- The entry is everything before the marker.
- The title is the rest of the marker's paragraph: what broke.
- The body is every paragraph after it: what changed, why, and how to upgrade.

`BREAKING-CHANGE:` works as the marker too.
All three parts are required, and a fragment has at most one upgrade section.
A line that starts with either token anywhere else, inside a code block or
partway through a paragraph, is an error.
To show the token instead, indent its line or put it in inline code.

## Credits

A fragment doesn't name its pull request.
The release notes credit it to the pull requests that wrote or changed it, read
from `main`'s history.
When those credits are wrong, `change_requests` replaces them:

```markdown
---
type: fixed
change_requests: [1207, 1211]
---
A turn that stalls while idle is retried instead of failing.
```

## Highlights

Most changes need only a fragment, and a highlight is for the handful of
features that deserve a story of their own.
It is a Markdown file in `highlights/`, named like a fragment:

```markdown
---
title: "Unified editor and inline reply widget"
authors: ["@JeanMertz"]
---
Every prompt in JP now runs through a single editor service, and short replies
can be written inline without leaving the conversation view.
Longer edits still open your editor, so the two paths share one backend and one
set of keybindings.
```

`title` heads the highlight's section of the release notes, and `authors` lists
at least one person to credit for the feature.
Quote a handle that starts with `@`, as the example does, since YAML reserves
the character.

A highlight has no `type` and no upgrade section, and adds no entry to the list
of changes, so a pull request with a highlight still adds a fragment.
A line that starts with `BREAKING CHANGE:` or `BREAKING-CHANGE:` is an error in
a highlight.

A highlight describes what is on `main`, since a release can be cut after any
pull request.
A feature that is still hidden gets its highlight in the pull request that makes
it visible.
One that is visible but incomplete gets a highlight describing what is there,
which later pull requests extend.
The release notes credit every pull request that wrote or changed the highlight,
and `change_requests` replaces those credits, as in a fragment.

## Changes users don't see

A pull request that changes nothing users see adds no fragment.
That covers most build, CI, documentation, refactoring, and test work.

JP's CI requires a fragment in every pull request into `main`, unless it carries
the `no-changelog` label.
Give a pull request that label when users won't notice its change, or when it
only corrects a fragment's wording.
A pull request whose title marks a breaking change with `!`, such as
`feat(cli)!: ...`, also needs an upgrade section in a fragment it adds.
The label covers that too, for a break users never see.

## Released fragments

Cutting a release moves its fragments and highlights into a directory named
after its version, which may also hold `overview.md`, the release's
introduction:

```text
changelog.d/
  README.md
  enable-explicit.md          # unreleased
  highlights/fan-out.md       # unreleased
  0.3.0/
    overview.md
    retry-idle-turns.md
    highlights/plugin-tools.md
```

Released fragments and highlights stay here; correct one like any other file.

## Checking

Every file here is checked against these rules, released ones included.
Anything else, such as another extension, a stray directory, or a README in
`highlights/`, is an error.
Run the check with:

```sh
cargo run --package changelog -- check
```

[RFD 121]: ../docs/rfd/121-changelog-fragments-and-validation.md
