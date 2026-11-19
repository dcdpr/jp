# RFD 139: Browser UIs in Rust with Leptos

- **Status**: Discussion
- **Category**: Decision
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-11-18
- **Required by**: [RFD 140]
- **Summary**: Adopts client-rendered Rust browser UIs with Leptos, shared API
  types, and bundles embedded in command plugins.

## Summary

JP's browser UIs are written in Rust, compiled to WebAssembly, and rendered in
the browser with Leptos.
A plugin embeds its UI in its binary and serves it beside the JSON API the UI
calls, and the repository gains no JavaScript for it.

## Context

JP serves three things to a browser:

- `jp-vite`'s documentation site, a VitePress app in Vue whose site files are
  synced into each workspace that uses it.
- `jp serve web`, which renders pages on the server with maud and runs
  hand-written scripts that `just typecheck-js` checks.
  It is a prototype, due to be rewritten.
- `jp review serve`, a JSON API whose browser frontend [RFD 136] left out of
  scope.

`jp-vite` stays as it is; beyond it, the project takes on no more JavaScript.

A review frontend needs code running in the browser that server rendering can't
replace:

- It maps a text selection to the source bytes it covers, through the spans
  md-html writes ([RFD 124]).
- It highlights commented ranges without rewriting the rendered HTML, through
  the CSS Custom Highlight API.
- It updates comments in place while the assistant answers them.

The Rust side already has what a WebAssembly client needs: `wasm-bindgen`,
`js-sys`, and `web-sys` are in `Cargo.lock` as transitive dependencies, and the
types a client reads from the server are Rust structs.

## Decision

### Stack

Browser UIs are Rust compiled for `wasm32-unknown-unknown` and rendered in the
browser with Leptos, pinned to `0.9.0-rc1` until 0.9.0 is released.

Rendering is client-side only.
The plugin serves a static HTML shell, the compiled bundle, and the JSON API the
bundle calls.
There is no server-side rendering or hydration, and the server doesn't depend on
Leptos.

Views use Leptos's builder API rather than the `view!` macro, as Leptos's own
[`counter_without_macros`][counter] example does:

```rust
div().child((
    button()
        .on(ev::click, move |_| count.update(|n| *n += 1))
        .child("+1"),
    span().child(move || count.get()),
))
```

rustfmt formats it, rust-analyzer completes it, and clippy reads it as ordinary
Rust.

The repository holds no JavaScript for these UIs.
The browser runs the module `wasm-bindgen` generates at build time, which the
HTML shell starts with a one-line module script.
Browser APIs that `web-sys` exposes only behind `web_sys_unstable_apis`, such as
the Custom Highlight API, get small `#[wasm_bindgen]` extern declarations in
Rust.

### Crates

A plugin with a browser UI has two crates beside its own, both workspace
members:

| Crate                             | Builds for      | Holds                                                             |
| --------------------------------- | --------------- | ----------------------------------------------------------------- |
| `<plugin>/api`, `jp-<plugin>-api` | host and wasm32 | The JSON API's request and response types, with serde and no I/O. |
| `<plugin>/ui`, `jp-<plugin>-ui`   | host and wasm32 | The browser UI.                                                   |

The server serializes the `api` types and the UI deserializes the same ones, so
the two can't drift apart.
A change that breaks the UI's use of a type, such as a renamed field or a
variant an exhaustive `match` doesn't handle, fails to compile.
A new field the UI should show, or a field whose meaning changes, compiles
either way: the `api` crate's tests pin the JSON each type reads and writes, and
the server's endpoint tests check what the values mean, such as the version an
anchor defaults to.

The `ui` crate keeps its logic in plain functions over owned data: turning a
selection into a byte range, laying out comment cards, deciding when to poll.
Components and DOM bindings stay thin.
The crate builds on the host as well, so `cargo check`, clippy, rustqual,
rust-analyzer, and `cargo test` cover it with the rest of the workspace.

### Testing

- Logic is unit-tested on the host, in `_tests.rs` files, like the rest of the
  workspace.
- Code that needs a DOM is tested with `wasm-bindgen-test`, configured with
  `run_in_browser`: `wasm-bindgen-test-runner` mounts components in a headless
  browser, and the test asserts on the resulting DOM.

### Building and embedding

- `rust-toolchain.toml` lists the `wasm32-unknown-unknown` target.
- `just build-ui <plugin>` builds the `ui` crate for wasm32 under a
  `wasm-release` profile tuned for size, runs `wasm-bindgen --target web` into
  `target/ui/<plugin>/`, and copies the UI's stylesheet beside it.
  The recipe installs the `wasm-bindgen` CLI at the version `Cargo.lock` pins
  for the `wasm-bindgen` crate, since the CLI refuses a module built against
  another version.
- The plugin's build script embeds the bundle when `JP_<PLUGIN>_UI` names its
  directory (`JP_REVIEW_UI`), and otherwise embeds a placeholder page that says
  how to build the UI.
  A `JP_<PLUGIN>_UI` naming a directory without a bundle fails the build rather
  than falling back to the placeholder.
  The build script runs no cargo.
- `just install-plugin <plugin>` builds the bundle before installing a plugin
  that has a `ui` crate.
  A plain `cargo install` gives the API and the placeholder.
- A plugin installed from a checkout or from Git may serve the placeholder.
  A published plugin ships the bundle built from its own source revision: its
  release build sets `JP_<PLUGIN>_UI`, and fails without the bundle.
  `just plugin-build`, which builds the published binaries, learns to build and
  name the bundle in the change that gives the first published plugin a `ui`
  crate.

### Tooling for the assistant

