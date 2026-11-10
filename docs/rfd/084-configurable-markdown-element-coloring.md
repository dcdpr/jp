# RFD 084: Configurable Markdown Element Coloring

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-12
- **Extends**: [RFD 004]
- **Summary**: Introduces `MarkdownStyleSheet` in `jp_md` and
  `style.markdown.elements.*` settings for per-element terminal markdown
  styling.

## Summary

This RFD introduces a `MarkdownStyleSheet` in `jp_md` that maps markdown
elements (headings, list markers, inline code, blockquotes, links, emphasis, and
strikethrough) to structured `Style` values configured under
`style.markdown.elements.*`.
It replaces fixed SGR escape constants and one-off style parameters in the
terminal renderer with a stack-based stylesheet.

## Motivation

The terminal markdown renderer in `jp_md` hard-codes styling decisions in the
AST walker using fixed SGR constants from `ansi.rs`:

- Headings use `BOLD_START`/`BOLD_END` regardless of level.
- Heading `#` glyphs are emitted unstyled while the heading text is bold, with
  no way to style the marker and text separately.
- Bullet markers (`-`, `*`) and ordered-list markers are unstyled.
- Inline code supports a background override (via `inline_code_bg`), defaulting
  to the theme background, but has no foreground setting.
- Blockquote bodies use the syntect theme's gutter foreground, and the `>`
  marker cannot be styled separately from the body.
- Thematic breaks, links, task checkboxes, and table borders have no
  configurable styling.

The one configurable attribute, inline code background, required threading
`Option<(String, String)>` through `Formatter`, `TerminalFormatter`, the table
formatter, and the render-site wiring in `jp_cli::render::chat`.
Adding further attributes (inline code foreground, per-level heading colors, dim
bullets) in the same way grows the `Formatter` builder and the config schema by
one scalar per attribute.

A structured style representation also simplifies state restoration in the
renderer.
`AnsiState` already tracks bold, italic, underline, strikethrough, `fg`, and
`bg` across wrap breaks (commit `b5bb5eff`), but the renderer only special-cases
a single snapshot/restore for blockquote `fg`.
A push/pop style stack unwinds overlapping element styles in order without
duplicating snapshot logic per element.

## Design

### User-facing configuration

The `style.markdown.elements` config table maps each element slot to a `Style`
value with `fg`, `bg`, `intensity`, `italic`, `underline`, and `strikethrough`
attributes:

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

[style.markdown.elements.math]
fg = "yellow"

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

Every slot and every attribute within a slot is optional.
Omitting an attribute leaves theme-derived defaults and renderer baselines in
effect, falling back to the surrounding context only when neither applies (see
"Effective style resolution").

`intensity` accepts `"normal"`, `"bold"`, `"dim"`, or `"inherit"`.
Because bold (SGR 1) and dim (SGR 2) share a single SGR slot cleared by SGR 22,
intensity is modeled as four mutually exclusive states rather than independent
booleans (see "Style composition").

`fg` and `bg` accept a color value (an ANSI 256-color index such as `244`, a
standard named ANSI color such as `"yellow"` or `"bright_yellow"` mapping to
palette indices `0` to `15`, or a hex RGB string such as `"#3c3836"`) or one of
two literal strings: `"default"` resets that channel to the terminal's own
default (SGR 39 / 49), and `"inherit"` (also accepted on `intensity`) suppresses
theme-derived and renderer defaults without resetting the channel, so the
surrounding context shows through.

The existing `style.inline_code.background` key becomes a deprecated alias for
`style.markdown.elements.inline_code.body.bg` (see Drawbacks and Phase 3).

### `jp_md` API

`jp_md::style` defines the stylesheet types:

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

