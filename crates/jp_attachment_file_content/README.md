# Attachment: File Content

This crate provides an attachment handler for the full content of a file.

## File URI Format

Write a file URI as `file:<path>`, with `path` relative to the workspace root.
The following URIs are equivalent:

- `file:path/to/file.txt`
- `file:/path/to/file.txt`
- `file:///path/to/file.txt`

Do not write `file://path/to/file.txt`.
In that form, `path` is parsed as the URL host, which the URL parser lowercases
and IDNA-encodes before the handler sees it (`file://Docs/notes.md` becomes
`docs/notes.md`).
For backward compatibility, the handler still treats a non-empty host as the
first path segment, but logs a warning suggesting `file:<path>`.

In a config table, `path` is used verbatim, so `?`, `#`, and `%` are treated as
literal characters in the file name:

```toml
[[conversation.attachments]]
type = "file"
path = "Docs/Notes.md"
```

Pass exclusions and other options in `params` rather than appending a query
string to `path`.
A table written as:

```toml
[[conversation.attachments]]
type = "file"
path = "docs/**?exclude"
```

fails to parse as a glob because `**` must be an entire path component.
Write it as:

```toml
[[conversation.attachments]]
type = "file"
path = "docs/**"
params = { exclude = "" }
```

Every path must resolve inside the workspace.
A path that escapes the workspace through `..` segments or a symlink is rejected
when attached directly, and skipped when matched by a glob.

## Usage

The handler supports glob patterns for file inclusion and exclusion of files:

```sh
jp attachment add 'file:**/*.md'
```

Exclusions can be specified using the `exclude` flag:

```sh
jp attachment add 'file:path/to/file.txt?exclude'
```

`jp` has built-in support for short-hand file URIs:

```sh
jp attachment add path/to/file.txt
jp attachment add !**/*.md
```

Exclusions are applied *after* inclusions, so if a file is both included and
excluded, it will be excluded.
