# RFD D57: Capability-Aware Terminal Theming

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-02
- **Extends**: [RFD 084], [RFD 048]

## Summary

Introduce a styling system split into two layers: a frontend-agnostic *semantic
styling* layer (a `Style` model, a base16 `Palette`, named element *scopes*, and
a theme) living in a neutral `jp_style` crate, and a terminal-specific
*rendering* layer owned by `jp_printer`.
The terminal sink adapts resolved styling to the terminal it is writing to:
downgrading 24-bit color to 256 or 16, selecting a light or dark default theme,
and stripping color and unsafe control sequences for non-terminal channels and
`NO_COLOR`.
This fixes unreadable inline code on light terminals and garbled syntax
highlighting on non-truecolor terminals (e.g. Apple Terminal), while keeping the
styling model reusable by future web or TUI frontends.

## Motivation

JP emits 24-bit ANSI color unconditionally and assumes a dark terminal.
Two concrete, reported failures follow:

1. **Inline code is unreadable on a light terminal.** `format_code` in `jp_md`
   sets a theme-derived *background* but never a *foreground*, so the code text
   falls back to the terminal's default foreground.
   On a white-background terminal that is black text on the dark `gruvbox-dark`
   background.
   The snapshot test today encodes exactly this: `Hello
   \x1b[48;2;34;34;34m`World`\x1b[49m!`.

2. **Syntax-highlighted tool output is garbled on non-truecolor terminals.**
   Apple Terminal sets `TERM=xterm-256color` and no `COLORTERM`, and does not
   support 24-bit color.
   A `\x1b[48;2;r;g;b` sequence is misparsed (everything after `2` is read as
   separate SGR parameters), so a syntax-highlighted file rendered by
   `fs_create_file`, one truecolor pair per token, turns to noise.

The shared root cause is structural: color is emitted as final bytes from many
places (syntect's `as_24_bit_terminal_escaped`, `theme_bg`,
`theme_blockquote_fg`, and assorted `jp_cli` chrome), and *no single layer knows
the terminal's capabilities*.
There is nowhere to put the fix.
If we do nothing, every new styled element repeats the same two bugs, and JP
stays unusable on a large class of terminals.

## Design

### Layering

Two layers, in separate homes:

- **Semantic styling (frontend-agnostic), in a new `jp_style` crate.** `Style`
  (foreground, background, intensity, italic, underline, strikethrough), the
  `Palette` and its base16 roles, the theme, the *scope* taxonomy
  (`markdown.heading.h1`, `tool_call.function_name`, ...), and stylesheet
  resolution.
  None of this is terminal-specific: a web frontend renders `Style` as CSS, a
  TUI as widget styles.
  `jp_style` is pure and depends on `anstyle` only for its `Color`
  representation.
  `jp_config` holds the config shapes and converts into `jp_style`; `jp_md` and
  `jp_printer` depend on `jp_style`.
- **Rendering target (terminal-specific), in `jp_term`/`jp_printer`.** The
  `ColorProfile`, the truecolor-to-256-to-16 downgrade, SGR byte construction,
  and the escape adapter.

The terminal is one *frontend sink*.
The general pattern, not just this instance: logical `Style`/scope is the
interchange, and each frontend has exactly one sink that turns it into
target-native output.
A future web or TUI frontend brings its own sink and reuses `jp_style`
unchanged.
This RFD builds the terminal sink and `jp_style`; it does not build the generic
abstraction (see Non-Goals).
`jp_md`'s terminal output remains terminal-specific for D57: a future frontend
renders the same stylesheet through its own serializer rather than reusing
`jp_md`'s SGR strings.

### The model

- **`ColorProfile`** carries a `depth` (`Truecolor | Ansi256 | Ansi16 | None`)
  and a `scheme: Option<Scheme>` where `Scheme` is `Light | Dark`.
  `None` means the scheme could not be determined (no `COLORFGBG`, no OSC 11
  answer, or the query was skipped); the readability rule depends on this.
  Depth and scheme describe the terminal *program* and are shared across its
  handles; whether a given handle emits color at all is decided per output
  target (is that handle a TTY?).
- **`Palette`** is a small set of semantic color *roles* using the base16
  vocabulary (`base00`..
  `base0F`: `base00` background, `base05` default foreground, `base08`
  red/error, `base0D` blue/function, ...).
  A *theme* name resolves to a `{ palette, syntect theme }` pair: the palette
  colors chrome and markdown elements, the syntect theme colors code blocks.
  JP ships curated light and dark default themes; the exhaustive set of built-in
  names is an implementation detail.