/// Intensity is a single SGR slot (bold = SGR 1, dim = SGR 2, normal = SGR 22).
/// Terminals cannot render bold and dim simultaneously, so the values are
/// modeled as one enum rather than independent booleans.
///
/// [`Intensity::Inherit`] is an *explicit* opt-out from the renderer baseline
/// (e.g. heading-bold) without committing to a specific intensity — the
/// surrounding context's intensity is preserved.
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
/// defaults and renderer baselines still apply.
/// The variants below are explicit overrides:
///
/// - [`ColorOverride::Inherit`] bypasses theme / renderer defaults and takes
///   the surrounding context's color.
/// - [`ColorOverride::Default`] resets the channel to the terminal's own
///   foreground / background (SGR 39 / 49).
/// - [`ColorOverride::Color`] is an explicit color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorOverride {
    Inherit,
    Default,
    Color(Color),
}

/// Logical color: ANSI 256-color index or 24-bit RGB.
/// Owned by `jp_md` so SGR escape construction stays inside the renderer crate
/// (see "Effective style resolution" for the boundary rationale).
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
    pub math: Style,
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

`MarkdownStyleSheet::default()` produces an empty override sheet where every
slot is `Style::default()` and every attribute is `None`.
Theme-derived and renderer-baseline defaults inside `jp_md` preserve the
existing output when no overrides are set (see "Effective style resolution").

`Formatter` exposes a builder method for the stylesheet:

```rust
impl Formatter {
    pub fn style_sheet(mut self, sheet: MarkdownStyleSheet) -> Self { ... }
}
```

`Formatter::inline_code_bg` is removed in favor of
`MarkdownStyleSheet::inline_code.body.bg`.
The CLI helper bridges `style.inline_code.background` into the stylesheet so
existing configurations continue to work without the legacy builder (see Phase
1).

### Effective style resolution

`MarkdownStyleSheet` carries overrides only; `jp_md` owns the theme defaults and
renderer baselines.
At render time, the effective style for each element is resolved in three steps:

1. **Theme defaults**: theme-derived values such as the inline-code background
   (`theme_bg(theme)`) and the blockquote-body foreground
   (`theme_blockquote_fg(theme)`), resolved from the `syntect::Theme` inside
   `jp_md`.

2. **Renderer baseline**: hard-coded baseline attributes (headings bold,
   `strong` bold, `emph` italic, `strikethrough` struck, `underline` underlined)
   that the AST walker previously emitted as fixed SGR pairs.

3. **User overrides**: values set on the `MarkdownStyleSheet` passed via
   `Formatter::style_sheet(...)`.
   `ColorOverride::Inherit` and `Intensity::Inherit` short-circuit steps 1 and 2
   for the affected field, suppressing the theme-derived default and the
   renderer baseline so the surrounding context shows through.

Steps 1 and 2 run entirely inside `jp_md`.
`jp_cli` constructs only step 3 (a sparse override sheet from user config) and
never inspects the resolved theme or the renderer baseline.

SGR escape construction also stays inside `jp_md`.
`jp_md` owns `Color` and `ColorOverride`, `jp_cli` converts from
`jp_config::types::color::Color` in the shared CLI helper (see "Translation in
`jp_cli`"), and `push_style` generates SGR parameters at write time.
To keep all SGR construction inside `jp_md`, `DefaultBackground` (in
`jp_md::format`, used for `style.reasoning.background`) replaces its pre-built
`param: String` field with a logical `Color`, and the `jp_cli` helper
`color_to_bg_param` is removed.
Folding `reasoning` styling onto `Style` itself remains a non-goal (see
Non-Goals).

### Style composition

Within a rendering context, attributes compose with surrounding styles as
follows:

- **`None` (key omitted)**: uses the theme-derived default (step 1) or the
  renderer baseline (step 2), or preserves the surrounding context if neither
  applies.
- **`Some(Inherit)`**: skips steps 1 and 2 for `intensity`, `fg`, or `bg` and
  preserves the surrounding context's value (for example, suppressing a
  theme-derived inline-code background or baseline heading bold without setting
  a replacement value).
