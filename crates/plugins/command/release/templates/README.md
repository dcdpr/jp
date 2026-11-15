# Changelog fragments

Each pull request that changes something users notice adds a fragment to this
directory: a short entry that tells them what changed.
A feature worth more than a line also gets a highlight, a few paragraphs of the
release notes that tell its story.
Release notes are built from these files rather than from commit messages, so
write for someone using the software, not for someone reading its code.
`jp release fixed`, like the command for each other type, opens a fragment in
your editor, `jp release highlight` opens a highlight, and `jp release check`
checks every file here.
The formats are specified in [RFD 121].

## Writing a fragment

A fragment is a Markdown file directly in this directory, named after the change
and its type, such as `enable-decomposition.added.md`.
Its text is the entry:

```markdown
`tools.<name>.enable` splits into `state` and `allow_toggle`, so you can pin a
tool on or off while still letting the assistant request the other state.
```

The type says what the change means for users, and is one of:

| Type          | Used for                                        |
| ------------- | ----------------------------------------------- |
| `added`       | A new capability the user can now reach.        |
| `changed`     | A change to existing behavior.                  |
| `deprecated`  | Something still working but slated for removal. |
| `removed`     | Something taken away.                           |
| `fixed`       | A bug fix the user would notice.                |
| `security`    | A fix with security relevance.                  |
| `performance` | A user-visible performance improvement.         |

It is not a conventional-commit type: an internal `feat` changes nothing users
see, and a `refactor` can change what they see.

`jp release added` opens `<slug>.added.md` in your editor, and checks it once
you save it.
The slug is the name of the git branch you are on, the part after its last `/`,
unless you name one: `jp release added <slug>`.
Running it again opens the same fragment for another edit, and saving a fragment
empty removes it.

A pull request that makes several user-visible changes adds a fragment for each,
and one slug can name a fragment of each type.
A follow-up to an earlier change adds a fragment of its own, saying what it
adds, whether or not the earlier one has been released.
Edit an existing fragment only to correct its wording.

## Breaking changes

A change that requires users to act carries its section of the release's upgrade
guide, which starts at a paragraph opening with `BREAKING CHANGE:`, as in
`enable-explicit.removed.md`:

````markdown
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
from `main`'s history, or to the commit itself when it reached `main` without
one.
When those credits are wrong, `Credits` fields at the top of the file replace
them, one per pull request or commit:

```markdown
- **Credits**: #1207
- **Credits**: 4e1f0a2

A turn that stalls while idle is retried instead of failing.
```

`Credits` also decide which release lists the fragment: the first one that ships
every change they name.
Without them, that is the first release that ships the commit adding the
fragment, so a fragment written after its change merged needs them.

A fragment takes no other field: its type is in its name.

## Highlights

Most changes need only a fragment, and a highlight is for the handful of
features that deserve a story of their own.
It is a Markdown file in `highlights/`, which opens with its title and credits
each author on a line of their own:

```markdown
# Unified editor and inline reply widget

- **Authors**: @JeanMertz

Every prompt in JP now runs through a single editor service, and short replies
can be written inline without leaving the conversation view.
Longer edits still open your editor, so the two paths share one backend and one
set of keybindings.
```

The title heads the highlight's section of the release notes, and at least one
author is credited for the feature.
`jp release highlight` opens a new one in your editor, crediting you, and names
its file after the title once you save it.
`jp release highlight <slug>` opens `highlights/<slug>.md`.

A highlight has no type and no upgrade section, and adds no entry to the list of
changes, so a pull request with a highlight still adds a fragment.
A line that starts with `BREAKING CHANGE:` or `BREAKING-CHANGE:` is an error in
a highlight.

A highlight describes what is on `main`, since a release can be cut after any
pull request.
A feature that is still hidden gets its highlight in the pull request that makes
it visible.
One that is visible but incomplete gets a highlight describing what is there,
which later pull requests extend.
The release notes credit every pull request that wrote or changed the highlight,
and `Credits` fields below its authors replace those credits, as in a fragment.

## Changes users don't see

A pull request that changes nothing users see adds no fragment.
That covers most build, CI, documentation, refactoring, and test work.
A project whose CI asks every pull request for a fragment also says how to mark
one that needs none, such as with a label.

## Released fragments

Cutting a release moves its fragments and highlights into a directory named
after its version, which may also hold `overview.md`, the release's
introduction:

```text
changelog.d/
  README.md
  enable-explicit.removed.md    # unreleased
  highlights/fan-out.md         # unreleased
  0.3.0/
    overview.md
    retry-idle-turns.fixed.md
    highlights/plugin-tools.md
```

Released fragments and highlights stay here; correct one like any other file.

## Checking

`jp release check` checks every file here against these rules, released ones
included.
Anything else, such as a fragment without a type in its name, another extension,
or a stray directory, is an error.

[RFD 121]: https://jp.computer/rfd/121-changelog-fragments-and-validation
