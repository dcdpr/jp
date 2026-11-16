# RFD 015: Simplified Attachment Handler Trait

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-27
- **Supersedes**: [RFD 014]
- **Required by**: [RFD 017]
- **Summary**: Simplifies attachment `Handler` implementations to three
  stateless methods (`schemes`, `validate`, `resolve`) and moves URL tracking to
  the host.

## Summary

This RFD reduces the attachment `Handler` trait from five stateful methods to
three stateless ones (`schemes`, `validate`, `resolve`) and moves URL tracking
to the host.
Handlers no longer store or serialize their own URL collections.

## Motivation

The current attachment system has two problems:

1. **Handlers own state they don't need.** Each handler stores a collection of
   URLs, implements `add`, `remove`, and `list` to manage it, and serializes it
   via `typetag`.
   Because every handler's state is derivable from its URL list, and the host
   already tracks attachment URLs in the conversation config, duplicating that
   list in the handler adds complexity for no benefit.

   Handlers were originally stateful to support eager loading: the `file`
   handler read and cached file contents when attachments were added rather than
   at query time.
   Keeping file content stable in the system prompt prevented LLM context cache
   invalidation when files changed on disk, but left the LLM operating on stale
   content and prone to editing errors.
   The caching benefit does not justify the correctness cost.
   Eager loading may return as an opt-in feature, but it does not belong in the
   core handler model.

2. **Handlers are hardcoded into the binary.** Adding a new handler requires
   writing a Rust crate, wiring it into the workspace, and recompiling.
   Users cannot add custom attachment types without forking the project.

This RFD addresses the first problem by reducing `Handler` to stateless
`validate` and `resolve` methods.
Plugin extensibility is left to future work.

## Design

### Simplified `Handler` trait

The current trait has five methods:

```rust
trait Handler {
    fn scheme(&self) -> &'static str;
    async fn add(&mut self, uri: &Url) -> Result<()>;
    async fn remove(&mut self, uri: &Url) -> Result<()>;
    async fn list(&self) -> Result<Vec<Url>>;
    async fn get(&self, cwd: &Utf8Path, mcp: Client) -> Result<Vec<Attachment>>;
}
```

`add` parses and stores a URL, `list` converts the stored state back into URLs,
and `get` resolves the stored references into attachments.
No handler stores anything beyond parsed URLs.

The new trait moves URL tracking to the host and makes handlers stateless:

```rust
trait Handler {
    /// The URI schemes this handler owns (e.g. ["http", "https"]).
    fn schemes(&self) -> &[&str];

    /// Validate that a URL is well-formed for this handler.
    ///
    /// Called when the user adds an attachment. Returns an error if
    /// the URL is malformed or the referenced resource doesn't exist.
    async fn validate(&self, uri: &Url, cwd: &Utf8Path) -> Result<()>;

    /// Resolve a batch of URLs into attachments.
    ///
    /// Called at query time with all URLs for this handler's scheme(s).
    /// The handler fetches content and returns the resolved attachments.
    async fn resolve(
        &self,
        uris: &[Url],
        cwd: &Utf8Path,
        mcp: Client,
    ) -> Result<Vec<Attachment>>;
}
```

**`schemes`** replaces `scheme` and returns all schemes the handler owns, so a
single handler can serve `["http", "https"]` without duplication.

**`validate`** replaces `add`.
When a user adds an attachment, the host calls `validate` to check that the URL
is parseable and, optionally, that the target resource exists (using `cwd` to
resolve relative paths).
On success, the host stores the URL; the handler stores nothing.

**`resolve`** replaces `get`, fetching content for all of the handler's URLs at
once and returning `Vec<Attachment>`.
Batching all URLs in a single call lets handlers like `file` evaluate
interacting include and exclude rules together.

The host handles `add`, `remove`, and `list` directly as operations on a
`Vec<Url>`, calling the handler only to validate new URLs.

### Handler registration

Stateless handlers do not need `typetag` serialization or linker-based
discovery.
All built-in handlers move from separate crates (such as
`jp_attachment_cmd_output`) into `jp_attachment::builtin` and dispatch through
the `BuiltinHandler` enum:

```rust
// jp_attachment/src/builtin/mod.rs
pub mod cmd;
pub mod file;
pub mod http;
pub mod mcp;

pub enum BuiltinHandler {
    Cmd(cmd::CmdHandler),
    File(file::FileHandler),
    Http(http::HttpHandler),
    Mcp(mcp::McpHandler),
}

impl Handler for BuiltinHandler {
    fn schemes(&self) -> &[&str] {
        match self {
            Self::Cmd(h) => h.schemes(),
            Self::File(h) => h.schemes(),
            Self::Http(h) => h.schemes(),
            Self::Mcp(h) => h.schemes(),
        }
    }

    async fn validate(&self, uri: &Url, cwd: &Utf8Path) -> Result<()> {
        match self {
            Self::Cmd(h) => h.validate(uri, cwd).await,
            Self::File(h) => h.validate(uri, cwd).await,
            Self::Http(h) => h.validate(uri, cwd).await,
            Self::Mcp(h) => h.validate(uri, cwd).await,
        }
    }

    async fn resolve(
        &self,
        uris: &[Url],
        cwd: &Utf8Path,
        mcp: Client,
    ) -> Result<Vec<Attachment>> {
        match self {
            Self::Cmd(h) => h.resolve(uris, cwd, mcp).await,
            Self::File(h) => h.resolve(uris, cwd, mcp).await,
            Self::Http(h) => h.resolve(uris, cwd, mcp).await,
            Self::Mcp(h) => h.resolve(uris, cwd, mcp).await,
        }
    }
}

pub fn all_handlers() -> Vec<BuiltinHandler> {
    vec![
        BuiltinHandler::Cmd(cmd::CmdHandler::default()),
        BuiltinHandler::File(file::FileHandler::default()),
        BuiltinHandler::Http(http::HttpHandler::default()),
        BuiltinHandler::Mcp(mcp::McpHandler::default()),
    ]
}
```