- **`italic` / `underline` / `strikethrough`**: independent booleans.
  `Some(true)` enables the attribute and `Some(false)` disables it regardless of
  the surrounding context.
  `pop_style` restores the prior `AnsiState` snapshot.
- **`intensity`**: a single SGR slot shared by SGR 1 (bold), SGR 2 (dim/faint),
  and SGR 22 (normal, which resets both).
  `Some(Intensity::Bold)` and `Some(Intensity::Dim)` set the slot;
  `Some(Intensity::Normal)` clears an active bold or dim from the surrounding
  context.
  `pop_style` restores the surrounding intensity, re-emitting SGR 1 or SGR 2
  after an SGR 22 transition when needed.
- **`fg` / `bg`**: innermost-wins.
  `Some(ColorOverride::Color(c))` sets the channel to `c`, and
  `Some(ColorOverride::Default)` resets the channel to the terminal default (SGR
  39 / 49), overriding any theme-derived default from step 1.
  `pop_style` restores the surrounding color via the `attrs.foreground` /
  `attrs.background` snapshot.

For example:

1. Inline code (`intensity: Some(Normal)`, `fg: Some(Color(magenta))`) inside an
   `h1` heading (`intensity: Some(Bold)`, `fg: Some(Color(red))`) renders as
   non-bold magenta, then restores the heading's bold red on exit for any
   trailing heading text.
2. Link text (`fg: Some(Color(blue))`, no `intensity`) inside `strong`
   (`intensity: Some(Bold)`) renders as bold blue, then restores the surrounding
   (or absent) foreground on exit while remaining bold.

#### Slot overlay contract

Some elements split styling across multiple slots that layer over each other or
the surrounding context:

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
| `code_fence`            | ` ` \`\`` glyphs and info string                                     | surrounding context                                                                          |

### Renderer changes

`AnsiState` (`crates/jp_term/src/ansi.rs`) replaces `bold: bool` with
`intensity: Intensity`.
`restore_sequence` re-emits SGR 1 or SGR 2 after any SGR 22 transition, so
wrap-break and `pop_style` restoration preserve `Intensity::Dim` alongside
`Intensity::Bold`.

In `TerminalFormatter`, hard-coded `write_escape(BOLD_START)` /
`write_escape(BOLD_END)` pairs are replaced by a style stack on
`TerminalWriter`:

```rust
impl TerminalWriter<'_> {
    pub(crate) fn push_style(&mut self, style: &Style) -> fmt::Result;
    pub(crate) fn pop_style(&mut self) -> fmt::Result;
}
```

`TerminalWriter` maintains a `Vec<AnsiState>` snapshot stack, generalizing the
blockquote `fg` snapshot/restore pattern.
`push_style` records the current `attrs`, applies `style` on top, and emits the
resulting SGR escape.
`pop_style` restores the snapshot and emits the SGR sequence needed to
transition back.

The AST walker pushes and pops styles per element:

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

`HeadingStyles::for_level(u8) -> &Style` maps levels `1..=6` to `h1` through
`h6`, falling back to `h6` for any other `u8` value from comrak's
`NodeHeading::level`.
`format_item` (bullet and ordered markers vs. body), `format_code`
(`inline_code.body` and `inline_code.delim`), and `format_block_quote`
(`blockquote.marker` and `blockquote.body`) follow the same pattern.

Soft-wrapped continuation lines also require styled prefixes.
`TerminalWriter` restores `fg` and `bg` across line breaks through `AnsiState`,
but `TerminalWriter::prefix` is a flat `String` written verbatim at the start of
each continuation line, which would render wrapped blockquote `>` glyphs with
`blockquote.body` instead of `blockquote.marker`.
To preserve the marker/body split across wraps, `prefix` becomes a
`Vec<PrefixSpan>` where each span carries an optional `Style`, and
`write_prefix` wraps each span in `push_style` / `pop_style`.
`format_block_quote` pushes a ` >  ` span styled with `blockquote.marker`, while
list items push unstyled indentation spans.

