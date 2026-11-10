# RFD 014: Attachment Handler Guide

- **Status**: Superseded
- **Category**: Guide
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-27
- **Superseded by**: [RFD 015]
- **Summary**: Documents the `Handler` trait, URL conventions, and registration
  for implementing context attachment handlers.

## Summary

This guide describes the JP attachment system for handler authors, covering URL
conventions, the `Handler` trait, and how handlers are registered and invoked.

## How Attachments Work

Attachments let users add context to a conversation: files, command output, web
pages, notes, or anything else a handler can fetch.
Each attachment is identified by a URL, and the URL's scheme selects the handler
that processes it.

Users attach content on the CLI with `-a` (`--attach` or `--attachment`):

```sh
jp -a "scheme:some-value"
jp -a "scheme://structured/url?with=params"
```

Attachments can also be declared in configuration files:

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

Hierarchical URLs are structured: they include a host, path segments, and query
parameters.
This form suits machine-generated URLs, config files, and inputs where
individual fields matter.

```
scheme://host/path?key=value&other=123
         ^^^^      ^^^^^^^^^^^^^^^^^
         host      query pairs
```

Handlers read these components through `Url::host_str()`, `Url::path()`, and
`Url::query_pairs()`.

### Opaque: `scheme:content`

Opaque URLs are human-friendly: everything after `scheme:` becomes a single
opaque path, with no host or structured query parsing.
The handler parses the path directly.

```
scheme:any content the handler understands
       ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
       path (opaque)
```

Opaque URLs have `Url::cannot_be_a_base() == true` and `Url::host_str() ==
None`.

### Supporting Both Forms

A handler receives a `&Url`, which may use either form.
Check `Url::cannot_be_a_base()` to distinguish opaque URLs from hierarchical
ones and parse accordingly:

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

Handlers that support only one form should return a clear error when given the
other.

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

- **`scheme()`**: Returns the URI scheme this handler owns (for example,
  `"file"` or `"http"`).
  Each handler's scheme must be unique.
- **`add(uri)`**: Stores an attachment reference when the user adds an
  attachment.
- **`remove(uri)`**: Removes a previously added reference.
- **`list()`**: Returns all stored attachment URLs for `jp attachment ls`.
  Implementations should return canonical (hierarchical) URLs for consistency.
- **`get(cwd)`**: Fetches and returns the attachment content by reading files,
  running commands, making HTTP requests, and so on.

Each handler is a stateful collection: `add` accumulates references, and `get`
resolves them at query time.

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

Annotate the `Handler` impl with `#[typetag::serde(name = "my_scheme")]` for
serialization, and import the crate in `jp_cli/src/cmd/attachment.rs` (even if
unused) so the linker includes it.

## The Attachment Struct

```rust
pub struct Attachment {
    pub source: String,
    pub description: Option<String>,
    pub content: String,
}
```

- **`source`**: Human-readable origin, such as a file path, command, or URL.
- **`description`**: Optional context describing the attachment.
- **`content`**: The resolved data, formatted as plain text, XML, JSON, or any
  other string representation the handler produces.

## Conventions

- **Accept both URL forms.** Users type opaque URLs on the CLI, while config
  files may use either form.
  Support both where practical.
- **Produce hierarchical URLs from `list()`.** The canonical form is easier to
  inspect and manipulate programmatically.
- **Fail clearly.** When a URL is malformed for your handler, return an error
  that explains what is wrong rather than a generic "invalid URI."
- **Use `cwd` for relative paths.** The `get` method receives the workspace
  root; resolve any relative references against it.

## References

- `jp_attachment` (`crates/jp_attachment/src/lib.rs`): Defines the `Handler`
  trait and `Attachment` struct.
- `jp_attachment_cmd_output` (`crates/jp_attachment_cmd_output/src/lib.rs`),
  `jp_attachment_file_content` (`crates/jp_attachment_file_content/src/lib.rs`),
  `jp_attachment_http_content` (`crates/jp_attachment_http_content/src/lib.rs`):
  Existing handler implementations.

[RFD 015]: 015-simplified-attachment-handler-trait.md
