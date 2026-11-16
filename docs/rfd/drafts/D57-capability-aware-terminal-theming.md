# RFD D57: Capability-Aware Terminal Theming

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-02
- **Extends**: [RFD 084], [RFD 048]
- **Summary**: Splits styling into a frontend-agnostic `jp_style` crate and
  capability-aware terminal color and theme adaptation in `jp_printer`.

## Summary

This RFD splits styling into two layers: a frontend-agnostic semantic layer in
`jp_style` (`Style`, a base16 `Palette`, named element scopes, and themes) and a
terminal rendering layer in `jp_printer`.
The terminal sink adapts resolved styles to the active terminal by selecting a
light or dark default theme, downgrading 24-bit color to 256 or 16 colors, and
stripping color and unsafe control sequences for `NO_COLOR` and non-terminal
channels.
This fixes unreadable inline code on light terminals and garbled syntax
highlighting on non-truecolor terminals such as Apple Terminal, while keeping
the styling model reusable across future web and TUI frontends.

## Motivation

JP emits 24-bit ANSI color unconditionally and assumes a dark terminal.
This causes two reported failures:

1. **Inline code is unreadable on a light terminal.** `format_code` in `jp_md`
   sets a theme-derived background (`gruvbox-dark`) without setting a
   foreground, so the code text falls back to the terminal's default foreground.
   On a light terminal, that renders black text on a dark background.
   The existing snapshot test encodes this directly: `Hello
   \x1b[48;2;34;34;34m`World`\x1b[49m!`.

2. **Syntax-highlighted tool output is garbled on non-truecolor terminals.**
   Apple Terminal sets `TERM=xterm-256color` without `COLORTERM` and does not
   support 24-bit color.
   It misparses `\x1b[48;2;r;g;b` by reading every parameter after `2` as a
   separate SGR code.
   When `fs_create_file` renders a syntax-highlighted file with one truecolor
   sequence per token, the output turns into visual noise.

Both failures have the same structural cause: multiple call sites emit final
ANSI bytes directly (syntect's `as_24_bit_terminal_escaped`, `theme_bg`,
`theme_blockquote_fg`, and `jp_cli` chrome), and no single layer knows the
terminal's capabilities.
Without a central rendering boundary, every new styled element repeats these
bugs and leaves JP unusable on light and non-truecolor terminals.

## Design

### Layering

Styling is split into two layers in separate crates:

- **Semantic styling (frontend-agnostic) in `jp_style`:** Defines `Style`
  (foreground, background, intensity, italic, underline, strikethrough),
  `Palette` and its base16 roles, themes, the scope taxonomy
  (`markdown.heading.h1`, `tool_call.function_name`), and stylesheet resolution.
  `jp_style` is pure and depends on `anstyle` only for its `Color`
  representation.
  `jp_config` holds the configuration types and converts them into `jp_style`;
  `jp_md` and `jp_printer` consume `jp_style`.
- **Terminal rendering (target-specific) in `jp_term` and `jp_printer`:** Owns
  `ColorProfile` detection, truecolor-to-256-to-16 color downgrading, SGR byte
  construction, and the escape-sequence adapter.

Logical `Style` values and scopes are the interchange format between the two
layers.
Each frontend has a single sink that translates resolved styles into
target-native output: the terminal sink emits ANSI sequences, while a future web
or TUI frontend can reuse `jp_style` unchanged to emit CSS or widget styles.
This RFD builds `jp_style` and the terminal sink, not a generic frontend trait
(see Non-Goals).
`jp_md` output remains terminal-specific in D57; a future non-terminal frontend
will render the shared stylesheet through its own serializer rather than reusing
`jp_md`'s SGR strings.

### Styling Model

- **`ColorProfile`** carries `depth` (`Truecolor | Ansi256 | Ansi16 | None`) and
  `scheme: Option<Scheme>`, where `Scheme` is `Light | Dark`.
  `scheme` is `None` when the terminal background cannot be determined (no
  `COLORFGBG`, no OSC 11 reply, or the query was skipped), which selects the
  fallback in the readability rule.
  Depth and scheme describe the terminal emulator and apply across all of its
  streams; whether a specific handle emits color depends on whether that output
  target is a TTY.