### Renderer data flow

Four rendering paths consume `MarkdownStyleSheet`:

1. **AST walker (`format_terminal` / `format_terminal_with`)**:
   `TerminalFormatter` holds the stylesheet and styles headings, lists,
   blockquotes, inline code, math, strong, emphasis, underline, strikethrough,
   links, and thematic breaks through `push_style` / `pop_style`.
   Comrak's `math_dollars` extension is enabled so `$..$` and `$$..$$` parse as
   `NodeValue::Math`, and `Buffer`'s inline ground-state scanner tracks `$`
   delimiters so streaming paragraphs hold open math spans across chunk
   boundaries.
2. **Streaming code fence helpers (`render_code_fence`, `render_closing_fence`,
   `render_code_line`)**: these helpers in `crates/jp_md/src/format.rs` (around
   `Formatter::render_code_*`) are called by `jp_cli::render::chat` during live
   streaming.
   They take the stylesheet so `code_fence` styling applies identically to
   streamed and buffered fenced code.
3. **Table renderer (`jp_md::table::format_table`)**:
   `crates/jp_md/src/table.rs` constructs a nested `TerminalFormatter` per cell.
   `format_table` receives the stylesheet and forwards it to each cell
   formatter, so `table.border`, `table.header`, and inline elements inside
   cells all use the same sheet.