The cargo tools already run optional passes behind tool options (`wiki_check`,
`qual_check`, `site_check`).
Two more:

- `cargo_check`'s `wasm_check` also runs clippy for `wasm32-unknown-unknown` on
  every browser crate.
- `cargo_test`'s `wasm_test` also runs every browser crate's browser tests.

A browser crate says what it is in its manifest, which the tools, `just`, and CI
read through `cargo metadata`:

```toml
[package.metadata.jp]
wasm = true
```

A `web-development` skill turns both options on and carries the conventions in
this RFD.
A `web-dev` persona loads it on top of `dev`, as `app-dev` adds the macOS app's
tools; other personas build nothing for wasm32.

### CI

A path-filtered `web` workflow, like `app.yml`, runs when a browser crate, an
`api` crate, md-html, `Cargo.lock`, or `rust-toolchain.toml` changes.
It lints the browser crates for wasm32, runs their browser tests in the runner's
Chrome through ChromeDriver, and builds every bundle.

## Consequences

Easier:

- One language and toolchain from the store to the page.
- The compiler checks the client's use of the server's wire types.
- UI logic is tested like the rest of the code, with the same tools.
- The `jp serve web` rewrite starts from a decided stack, without JavaScript.

Harder:

- A build step outside a plain `cargo build`, a pinned `wasm-bindgen` CLI, and a
  second target.
- A plugin built without its bundle serves the placeholder, so `cargo install`
  alone doesn't produce the UI.
- Leptos and its dependencies enter cargo-vet and cargo-deny, and compile in
  every host build of the workspace.
- A pre-release dependency, whose API can change before 0.9.0.
- A change that breaks only the wasm32 build passes other personas' checks; the
  `web-dev` persona or CI catches it, as with the macOS app.
- A WebAssembly bundle is larger than equivalent hand-written scripts, which
  matters little for a server on the user's own machine.

## Alternatives

- **Server-rendered pages with small scripts**, as `jp serve web` has.
  Selection mapping and highlights still need client code, so this means
  JavaScript or a WebAssembly component anyway, and the server grows an
  HTML-shaped surface beside its JSON API.
- **Server-rendered pages with WebAssembly islands** for selection: two
  rendering models in one UI, and full reloads to show updates.
- **Vue in `jp-vite`**: more JavaScript, and a plugin's UI tied to the
  documentation site's sync and release cycle.
- **Other Rust frameworks**: Dioxus builds and bundles through its own CLI
  (`dx`); Yew renders through a virtual DOM and is less active; Sycamore has
  fine-grained reactivity like Leptos, with a smaller ecosystem.
- **Leptos's `view!` macro**: shorter, but needs `leptosfmt` beside rustfmt, and
  errors point into the macro's expansion.
- **Trunk or cargo-leptos**: both download `wasm-bindgen` and `wasm-opt`
  binaries while building, where a UI needs only `cargo build` and
  `wasm-bindgen`.
- **Building the bundle in the plugin's build script**: `cargo install` alone
  would produce the UI, but the script still needs the `wasm-bindgen` CLI, or
  compiles its library into every build of the plugin, and runs cargo inside
  cargo under the environment clippy and coverage set.
- **Gating the `ui` crate to wasm32**: host builds wouldn't compile Leptos, but
  rust-analyzer and the host checks would skip the components, and a wire-type
  change that breaks the UI's build would pass every persona but `web-dev`.

## Risks and Open Questions

- **Leptos on the host.** The decision assumes Leptos builds for the host as it
  does for wasm32.
  If it doesn't, or adds more than about 10% to a cold `cargo check
  --workspace`, the `ui` crate's components move behind `cfg(target_arch =
  "wasm32")`.
- **rustqual and browser tests.** rustqual may not count `#[wasm_bindgen_test]`
  functions as tests, and report components as untested; the fix belongs in
  `qual`, not in markers.
- **nextest.** `cargo_test` runs nextest; whether nextest can drive
  `wasm-bindgen-test-runner` decides whether the browser pass reuses its output
  parsing or runs `cargo test`.
- **A WebDriver.** Browser tests need ChromeDriver locally, or `safaridriver
  --enable` run once on macOS.
- **Leptos 0.9.0-rc1** may change before 0.9.0.

## Implementation Plan

One phase, landing with the first plugin that has a browser UI, since nothing
exercises it before then:

1. `wasm32-unknown-unknown` in `rust-toolchain.toml`, the `wasm-release`
   profile, workspace members for `*/api` and `*/ui`, `just build-ui`, and
   `install-plugin` building bundles.
2. `wasm_check` and `wasm_test` in the cargo tools, the `web-development` skill,
   and the `web-dev` persona.
3. The `web` CI workflow.
4. cargo-vet and cargo-deny entries for Leptos's dependencies.

Release builds stay as they are until a published plugin gets a `ui` crate, as
[Building and embedding](#building-and-embedding) describes.

## References

- [RFD 124: Source-Mapped Markdown Rendering][RFD 124]
- [RFD 136: Review Engine for Workspace Files][RFD 136]
- [Leptos's `counter_without_macros` example][counter]
- [`wasm-bindgen-test`][wbt]
- [CSS Custom Highlight API][highlight]

[RFD 124]: 124-source-mapped-markdown-rendering.md
[RFD 136]: 136-review-engine-for-workspace-files.md
[RFD 140]: 140-browser-frontend-for-the-review-engine.md
[counter]: https://github.com/leptos-rs/leptos/tree/main/examples/counter_without_macros
[highlight]: https://developer.mozilla.org/en-US/docs/Web/API/CSS_Custom_Highlight_API
[wbt]: https://docs.rs/wasm-bindgen-test
