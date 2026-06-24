# RFD 084: Configurable Markdown Element Coloring

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-12
- **Extends**: [RFD 004]

## Summary

Introduce a `MarkdownStyleSheet` in `jp_md` that maps the markdown elements the
terminal renderer emits (headings per level and their markers, list markers,
inline code, blockquote marker, links, emphasis, strikethrough, underline, …)
to a structured `Style` value, and expose it as user config under
`style.markdown.elements.*`.
This replaces today's hard-coded SGR escapes in the terminal renderer and the
one-off `inline_code_bg` plumbing with a single coherent abstraction.

## Motivation

The terminal markdown renderer in `jp_md` currently bakes every styling decision
into the AST walker as fixed SGR constants from `ansi.rs`:

- Headings use `BOLD_START`/`BOLD_END` regardless of level.
- The `#` glyphs are emitted unstyled while the heading text is bold; there is
  no way to style the marker independently of the text.
- Bullet markers (`-`, `*`) and ordered-list markers carry no styling.
- Inline code can override its background (via `inline_code_bg`) but not its
  foreground; the only colored aspect of inline code is the theme-derived
  background.
- Blockquote bodies pick up the syntect theme's gutter foreground, but the `>`
  marker can't be styled differently from the body.
- Thematic breaks, links, task checkboxes, and table borders have no
  user-controllable styling at all.

The one place we *did* add a knob — inline code background — required
threading `Option<(String, String)>` through `Formatter`, `TerminalFormatter`,
the table formatter, and the render-site wiring in `jp_cli::render::chat`.
Adding the next knob (inline code foreground, heading-level colors, dim bullets)
by the same pattern compounds the surface area linearly per attribute.
Users increasingly want per-element control; doing this piecemeal produces a
builder with dozens of scalar methods and a config schema that has to grow in
lockstep.

The renderer also has correctness work that a structured representation makes
easier: `AnsiState` already tracks bold, italic, underline, strikethrough, fg,
and bg across wrap breaks (see commit `b5bb5eff`).
What's missing is a push/pop discipline so that multiple overlapping element
styles can be unwound in order; the current code special-cases one
snapshot/restore for blockquote fg.
Generalizing it once (push/pop stack of element styles) costs less than
open-coding the same logic per element.

## Design

### What the user sees

A new `style.markdown.elements` subtree, where each leaf is a `Style` value with
`fg`, `bg`, `intensity`, `italic`, `underline`, and `strikethrough`:

```toml
[style.markdown.elements.heading.h1]
fg = "#fb4934"
intensity = "bold"

[style.markdown.elements.heading.h2]
fg = "#fabd2f"

[style.markdown.elements.heading_marker.h1]
fg = 244
intensity = "dim"

[style.markdown.elements.bullet_marker]
fg = "#83a598"

[style.markdown.elements.ordered_marker]
fg = "#83a598"
intensity = "bold"

[style.markdown.elements.inline_code.body]
fg = "#d3869b"
bg = "#3c3836"

# Suppress the theme-derived blockquote-body foreground without resetting
# the channel, so the surrounding context's foreground shows through.
[style.markdown.elements.blockquote.body]
fg = "inherit"

[style.markdown.elements.blockquote.marker]
fg = "#928374"
intensity = "bold"

# Dim the link framing characters; style label and URL separately.
[style.markdown.elements.link.delimiters]
intensity = "dim"
```

Every slot is optional, and every attribute within a slot is independently
optional.
Omitting an attribute means "no user override" — theme-derived defaults and
renderer baselines still apply (see "Effective style resolution"); only when
neither applies does the surrounding context show through.
Use `"inherit"` on `fg` / `bg` / `intensity` to explicitly suppress those
defaults and force the surrounding-context fallback.
Most users will only want to set one or two attributes per slot.

`intensity` accepts `"normal"`, `"bold"`, `"dim"`, or `"inherit"`.
Bold and dim share a single SGR slot (SGR 1 / 2, both cleared by SGR 22), so
they are modeled as four mutually-exclusive states rather than independent
booleans — see "Style composition" below.

`fg` and `bg` accept a color value (an ANSI 256-color index such as `244`, or a
hex RGB string such as `"#3c3836"`) or one of two literal strings: `"default"`
resets that channel to the terminal's own default (SGR 39 / 49); `"inherit"`
suppresses theme-derived and renderer defaults *without* resetting the channel,
so the surrounding context shows through.
Omitting the key keeps today's behavior — theme-derived defaults apply first,
then the renderer baseline, then the surrounding context.