4. **Tool-result renderer (`jp_cli::render::tool::ToolRenderer`)**:
   `crates/jp_cli/src/render/tool.rs` constructs its `Formatter` through the
   same shared CLI helper as `ChatRenderer`, and `ToolRenderer::render_result`
   migrates from manual ` ``` ` emission to `render_code_fence` /
   `render_closing_fence` alongside `begin_code_block` / `render_code_line`.
   Tool-result bodies are not parsed as markdown by this RFD; inline element
   styles (`strong`, `emph`, `link.*`, `blockquote.*`) apply to tool results
   only if a future change routes those bodies through `format_terminal`.

### Configuration types

`jp_config` gains:

- `style::Style`: a `Config`-derived struct mirroring `jp_md::Style` with
  optional `fg`, `bg`, `intensity`, `italic`, `underline`, and `strikethrough`
  fields.
  `fg` and `bg` deserialize from a color value (ANSI 256 number or hex string
  via `jp_config::types::color::Color`) or the literal strings `"default"` and
  `"inherit"`, producing a `ColorOverride`.
  `intensity` deserializes from `"normal"`, `"bold"`, `"dim"`, or `"inherit"`.
- `style::markdown::HeadingStylesConfig`: a `Config`-derived struct with six
  named `Style` fields (`h1` through `h6`) mirroring `HeadingStyles` in `jp_md`.
  Fixed fields avoid runtime range validation; unknown segments such as `h0`,
  `h7`, or `foo` are rejected by the assignment glue as missing keys.
- Grouped sub-style configs mirroring their `jp_md` counterparts, with `Style`
  leaf fields: `InlineCodeStylesConfig` (`body`, `delim`),
  `BlockquoteStylesConfig` (`marker`, `body`), `TableStylesConfig` (`border`,
  `header`), `TaskCheckboxStylesConfig` (`base`, `checked`), and
  `LinkStylesConfig` (`text`, `url`, `delimiters`).
- `style::markdown::ElementsConfig`: the slot tree from `MarkdownStyleSheet`,
  mirrored with `Style` and grouped sub-style configs (`heading` and
  `heading_marker` use `HeadingStylesConfig`).
- `MarkdownConfig::elements`: an `ElementsConfig` field on `MarkdownConfig`,
  wired with the standard `PartialConfigDelta`, `FillDefaults`,
  `AssignKeyValue`, and `ToPartial` implementations (matching `MarkdownConfig`
  and `InlineCodeConfig`).

CLI assignment uses path segments per the `KvAssignment` convention:

```sh
jp query --cfg style.markdown.elements.heading.h1.fg="#fb4934"
jp query --cfg style.markdown.elements.heading.h1.intensity=bold
jp query --cfg style.markdown.elements.bullet_marker.fg="#83a598"
jp query --cfg style.markdown.elements.blockquote.marker.intensity=bold
jp query --cfg style.markdown.elements.link.delimiters.intensity=dim
```

### Translation in `jp_cli`

`formatter_from_config` (in `crates/jp_cli/src/render/chat.rs`, lifted into a
sibling module shared by `ChatRenderer` and `ToolRenderer`) is the single place
that translates `style.markdown.elements.*` (`jp_config::style::Style`,
including `Color` -> `jp_md::Color` and `"default"` -> `ColorOverride::Default`)
into a sparse `MarkdownStyleSheet` (step 3 of "Effective style resolution").
Both renderers construct their `Formatter` through this helper, receiving
identical stylesheets and themes, while theme defaults and renderer baselines
stay inside `jp_md`.

The legacy `inline_code_bg` branch moves into stylesheet construction,
`ChatRenderer::terminal_options` passes a logical `Color` to
`DefaultBackground`, and `crate::format::color_to_bg_param` is deleted.
After Phase 2, no `jp_cli` code path constructs SGR escapes for markdown element
styling or `DefaultBackground`.
Terminal chrome owned by `jp_cli` (role headers, the reasoning-timer line,
tool-call labels, and progress indicators) keeps its existing styling path.

## Drawbacks

- **Upfront diff size.** The change adds a new module in `jp_md`, refactors
  roughly 12 call sites in `render.rs`, and introduces config types with
  schematic glue and snapshot tests.
- **Stack discipline invariant.** Every `push_style` requires a matching
  `pop_style` across error paths and comrak's pre/post-order traversal, which is
  harder to audit visually than symmetric `START`/`END` constant pairs.
  A debug assertion in `finish()` verifies that the stack is empty on exit.
- **Style composition becomes a user-visible contract.** The rule for omitted
  keys (theme default -> renderer baseline -> surrounding context), the explicit
  `Inherit` opt-out for `intensity` and `fg` / `bg`, the four-state `intensity`
  semantics, and the innermost-wins / explicit-default rule for `fg` / `bg` (see
  "Style composition") become part of the config contract once shipped.
  Snapshot tests for nested elements (inline code in a heading, link in
  `strong`) pin this behavior.
- **Styled continuation prefixes.** Changing `TerminalWriter::prefix` from a
  flat `String` to a list of styled spans touches the line-wrapping path and
  rewrites prefix bookkeeping in `format_block_quote` and `format_item`, though
  the per-line cost is bounded by nesting depth.
- **Deprecated config key.** `style.inline_code.background` remains as a
  deprecated alias for `style.markdown.elements.inline_code.body.bg` (see Phase
  3) so existing config files, environment variables, `--cfg` assignments, and
  stored conversation deltas continue to load until hard removal in a follow-up.
- **Larger config schema.** `style.markdown.elements.*` adds roughly 30 leaf
  slots with 6 attributes each to the schema.

## Alternatives

### A. Incremental scalar knobs

Add builder methods such as `Formatter::heading_fg(level, color)`,
`Formatter::bullet_fg(color)`, and `Formatter::inline_code_fg(color)` on demand,
each with a matching `style.markdown.heading_h1_fg`-style config key.

Rejected because the Cartesian product (~30 leaf slots by ~6 attributes) makes
the builder and config schema unwieldy, and each addition touches `Formatter`,
`TerminalFormatter::new`, its `Debug` impl, the config schema, the assignment
glue, and `formatter_from_config`.

### B. Adopt `termimad`'s `MadSkin`

`termimad` models per-element terminal styling with `MadSkin`, `LineStyle`, and
`CompoundStyle`, which served as a reference for `Style` and
`MarkdownStyleSheet`.

Adopting `termimad` directly was rejected because it ships its own parser,
soft-wrapper, and table renderer without a streaming API, whereas `jp_md` is
comrak-based with ANSI-aware soft-wrapping, default-background fills for
reasoning blocks, OSC-8 file/copy links, and a streaming code-block API consumed
by `jp_cli::render::chat`.
Importing `termimad` for the `MadSkin` schema structs alone would add an
unnecessary crate dependency.

### C. Map directly onto the syntect theme

Reuse `syntect::highlighting::Theme` scopes (such as `markup.heading`) for
element coloring.
Rejected because syntect themes target tokenized source code rather than
block-level markdown: they have no scopes separating the `#` glyph from heading
text or styling bullet markers, and coupling markdown element appearance to code
syntax highlighting prevents configuring the two independently.

