# RFD 017: Wasm Attachment Handlers

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-28
- **Requires**: [RFD 015], [RFD 016]
- **Summary**: Loads third-party attachment handlers from Wasm plugins and
  adapts `jp:plugin/attachment` exports to the native `Handler` trait.

## Summary

This RFD adds Wasm plugin support for attachment handlers.
The host loads third-party handlers at runtime as Wasm components exporting the
`jp:plugin/attachment` WIT interface and wraps each component in a `WasmHandler`
adapter that implements the native `Handler` trait from [RFD 015].

## Motivation

[RFD 015] simplified the `Handler` trait to three stateless methods, but all
handlers remain hardcoded into the binary.
Adding an attachment type requires writing a Rust crate, wiring it into the
workspace, and recompiling, so users cannot add custom attachment types without
forking the project.

This RFD builds on the plugin infrastructure from [RFD 016] to expose attachment
handling as a capability interface that third-party plugins can export.

## Design

### Built-in vs external handlers

Built-in handlers implement the `Handler` trait directly in Rust ([RFD 015]),
while external handlers are Wasm plugins that export the `jp:plugin/attachment`
capability interface.
Because the host wraps external handlers in a `WasmHandler` adapter implementing
`Handler`, the rest of the attachment pipeline treats both uniformly.

### Attachment WIT interface

The `jp:plugin` package defines the `attachment` capability interface, which
mirrors the `Handler` trait from [RFD 015] (see [RFD 016] for the plugin model,
host imports, and capability discovery):

```wit
package jp:plugin@0.1.0;

interface attachment {
    use types.{error};

    record attachment {
        source: string,
        description: option<string>,
        content: string,
    }

    /// The URI schemes this handler owns (e.g. ["jira"]).
    schemes: func() -> list<string>;

    /// Validate that a URL is well-formed for this handler.
    validate: func(uri: string, cwd: string) -> result<_, error>;

    /// Resolve URLs into attachments.
    resolve: func(
        uris: list<string>,
        cwd: string,
    ) -> result<list<attachment>, error>;
}
```

URLs cross the boundary as strings so the guest can parse them with its
language's URL library.

For attachment-only plugins, JP provides the `attachment-plugin` convenience
world; plugins that export additional capabilities (such as tools) compose their
own world:

```wit
world attachment-plugin {
    import jp:host/process;
    import jp:host/http;
    import jp:host/filesystem;

    export jp:plugin/plugin;
    export jp:plugin/attachment;
}
```

### `WasmHandler` adapter

`WasmHandler` is a stateless bridge between the native `Handler` trait and a
Wasm component exporting `jp:plugin/attachment`, requiring neither handler state
nor `typetag` serialization:

```rust
pub struct WasmHandler {
    name: String,
    schemes: Vec<String>,
    component: Component,
    sandbox: SandboxConfig,
}

#[async_trait]
impl Handler for WasmHandler {
    fn schemes(&self) -> &[&str] {
        self.schemes.iter().map(String::as_str).collect()
    }

    async fn validate(&self, uri: &Url, cwd: &Utf8Path) -> Result<()> {
        call_guest_validate(&self.component, uri.as_str(), cwd.as_str())
    }

    async fn resolve(
        &self,
        uris: &[Url],
        cwd: &Utf8Path,
        _mcp: Client,
    ) -> Result<Vec<Attachment>> {
        let uri_strings: Vec<&str> = uris.iter().map(Url::as_str).collect();
        call_guest_resolve(&self.component, &uri_strings, cwd.as_str())
    }
}
```

If a plugin exports the `attachment` interface but `schemes()` returns an empty
list, the host emits a warning and skips registering it as an attachment
handler.
If two plugins claim the same scheme, the host fails at startup with an error
identifying both plugins.

### Removing `bear` from the binary

The `bear` handler is too niche for the core binary and moves to an external
Wasm plugin:

