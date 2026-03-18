# RFD 017: Wasm Attachment Handlers

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-28

## Summary

This RFD adds Wasm plugin support for attachment handlers. Third-party handlers
are loaded as Wasm components at runtime, exposing the same
`jp:plugin/attachment` interface. The host wraps them in a `WasmHandler` adapter
that implements the native `Handler` trait from [RFD 015], making the
plugin/native distinction transparent to the rest of the system.

## Motivation

[RFD 015] simplified the `Handler` trait to three stateless methods, but all
handlers are still hardcoded into the binary. Adding a new attachment type means
writing a Rust crate, wiring it into the workspace, and recompiling. Users
cannot add custom attachment types without forking the project.

This RFD leverages the plugin infrastructure from [RFD 016] to support
third-party attachment handlers. Handlers become just another capability
interface that plugins can export.

## Design

### Built-in vs external handlers

Built-in handlers implement the `Handler` trait directly in Rust (as defined in
[RFD 015]). External handlers are Wasm plugins that export the
`jp:plugin/attachment` capability interface. The host wraps external handlers in
a `WasmHandler` adapter that implements the same `Handler` trait, so the rest of
the system is unaware of the distinction.

### Attachment WIT interface

The `attachment` interface is a capability interface in the `jp:plugin` package.
See [RFD 016] for the plugin model, host imports, and capability discovery
mechanism.

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

The interface mirrors the simplified `Handler` trait from [RFD 015]. URLs are
passed as strings; the guest parses them using whatever URL library its language
provides.

A convenience world for attachment-only plugins:

```wit
world attachment-plugin {
    import jp:host/process;
    import jp:host/http;
    import jp:host/filesystem;

    export jp:plugin/plugin;
    export jp:plugin/attachment;
}
```

Plugins that provide additional capabilities (e.g., tools) compose their own
world. See [RFD 016].

### `WasmHandler` adapter

For plugins that export the `attachment` interface, the host wraps the component
in a `WasmHandler` adapter that implements the native `Handler` trait:

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

No state, no `typetag`. The `WasmHandler` is a stateless bridge between the host
and the Wasm component.

If a plugin exports the `attachment` interface but `schemes()` returns an empty
list, the host emits a warning and skips registering it as an attachment
handler.

If two plugins claim the same scheme, the host errors at startup with a clear
message identifying both plugins.

### Removing `bear` from the binary

The `bear` handler is too niche for the core binary. It becomes an external Wasm
plugin:

1. Create `crates/jp_attachment_handler_bear/` targeting `wasm32-wasip2`.
2. Implement the `jp:plugin/plugin` and `jp:plugin/attachment` interfaces.
3. Port the SQLite reading logic (using `jp:host/filesystem` to access the
   database file, or bundling a Wasm-compatible SQLite build).
4. Publish the `.wasm` binary as a release artifact.
5. Remove `jp_attachment_bear_note` from the workspace.
6. Remove the `use jp_attachment_bear_note as _` import from
   `jp_cli/src/cmd/attachment.rs`.
7. Document the migration for existing users.

## Drawbacks

- **Two handler models.** Native Rust handlers and Wasm handlers coexist. This
  is intentional (native for core handlers, Wasm for extensibility) but adds
  maintenance surface.
- **Wasm call overhead.** External handlers pay Wasm instantiation and call
  overhead that native handlers avoid. For attachment resolution (which
  typically involves I/O), this overhead is negligible.

## Alternatives

### Port all handlers to Wasm (no native handlers)

Simpler mental model: everything is Wasm. With host imports for process, HTTP,
and filesystem, this is technically feasible. But it means:

- The `file` handler loses parallel directory walking and `.ignore` integration.
- The `mcp` handler loses access to the MCP client transport.
- Every handler invocation pays Wasm call overhead for no user benefit.
- Built-in handler development becomes harder.

### Use a different plugin architecture

Instead of the capability-based model from [RFD 016], use a simpler approach
(e.g., dynamic libraries, embedded scripting languages). This trades off
sandboxing and cross-platform compatibility for reduced binary size and
complexity.

## Non-Goals

- **Porting built-in handlers to Wasm.** The `cmd`, `file`, `http`/`https`, and
  `mcp` handlers stay as native Rust for performance and functionality.

## Risks and Open Questions

1. **Bear handler SQLite in Wasm.** The Wasm guest needs to read a SQLite
   database. Options: bundle a SQLite build compiled to Wasm, or read the raw
   file through `jp:host/filesystem` and parse it in the guest. Needs
   prototyping.

2. **Host import surface.** Each additional capability granted to attachment
   handlers (e.g., `jp:host/mcp` for accessing MCP servers) expands the attack
   surface. We should start minimal and add capabilities only when clear use
   cases emerge.

## Implementation Plan

- Define WIT for `jp:plugin/attachment` interface in `jp_wasm`.
- Implement `WasmHandler` adapter in `jp_wasm` that bridges the `attachment`
  interface to the native `Handler` trait.
- Wire discovered attachment plugins into `jp_attachment`'s handler registry at
  startup.
- Integration test: load a test attachment plugin, call `validate`/`resolve`,
  verify output.
- **Dependency:** [RFD 015] (simplified `Handler` trait), [RFD 016] phases 1-2
  (plugin infrastructure and capability discovery).

## Future Work

- **Port `bear` handler.** Create the external Wasm plugin and publish it as a
  release artifact (implementation plan step 1-7 above).
- **Example attachment handler.** Create a minimal example (e.g., a "note"
  handler that reads from a notes directory) as a reference implementation and
  test case.
- **Plugin author guide.** Document how to write attachment handler plugins,
  including WIT usage, URL parsing conventions, and testing strategies.

## References

- [RFD 014] — URL conventions and handler authoring guide (needs updating for
  the simplified trait).
- [RFD 015] — the native `Handler` trait that `WasmHandler` implements.
- [RFD 016] — plugin infrastructure, host imports, sandbox configuration,
  capability discovery.

[RFD 014]: 014-attachment-handler-guide.md
[RFD 015]: 015-simplified-attachment-handler-trait.md
[RFD 016]: 016-wasm-plugin-architecture.md
