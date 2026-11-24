# RFD 139: Browser UIs in Rust with Leptos

- **Status**: Accepted
- **Category**: Decision
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-11-18
- **Required by**: [RFD 140]
- **Summary**: Adopts client-rendered Rust browser UIs with Leptos, shared API
  types, and bundles embedded in command plugins.

## Summary

JP's browser UIs are written in Rust, compiled to WebAssembly, and rendered on
the client with Leptos.
Each command plugin embeds its compiled UI bundle in its binary and serves it
alongside its JSON API, adding no handwritten JavaScript to the repository.

## Context

JP has three browser-facing surfaces:

- `jp-vite`'s documentation site, a VitePress and Vue application whose files
  are synced into each workspace that uses it.
- `jp serve web`, a prototype that renders HTML on the server with maud and runs
  handwritten JavaScript checked by `just typecheck-js`; it is scheduled for a
  rewrite.
- `jp review serve`, a local JSON API whose browser frontend [RFD 136] deferred.

`jp-vite` remains on VitePress and Vue, but new browser UIs will not add more
JavaScript to the repository.

The review frontend requires client-side behavior that server rendering cannot
replace:

- Mapping DOM text selections to source byte offsets through the `data-s` and
  `data-e` spans emitted by `md-html` ([RFD 124]).
- Highlighting commented ranges without mutating the rendered DOM, using the
  [CSS Custom Highlight API][highlight].
- Updating comment threads in place as the assistant replies during a pass.

The workspace already carries the core WebAssembly bindings (`wasm-bindgen`,
`js-sys`, and `web-sys`) as transitive dependencies in `Cargo.lock`, and the API
payloads exchanged with the server are already defined as Rust structs.

## Decision

### Stack

Browser UIs are written in Rust, compiled for `wasm32-unknown-unknown`, and
rendered in the browser with Leptos (pinned to `0.9.0-rc1` until `0.9.0` is
released).

Rendering is strictly client-side.
The plugin serves a static HTML shell, the compiled WebAssembly bundle, and the
JSON API that the client calls.
There is no server-side rendering or hydration, and the server crate does not
depend on Leptos.

Views use Leptos's builder syntax rather than the `view!` macro, following
Leptos's [`counter_without_macros`][counter] example:

```rust
div().child((
    button()
        .on(ev::click, move |_| count.update(|n| *n += 1))
        .child("+1"),
    span().child(move || count.get()),
))
```

Builder syntax is standard Rust: `rustfmt` formats it, `rust-analyzer` completes
it, and `clippy` lints it without macro-expansion indirection or `leptosfmt`.

UI crates depend on the Leptos crates that client-side rendering in builder
syntax uses, not on the `leptos` crate itself: `tachys` for the renderer and
builder API, `reactive_graph` for signals and effects, `any_spawner` for task
spawning, and `leptos_dom` for DOM helpers.
The `leptos` crate always compiles server functions, configuration-file
loading, and the `view!` macro with its parser, which account for more than
half of the code it adds to the dependency tree.

The repository contains no JavaScript source files for these UIs.
At build time, `wasm-bindgen` generates the JS glue module, and the HTML shell
loads it with a one-line `<script type="module">`.
Browser APIs that `web-sys` gates behind `web_sys_unstable_apis` (such as the
CSS Custom Highlight API) are bound directly in Rust with small
`#[wasm_bindgen]` `extern` blocks.

### Crates

A plugin with a browser UI adds two sibling crates to the workspace:

| Crate                              | Target          | Role                                                           |
| ---------------------------------- | --------------- | -------------------------------------------------------------- |
| `<plugin>/api` (`jp-<plugin>-api`) | host and wasm32 | Request and response types for the JSON API (`serde`, no I/O). |
| `<plugin>/ui` (`jp-<plugin>-ui`)   | host and wasm32 | Client-side Leptos UI.                                         |

Because the server serializes and the UI deserializes the same `api` types,
wire-format changes cannot silently drift between server and client.
Renaming a field or adding an enum variant that an exhaustive `match` does not
handle fails compilation immediately.
Semantic changes and newly added optional fields are covered by tests: unit
tests in the `api` crate pin the serialized JSON representation of each type,
and endpoint tests in the server crate verify behavior (such as default anchor
versions).

The `ui` crate keeps domain and layout logic in pure functions over owned types
(mapping selections to byte ranges, positioning comment cards, and scheduling
polls) while keeping reactive components and DOM bindings thin.
Because `ui` also compiles on the host target, `cargo check`, `clippy`,
`rustqual`, `rust-analyzer`, and `cargo test` check it alongside the rest of the
workspace.

### Styling

UIs are styled with [Tailwind CSS] utility classes, written on the elements in
the views.
A UI's `style.css` is the input Tailwind compiles.
It holds what a class on an element can't say: the theme (fonts, and colors that
follow the system's light or dark scheme), the styles of HTML the server
rendered (Markdown from `md-html`), and rules such as `::highlight()`.