Cargo feature flags control which built-in handlers are compiled:

```toml
[features]
default = ["attachment-file", "attachment-http", "attachment-cmd", "attachment-mcp"]
attachment-file = []
attachment-http = []
attachment-cmd = []
attachment-mcp = []
```

`all_handlers()` registers only the enabled handlers:

```rust
pub fn all_handlers() -> Vec<BuiltinHandler> {
    vec![
        #[cfg(feature = "attachment-cmd")]
        BuiltinHandler::Cmd(cmd::CmdHandler::default()),
        #[cfg(feature = "attachment-file")]
        BuiltinHandler::File(file::FileHandler::default()),
        #[cfg(feature = "attachment-http")]
        BuiltinHandler::Http(http::HttpHandler::default()),
        #[cfg(feature = "attachment-mcp")]
        BuiltinHandler::Mcp(mcp::McpHandler::default()),
    ]
}
```

Third-party handler extensibility is out of scope for this RFD.

### What stays the same

- **URL-based dispatch.** Users still pass `jp -a "scheme:..."`, and the scheme
  selects the handler.
- **The `Attachment` struct.** Handlers still produce `Vec<Attachment>` with
  `source`, `description`, and `content` fields.
- **Both URL forms.** Handlers receive `Url` values that can be hierarchical
  (`scheme://host/path?query`) or opaque (`scheme:content`), as defined in [RFD
  014].
- **Config-file attachment syntax.** Both URL strings and the
  `type`/`path`/`params` object form continue to work.

## Drawbacks

- **Breaking change.** Every existing handler must be rewritten for the new
  trait, which is acceptable because handlers are currently internal-only.

## Alternatives

### Keep handlers stateful (current design)

Retaining the `add`/`remove`/`list`/`get` trait with handler-owned state works
for native Rust handlers, but complicates Wasm plugins by requiring opaque state
blobs to be serialized, versioned, and persisted.
Because every handler's state is derivable from its URL list, that complexity is
unnecessary.

### Shared state registry

Instead of moving URL tracking to the host, handlers could query a shared state
registry.
Handlers would still need to implement state management methods, adding
complexity when all handler state is already derivable from the URL list.

## Non-Goals

- **Third-party extensibility.** This RFD only simplifies the handler interface;
  plugin support for third-party handlers is left to future work.

  > [!TIP]
  > [RFD 016] introduces the plugin system.
  > [RFD 017] defines the Wasm attachment handler interface as the first
  > capability built on top of it.

## Risks and Open Questions

1. **`validate` semantics.** Should `validate` check only URL syntax, or also
   verify that the target file, command, or endpoint exists?
   Current handlers are inconsistent: `file` checks existence, while `cmd` does
   not.
   Passing `cwd` to `validate` supports existence checks without mandating them.

## Implementation Plan

- Change `scheme() -> &str` to `schemes() -> &[&str]`.
- Replace `add`/`remove`/`list`/`get` with `validate`/`resolve`.
- Move URL tracking to the host (`jp_attachment` or `jp_cli`).
- Update all built-in handlers to the new trait.
- Merge `http` and `https` into a single handler returning `["http", "https"]`.
- Remove `typetag::serde` from the `Handler` trait (the host serializes URLs
  directly; handlers have no state to serialize).
- Create the `jp_attachment::builtin` module with the `BuiltinHandler` enum.
- Consolidate handler crates into `jp_attachment::builtin::*` submodules.
- Remove individual handler crates (`jp_attachment_cmd_output`,
  `jp_attachment_file_content`, `jp_attachment_http_content`,
  `jp_attachment_mcp_resource`).
- Add Cargo feature flags for each built-in handler.
- Replace `linkme` registration with `builtin::all_handlers()`.
- Remove `use jp_attachment_* as _` imports from `jp_cli`.
- Update [RFD 014] to reflect the new trait and registration approach.
- Update tests.
- **Dependency:** None; can merge independently.

## Future Work

- **Plugin support.** Support third-party attachment handlers via a plugin
  system with sandboxing, capability-based security, and host-mediated I/O.
- **Extract niche handlers.** Move the `bear` handler to an external plugin once
  plugin support is available.

## References

- [RFD 014: Attachment Handler Guide][RFD 014]: how handlers work prior to this
  RFD.

[RFD 014]: 014-attachment-handler-guide.md
[RFD 016]: 016-wasm-plugin-architecture.md
[RFD 017]: 017-wasm-attachment-handlers.md