The existing `style.inline_code.background` key becomes a deprecated alias for
`style.markdown.elements.inline_code.body.bg`; see Drawbacks and Phase 3.

### `jp_md` API

A new `jp_md::style` module:

````rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Style {
    pub fg: Option<ColorOverride>,
    pub bg: Option<ColorOverride>,
    pub intensity: Option<Intensity>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strikethrough: Option<bool>,
}

/// Intensity is a single SGR slot (bold = SGR 1, dim = SGR 2, normal =
/// SGR 22). Terminals cannot render bold and dim simultaneously, so the
/// values are modeled as one enum rather than independent booleans.
///
/// [`Intensity::Inherit`] is an *explicit* opt-out from the renderer
/// baseline (e.g. heading-bold) without committing to a specific
/// intensity — the surrounding context's intensity is preserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intensity {
    Inherit,
    Normal,
    Bold,
    Dim,
}

/// A color override for `fg` / `bg`.
///
/// `Option::None` at the field level means "no override" — theme-derived
/// defaults and renderer baselines still apply. The variants below are
/// explicit overrides:
///
/// - [`ColorOverride::Inherit`] bypasses theme / renderer defaults and
///   takes the surrounding context's color.
/// - [`ColorOverride::Default`] resets the channel to the terminal's
///   own foreground / background (SGR 39 / 49).
/// - [`ColorOverride::Color`] is an explicit color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorOverride {
    Inherit,
    Default,
    Color(Color),
}

/// Logical color: ANSI 256-color index or 24-bit RGB. Owned by `jp_md`
/// so SGR escape construction stays inside the renderer crate (see
/// "Effective style resolution" for the boundary rationale).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Ansi256(u8),
    Rgb { r: u8, g: u8, b: u8 },
}

#[derive(Debug, Clone, Default)]
pub struct HeadingStyles {
    pub h1: Style,
    pub h2: Style,
    pub h3: Style,
    pub h4: Style,
    pub h5: Style,
    pub h6: Style,
}

#[derive(Debug, Clone, Default)]
pub struct InlineCodeStyles {
    pub body: Style,        // code-span literal and span background
    pub delim: Style,       // backticks, overlays `body`
}

#[derive(Debug, Clone, Default)]
pub struct BlockquoteStyles {
    pub marker: Style,      // `>` glyph, including on wrapped continuation lines
    pub body: Style,        // quote text
}

#[derive(Debug, Clone, Default)]
pub struct TableStyles {
    pub border: Style,      // grid characters, padding, separator rows
    pub header: Style,      // header cell content only
}

#[derive(Debug, Clone, Default)]
pub struct TaskCheckboxStyles {
    pub base: Style,        // applies to both states
    pub checked: Style,     // overlays on top when checked
}

#[derive(Debug, Clone, Default)]
pub struct LinkStyles {
    pub text: Style,        // visible label
    pub url: Style,         // URL and title text
    pub delimiters: Style,  // `[`, `](`, ` "`, `"`, `)` inline; `<`, `>` autolinks
}

#[derive(Debug, Clone, Default)]
pub struct MarkdownStyleSheet {
    pub heading: HeadingStyles,
    pub heading_marker: HeadingStyles,
    pub bullet_marker: Style,
    pub ordered_marker: Style,
    pub task_checkbox: TaskCheckboxStyles,
    pub inline_code: InlineCodeStyles,
    pub code_fence: Style,              // ``` glyphs and info string
    pub blockquote: BlockquoteStyles,
    pub link: LinkStyles,
    pub thematic_break: Style,
    pub strong: Style,
    pub emph: Style,
    pub underline: Style,
    pub strikethrough: Style,
    pub table: TableStyles,
}
````

`MarkdownStyleSheet::default()` produces an empty override sheet — every slot
is `Style::default()` and every attribute is `None`.
Today's rendered output is preserved by the theme-derived and renderer-baseline
defaults `jp_md` already applies; see "Effective style resolution" below.

`Formatter` grows one builder method:

```rust
impl Formatter {
    pub fn style_sheet(mut self, sheet: MarkdownStyleSheet) -> Self { ... }
}
```

`Formatter::inline_code_bg` is removed; callers configure inline code background
through `MarkdownStyleSheet::inline_code.body.bg` instead.
The CLI helper that today reads `style.inline_code.background` is updated to
bridge that field into the stylesheet so existing user config keeps working
without the legacy builder (see Phase 1).

### Effective style resolution