- **`Palette`** defines a set of semantic color roles using the base16
  vocabulary (`base00` through `base0F`: `base00` background, `base05` default
  foreground, `base08` red/error, `base0D` blue/function, and so on).
  A theme name resolves to a `{ palette, syntect theme }` pair: the palette
  colors UI chrome and markdown elements, while the syntect theme colors fenced
  code blocks.
  JP ships curated light and dark default themes; the exact set of built-in
  theme names is an implementation detail.
- **`Scope` and stylesheet:** Every styled element is addressed by an open-set
  dot-separated path.
  The default stylesheet maps each `Scope` to a palette role rather than a raw
  color, so switching themes recolors every scope automatically.
  Users can override individual scopes with either a palette role or a literal
  color (see Configuration Surface).

### Terminal Sink

The terminal sink is the sole component that converts resolved styles into
capability-adapted terminal bytes.
Two producers feed it:

- `jp_md` resolves markdown scopes against the shared stylesheet and palette and
  emits canonical truecolor SGR strings.
  Returning `String` keeps `jp_md` usable as a standalone library, and the sink
  does not need to know about markdown scopes.
- UI chrome passes a logical `Style` or a chrome scope that the sink resolves.

Color adaptation runs in the per-sink writer (`AnsiStripper`/`Sink`, introduced
in `e2db8c90`) rather than in `Printer::print`.
Because many call sites write through borrowed `out_writer` and `err_writer`
handles instead of calling `Printer::print`, placing the policy in the
underlying sink wrapper gives `print` and borrowed writers identical behavior
without changes at call sites.
Each sink maintains a persistent `vte::Parser` state machine rather than
transforming strings per write, so escape sequences split across `write` calls
or emitted byte-by-byte through the typewriter path are parsed intact.
Unit tests cover split CSI sequences and typewriter output.

### Escape and Color Policy

The sink filters escape sequences by sequence category and output target, never
by byte provenance.
Because borrowed writers do not carry origin metadata, the sink cannot
distinguish JP-generated bytes from relayed tool output and treats all content
writes uniformly:

| Sequence                                         | Styled TTY                                              | Non-styled (non-TTY / structured) |
| ------------------------------------------------ | ------------------------------------------------------- | --------------------------------- |
| SGR color (fg/bg)                                | adapt to `depth`; dropped if color is off               | strip                             |
| SGR attributes (bold/italic/underline/strike)    | keep                                                    | strip                             |
| OSC 8 hyperlink (inline)                         | keep                                                    | strip                             |
| Out-of-band cursor/erase control                 | strip from content; JP emits via a trusted control path | strip                             |
| Dangerous OSC (52 clipboard, 0/1/2 title), other | strip                                                   | strip                             |

Two rules follow from this table:

- **Disabling color is distinct from disabling styling.** Setting `NO_COLOR` or
  `depth = None` drops foreground and background colors while preserving text
  attributes (bold, italic, underline, strikethrough), following the [NO_COLOR]
  convention.
  All SGR sequences are stripped only on non-styled channels: non-TTY streams
  and structured JSON output (which is never styled, regardless of color
  environment variables).
- **Out-of-band cursor and erase sequences use a dedicated control path.** JP
  emits its own cursor and line-erase sequences (such as progress redraws using
  `\r\x1b[K`) through an explicit control method that bypasses content
  stripping; any cursor or erase sequence written through normal content handles
  is stripped.
  This fails closed: forgetting to use the control method drops a progress
  redraw rather than leaking a relayed escape sequence, without requiring
  per-caller flags or pre-sanitization.

Allowing inline OSC 8 hyperlinks by category means tool output can also emit
clickable links.
This is an acceptable tradeoff: the terminal does not open URLs automatically,
the link text remains visible, and the sink avoids tracking byte provenance.
A broader trust-boundary policy for tool-emitted terminal control belongs in
[RFD 075]; D57 defines only the default sink behavior.

### Readability Rule

Any element that sets a background color must remain legible against its
foreground text.
JP resolves foreground and background pairs based on `ColorProfile.scheme`:

- **`Some(Light | Dark)`:** Keep the terminal's default foreground and set only
  the scheme-matched background (for example, `inline_code.body.bg = base01`).
  Because the theme matches the terminal's light or dark scheme, the user's
  default text color already contrasts with `base01`.
- **`None` (undetermined):** Set an explicit foreground and background pair (`fg
  = base05`, `bg = base01`) so the element is legible regardless of the
  terminal's default text color.

This prevents bug 1 (a dark inline-code background paired with inherited black
text when the scheme is unknown) while preserving the user's default text color
when the scheme is known.
If a user explicitly configures `bg` without `fg` on a scope, JP respects that
override and does not inject a fallback `fg`.

### Configuration Surface

The `style` configuration uses a node-major layout: each presentation node
groups its behavioral settings with its visual `elements` table, while two
top-level sections (`style.theme` and `style.terminal`) configure global theme
selection and terminal capabilities.

```toml
# Global: the color source (shared by every frontend)
[style.theme]
default = "auto" # auto picks dark/light by detected scheme, or a name
dark = "gruvbox-dark"
light = "gruvbox-light"

[style.theme.palettes.my-dark] # optional custom base16 palette
base00 = "#282828"
base05 = "#ebdbb2"

# Terminal rendering target (capability overrides only)
[style.terminal]
color_depth = "auto" # auto | truecolor | 256 | 16 | none
background = "auto" # auto | light | dark

# Per node: behavior and visual elements together
[style.markdown]
wrap_width = 80
[style.markdown.elements.heading.h1]
fg = "base08" # see the value grammar below
intensity = "bold"
[style.markdown.elements.inline_code.body]
bg = "base01"

[style.tool_call]
parameters = "function_call"
[style.tool_call.elements.function_name]
fg = "base0D"
intensity = "bold"
```

- **`fg` and `bg` value grammar:** Following [RFD 084], color fields accept a
  palette role (`"base0D"`), a hex literal (`"#fb4934"`), an ANSI-256 index
  (`244`), `"default"` (reset the channel to the terminal default via SGR
  39/49), or `"inherit"` (omit the SGR code so the surrounding context shows
  through).
  The default stylesheet uses `"inherit"` to preserve the user's terminal
  foreground when the scheme is known.
- **Per-node `elements` tables:** Visual styling for every node lives under
  `style.<node>.elements.*`, generalizing `style.markdown.elements.*` from [RFD
  084].
  Behavioral settings remain directly on the node (`style.markdown.wrap_width`,
  `style.tool_call.parameters`).
- **Global theme configuration:** Theme selection moves from
  `style.markdown.theme` to `style.theme` because the active theme drives UI
  chrome and tool-call colors in addition to markdown.
  Setting `default = "auto"` selects the `dark` or `light` theme based on the
  detected terminal scheme; both keys accept a built-in theme name or a custom
  palette defined under `style.theme.palettes`.
  `style.inline_code` moves into `style.markdown.elements.inline_code`.
- **Terminal capability overrides:** `style.terminal` holds only
  rendering-target capability overrides (`color_depth` and `background`), so it
  sits as a sibling to future frontend target sections rather than acting as a
  catch-all.

Restructuring `style` is an intentional breaking change.
Because config structs reject unknown fields, the loader explicitly maps the
previous keys instead of ignoring them.
This replaces the deprecation-alias phase for `style.inline_code.background` in
[RFD 084] so D57 and [RFD 084] target a single configuration layout.

### Data Flow

```
jp_md     -->  markdown scopes resolved to truecolor  -+
chrome    -->  logical Style / chrome scope            +-->  per-sink adapter  -->  bytes
tool/raw  -->  relayed bytes                           -+
```

For every write through `Printer::print`, `out_writer`, or `err_writer`, the
per-sink adapter resolves chrome scopes, downgrades each color to
`ColorProfile.depth`, enforces the category-based escape policy, and strips all
styling on non-styled targets.

### Capability Detection and Startup Order

`jp_term` detects terminal capabilities and produces a `ColorProfile`.
While `depth` and `scheme` apply across the terminal, color emission is decided
independently for each `PrintTarget` (`Out`, `Err`, `Tty`) based on whether that
handle is a TTY.
Redirecting stdout (`jp query > file`) therefore keeps colored chrome on a TTY
stderr, while structured output remains unstyled.