- **`Scope` stylesheet.** Every styled element is named by an open-set string
  path.
  The default stylesheet maps each scope to a palette *role* (not a raw color),
  so switching the theme reassigns every scope for free.
  Users override per scope, with either a role reference or a literal color (see
  Configuration surface).

### The terminal sink

Logical `Style`/scope is the interchange; the terminal sink is the only place
that turns it into capability-adapted bytes.
Two producers feed it:

- `jp_md` resolves *markdown* scopes against the shared stylesheet and palette
  and emits **canonical truecolor** strings.
  It keeps string output and stays usable as a library; the sink does not know
  markdown scopes.
- Chrome hands the sink a logical `Style` (or a chrome scope the sink resolves).

The adaptation lives in the **per-sink writer**, not in a single `print` method.
Many call sites do not call `Printer::print`; they obtain an `out_writer` /
`err_writer` handle and write to it.
The policy therefore lives in the per-sink adapter those handles are already
wrapped in (the `AnsiStripper`/`Sink` layer introduced in `e2db8c90`), so
`print` and borrowed writers get identical treatment with no caller awareness.
The adapter is a **per-sink state machine with a persistent `vte::Parser`**, not
a per-write string transform, so escape sequences split across `write` calls
(and the per-character typewriter path) are handled correctly.
Tests cover split CSI sequences and typewriter output.

### Escape and color policy

The policy is keyed on **sequence category and target**, never on the source of
the bytes.
This is what makes it work uniformly through borrowed writers without threading
provenance: the sink cannot tell JP-generated bytes from relayed bytes, so it
does not try.
For every write:

| Sequence                                         | Styled TTY                                              | Non-styled (non-TTY / structured) |
| ------------------------------------------------ | ------------------------------------------------------- | --------------------------------- |
| SGR color (fg/bg)                                | adapt to `depth`; dropped if color is off               | strip                             |
| SGR attributes (bold/italic/underline/strike)    | keep                                                    | strip                             |
| OSC 8 hyperlink (inline)                         | keep                                                    | strip                             |
| Out-of-band cursor/erase control                 | strip from content; JP emits via a trusted control path | strip                             |
| Dangerous OSC (52 clipboard, 0/1/2 title), other | strip                                                   | strip                             |

Two consequences:

- **Color off is not the same as styling off.** `NO_COLOR` (and `depth = None`)
  drops foreground/background color but keeps text attributes, per the
  [NO_COLOR] convention.
  Stripping *all* SGR happens only on a non-styled channel (non-TTY or
  structured/JSON output, which always runs non-styled regardless of color env
  vars).
- **Out-of-band control needs a trusted path.** JP's own cursor/erase usage
  (progress lines, `\r\x1b[K`) is emitted through a dedicated control method
  that bypasses the strip; anything written as normal content is stripped.
  The failure mode is therefore safe: forget the trusted path and you lose a
  progress redraw, never leak a relayed escape.
  There is no per-caller flag and no "sanitize first" rule.

Deliberate tradeoff: OSC 8 hyperlinks are allowed inline by category, so a
tool's output *can* emit a hyperlink.
This is acceptable (the URL is not auto-followed, the link text is visible) and
avoids reintroducing source tracking.
A complete trust-boundary policy for tool-emitted terminal control belongs with
[RFD 075]; D57 only sets the default sink behavior.

### Readability rule

A background-bearing element must contrast with whatever foreground is shown.
JP resolves this on `ColorProfile.scheme`:

- **`Some(Light | Dark)`:** honor the user's terminal foreground and set only
  the scheme-matched background (e.g. `inline_code.body.bg = base01`).
  A light theme yields a light code background that the user's dark text reads
  on; JP does not override a foreground it cannot improve on.
- **`None` (undetermined):** set a self-contained pair (`fg = base05`, `bg =
  base01`) so the element is legible without depending on the terminal's
  foreground.

So inline code never renders as background-only-with-inherited-foreground when
the scheme is unknown (the cause of bug 1), and when the scheme is known it
honors the user's foreground.
A user override that sets `bg` without `fg` is accepted as an explicit escape
hatch and is not auto-completed.

### Configuration surface

`style` is redesigned around a node-major layout.
Each presentation *node* owns its behavior and its visual *elements*; two global
sections hold the cross-cutting concerns.

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

Rules:

- **`fg`/`bg` value grammar**, consistent with [RFD 084]: a palette role
  (`"base0D"`), a hex literal (`"#fb4934"`), an ANSI-256 index (`244`),
  `"default"` (reset the channel to the terminal default, SGR 39/49), or
  `"inherit"` (suppress theme/renderer defaults without resetting, so the
  surrounding context shows through).
  `"inherit"` is how the stylesheet expresses the readability rule's "honor the
  user's foreground."