The `MarkdownStyleSheet` carries *overrides only*.
`jp_md` owns the theme defaults and renderer baselines that make up today's
rendering.
At render time, the effective style for each element is composed in this order:

1. **Theme defaults.** Theme-derived values such as the inline-code background
   (`theme_bg(theme)`) and the blockquote-body foreground
   (`theme_blockquote_fg(theme)`).
   Only `jp_md` has access to the resolved `syntect::Theme`.

2. **Renderer baseline.** Hard-coded baseline attributes — headings bold,
   `strong` bold, `emph` italic, `strikethrough` struck, `underline` underlined
   — that the AST walker emits today as fixed SGR pairs.

3. **User overrides.** Values set in the `MarkdownStyleSheet` passed via
   `Formatter::style_sheet(...)`.

   `Inherit` overrides (both `ColorOverride::Inherit` and `Intensity::Inherit`)
   are an explicit short-circuit: they bypass steps 1 and 2 for the affected
   field, suppressing the theme-derived default and the renderer baseline
   without committing to a specific replacement.

Steps 1 and 2 live entirely inside `jp_md`.
`jp_cli` only constructs step 3 — a sparse override sheet from user config —
and never touches the resolved theme or the renderer baseline.
This keeps theme knowledge out of the crate boundary.

SGR escape construction also stays inside `jp_md`.
The `Color` and `ColorOverride` types above are owned by `jp_md`; `jp_cli`
converts from `jp_config::types::color::Color` at the construction boundary in
the shared CLI helper (see "Translation in `jp_cli`" below).
`push_style` generates the SGR parameters at write time — the today-only
`jp_cli` helper `color_to_bg_param` is removed.

For this boundary to actually hold, the reasoning-block background path is
migrated alongside: `DefaultBackground` (in `jp_md::format`) today carries a
pre-built SGR parameter populated by `color_to_bg_param` in `jp_cli`.
Its `param: String` field becomes a logical `Color`, and `jp_md` builds the
escape internally.
The user-visible `style.reasoning.background` knob keeps working unchanged;
folding `reasoning` styling onto the new `Style` type itself remains a non-goal
(see Non-Goals).

### Style composition

Within a single rendering context, attributes compose as follows:

- **`None` (key omitted) falls through defaults.** With no user override, the
  field picks up its theme-derived default (step 1 of effective style
  resolution), then the renderer baseline (step 2); if neither applies, the
  surrounding context is used.
- **`Some(Inherit)` is an explicit opt-out from defaults.** For both `intensity`
  and `fg`/`bg`, `Inherit` skips steps 1 and 2 of effective style resolution for
  that field and takes the surrounding context's value instead.
  Use this when you want to suppress a theme-derived inline-code background or a
  renderer-baseline heading-bold without committing to a specific replacement.
- **`italic` / `underline` / `strikethrough`** are independent booleans.
  `Some(true)` enables the attribute regardless of context; `Some(false)`
  disables it even if the surrounding context had it on.
  On `pop_style`, the prior `AnsiState` snapshot is restored.
- **`intensity` is enumerated, not two booleans.** SGR has a single intensity
  slot: SGR 1 (bold), SGR 2 (dim/faint), SGR 22 (normal, which resets both).
  `Some(Intensity::Bold)` or `Some(Intensity::Dim)` set the slot;
  `Some(Intensity::Normal)` clears it even if the surrounding context was bold
  or dim.
  On `pop_style`, the surrounding intensity is restored — emitting SGR 1 or SGR
  2 again if needed after an SGR 22 transition.
- **`fg` / `bg` are innermost-wins.** `Some(ColorOverride::Color(c))` sets the
  channel to `c`; `Some(ColorOverride::Default)` resets the channel to the
  terminal default (SGR 39 / 49), overriding any theme-derived default that step
  1 of resolution would otherwise apply.
  On `pop_style` the surrounding color is restored.
  This matches what the existing renderer already does via `attrs.foreground` /
  `attrs.background` snapshot/restore.

Two worked examples:

1. Inline code (`intensity: Some(Normal)`, `fg: Some(Color(magenta))`) inside an
   `h1` heading (`intensity: Some(Bold)`, `fg: Some(Color(red))`): the inline
   code renders non-bold magenta.
   On exit, the heading's bold red is restored for any trailing heading text.
2. Link text (`fg: Some(Color(blue))`, no `intensity`) inside `strong`
   (`intensity: Some(Bold)`): the link text renders bold blue.
   On exit, `strong` continues bold with the surrounding (or absent) foreground
   restored.

#### Slot overlay contract