1. Create `crates/jp_attachment_handler_bear/` targeting `wasm32-wasip2`.
2. Implement the `jp:plugin/plugin` and `jp:plugin/attachment` interfaces.
3. Port the SQLite reading logic, either by reading the database file through
   `jp:host/filesystem` or by bundling a Wasm-compatible SQLite build.
4. Publish the `.wasm` binary as a release artifact.
5. Remove `jp_attachment_bear_note` from the workspace.
6. Remove the `use jp_attachment_bear_note as _` import from
   `jp_cli/src/cmd/attachment.rs`.
7. Document the migration for existing users.

## Drawbacks

- **Two handler models.** Maintaining both native Rust handlers (for core
  schemes) and Wasm handlers (for third-party extensions) increases the
  maintenance surface.
- **Wasm call overhead.** External handlers incur Wasm instantiation and call
  overhead that native handlers avoid, though this cost is negligible alongside
  the I/O of attachment resolution.

## Alternatives

### Port all handlers to Wasm

Running every handler as a Wasm plugin would leave a single handler model, and
the host imports for processes, HTTP, and the filesystem make this technically
feasible.
However, porting the built-in handlers has significant costs:

- The `file` handler loses parallel directory walking and `.ignore` integration.
- The `mcp` handler loses access to the MCP client transport.
- Every handler invocation pays Wasm call overhead for no user benefit.
- Built-in handler development becomes harder.

### Use a different plugin architecture

Simpler plugin mechanisms, such as dynamic libraries or an embedded scripting
language, would reduce binary size and runtime complexity compared to the
capability-based Wasm model from [RFD 016], at the cost of sandboxing and
cross-platform compatibility.

## Non-Goals

- **Porting built-in handlers to Wasm.** The `cmd`, `file`, `http`/`https`, and
  `mcp` handlers remain native Rust implementations for performance and
  functionality.

## Risks and Open Questions

1. **SQLite access in the `bear` plugin.** The `bear` Wasm guest must read a
   SQLite database, either by bundling a Wasm-compiled SQLite build or by
   reading the raw database file through `jp:host/filesystem` and parsing it in
   the guest.
   Both approaches require prototyping.

2. **Host import surface.** Each additional capability granted to attachment
   handlers (such as `jp:host/mcp` for accessing MCP servers) expands the attack
   surface.
   The host should start with a minimal import set and add capabilities only
   when clear use cases emerge.

## Implementation Plan

- Define the `jp:plugin/attachment` WIT interface in `jp_wasm`.
- Implement the `WasmHandler` adapter in `jp_wasm` to bridge the `attachment`
  interface to the native `Handler` trait.
- Wire discovered attachment plugins into `jp_attachment`'s handler registry at
  startup.
- Add an integration test that loads a test attachment plugin, calls `validate`
  and `resolve`, and verifies the output.
- **Dependencies:** [RFD 015] (simplified `Handler` trait) and [RFD 016] phases
  1-2 (plugin infrastructure and capability discovery).

## Future Work

- **Port the `bear` handler.** Extract `jp_attachment_bear_note` into the
  external Wasm plugin and publish it as a release artifact, following steps 1-7
  above.
- **Example attachment handler.** Create a minimal example plugin (such as a
  "note" handler that reads from a notes directory) as a reference
  implementation and test case.
- **Plugin author guide.** Document how to write attachment handler plugins,
  including WIT usage, URL parsing conventions, and testing strategies.

## References

- [RFD 014]: URL conventions and handler authoring guide (needs updating for the
  simplified trait).
- [RFD 015]: The native `Handler` trait that `WasmHandler` implements.
- [RFD 016]: Plugin infrastructure, host imports, sandbox configuration, and
  capability discovery.

[RFD 014]: 014-attachment-handler-guide.md
[RFD 015]: 015-simplified-attachment-handler-trait.md
[RFD 016]: 016-wasm-plugin-architecture.md