- Tailwind writes the stylesheet from the class names it finds in the `ui`
  crate's sources, which it reads as plain text.
  A class name is therefore written out in full wherever it is used, never
  assembled from parts.
- `just build-ui` compiles the stylesheet with Tailwind's [standalone CLI], a
  single executable that needs no Node.js, so the repository gains no
  `package.json` and no JavaScript.
  The recipe downloads the CLI at a pinned version into cargo's target
  directory and checks the download against a pinned SHA-256.
- Sizes for touch use the `pointer-coarse:` variant and layout changes use the
  breakpoint variants, so what an element looks like on every screen is written
  on the element.

### Testing

- Pure UI logic is unit-tested on the host in `_tests.rs` files, matching the
  rest of the workspace.
- DOM interactions and mounted components are tested with
  [`wasm-bindgen-test`][wbt] configured with `run_in_browser`, running inside a
  headless browser via `wasm-bindgen-test-runner`.

### Building and embedding

- `rust-toolchain.toml` includes the `wasm32-unknown-unknown` target.
- `just build-ui <plugin>` compiles the `ui` crate for `wasm32-unknown-unknown`
  using a size-optimized `wasm-release` profile, runs `wasm-bindgen --target
  web` into `target/ui/<plugin>/`, and compiles the UI's stylesheet into that
  directory with Tailwind CSS (see [Styling](#styling)).
  The recipe installs the `wasm-bindgen` CLI at the exact version pinned for the
  `wasm-bindgen` crate in `Cargo.lock`, because the CLI rejects modules compiled
  against a mismatched crate version.
- When `JP_<PLUGIN>_UI` (e.g. `JP_REVIEW_UI`) is set at compile time, the
  plugin's `build.rs` embeds the bundle from that directory; if the directory
  does not contain a valid bundle, `build.rs` fails the build rather than
  falling back.
  When `JP_<PLUGIN>_UI` is unset, `build.rs` embeds a placeholder HTML page
  explaining how to build the UI.
  `build.rs` never invokes `cargo`.
- `just install-plugin <plugin>` runs `just build-ui <plugin>` before installing
  any plugin that has a `ui` crate.
  Running `cargo install` directly installs the plugin binary with the JSON API
  and the placeholder page.
- While local or Git installs without `JP_<PLUGIN>_UI` may serve the placeholder
  page, published plugin binaries must embed the bundle built from the same
  revision: release builds set `JP_<PLUGIN>_UI` and fail if the bundle is
  missing.
  `just plugin-build` will be updated to build and pass the bundle when the
  first published plugin gains a `ui` crate.

### Tooling for the assistant

The workspace's `cargo_check` and `cargo_test` tools already gate optional
verification passes behind tool options (`wiki_check`, `qual_check`,
`site_check`).
This RFD adds two options for WebAssembly crates:

- `wasm_check` on `cargo_check` runs `clippy` for `wasm32-unknown-unknown` on
  every browser crate.
- `wasm_test` on `cargo_test` runs the headless-browser test suite for every
  browser crate.

Browser crates declare themselves in `Cargo.toml`, where the tools, `just`, and
CI discover them via `cargo metadata`:

```toml
[package.metadata.jp]
wasm = true
```

A `web-development` skill enables both options and documents the conventions
from this RFD.
A `web-dev` persona layers that skill on top of `dev` (paralleling `app-dev` for
the macOS app), while other personas skip `wasm32` builds.

### CI

A path-filtered `web` GitHub Actions workflow (modeled on `app.yml`) runs
whenever a browser crate, an `api` crate, `md-html`, `Cargo.lock`, or
`rust-toolchain.toml` changes.
It runs `clippy` on all browser crates for `wasm32-unknown-unknown`, executes
their browser tests in headless Chrome via ChromeDriver, and builds every UI
bundle.

## Consequences

Benefits:

- A single language and toolchain from on-disk storage to the browser DOM.
- Compile-time verification that client code matches the server's request and
  response types.
- Host-side unit tests and lints run on UI logic with the workspace's standard
  Rust tooling.
- A settled, JavaScript-free architecture for rewriting `jp serve web`.

Costs:

- Building a full plugin UI requires a `wasm-bindgen` post-processing step, a
  Tailwind CSS step, and the `wasm32-unknown-unknown` target outside a plain
  `cargo build`.
- The first build of a UI downloads the Tailwind CSS standalone CLI, about 100
  MB, and so does the first after its pinned version changes.
- Installing a plugin with plain `cargo install` (without building the bundle
  first) serves the placeholder page instead of the UI.
- Leptos and its dependency tree must be audited in `cargo-vet` and `cargo-deny`
  and are compiled during host workspace builds.
- Leptos `0.9.0-rc1` is a pre-release dependency whose API may change before
  `0.9.0`.
- Target-specific `wasm32` build failures are caught only under the `web-dev`
  persona or in CI, matching the tradeoff for the macOS app.
- WebAssembly bundles are larger than minimal handwritten scripts, though bundle
  size is negligible for a local loopback server.

## Alternatives

- **Server-rendered HTML with small scripts** (as in `jp serve web`).
  Mapping selections to source byte offsets and rendering CSS custom highlights
  still require client-side code, and server rendering forces the plugin to
  maintain an HTML view layer alongside its JSON API.
- **Server-rendered HTML with WebAssembly islands** for selection handling.
  Splits the UI across two rendering models and requires full-page or fragment
  reloads to reflect live state changes.
- **Vue components inside `jp-vite`**.
  Adds more JavaScript and couples a command plugin's UI to the documentation
  site's sync and release cycle.
- **Other Rust UI frameworks**.
  Dioxus relies on its own `dx` build CLI; Yew uses a virtual DOM and sees less
  active development; Sycamore offers fine-grained reactivity similar to Leptos
  but with a smaller ecosystem.
- **Leptos's `view!` macro**.
  More concise than builder syntax, but requires `leptosfmt` alongside `rustfmt`
  and produces diagnostics that point into macro expansions.
- **Trunk or `cargo-leptos`**.
  Both tools download external `wasm-bindgen` and `wasm-opt` binaries during
  builds, whereas JP's client-only bundles require only `cargo build`,
  `wasm-bindgen`, and the Tailwind CSS CLI, each at a version the workspace
  pins.
- **Hand-written CSS**.
  Needs no build step, but each UI then defines its own spacing, type, and color
  scales, and what an element looks like per screen and pointer is spread over
  rules and media queries away from the element.
- **Tailwind CSS through npm**.
  The same stylesheet, but adds Node.js, a `package.json`, and a lock file to
  the build of a Rust crate.
- **Compiling the WebAssembly bundle inside `build.rs`**.
  Would allow a plain `cargo install` to embed the UI, but still requires an
  external `wasm-bindgen` binary (or compiling `wasm-bindgen-cli-support` into
  every build script) and spawns nested `cargo` invocations that inherit host
  `clippy` or coverage flags.
- **Restricting the `ui` crate to `cfg(target_arch = "wasm32")`**.
  Avoids compiling Leptos on the host, but prevents `rust-analyzer` and default
  host checks from inspecting UI components, allowing `api` changes that break
  the UI to pass outside `web-dev`.

## Risks and Open Questions

- **Host compilation overhead.** This design assumes Leptos compiles cleanly on
  the host target without significant overhead.
  If host compilation fails or adds more than roughly 10% to a cold `cargo check
  --workspace`, component modules in `ui` crates will be gated behind
  `#[cfg(target_arch = "wasm32")]`.
- **`rustqual` test recognition.** `rustqual` may not recognize
  `#[wasm_bindgen_test]` functions as tests and could flag UI components as
  untested; if so, `qual` should be updated rather than adding suppression
  markers.
- **`nextest` integration.** `cargo_test` uses `cargo nextest`; if `nextest`
  cannot drive `wasm-bindgen-test-runner`, `wasm_test` will invoke `cargo test
  --target wasm32-unknown-unknown` directly.
- **Local WebDriver availability.** Running browser tests locally requires
  `chromedriver` on `PATH` (or running `safaridriver --enable` once on macOS).
- **Leptos `0.9.0-rc1` stability.** Minor API adjustments may be required when
  upgrading to the final `0.9.0` release.

## Implementation Plan

A single phase, landing alongside the first plugin browser UI ([RFD 140]):

1. Add `wasm32-unknown-unknown` to `rust-toolchain.toml`, configure the
   `wasm-release` profile, add workspace glob patterns for `*/api` and `*/ui`,
   and add `just build-ui`, with the pinned Tailwind CSS CLI, and bundle
   building in `just install-plugin`.
2. Add `wasm_check` and `wasm_test` to `cargo_check` and `cargo_test`, and
   create the `web-development` skill and `web-dev` persona.
3. Add the path-filtered `web` CI workflow.
4. Add `cargo-vet` and `cargo-deny` entries for Leptos and its dependencies.

Release builds remain unchanged until a published plugin gains a `ui` crate (see
[Building and embedding](#building-and-embedding)).

## References

- [RFD 124: Source-Mapped Markdown Rendering][RFD 124]
- [RFD 136: Review Engine for Workspace Files][RFD 136]
- [Leptos's `counter_without_macros` example][counter]
- [`wasm-bindgen-test`][wbt]
- [CSS Custom Highlight API][highlight]
- [Tailwind CSS]
- [Tailwind CSS standalone CLI][standalone CLI]

[RFD 124]: 124-source-mapped-markdown-rendering.md
[RFD 136]: 136-review-engine-for-workspace-files.md
[RFD 140]: 140-browser-frontend-for-the-review-engine.md
[counter]: https://github.com/leptos-rs/leptos/tree/main/examples/counter_without_macros
[highlight]: https://developer.mozilla.org/en-US/docs/Web/API/CSS_Custom_Highlight_API
[standalone CLI]: https://tailwindcss.com/blog/standalone-cli
[Tailwind CSS]: https://tailwindcss.com/
[wbt]: https://docs.rs/wasm-bindgen-test