A few slots compose with each other within the same element rather than with the
surrounding context.
The table below pins the contract so config behavior is stable before users
build themes against it.

| Slot                    | Applies to                                                           | Overlays                                                                                     |
| ----------------------- | -------------------------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| `inline_code.body`      | code-span literal and span background                                | surrounding context                                                                          |
| `inline_code.delim`     | opening / closing backticks                                          | `inline_code.body`                                                                           |
| `task_checkbox.base`    | `[ ]` / `[x]` marker (both states)                                   | surrounding context                                                                          |
| `task_checkbox.checked` | the marker when checked                                              | `task_checkbox.base`                                                                         |
| `blockquote.marker`     | `>` glyph (including on wrapped continuation lines)                  | surrounding context                                                                          |
| `blockquote.body`       | quote text                                                           | surrounding context (inline element styles overlay on top)                                   |
| `link.text`             | the visible label                                                    | surrounding context                                                                          |
| `link.url`              | URL and title text                                                   | surrounding context                                                                          |
| `link.delimiters`       | `[`, `](`, `  " `, `"`, `)` for inline links; `<`, `>` for autolinks | surrounding context                                                                          |
| `table.border`          | grid characters, padding, separator rows                             | surrounding context                                                                          |
| `table.header`          | header cell content only                                             | surrounding context (does not affect padding or separators, which stay under `table.border`) |
| `code_fence`            | ` ` \`\`\` glyphs and info string                                    | surrounding context                                                                          |

### Renderer changes

`AnsiState` (`crates/jp_md/src/ansi.rs`) grows an `intensity: Intensity` field
in place of its existing `bold: bool`.
`restore_sequence` re-emits SGR 1 or SGR 2 as appropriate after any SGR 22
transition, so wrap-break and pop-style restoration handle dim with the same
guarantees today's renderer offers for bold.
This is what allows `Intensity::Dim` to be a first-class attribute rather than a
layering hack.

The hard-coded `write_escape(BOLD_START)` / `write_escape(BOLD_END)` pairs in
`TerminalFormatter` are replaced by calls into a stylesheet-aware helper on the
writer:

```rust
impl TerminalWriter<'_> {
    pub(crate) fn push_style(&mut self, style: &Style) -> fmt::Result;
    pub(crate) fn pop_style(&mut self) -> fmt::Result;
}
```

Internally this is a `Vec<AnsiState>` snapshot stack.
`push_style` records the current `attrs`, applies the new style on top, and
emits the resulting SGR escape.
`pop_style` restores the snapshot and emits whatever combination of start/end
escapes (or full re-establish) is needed to transition.
This generalizes the existing blockquote fg push/restore pattern.

The AST walker calls into these per element:

```rust
// In format_heading:
let marker = self.sheet.heading_marker.for_level(level);
let text = self.sheet.heading.for_level(level);
self.writer.push_style(marker)?;
for _ in 0..nh.level { self.writer.output("#", false)?; }
self.writer.output(" ", false)?;
self.writer.pop_style()?;
self.writer.push_style(text)?;
// ... children rendered ...
// On exit: pop heading style.
```

`HeadingStyles::for_level(u8) -> &Style` is a small accessor that maps `1..=6`
to the corresponding named field.
Out-of-range levels (none should reach the renderer, but comrak's
`NodeHeading::level` is a `u8`) fall back to `h6`.

Same mechanism for `format_item` (bullet / ordered marker vs body),
`format_code` (`inline_code.body` and `inline_code.delim`), `format_block_quote`
(`blockquote.marker` and `blockquote.body`), etc.

The wrap-break path inside `TerminalWriter` restores fg/bg across line breaks
through `AnsiState`, but the prefix mechanism is a separate concern that the
stylesheet exposes for the first time.
`TerminalWriter::prefix` is currently a flat `String` written verbatim at the
start of each continuation line — so the `>` glyph on wrapped blockquote lines
would inherit body styling, not `blockquote.marker`.
To honor the marker / body split, `prefix` becomes a sequence of styled spans
(`Vec<PrefixSpan>`, each carrying an optional `Style`).
`write_prefix` walks the spans and emits `push_style` / `pop_style` around each.
`format_block_quote` pushes a ` >  ` span styled as `blockquote.marker`; list
items push their indentation span with no style.
The continuation-line styling now matches the first line by construction.

### Renderer data flow

The stylesheet has to reach every code path that emits styled output, not only
the AST walker.
Four paths need wiring:

1. **AST walker (`format_terminal` / `format_terminal_with`).** The stylesheet
   is held on `TerminalFormatter`.
   All AST elements — headings, lists, blockquotes, inline code, strong,
   emphasis, underline, strikethrough, links, thematic breaks — go through
   `push_style` / `pop_style` against it.
2. **Streaming code fence helpers (`render_code_fence`, `render_closing_fence`,
   `render_code_line`).** These live outside the AST walker
   (`crates/jp_md/src/format.rs`, around the `Formatter::render_code_*` block)
   and are consumed by `jp_cli::render::chat` for live streaming.
   They take the stylesheet so `code_fence` styling applies identically to
   streamed and buffered fenced code.
3. **Table renderer (`jp_md::table::format_table`).** Tables go through a nested
   `TerminalFormatter` per cell (`crates/jp_md/src/table.rs`).
   The stylesheet is threaded through `format_table` and forwarded to each cell
   formatter, so `table.border`, `table.header`, and inline element styles
   inside cells all use the same sheet.
4. **Tool-result renderer (`jp_cli::render::tool::ToolRenderer`).** Tool call
   results are rendered through a separately-constructed `Formatter`
   (`crates/jp_cli/src/render/tool.rs`) that calls `begin_code_block` /
   `render_code_line` and emits its own fence markers.
   The same shared CLI helper that produces `ChatRenderer`'s formatter produces
   this one, so `code_fence` styling and syntax highlighting are identical
   across chat output and tool-result output.
   Tool-result bodies are not parsed as markdown by this RFD; inline markdown
   element styling (`strong`, `emph`, `link.*`, `blockquote.*`, etc.) applies to
   tool results only if a future change routes those bodies through
   `format_terminal`.
   The current manual ` ``` ` emission in `ToolRenderer::render_result` migrates
   to `render_code_fence` / `render_closing_fence` so `code_fence` styling
   actually applies.