- **Visual styling for every node lives under `style.<node>.elements.*`**,
  generalizing [RFD 084]'s `style.markdown.elements.*`.
  Behavioral knobs stay on the node (`style.markdown.wrap_width`,
  `style.tool_call.parameters`).
- **Theme is global.** It moves from `style.markdown.theme` to `style.theme`,
  because post-D57 it drives chrome and tool-call colors, not only markdown.
  `default = "auto"` is what makes the absence of an explicit choice trigger
  scheme-based selection; `dark`/`light` name the theme used for each scheme and
  may reference a built-in or a custom palette.
  `style.inline_code` folds into `style.markdown.elements.inline_code`.
- **`style.terminal`** holds only rendering-target capability overrides, so it
  sits as a sibling to any future frontend's target section rather than being a
  catch-all.

This is a deliberate breaking change to the `style` config, accepted as part of
accepting this RFD.
Old keys that no longer exist will not be silently ignored (config structs deny
unknown fields), so the rename is handled explicitly: [RFD 084]'s
deprecation-alias phase for `style.inline_code.background` is dropped in favor
of this clean rename, and the implementation maps the known old keys during
load.
D57 and [RFD 084] share this single target shape.

### Data flow

```
jp_md     -->  markdown scopes resolved to truecolor  -+
chrome    -->  logical Style / chrome scope            +-->  per-sink adapter  -->  bytes
tool/raw  -->  relayed bytes                           -+
```

The per-sink adapter (behind both `Printer::print` and the borrowed
`out_writer`/`err_writer` handles): resolves chrome scopes, adapts each color to
`ColorProfile.depth`, applies the category-based escape policy, and gates on the
target (styled vs non-styled).

### Detection and startup order

Detection lives in `jp_term` and produces a `ColorProfile`.
Depth and scheme are terminal-wide; whether color is emitted on a given handle
is per `PrintTarget` (`Out`, `Err`, `Tty`), based on that handle's own TTY
state.
So `jp query > file` still colors chrome on a TTY stderr, and the structured
channel always runs non-styled.

This is a deliberate amendment to [RFD 048]: the *serialization format*
(text/json) still resolves once from stdout per RFD 048, but the *color
decision* is now per-target rather than following the global format.
In `--format json`, both streams emit NDJSON and are never styled, regardless of
color env vars or TTY state.

Because `style.terminal.*` has the highest precedence, the `ColorProfile` is
resolved in `jp_cli` *after* config loads (merging config overrides with
`jp_term` detection) and installed into the sink before any command rendering
begins.
Output emitted before that point (final error printing, bootstrap messages
already outside the configured `Printer`) stays un-themed; those direct writes
are out of scope (see The terminal sink).

Precedence:

- **Emit color on a target:** explicit config -\> `NO_COLOR` (off) -\>
  `CLICOLOR_FORCE` (on, even when not a TTY) -\> handle is a TTY (on) -\> off.
- **Depth (terminal-wide):** explicit config -\> `COLORTERM` truecolor -\>
  `TERM` `*-256color` -\> colorful `TERM` (16).
- **Scheme (terminal-wide):** explicit config -\> `COLORFGBG` -\> best-effort
  OSC 11 query -\> `None`.

`CLICOLOR_FORCE` precedes the TTY check, so it can force color into a pipe;
`NO_COLOR` still wins over it.
Apple Terminal needs no special-casing: no `COLORTERM` plus
`TERM=xterm-256color` lands it on `Ansi256`.

### Reusing the ecosystem

The mechanical substrate is largely already in `Cargo.lock` via the `anstyle`
family (pulled through clap), all MIT/Apache and already audited:

- **`anstyle`** is the color model `jp_style` wraps, so the rest composes with
  no adapter code.
  This amends [RFD 084], which currently defines a bespoke `Color` enum; the
  enum maps 1:1.
- **`anstyle-query`** provides the `COLORTERM`/`TERM`/`NO_COLOR`/`CLICOLOR`
  heuristics.
- **`anstyle-parse` / `vte`** provide the SGR/escape parsing for the per-sink
  adapter.

Two small additions: **`anstyle-lossy`** (MIT/Apache, `const fn`
truecolor-to-256-to-16 math) and **`termbg`** (light/dark detection).
The base16 *format* is parsed in-house (a few dozen lines of serde over 16 hex
strings); we deliberately do not pull `tinted-builder` (GPL-3.0) or
`ansi_colours` (LGPL).