## Non-Goals

- **Replacing the syntect theme.** Code-block bodies continue to use the syntect
  theme selected via `style.markdown.theme`.
  The stylesheet affects only markdown elements (headings, lists, etc.), not
  tokens inside fenced code.
- **Migrating `tool_call`, `reasoning`, and other style sections to `Style`.**
  `Style` is reusable, but folding existing style sections into it is deferred
  to a follow-up.
- **Light/dark theme presets.** Users compose their own stylesheet; built-in
  presets are out of scope.
- **Terminal capability detection.** The renderer emits the configured SGR
  sequences (24-bit RGB, ANSI 256, italic, dim) as-is without probing terminal
  support.
- **Styling every comrak node.** Footnote definitions and references, wiki
  links, image delimiters, raw/HTML inline and block content, front matter, and
  escaped tags remain unstyled.

## Risks and Open Questions

- **Config key naming.** `style.markdown.elements` was chosen over
  `style.markdown.colors` (slots also control `intensity`, `italic`,
  `underline`, and `strikethrough`) and over flat per-element keys under
  `style.markdown.*` (which would conflict with scalar fields such as
  `wrap_width`, `theme`, and `hr_style`).
- **Stack-discipline regressions.** A `push_style` without a matching
  `pop_style` leaks styling into subsequent output.
  Mitigated by a debug assertion in `TerminalWriter::finish` that the stack is
  empty, plus per-element snapshot tests comparing exact ANSI byte sequences.

## Implementation Plan

The implementation splits into three reviewable phases shipped together so no
intermediate config surface is exposed.

### Phase 1: `Style` and the writer stack (internal)

- Add `jp_md::style` with `Style`, `Intensity`, `ColorOverride`, `Color`,
  `HeadingStyles`, `InlineCodeStyles`, `BlockquoteStyles`, `TableStyles`,
  `TaskCheckboxStyles`, `LinkStyles`, and `MarkdownStyleSheet` as sparse
  override types with optional fields and an empty `Default`.
- Replace `AnsiState::bold: bool` with `AnsiState::intensity: Intensity` and
  update `restore_sequence` / `update` to handle the SGR 22 bold/dim reset.
- Restructure `TerminalWriter::prefix` from `String` to `Vec<PrefixSpan>` (each
  span carrying an optional `Style`) so `blockquote.marker` reaches wrap-break
  continuation lines, and update `format_block_quote` and `format_item` to push
  styled and unstyled spans.
- Implement theme-derived and renderer-baseline style resolution inside `jp_md`
  (see "Effective style resolution").
  Existing snapshot tests must continue to pass byte-for-byte.
- Add `TerminalWriter::push_style` / `pop_style` with the `Vec<AnsiState>`
  snapshot stack and the empty-stack debug assertion in `finish()`.
- Refactor `TerminalFormatter`, the streaming code fence helpers
  (`render_code_fence`, `render_closing_fence`), and `table::format_table` to
  apply styles via `push_style` / `pop_style` instead of hard-coded `BOLD_START`
  / `BOLD_END` constants.
- Add the `Formatter::style_sheet` builder method, bridge
  `style.inline_code.background` into `MarkdownStyleSheet::inline_code.body.bg`
  inside `formatter_from_config`, and remove `Formatter::inline_code_bg`.