This amends [RFD 048]: the serialization format (text or JSON) still resolves
once from stdout, but color emission is decided per target rather than following
the global format.
Under `--format json`, both streams emit NDJSON and are never styled, regardless
of color environment variables or TTY state.

Because `style.terminal.*` has the highest precedence, `jp_cli` resolves the
final `ColorProfile` after loading configuration (merging config overrides with
`jp_term` detection) and installs it into the sink before command rendering
begins.
Direct writes outside the configured `Printer`, such as early bootstrap messages
and top-level fatal error output, remain unstyled and out of scope (see Terminal
Sink).

Each setting resolves in the following precedence order:

- **Color emission (per target):** explicit config -> `NO_COLOR` (off) ->
  `CLICOLOR_FORCE` (on, even when not a TTY) -> handle is a TTY (on) -> off.
- **Depth (terminal-wide):** explicit config -> `COLORTERM` truecolor -> `TERM`
  `*-256color` -> color-capable `TERM` (16).
- **Scheme (terminal-wide):** explicit config -> `COLORFGBG` -> best-effort OSC
  11 query -> `None`.

Because `CLICOLOR_FORCE` precedes the TTY check, it forces color output into
pipes, while `NO_COLOR` still takes precedence over `CLICOLOR_FORCE`.
Apple Terminal requires no special case: the absence of `COLORTERM` combined
with `TERM=xterm-256color` selects `Ansi256`.

### Dependencies

Most of the required crates are already in `Cargo.lock` through `clap`'s
`anstyle` dependencies (all MIT/Apache-2.0 licensed and audited):

- **`anstyle`:** Provides the core color model wrapped by `jp_style`, allowing
  the rest of the `anstyle` crates to compose without adapter code.
  This amends [RFD 084] by replacing its custom `Color` enum with `anstyle`'s
  1:1 equivalent.
- **`anstyle-query`:** Implements the `COLORTERM`, `TERM`, `NO_COLOR`, and
  `CLICOLOR` detection heuristics.
- **`anstyle-parse` and `vte`:** Parse SGR and escape sequences in the per-sink
  adapter.

Two dependencies are added: **`anstyle-lossy`** (MIT/Apache-2.0, `const fn`
truecolor-to-256-to-16 quantization) and **`termbg`** (light/dark scheme
detection).
Base16 palettes are parsed in-house with `serde` over 16 hex strings (a few
dozen lines of code) to avoid the copyleft licenses of `tinted-builder`
(GPL-3.0) and `ansi_colours` (LGPL).

## Drawbacks

