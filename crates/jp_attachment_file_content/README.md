# Attachment: File Content

This crate provides an attachment handler for the full content of a file.

## File URI Format

Write a file URI as `file:<path>`, with `path` relative to the workspace root.
These URIs are equivalent:

- `file:path/to/file.txt`
- `file:/path/to/file.txt`
- `file:///path/to/file.txt`

Do not write `file://path/to/file.txt`. There, `path` is the URL host, which
URL parsing lowercases and IDNA-encodes before the handler sees it, so
`file://Docs/notes.md` reads as `docs/notes.md`. The handler still reads the
host as the first path segment, so existing URIs keep resolving, but logs a
warning naming the `file:<path>` spelling.

As a config table, `path` is taken exactly as written:

```toml
[[conversation.attachments]]
type = "file"
path = "Docs/Notes.md"
```

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