Without (2)–(4), schema slots like `code_fence`, `table.border`, and
`table.header` would be visible in config but never reach the output, and
buffered vs streamed vs tool-result rendering of the same fenced block could
diverge.

### Configuration types

`jp_config` gains:

- `style::Style` — a `Config`-derived struct mirroring `jp_md::Style`: `fg`,
  `bg`, `intensity`, `italic`, `underline`, `strikethrough`, each `Option<...>`.
  `fg` / `bg` deserialize from either a color value (ANSI 256 number or hex
  string via the existing `jp_config::types::color::Color`) or one of the
  literal strings `"default"` / `"inherit"`, producing a `ColorOverride`.
  `intensity` deserializes from `"normal"` / `"bold"` / `"dim"` / `"inherit"`.
- `style::markdown::HeadingStylesConfig` — a `Config`-derived struct with six
  named fields `h1` through `h6`, each a `Style`.
  Mirrors `HeadingStyles` in `jp_md`.
  Six fixed levels means no runtime range validation is needed; an unknown
  segment such as `h0`, `h7`, or `foo` is rejected by the assignment glue as a
  missing key.
- Grouped sub-style configs mirror their `jp_md` counterparts:
  `InlineCodeStylesConfig` (`body`, `delim`), `BlockquoteStylesConfig`
  (`marker`, `body`), `TableStylesConfig` (`border`, `header`),
  `TaskCheckboxStylesConfig` (`base`, `checked`), and `LinkStylesConfig`
  (`text`, `url`, `delimiters`).
  Each leaf field is a `Style`.
- `style::markdown::ElementsConfig` — the slot tree from `MarkdownStyleSheet`,
  mirrored with `Style` and grouped sub-style values; `heading` and
  `heading_marker` use `HeadingStylesConfig`.
- `MarkdownConfig` gains an `elements: ElementsConfig` field.

The shared CLI helper translates `jp_config::style::Style` values into
`jp_md::Style` at the boundary — including the `Color` → `jp_md::Color` and
`"default"` → `ColorOverride::Default` conversions.
SGR escape generation stays inside `jp_md`.

CLI assignment uses path segments per the existing `KvAssignment` convention:

```sh
jp query --cfg style.markdown.elements.heading.h1.fg="#fb4934"
jp query --cfg style.markdown.elements.heading.h1.intensity=bold
jp query --cfg style.markdown.elements.bullet_marker.fg="#83a598"
jp query --cfg style.markdown.elements.blockquote.marker.intensity=bold
jp query --cfg style.markdown.elements.link.delimiters.intensity=dim
```

The usual `PartialConfigDelta`, `FillDefaults`, `AssignKeyValue`, and
`ToPartial` glue follows the existing pattern (see `MarkdownConfig` and
`InlineCodeConfig` today).

