# RFD 015: Simplified Attachment Handler Trait

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-27
- **Supersedes**: [RFD 014](014-attachment-handler-guide.md)

## Summary

This RFD simplifies the attachment handler system. The `Handler` trait is
reduced from five methods to three (`schemes`, `validate`, `resolve`), with URL
tracking moved to the host. Handlers become stateless, eliminating redundant
state management and serialization complexity.

## Motivation

The current attachment system has two problems:

1. **Handlers own state they don't need.** Each handler stores a collection of
   URLs, implements `add`/`remove`/`list` to manage that collection, and
   serializes it via `typetag`. But every handler's state is fully derivable
   from its URL list. The host already tracks attachment URLs in the
   conversation config — duplicating this in the handler adds complexity for no
   benefit.

   Handlers were originally stateful to support eager loading: the `file`
   handler read and cached file contents when attachments were added, rather
   than at query time. This kept file content in the system prompt stable even
   if files changed on disk, preventing LLM context cache invalidation. However,
   this meant the LLM operated on stale content and was more prone to making
   errors when editing files. The caching benefit doesn't justify the
   correctness cost.

   Eager loading may return as an opt-in feature, but it's not core to the
   handler model. Handlers can be stateless.

2. **Handlers are hardcoded into the binary.** Adding a new handler means
   writing a Rust crate, wiring it into the workspace, and recompiling. Users
   cannot add custom attachment types without forking the project.

This RFD addresses the first problem by simplifying the trait to stateless
`validate` + `resolve`. The second problem (extensibility via plugins) is
addressed in future work.

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

Handlers are stateful objects. `add` parses a URL into an internal
representation and stores it. `list` converts the internal representation back
to URLs. `get` resolves all stored references into attachments. Every handler's
internal state is fully reconstructible from its URL list, none store anything
beyond parsed URLs.

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

**`schemes`** returns multiple schemes, replacing the single `scheme` method.
This eliminates the `http`/`https` handler duplication - a single handler
returns `["http", "https"]`.

**`validate`** replaces `add`. The host calls it when a user adds an attachment.
The handler checks that the URL is parseable and (optionally) that the resource
exists. It receives `cwd` for handlers that need to check file existence or
resolve relative paths. The host stores the URL on success; the handler stores
nothing.

**`resolve`** replaces `get`. It receives all URLs for this handler at once,
which is important for handlers like `file` where includes and excludes
interact. The handler fetches content and returns `Vec<Attachment>`.

The host owns `add`/`remove`/`list` - these are now just URL collection
operations on a `Vec<Url>`, with no handler involvement beyond validation.

### Handler registration

With handlers now stateless and `typetag` removed, we no longer need
serialization or linker-based discovery. Built-in handlers move into
`jp_attachment::builtin` and are registered via an enum:

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

All built-in handlers are consolidated into `jp_attachment::builtin`. Individual
handler crates (e.g., `jp_attachment_cmd_output`) are removed. Handlers can be
enabled or disabled via cargo feature flags:

```toml
[features]
default = ["attachment-file", "attachment-http", "attachment-cmd", "attachment-mcp"]
attachment-file = []
attachment-http = []
attachment-cmd = []
attachment-mcp = []
```

The `all_handlers()` function conditionally includes handlers based on enabled
features:

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

Third-party handlers are out of scope for this RFD. A future change can
introduce extensibility for custom attachment types.

### What stays the same

- **URL-based dispatch.** Users still type `jp -a "scheme:..."`. The scheme
  determines which handler runs.
- **The `Attachment` struct.** Handlers still produce `Vec<Attachment>` with
  `source`, `description`, and `content` fields.
- **Both URL forms.** Handlers receive `Url` values that can be hierarchical
  (`scheme://host/path?query`) or opaque (`scheme:content`). See [RFD 014].
- **Config-file attachment syntax.** Both URL strings and the `type`/`path`/
  `params` object form continue to work.

## Drawbacks

- **Breaking change.** All existing handlers must be rewritten to the new trait.
  This is acceptable since handlers are currently internal-only.

## Alternatives

### Keep handlers stateful (current design)

The current `add`/`remove`/`list`/`get` trait with handler-owned state. This
works for native Rust handlers but creates problems for Wasm plugins: opaque
state blobs that need serialization, versioning, and persistence. Since every
handler's state is derivable from its URL list, the complexity is unnecessary.

### Use a different state management approach

Instead of moving URL tracking to the host, use a shared state management
pattern (e.g., a registry that handlers query). This still requires handlers to
implement state management methods, which adds complexity without clear benefit
given that all handler state is derivable from the URL list.

## Non-Goals

- **Third-party extensibility.** This RFD focuses on simplifying the handler
  interface. Plugin support for third-party handlers is future work.

  > [!TIP]
  > [RFD 016] introduces the plugin system. [RFD 017] defines the Wasm
  > attachment handler interface as the first capability built on top of it.

## Risks and Open Questions

1. **Validate semantics.** Should `validate` be purely syntactic (can I parse
   this URL?) or also check existence (does this file/command/endpoint exist)?
   Current handlers are inconsistent — `file` checks existence, `cmd` doesn't.
   The RFD proposes passing `cwd` to support existence checks, but doesn't
   mandate them.

## Implementation Plan

- Change `scheme() -> &str` to `schemes() -> &[&str]`.
- Replace `add`/`remove`/`list`/`get` with `validate`/`resolve`.
- Move URL tracking to the host (`jp_attachment` or `jp_cli`).
- Update all built-in handlers to the new trait.
- Merge `http` and `https` into a single handler returning `["http", "https"]`.
- Remove `typetag::serde` from the `Handler` trait (the host serializes URLs
  directly; handlers have no state to serialize).
- Create `jp_attachment::builtin` module with `BuiltinHandler` enum.
- Consolidate handler crates into `jp_attachment::builtin::*` submodules.
- Remove individual handler crates (`jp_attachment_cmd_output`,
  `jp_attachment_file_content`, `jp_attachment_http_content`,
  `jp_attachment_mcp_resource`).
- Add cargo feature flags for each built-in handler.
- Replace `linkme` registration with `builtin::all_handlers()`.
- Remove `use jp_attachment_* as _` imports from `jp_cli`.
- Update [RFD 014] to reflect the new trait and registration approach.
- Update tests.
- **Dependency:** None. Can merge independently.

## Future Work

- **Plugin support.** Allow third-party attachment handlers via plugins. This
  requires defining a plugin infrastructure with sandboxing, capability-based
  security, and host-mediated I/O.
- **Remove niche handlers.** The `bear` handler could be moved to an external
  plugin once plugin support is available.

## References

- [RFD 014: Attachment Handler Guide][RFD 014] — how handlers work today.

[RFD 014]: 014-attachment-handler-guide.md
[RFD 016]: 016-wasm-plugin-architecture.md
[RFD 017]: 017-wasm-attachment-handlers.md