## Drawbacks

- **The scope taxonomy is a public contract.** Once a user writes
  `tool_call.function_name.fg = "..."`, that name is fixed forever ([Hyrum's
  Law]).
  The taxonomy must grow deliberately, not churn.
- **The per-sink adapter reparses output to quantize.** Adapting truecolor to a
  lower tier means parsing the stream per write.
  Mitigated by a memoized RGB-to-index cache keyed on the active `ColorProfile`.
- **It amends [RFD 084] and [RFD 048] while both interact.** 084's "SGR
  construction stays in `jp_md`" narrows to "`jp_md` resolves markdown scopes to
  canonical truecolor; the terminal sink owns tier adaptation," and 084's
  deprecation-alias phase is dropped in favor of the clean `style` rename. 048's
  global color behavior becomes per-target.
  The RFDs must land coherently.
- **Breaking the `style` config** resets users' existing style customizations on
  upgrade and requires the explicit key migration noted above.
  Accepted as part of this RFD.
- **Curated per-tier light/dark default themes are real work**, distinct from
  auto-downgrading an arbitrary theme.

## Alternatives

- **Set the inline-code foreground from the theme unconditionally.** Rejected as
  the *default*: it overrides a foreground the user chose even when the scheme
  is correctly detected.
  Adopted only as the unknown-scheme fallback.
- **Tag each print with a `PrintOrigin` (JP chrome / markdown / tool / ...).**
  Rejected: it pushes a per-call-site classification onto every `Printer`
  caller, inviting "you used it wrong" bugs.
  The category-based sink policy needs no provenance and keeps callers unaware.
- **Sanitize tool output at the call site before printing.** Rejected for the
  same reason: it is caller awareness by another name.
  Safe-by-default in the sink is the only approach that keeps borrowed-writer
  callers unaware.
- **Downgrade at each emission seam instead of in the sink.** Rejected: color is
  emitted from the 084 stylesheet, the syntect path, and `jp_cli` chrome; a
  single sink covers all three and is the natural per-frontend boundary.
- **Default to 256 or base16 always.** Rejected: throws away color quality on
  terminals that do support truecolor.
- **Aspect-major config (`style.terminal.elements.<node>`).** Rejected: splits a
  node's config across two trees and contradicts [RFD 084]'s shipped
  `style.markdown.elements.*`.
- **Adopt `termprofile` as the all-in-one engine.** Rejected for now: it pulls
  the heavy `palette` dependency, while `anstyle-query` + `anstyle-lossy` cover
  the same ground.
  Worth reading as a reference.

## Non-Goals

- **Building the web/TUI frontends or a generic frontend/sink abstraction.** D57
  ships the terminal sink and `jp_style`.
  The generic abstraction is introduced when a second frontend exists; defining
  it now, with one implementation, would be speculative.
- **A full chrome scope taxonomy.** This RFD ships `markdown.*` (via [RFD 084])
  and `tool_call.*` scopes; other chrome comes in follow-on RFDs against the
  same engine.
- **Palette generation / LAB interpolation** (the base16-to-256-cube technique
  from the [color256] writeup).
  Curated defaults need no LAB.
- **Converting behavioral style sections onto the `Style` type.** Adding an
  `elements` tree to a node does not change its behavioral config (`hidden`,
  `parameters`, reasoning display mode).
- **Replacing the `syntect` theme.** Code blocks stay themed by `syntect`; only
  their emitted colors flow through the same depth downgrade.
- **Perfect scheme detection.** Best-effort plus a config override is the
  contract; `None` is a first-class outcome.
- **A complete tool-emitted-escape trust policy.** D57 sets the default sink
  behavior; the fuller policy is [RFD 075]'s.

## Risks and Open Questions

- **Unknown-scheme fallback.** With `scheme: Option<Scheme>`, the readability
  rule has an explicit `None` path that picks a self-contained pair, so the
  legibility risk is removed.
  The residual question is the exact default pair and keeping the set of
  background-bearing default scopes small.
- **Scheme query reliability.** `termbg`/OSC 11 put the terminal into raw mode
  and read a reply.
  This runs once at startup, before any background work or streaming, so it is
  isolated.
  The only requirement is a short timeout with a clean fallback to `None`, and
  skipping the query when stdout is not a TTY.
- **Cache invalidation.** The memoized RGB-to-index cache must key on the active
  `ColorProfile` so a mid-session change does not serve stale colors.