`Style` is reusable: `tool_call`, `reasoning`, and other style sections that
currently roll their own attribute bags can migrate to it as a follow-up, but
that's out of scope for this RFD.

### Translation in `jp_cli`

A single shared helper (today: `formatter_from_config` in
`crates/jp_cli/src/render/chat.rs`, ~20 lines; lifted into a sibling module
shared by both renderers) becomes the only place that maps
`style.markdown.elements.*` config into a sparse `MarkdownStyleSheet` — the
user-override layer, step 3 of "Effective style resolution".
Both `ChatRenderer` and `ToolRenderer` construct their `Formatter` through this
helper, so they receive identical stylesheets and themes.
The helper does not resolve theme defaults or apply renderer baselines; those
stay inside `jp_md`.
The existing `inline_code_bg` branch is deleted; its logic moves into the
stylesheet construction.
The config-color-to-SGR translation (`crate::format::color_to_bg_param`) is also
deleted: `jp_md::Color` / `ColorOverride` are passed through and `jp_md` emits
the SGR.
The reasoning-background path constructs `DefaultBackground` with a logical
`Color` (in `ChatRenderer::terminal_options`); the SGR is built inside `jp_md`.
No `jp_cli` code path constructs SGR escapes for markdown element styling or
`DefaultBackground` after Phase 2 completes.
Other terminal chrome owned by `jp_cli` — role headers, the reasoning-timer
line, tool-call labels, progress indicators — keeps its existing styling path
and is out of scope for this RFD.

## Drawbacks

- **Upfront diff size.** New module in `jp_md`, refactor of ~12 call sites in
  `render.rs`, new config types with full schematic glue, new tests.
  Bigger than "add one more scalar knob."
- **Stack discipline becomes a correctness invariant.** Every `push_style` needs
  a matching `pop_style`, including in error paths and in the comrak
  pre/post-order traversal.
  Today's symmetric `START`/`END` pairs are easier to audit visually.
  A debug assertion in `finish()` that the stack is empty mitigates this but
  doesn't eliminate it.
- **Style composition becomes a user-visible contract.** The rule for omitted
  keys (theme default → renderer baseline → surrounding context), the explicit
  `Inherit` opt-out for both `intensity` and `fg` / `bg`, the four-state
  `intensity` semantics, and the innermost-wins / explicit-default rule for `fg`
  / `bg` (see "Style composition") are part of the config surface once shipped.
  Snapshot tests for nested cases (inline code in heading, link in strong) pin
  the behavior.
- **Prefix becomes styled.** `TerminalWriter::prefix` changes from a flat
  `String` to a list of styled spans so the blockquote `>` marker can be styled
  independently of body content on wrapped continuation lines.
  The per-line cost is small (prefix length is bounded by nesting depth × a few
  characters), but it touches a hot path and rewrites prefix bookkeeping in
  `format_block_quote` / `format_item`.
- **Deprecated config key.** `style.inline_code.background` becomes a deprecated
  alias for `style.markdown.elements.inline_code.body.bg` (see Phase 3).
  This RFD does not hard-break existing config files, environment variables,
  `--cfg` assignments, or stored conversation deltas; hard removal is deferred
  to a follow-up.
- **Config surface gets bigger.** `style.markdown.elements.*` is roughly 30 leaf
  slots × 6 attributes.
  Most users will set none of these and rely on defaults, but the schema is
  visibly larger.

## Alternatives

### A. Incremental scalar knobs

Add `Formatter::heading_fg(level, color)`, `Formatter::bullet_fg(color)`,
`Formatter::inline_code_fg(color)`, etc., one at a time as users ask.
Each with a matching `style.markdown.heading_h1_fg`-style config key.

Rejected because the Cartesian product (~30 leaf slots × ~6 attributes) makes
the builder unwieldy and the config schema visually noisy.
Each addition touches `Formatter`, `TerminalFormatter::new`, the `Debug` impl,
the config schema, the assignment glue, and the wiring in
`formatter_from_config`.
The piecemeal cost is acceptable for one knob; for twenty it is not.

### B. Adopt `termimad`'s `MadSkin`

`termimad` already solved the "per-element terminal styling" schema problem with
`MadSkin` / `LineStyle` / `CompoundStyle`.
Adopting it wholesale would replace `jp_md`'s renderer.

