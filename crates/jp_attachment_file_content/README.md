# Attachment: File Content

This crate provides an attachment handler for the full content of a file.

## File URI Format

A file URI has the format

```sh
file://host/path
```

However, this attachment handler enforces the following restrictions:

- `host` is always ignored<sup>*</sup> (and can be omitted)
- `path` is always relative to the workspace root

This means that the following URIs are equivalent:

- `file:path/to/file.txt`
- `file:/path/to/file.txt`
- `file://path/to/file.txt`
- `file:///path/to/file.txt`

<sup>*Technically, for `file://path/to/file.txt`, the host is `path`, but
this handler considers this part of the final path, so it is not ignored, but
prepended to the path.</sup>

## Usage

The handler supports glob patterns for file inclusion and exclusion of files:

```sh
jp attachment add file://**/*.md
```

Exclusions can be specified using the `exclude` flag:

```sh
jp attachment add file://path/to/file.txt?exclude
```

`jp` has built-in support for short-hand file URIs:

```sh
jp attachment add path/to/file.txt
jp attachment add !**/*.md
```

Exclusions are applied *after* inclusions, so if a file is both included and
excluded, it will be excluded.