- **`anstyle` version coupling.** `jp_style` wrapping `anstyle` ties us to its
  (stable, clap-maintained) API; acceptable given it is already a transitive
  dependency.

## Implementation Plan

### Phase 1: `ColorProfile` and the terminal sink

- Add `ColorProfile { depth, scheme: Option<Scheme> }` and detection in
  `jp_term` (`anstyle-query` + `TERM`; `scheme` stubbed to `None` for now).
  Resolve the profile in `jp_cli` after config loads and install it into the
  sink before rendering, with per-`PrintTarget` color gating.
- Extend the per-sink adapter (the `AnsiStripper`/`Sink` layer, persistent
  `vte::Parser`) to adapt SGR depth via `anstyle-lossy`, apply the
  category-based escape policy, and run non-styled channels at strip-all.
  Add the trusted out-of-band control path for JP's progress/cursor sequences.
  The adapter covers both `Printer::print` and borrowed
  `out_writer`/`err_writer` handles.
  Tests cover split CSI sequences and typewriter output.
- `jp_md` and chrome emit canonical truecolor; the sink adapts on the way out.
  Add `anstyle-lossy`.
- **Fixes bug (2)** (garbled tool output on non-truecolor terminals).
  The only new config is `style.terminal.color_depth`.

### Phase 2: `jp_style`, themes, and scheme selection

- Add the `jp_style` crate (`Style`, `Palette`, `Scope`, stylesheet resolution,
  wrapping `anstyle::Color`), an in-house base16 loader, and curated light/dark
  default themes.
  Align `jp_md`'s logical `Color` with `anstyle` (amends [RFD 084]).
- Add scheme detection (`termbg` + `COLORFGBG`) producing `Option<Scheme>`, and
  scheme-driven default theme selection.
  Apply the hybrid readability rule.
- Move `theme` to the global `style.theme` with `default = "auto"`.
- **Fixes bug (1)** (inline-code readability).
  Depends on Phase 1.

### Phase 3: Node-major config surface and chrome scopes

- Land the node-major `style` layout: global `style.theme` and `style.terminal`,
  per-node `style.<node>.elements.*`.
  Land [RFD 084]'s `style.markdown.elements.*` on this engine (084 amended to
  drop its alias phase), and add `tool_call.*` scopes.
  Implement the `fg`/`bg` grammar and the theme resolution model (`name -> {
  palette, syntect }`).
- This is a breaking change to `style`; the load path maps the known old keys
  and documents the new shape.
  The full scope taxonomy beyond markdown + tool\_call is deferred to follow-on
  RFDs.
  Depends on Phase 2.

## References

- [RFD 004]: the custom `jp_md` terminal renderer this builds on.
- [RFD 048]: the four-channel output model.
  D57 reuses its per-target channels and amends its global color behavior to be
  per-target.
- [RFD 075]: tool sandbox and access policy.
  D57 sets the default sink sanitization of tool-emitted escapes; a future RFD
  075 revision may define a fuller trust-boundary policy.
- [RFD 084]: configurable markdown element coloring.
  D57 extends it: it reuses the `Style`/stylesheet design, narrows the
  SGR-emission boundary to the sink, drops the alias phase, and shares the
  node-major `style` shape.
  The `Extended by` back-link on 084 is filled when this draft is promoted.
- [RFD 096]: terminal output sanitization for untrusted content.
  It neutralizes untrusted content where it enters the render pipeline; this
  draft's terminal sink adapts already-trusted styling downstream of it.
  The untrusted-content SGR allowlist is owned by RFD 096, not the sink, so the
  sink must not grow a second untrusted-content policy.
- [color256]: terminal 256-color palette generation from base16 (informative).
- [`anstyle`], [`termbg`]: the crates this RFD reuses or adds.
- [base16 styling spec]: the palette role vocabulary and file format.

[Hyrum's Law]: https://www.hyrumslaw.com/
[NO_COLOR]: https://no-color.org/
[RFD 004]: ../004-streaming-md-parser-renderer.md
[RFD 048]: ../048-four-channel-output-model.md
[RFD 075]: ../075-tool-sandbox-and-access-policy.md
[RFD 084]: ../084-configurable-markdown-element-coloring.md
[RFD 096]: ../096-terminal-output-sanitization-for-untrusted-content.md
[`anstyle`]: https://crates.io/crates/anstyle
[`termbg`]: https://crates.io/crates/termbg
[base16 styling spec]: https://github.com/tinted-theming/home/blob/main/styling.md
[color256]: https://gist.github.com/jake-stewart/0a8ea46159a7da2c808e5be2177e1783