Rejected because termimad ships its own parser, its own soft-wrap, its own table
renderer, and no streaming API.
JP's renderer is comrak-based with ANSI-aware soft-wrap, default-background
fills for reasoning blocks, OSC-8 file/copy links, and a streaming code-block
API consumed by `jp_cli::render::chat`.
Replacing it is a rewrite, not a refactor.
Importing termimad for the `MadSkin` schema alone is poor coupling for a serde
struct.

The shape of `MadSkin` is, however, a useful reference for the `Style` and
`MarkdownStyleSheet` design above.

### C. Map directly onto the syntect theme

Reuse `syntect::highlighting::Theme` for element coloring (it already supports
named scopes like `markup.heading`).
Rejected because syntect themes have no mapping for "the `#` glyph vs the
heading text," no "bullet marker" scope, and are oriented around tokenized
source code, not block-level markdown.
Mixing the two namespaces would conflate "code syntax highlighting" with
"markdown element appearance" — they are independently meaningful and should
stay decoupled.

## Non-Goals

- **Replacing the syntect theme.** Code-block bodies continue to be styled by
  the syntect theme selected via `style.markdown.theme`.
  The stylesheet only affects markdown *element* styling (headings, lists,
  etc.), not the tokens inside fenced code.
- **Migrating `tool_call`, `reasoning`, and friends to `Style`.** The new
  `Style` type is reusable, but folding existing style sections into it is a
  follow-up.
  This RFD focuses on the markdown renderer.
- **Light/dark theme presets.** Out of scope.
  Users compose their own stylesheet; we may ship presets later but not in this
  RFD.
- **Terminal capability detection.** The stylesheet is applied as-is.
  Whether the terminal actually renders 24-bit RGB, ANSI 256, italic, dim, etc.
  is the terminal's problem.
  Detection is a separate concern.
- **Styling every comrak node.** Math, footnote definitions and references, wiki
  links, image delimiters, raw/HTML inline and block content, front matter, and
  escaped tags are passed through unstyled today and remain so.
  Slots can be added in a later RFD if a concrete need appears.

## Risks and Open Questions

- **Naming bikeshed.** `style.markdown.elements` vs `style.markdown.colors` vs
  per-element keys hanging directly off `style.markdown.*`.
  The proposal picks `elements` because "colors" undersells it (we also set
  intensity, italic, underline, strikethrough) and because flat per-element keys
  would conflict with existing scalar fields (`wrap_width`, `theme`,
  `hr_style`).
  Worth confirming.
- **Stack-discipline regressions.** A push without a pop would leak styling into
  following content.
  Mitigated by: debug-assert empty stack in `TerminalWriter::finish`, plus
  per-element snapshot tests that compare exact ANSI byte sequences (the
  renderer already has these for code blocks).

## Implementation Plan

Three phases, each reviewable as a logical slice, shipped together so no
intermediate config surface is exposed.

### Phase 1: `Style` and the writer stack (internal)

- Add `jp_md::style` with `Style`, `Intensity`, `ColorOverride`, `Color`,
  `HeadingStyles`, `InlineCodeStyles`, `BlockquoteStyles`, `TableStyles`,
  `TaskCheckboxStyles`, `LinkStyles`, and `MarkdownStyleSheet` as sparse
  override types — every field optional, `Default` empty.
- Replace `AnsiState::bold: bool` with `AnsiState::intensity: Intensity` and
  update `restore_sequence` / `update` to handle the SGR 22 bold / dim
  collision.
- Restructure `TerminalWriter::prefix` from `String` to `Vec<PrefixSpan>` (each
  span carrying an optional `Style`) so the `blockquote.marker` style reaches
  wrap-break continuation lines.
  Update `format_block_quote` and `format_item` to push styled / unstyled spans
  accordingly.
- Implement theme-derived and renderer-baseline style resolution inside `jp_md`
  (see "Effective style resolution").
  Existing snapshot tests must continue to pass byte-for-byte.
- Add `TerminalWriter::push_style` / `pop_style` with the `Vec<AnsiState>`
  snapshot stack and the debug-assert in `finish()`.
- Refactor `TerminalFormatter`, the streaming code fence helpers
  (`render_code_fence`, `render_closing_fence`), and `table::format_table` to
  consult the stylesheet via `push_style` / `pop_style` instead of hard-coded
  `BOLD_START` / etc.
- Add the `Formatter::style_sheet` builder method.
- Bridge the existing `style.inline_code.background` config field into
  `MarkdownStyleSheet::inline_code.body.bg` inside `formatter_from_config`, so
  removing `Formatter::inline_code_bg` doesn't regress current user config.