### Phase 2: Config surface

- Add `jp_config::style::Style`, the grouped sub-style configs
  (`InlineCodeStylesConfig`, `BlockquoteStylesConfig`, `TableStylesConfig`,
  `TaskCheckboxStylesConfig`, `LinkStylesConfig`), and `ElementsConfig`, and
  wire `ElementsConfig` into `MarkdownConfig` with `PartialConfigDelta`,
  `FillDefaults`, `AssignKeyValue`, and `ToPartial`.
- Lift `formatter_from_config` out of `jp_cli::render::chat` into a shared
  module used by both `ChatRenderer` and `ToolRenderer`, and translate
  `style.markdown.elements.*` into `MarkdownStyleSheet` alongside the Phase-1
  bridge for `style.inline_code.background`.
- Migrate `ToolRenderer::render_result` from manual ` ``` ` emission to
  `Formatter::render_code_fence` / `render_closing_fence` so `code_fence`
  styling reaches tool-result output.
- Migrate `DefaultBackground` (in `jp_md::format`) from `param: String` to
  `color: Color`, update `jp_cli::render::chat::terminal_options` to pass
  `Color` directly, and delete `jp_cli::format::color_to_bg_param`.
- Add snapshot tests for per-element rendering, soft-wrap tests verifying that
  `fg` colors and `blockquote.marker` survive line breaks, and a tool-result
  snapshot exercising `code_fence` styling.

Depends on Phase 1.

### Phase 3: Deprecate `style.inline_code.background`

Hard removal is out of scope for this RFD: stored conversation
`base_config.json` snapshots and event deltas already contain
`style.inline_code.background`, and
`jp_conversation::compat::strip_unknown_fields` strips fields absent from the
schema before deserializing, which would silently drop the override on load.
Instead, this phase replaces the Phase-1 CLI bridge with a config-layer alias:

- Keep `style.inline_code.background` as a deprecated schema field on
  `InlineCodeConfig` so `strip_unknown_fields` preserves it when loading stored
  conversations.
- During the config load / `FillDefaults` pass, copy the alias value into
  `style.markdown.elements.inline_code.body.bg` if the new key is unset, then
  clear the alias.
  This applies uniformly to TOML files, `--cfg
  style.inline_code.background=...`, `JP_CFG_STYLE_INLINE_CODE_BACKGROUND`, and
  stored conversation base configs and config delta events.
- Retire the Phase-1 `formatter_from_config` bridge for
  `style.inline_code.background`, since the value already lives at the new key
  when the formatter helper runs.
- Emit a `tracing::warn!` deprecation message naming the replacement key when
  the old key is observed in any source.
- Document the mapping in the change log (`style.inline_code.background` ->
  `style.markdown.elements.inline_code.body.bg` and
  `JP_CFG_STYLE_INLINE_CODE_BACKGROUND` ->
  `JP_CFG_STYLE_MARKDOWN_ELEMENTS_INLINE_CODE_BODY_BG`), update the schema
  snapshot, and update user documentation to reference the new key.

Hard removal of `InlineCodeConfig::background` is deferred to a follow-up RFD or
release once the alias has been in place long enough to migrate, with
`--cfg=NONE` (RFD 038) as the recovery path if broken config prevents JP from
starting.

Depends on Phase 2.

## References

- [RFD 004]: original decision to maintain a custom comrak-based terminal
  renderer in `jp_md`, which this RFD extends.
- `jp_md::render::TerminalFormatter`: the AST walker whose hard-coded SGR calls
  this RFD replaces.
- `jp_term::ansi::AnsiState`: the state-tracking primitive that the writer style
  stack builds on.
- `termimad`'s `MadSkin`: referenced as schema inspiration; not adopted.

[RFD 004]: 004-streaming-markdown-parser-and-terminal-renderer-in-jp_md.md
