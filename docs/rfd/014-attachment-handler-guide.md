# RFD 014: Attachment Handler Guide

- **Status**: Superseded
- **Category**: Guide
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-27
- **Superseded by**: [RFD 015](015-simplified-attachment-handler-trait.md)

## Summary

This document describes how the JP attachment system works from a handler
author's perspective. It covers the `Handler` trait, URL conventions, and how
handlers are registered and invoked.

## How Attachments Work

Attachments let users provide additional context to a conversation — files,
command output, web pages, notes, or anything else a handler can fetch. Each
attachment is identified by a URL. The URL's scheme determines which handler
processes it.

Users attach content through the CLI:

```sh
jp -a "scheme:some-value"
jp -a "scheme://structured/url?with=params"
```

_(also works: `--attachment` or `--attach`)_

Or through configuration files:

```toml
# URL form
conversation.attachments = ["scheme://host/path?key=value"]

# Object form
[[conversation.attachments]]
type = "scheme"
path = "host/path"
params = { key = "value" }
```

## URL Forms

The `url` crate (WHATWG URL standard) recognizes two forms for non-special
schemes:

### Hierarchical: `scheme://authority/path?query`

The structured form. Has a host, path segments, and query parameters. Useful for
machine-generated URLs, config files, and cases where individual fields matter.

```
scheme://host/path?key=value&other=123
         ^^^^      ^^^^^^^^^^^^^^^^^
         host      query pairs
```

Handlers access these parts through `Url::host_str()`, `Url::path()`,
`Url::query_pairs()`, etc.

### Opaque: `scheme:content`

The human-friendly form. Everything after `scheme:` becomes the opaque path. No
host, no structured query parsing. The handler interprets the path however it
wants.

```
scheme:any content the handler understands
       ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
       path (opaque)
```

These URLs have `Url::cannot_be_a_base() == true` and `Url::host_str() == None`.

### Both forms are valid input

A handler receives a `&Url` and must handle whatever the user provides. In
practice this means checking whether the URL is hierarchical or opaque and
parsing accordingly:

```rust
fn parse(uri: &Url) -> Result<MyData, Box<dyn Error + Send + Sync>> {
    if uri.cannot_be_a_base() {
        // Opaque form: "myscheme:some human-friendly input"
        let input = uri.path();
        // ... parse `input` however makes sense for this handler
    } else {
        // Hierarchical form: "myscheme://host/path?key=value"
        let host = uri.host_str().ok_or("missing host")?;
        // ... parse structured fields
    }
}
```

Handlers that only support one form should return a clear error for the other.

## The Handler Trait

```rust
#[typetag::serde(tag = "type")]
#[async_trait]
pub trait Handler: Debug + DynClone + DynHash + Send + Sync {
    fn scheme(&self) -> &'static str;

    async fn add(&mut self, uri: &Url) -> Result<(), BoxError>;
    async fn remove(&mut self, uri: &Url) -> Result<(), BoxError>;
    async fn list(&self) -> Result<Vec<Url>, BoxError>;
    async fn get(&self, cwd: &Utf8Path, mcp: Client) -> Result<Vec<Attachment>, BoxError>;
}
```

- **`scheme()`** — returns the URI scheme this handler owns (e.g. `"file"`,
  `"http"`). Must be unique across all handlers.
- **`add(uri)`** — stores the attachment reference. Called when the user adds an
  attachment.
- **`remove(uri)`** — removes a previously added reference.
- **`list()`** — returns all stored attachment URLs. Used by `jp attachment ls`.
  Should produce canonical (hierarchical) URLs for consistency.
- **`get(cwd, mcp)`** — fetches and returns the actual attachment content. This
  is where the handler does its real work: reading files, running commands,
  making HTTP requests, etc.

Each handler is a stateful collection. `add` accumulates references, `get`
resolves them all at query time.

## Registration

Handlers register themselves at link time using `linkme::distributed_slice`:

```rust
use jp_attachment::{BoxedHandler, Handler, HANDLERS, distributed_slice, linkme};

#[distributed_slice(HANDLERS)]
#[linkme(crate = linkme)]
static HANDLER: fn() -> BoxedHandler = || {
    (Box::new(MyHandler::default()) as Box<dyn Handler>).into()
};
```

The handler must also be annotated with `#[typetag::serde(name = "my_scheme")]`
on its `Handler` impl for serialization. The crate must be imported (even if
unused) in `jp_cli/src/cmd/attachment.rs` so the linker includes it.

## The Attachment Struct

```rust
pub struct Attachment {
    pub source: String,
    pub description: Option<String>,
    pub content: String,
}
```

- **`source`** — human-readable origin (a file path, a command, a URL).
- **`description`** — optional context about what this attachment is.
- **`content`** — the actual data. Can be plain text, XML, JSON, or any string
  representation the handler produces.

## Conventions

- **Accept both URL forms.** Users type opaque URLs on the CLI; config files may
  use either form. Handlers should handle both where practical.
- **Produce hierarchical URLs from `list()`.** The canonical form is easier to
  inspect and manipulate programmatically.
- **Fail clearly.** If a URL is malformed for your handler, return an error that
  says what's wrong, not a generic "invalid URI."
- **Use `cwd` for relative paths.** The `get` method receives the workspace root
  — resolve any relative references against it.

## References

- `jp_attachment` (`crates/jp_attachment/src/lib.rs`) — the `Handler` trait
  and `Attachment` struct.
- `jp_attachment_cmd_output` (`crates/jp_attachment_cmd_output/src/lib.rs`),
  `jp_attachment_file_content` (`crates/jp_attachment_file_content/src/lib.rs`),
  `jp_attachment_http_content` (`crates/jp_attachment_http_content/src/lib.rs`)
  — existing handler implementations.