- Remove `Formatter::inline_code_bg`.

Reviewable in isolation.
No new config keys; existing `style.inline_code.background` keeps working
through the `formatter_from_config` bridge.

### Phase 2: Config surface

- Add `jp_config::style::Style` config struct (with the six optional attribute
  fields), the grouped sub-style configs (`InlineCodeStylesConfig`,
  `BlockquoteStylesConfig`, `TableStylesConfig`, `TaskCheckboxStylesConfig`,
  `LinkStylesConfig`), and `ElementsConfig`.
- Wire it through `MarkdownConfig` with the standard `PartialConfigDelta` /
  `FillDefaults` / `AssignKeyValue` / `ToPartial` glue.
- Lift `formatter_from_config` out of `jp_cli::render::chat` into a shared
  module reachable from both `ChatRenderer` and `ToolRenderer`, and have it also
  translate `style.markdown.elements.*` into the `MarkdownStyleSheet`.
  The Phase-1 bridge for the legacy `style.inline_code.background` key stays in
  place alongside.
  Construct `ToolRenderer`'s `Formatter` through the same helper.
- Migrate `ToolRenderer::render_result` from manual ` ``` ` emission to
  `Formatter::render_code_fence` / `render_closing_fence` so `code_fence`
  styling reaches tool-result output.
- Migrate `DefaultBackground` (in `jp_md::format`) from `param: String` to a
  logical `color: Color` field so SGR construction for the reasoning-background
  path lives inside `jp_md`.
  Update `jp_cli::render::chat::terminal_options` to pass `Color` directly.
- Delete `jp_cli::format::color_to_bg_param`.
  The stylesheet path already routes `Color` / `ColorOverride` through `jp_md`;
  with `DefaultBackground` migrated, no callers remain.
- Snapshot tests covering per-element rendering, plus soft-wrap tests that
  verify fg colors *and* the `blockquote.marker` style survive line breaks.
  Add a tool-result snapshot that exercises fenced-code styling.

Depends on Phase 1.

### Phase 3: Deprecate `style.inline_code.background`

Hard removal is out of scope for this RFD — stored conversation
`base_config.json` snapshots and event deltas already contain the old key, and
`jp_conversation::compat::strip_unknown_fields` strips anything not in the
schema before deserializing.
Removing the field would silently drop the override on load.
Instead, this phase promotes the Phase-1 CLI bridge into a proper config-layer
alias:

- Keep `style.inline_code.background` as a first-class but deprecated schema
  field on `InlineCodeConfig`.
  It must remain in the schema so `strip_unknown_fields` does not strip it from
  persisted conversation data before the alias logic runs.
- During the config load / `FillDefaults` pass, copy the alias value into
  `style.markdown.elements.inline_code.body.bg` if the new key is unset, then
  clear the alias.
  Applies uniformly to: TOML files, `--cfg style.inline_code.background=...`,
  `JP_CFG_STYLE_INLINE_CODE_BACKGROUND`, and stored conversation base configs /
  config delta events.
- With the config-layer alias in place, retire the Phase-1
  `formatter_from_config` bridge for `style.inline_code.background` — by the
  time the formatter helper runs, the value lives at the new key.
- Emit a `tracing::warn!` deprecation message when the old key is observed in
  any of those sources, naming the replacement key.
- Document the mapping in the change log:
  - `style.inline_code.background` →
    `style.markdown.elements.inline_code.body.bg`
  - `JP_CFG_STYLE_INLINE_CODE_BACKGROUND` →
    `JP_CFG_STYLE_MARKDOWN_ELEMENTS_INLINE_CODE_BODY_BG`
- Update the schema snapshot to reflect the deprecation, and update the docs to
  point users at the new key.

Hard removal of `InlineCodeConfig::background` is scheduled for a follow-up RFD
or release once the alias has been in place long enough to migrate.
At that point, `--cfg=NONE` (RFD 038) is the recovery path if broken config
prevents JP from starting.

Depends on Phase 2.

## References

- [RFD 004]: original decision to maintain a custom comrak-based terminal
  renderer in `jp_md`.
  This RFD extends that renderer's styling surface.
- `jp_md::render::TerminalFormatter` — the AST walker whose hard-coded SGR
  calls this RFD replaces.
- `jp_md::ansi::AnsiState` — the existing state-tracking primitive that the
  proposed writer stack builds on.
- `termimad`'s `MadSkin` — referenced as schema inspiration; not adopted.

[RFD 004]: 004-streaming-md-parser-renderer.md