- **The scope taxonomy is a permanent public contract.** Once users configure
  keys such as `tool_call.function_name.fg = "..."`, those scope names cannot be
  changed without breaking user configs ([Hyrum's Law]).
  New scopes must be added deliberately.
- **Per-write ANSI parsing overhead.** Downgrading truecolor sequences to 256 or
  16 colors requires parsing the byte stream on every write.
  A memoized RGB-to-index lookup cache keyed on the active `ColorProfile` keeps
  quantization cost negligible.
- **Cross-RFD coordination with [RFD 084] and [RFD 048].** In [RFD 084],
  `jp_md`'s responsibility narrows from final SGR construction to emitting
  canonical truecolor for markdown scopes while the terminal sink handles depth
  adaptation, and its deprecation-alias phase is dropped in favor of the `style`
  rename.
  In [RFD 048], color emission changes from a global decision to a per-target
  decision.
  These changes must land together coherently.
- **Breaking changes to the `style` configuration.** Restructuring `style`
  invalidates existing user style customizations unless migrated, requiring the
  explicit key-mapping pass during config loading.
- **Curating default themes across color tiers and schemes.** Maintaining
  built-in light and dark themes that read well at truecolor, 256-color, and
  16-color depths requires manual curation beyond automatic color quantization.

## Alternatives

- **Always set the inline-code foreground from the theme.** Overriding the
  foreground unconditionally replaces the user's chosen terminal text color even
  when the light or dark scheme is known.
  JP sets an explicit foreground only as the fallback when `scheme` is `None`.
- **Tag each print call with a `PrintOrigin` (chrome, markdown, tool output).**
  Requiring every `Printer` caller to classify its output is error-prone and
  does not work across borrowed `Write` handles.
  Keying the sink policy on sequence category and output target avoids tracking
  provenance.
- **Sanitize tool output at each call site before printing.** Like
  `PrintOrigin`, call-site sanitization requires every caller (including code
  writing to borrowed `out_writer` and `err_writer` handles) to sanitize before
  writing.
  Enforcing the policy inside the sink makes every write safe by default.
- **Downgrade colors at each emission site instead of in the sink.** Colors
  originate from the [RFD 084] markdown stylesheet, `syntect`, and `jp_cli`
  chrome.
  Adapting colors once in the terminal sink covers all three sources at the
  frontend boundary.
- **Always emit 256-color or 16-color output.** Fixed low-depth output discards
  color fidelity on truecolor terminals.
- **Use an aspect-major config layout (`style.terminal.elements.<node>`).**
  Splitting a node's behavioral and visual settings across two config trees is
  harder to navigate and conflicts with `style.markdown.elements.*` from [RFD
  084].
- **Use `termprofile` as an all-in-one engine.** `termprofile` pulls in the
  heavyweight `palette` dependency, whereas `anstyle-query` and `anstyle-lossy`
  cover the same functionality with minimal dependencies.

## Non-Goals

- **Web or TUI frontends, or a generic frontend sink trait.** D57 ships
  `jp_style` and the terminal sink.
  A generic sink abstraction waits until a second frontend exists to validate
  its interface.
- **A full chrome scope taxonomy.** This RFD ships `markdown.*` (via [RFD 084])
  and `tool_call.*` scopes; additional chrome scopes will follow in subsequent
  RFDs on the same engine.
- **Automatic palette generation via LAB interpolation** (such as the
  base16-to-256 cube technique from the [color256] writeup).
  JP ships curated default themes instead.
- **Migrating behavioral style settings onto `Style`.** Adding an `elements`
  table to a node leaves its behavioral configuration (`hidden`, `parameters`,
  reasoning display mode) unchanged.
- **Replacing `syntect` themes for code blocks.** `syntect` continues to theme
  fenced code blocks; its emitted 24-bit colors pass through the terminal sink's
  depth downgrade.
- **Infallible scheme detection.** Terminal scheme detection is best-effort with
  a configuration override (`style.terminal.background`), and `None` is a
  supported outcome.
- **A full trust policy for tool-emitted escape sequences.** D57 defines default
  sink filtering; broader sandbox and trust-boundary policy belongs to [RFD
  075].

## Risks and Open Questions

- **Unknown-scheme fallback defaults.** When `scheme` is `None`, the readability
  rule sets a self-contained foreground and background pair so text remains
  legible.
  Choosing the exact default pair and keeping the set of background-bearing
  default scopes small remain open details for implementation.
- **OSC 11 scheme query reliability.** `termbg` queries the terminal via OSC 11
  by briefly entering raw mode and reading the reply.
  Because this runs once at startup before any background tasks or output
  streaming begin, it does not race with other I/O, provided it uses a short
  timeout, falls back cleanly to `None`, and skips the query when stdout is not
  a TTY.
- **Quantization cache invalidation.** The memoized RGB-to-index cache must
  include the active `ColorProfile` in its key so mid-session profile changes do
  not return stale color indices.
- **`anstyle` API coupling.** Wrapping `anstyle` types in `jp_style` couples JP
  to `anstyle`'s API.
  This risk is low because `anstyle` is stable, maintained by the `clap`
  project, and already a transitive dependency.

## Implementation Plan

### Phase 1: `ColorProfile` and the terminal sink

- Add `ColorProfile { depth, scheme: Option<Scheme> }` and capability detection
  in `jp_term` (`anstyle-query` and `TERM`, with `scheme` initially stubbed to
  `None`).
  Resolve the profile in `jp_cli` after configuration loads and install it into
  the sink before rendering, with per-`PrintTarget` color gating.
- Extend the per-sink adapter (`AnsiStripper`/`Sink` with a persistent
  `vte::Parser`) to downgrade SGR depth via `anstyle-lossy`, enforce the
  category-based escape policy, and strip all SGR sequences on non-styled
  channels.
  Add the trusted out-of-band control path for JP's progress and cursor
  sequences, covering both `Printer::print` and borrowed
  `out_writer`/`err_writer` handles, with tests for split CSI sequences and
  typewriter output.
- Keep `jp_md` and UI chrome emitting canonical truecolor so the sink adapts
  colors on write.
  Add `anstyle-lossy` and `style.terminal.color_depth`.
- This phase fixes bug 2 (garbled syntax-highlighted tool output on
  non-truecolor terminals).

### Phase 2: `jp_style`, themes, and scheme selection

- Create the `jp_style` crate (`Style`, `Palette`, `Scope`, stylesheet
  resolution, wrapping `anstyle::Color`), an in-house base16 loader, and curated
  light and dark default themes.
  Align `jp_md`'s logical `Color` type with `anstyle` (amending [RFD 084]).
- Add scheme detection (`termbg` and `COLORFGBG`) producing `Option<Scheme>`,
  scheme-driven default theme selection, and the readability rule.
- Move `theme` to the global `style.theme` section with `default = "auto"`.
- This phase depends on Phase 1 and fixes bug 1 (unreadable inline code on light
  terminals).

### Phase 3: Node-major config surface and chrome scopes

- Implement the node-major `style` layout: global `style.theme` and
  `style.terminal` sections, and per-node `style.<node>.elements.*` tables.
  Build `style.markdown.elements.*` from [RFD 084] (without its
  deprecation-alias phase) and `tool_call.*` scopes on this engine, along with
  the `fg`/`bg` value grammar and `name -> { palette, syntect }` theme
  resolution.
- Map the previous `style` configuration keys during loading and document the
  new schema.
  Scopes beyond `markdown.*` and `tool_call.*` are deferred to follow-up RFDs.
  This phase depends on Phase 2.

## References

- [RFD 004]: Custom `jp_md` terminal renderer that this design builds on.
- [RFD 048]: Four-channel output model.
  D57 reuses its per-target channels and amends color emission from a global
  decision to a per-target decision.
- [RFD 075]: Tool sandbox and access policy.
  D57 defines default sink sanitization for tool-emitted escape sequences; a
  future RFD 075 revision may define a broader trust-boundary policy.
- [RFD 084]: Configurable markdown element coloring.
  D57 extends it by reusing its `Style` and stylesheet design, moving SGR depth
  adaptation to the terminal sink, dropping the deprecation-alias phase, and
  sharing the node-major `style` configuration layout.
  Promoting this draft adds the `Extended by` back-link to RFD 084.
- [RFD 096]: Terminal output sanitization for untrusted content.
  RFD 096 neutralizes untrusted content where it enters the render pipeline,
  while D57's terminal sink adapts trusted styling downstream.
  RFD 096 owns the SGR allowlist for untrusted content, so the terminal sink
  must not introduce a second untrusted-content policy.
- [color256]: Writeup on generating 256-color terminal palettes from base16
  themes (informative).
- [`anstyle`], [`termbg`]: Terminal styling and background-detection crates
  reused or added by this RFD.
- [base16 styling spec]: Specification for the palette role vocabulary and theme
  file format.

[Hyrum's Law]: https://www.hyrumslaw.com/
[NO_COLOR]: https://no-color.org/
[RFD 004]: ../004-streaming-markdown-parser-and-terminal-renderer-in-jp_md.md
[RFD 048]: ../048-four-channel-output-model.md
[RFD 075]: ../075-tool-sandbox-and-access-policy.md
[RFD 084]: ../084-configurable-markdown-element-coloring.md
[RFD 096]: ../096-terminal-output-sanitization-for-untrusted-content.md
[`anstyle`]: https://crates.io/crates/anstyle
[`termbg`]: https://crates.io/crates/termbg
[base16 styling spec]: https://github.com/tinted-theming/home/blob/main/styling.md
[color256]: https://gist.github.com/jake-stewart/0a8ea46159a7da2c808e5be2177e1783
