# RFD D48: Frontend-Neutral Styling and Terminal Color Adaptation

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-02
- **Extends**: [RFD 048], [RFD 004]
- **Summary**: Introduces a frontend-neutral semantic style model and adapts
  terminal color output in `jp_printer` to the active terminal profile.

## Summary

This RFD introduces a frontend-neutral semantic style model for JP and a
terminal color adapter for the CLI.
Each frontend resolves semantic styles into its native representation, and
`jp_printer` adapts ANSI color sequences to the active terminal profile.

## Motivation

JP conflates three distinct concerns:

1. **Semantic style**: UI elements such as inline code, tool names, role
   headers, progress labels, and markdown headings have semantic meaning
   independent of how they are colored.
2. **Frontend rendering**: the CLI emits ANSI escape sequences, a web UI uses
   CSS, a TUI uses widget styles, and a native app uses platform text
   attributes.
3. **Terminal capability**: a terminal may support only 256 or 16 colors (for
   example, Apple Terminal advertises `TERM=xterm-256color` and rejects
   truecolor), yet JP emits 24-bit truecolor sequences unconditionally.

Conflating these concerns causes two user-visible bugs:

- Inline code is unreadable on light terminals.
  `jp_md` sets a dark background for inline code while inheriting the terminal's
  black foreground, producing black text on a near-black background.
- Syntax-highlighted output breaks in 256-color terminals.
  Multiple components emit `38;2;r;g;b` and `48;2;r;g;b` truecolor sequences
  without checking what the terminal supports.

Fixing these bugs at individual call sites would leave every new styled element
prone to the same mistakes.
Conversely, the terminal sink cannot resolve semantic scopes itself: it receives
raw byte streams and cannot tell whether `\x1b[38;2;...]` came from inline code,
a tool name, a progress line, or relayed tool output.

Separating these responsibilities establishes a clear boundary:

> Renderers resolve semantic scopes into their frontend's native style
> representation.
> Terminal byte adaptation happens in the CLI sink.

## Design

### User-facing behavior

By default, JP adapts to both light backgrounds and 256-color terminals.
In Apple Terminal (`TERM=xterm-256color`), JP downgrades colors to the 256-color
palette instead of emitting raw truecolor sequences.
On a light terminal, inline code uses a readable foreground and background pair
instead of setting a dark background over the terminal's default text color.

Users can override terminal capability and theme selection:

```toml
[style.terminal]
color_depth = "auto" # auto | truecolor | 256 | 16 | none
background = "auto" # auto | light | dark | unknown

[style.theme]
mode = "auto" # auto | dark | light | <theme-name>
dark = "gruvbox-dark"
light = "gruvbox-light"
```

Visual styles are configured under `elements` tables by semantic scope:

```toml
[style.markdown.elements.inline_code.body]
bg = "base01"

[style.markdown.elements.inline_code.delim]
fg = "base04"

[style.tool_call.elements.function_name]
fg = "base0D"
intensity = "bold"
```

Behavioral settings remain at their existing paths:
`conversation.tools.*.style.parameters` does not move to
`style.tool_call.parameters`, because it controls how a specific tool's
arguments are formatted rather than a visual style scope.

### Architecture

The system has three layers.

#### 1. Shared semantic style

A new pure `jp_style` crate owns the frontend-neutral vocabulary:

- `Style`: foreground, background, intensity, italic, underline, strikethrough,
  and related text attributes.
- `Color`: palette role, ANSI-256 index, 24-bit RGB, terminal default, or
  inherit.
- `Palette`: base16-style roles such as `base00` through `base0F`.
- `Scope`: stable semantic paths such as `markdown.inline_code.body` and
  `tool_call.function_name`.
- Stylesheet resolution from scope plus theme to a `Style` value.

`jp_style` has no `syntect` dependency, performs no terminal detection, and
emits no ANSI sequences.
It can convert to and from `anstyle` color types, but it is not a terminal
renderer.

#### 2. Frontend renderers

Each frontend resolves semantic scopes while the underlying structure is still
known:

- The CLI markdown renderer in `jp_md` maps markdown AST nodes to markdown
  scopes.
  It also owns or receives the `syntect::Theme` for syntax highlighting, which
  is a code-rendering concern rather than part of `jp_style`.
- CLI chrome renderers in `jp_cli` map UI elements such as tool call headers,
  role headers, and progress labels to chrome scopes.
- Web, TUI, and native frontends map the same scopes to CSS classes or
  variables, widget styles, and platform text attributes.

Non-CLI frontends do not use `jp_printer` or consume ANSI strings from `jp_cli`.
When multiple frontends need to share rendering logic, JP will extract a
target-neutral view model or styled-span representation rather than routing
non-CLI frontends through terminal bytes.

#### 3. Terminal sink

`jp_printer` is CLI-specific: it receives formatted byte streams and adapts them
to each output target.

The sink is responsible for:

- enabling or disabling color per output target,
- downgrading SGR colors to the target's color depth, or stripping them when
  color is disabled,
- stripping unsafe control sequences from normal content while preserving
  allowed text attributes,
- reassembling escape sequences split across writes.

The sink does not resolve style scopes, interpret markdown or tool-call
semantics, select a `syntect` theme, or participate in web, TUI, or native
rendering.

### Terminal profile

Terminal capability detection lives in `jp_term`.

```rust
pub struct ColorProfile {
    pub depth: ColorDepth,
    pub scheme: Option<ColorScheme>,
}

pub enum ColorDepth {
    Truecolor,
    Ansi256,
    Ansi16,
    None,
}

pub enum ColorScheme {
    Light,
    Dark,
}
```

Color capability (`ColorProfile`) is detected once for the terminal, whereas
color emission is decided per output target so that redirecting stdout does not
disable color on stderr or `/dev/tty` prompts:

```rust
pub struct TargetColorProfile {
    pub color_enabled: bool,
    pub profile: ColorProfile,
}
```

`PrintTarget::Out`, `PrintTarget::Err`, and `PrintTarget::Tty` each receive
their own `TargetColorProfile`.

### Environment and config precedence

Per-target color enablement resolves in this order:

1. Explicit JP config or CLI override.
2. `NO_COLOR`: disables foreground and background color.
3. `CLICOLOR_FORCE`: enables color even when the target is not a TTY, unless
   `NO_COLOR` is set.
4. `CLICOLOR=0`: disables color.
5. Target TTY state: enabled when the target is a TTY, disabled otherwise.

Terminal-wide color depth resolves in this order:

1. Explicit `style.terminal.color_depth` setting.
2. `COLORTERM=truecolor` or `COLORTERM=24bit`: selects `Truecolor`.
3. `TERM` containing `256color`: selects `Ansi256`.
4. Any other color-capable `TERM` value: selects `Ansi16`.
5. Fallback: `None` (no color).

Terminal-wide background scheme resolves in this order:

1. Explicit `style.terminal.background` setting.
2. `COLORFGBG`, when present and parseable.
3. A short-timeout query against the controlling terminal, when available.
4. Fallback: `None` (unknown).

An unknown scheme (`None`) does not default to dark.
When the background scheme is `None`, any default style that sets a background
color must also set a contrasting foreground color, or omit the background
entirely.
This prevents a dark inline-code background from pairing with a light terminal's
black default text.

### Startup order

Because `Printer` depends on the loaded configuration, JP initializes terminal
styling in stages so early startup errors can still be printed:

1. Emit any pre-config bootstrap output as plain text or minimal unthemed ANSI.
2. Load configuration.
3. Detect TTY state for stdout, stderr, and the controlling terminal.
4. Resolve `ColorProfile` from configuration, environment variables, and
   terminal queries.
5. Configure `Printer` with the resolved per-target profiles.
6. Run the command.

To detect the background scheme, JP queries the controlling terminal
(`/dev/tty`) rather than stdout, which may be redirected to a pipe or file even
in an interactive session.

### Terminal byte adaptation

`jp_printer` extends the `AnsiStripper` model into an ANSI adapter with a
persistent `vte::Parser` per sink so escape sequences split across writes are
handled intact.

For normal content writes:

| Sequence category                             | Styled terminal target            | Non-styled target           |
| --------------------------------------------- | --------------------------------- | --------------------------- |
| SGR foreground/background color               | Adapt to target depth or strip    | Strip                       |
| SGR text attributes                           | Preserve where supported          | Strip for structured output |
| OSC 8 hyperlinks                              | Preserve for terminal text output | Strip                       |
| Cursor movement, erase, title, clipboard, etc | Strip from normal content         | Strip                       |

Cursor and erase sequences emitted by JP itself, such as progress-line redraws,
go through a dedicated control method on `Printer` rather than normal content
writes.
Stripping cursor controls from normal content ensures that relayed tool output
cannot manipulate the terminal; if JP accidentally writes a control sequence
through the normal path, it simply loses the redraw.

### Relationship to RFD 084

[RFD 084] defines configurable markdown element coloring inside `jp_md`.
This RFD preserves that model while adjusting the crate boundaries:

- `jp_md` resolves markdown element scopes, where the markdown AST context is
  available.
- `jp_md` emits canonical terminal styles for the CLI, while `jp_printer` adapts
  color depth to the target terminal.
- `jp_style` owns shared style types and palettes, whereas `syntect::Theme`
  stays in `jp_md`.
- Config loading maps the deprecated `style.inline_code.background` key to
  `style.markdown.elements.inline_code.body.bg`.

## Drawbacks

- **Public scope taxonomy**: Scope names become part of the user-facing
  configuration schema and must remain stable once shipped.
- **Sink parsing overhead**: `jp_printer` parses all terminal output with
  `vte::Parser`, adding runtime cost on output with frequent color changes such
  as syntax-highlighted code.
- **Cross-crate footprint**: The change spans `jp_style`, `jp_term`,
  `jp_printer`, `jp_md`, `jp_cli`, and `jp_config`.
- **Theme curation**: Automatic color-depth downgrading does not make a dark
  palette readable on a light background; both light and dark default themes
  must be curated.
- **Conversation config migration**: Renaming config keys affects persisted
  conversation configs and config deltas in addition to user TOML files.

## Alternatives

### Set an inline-code foreground only

Setting an explicit foreground color whenever inline code sets a background
would fix the immediate readability bug on light terminals, but it leaves
256-color terminals broken and keeps one-off style configuration in place.

### Emit ANSI 256 colors everywhere

Emitting only ANSI 256 colors by default would fix Apple Terminal at the expense
of color fidelity on truecolor terminals, while doing nothing for light-terminal
readability or frontend-neutral styling.

### Put scope resolution in `jp_printer`

`jp_printer` receives raw bytes rather than semantic nodes; by the time output
reaches the sink, `markdown.inline_code.body` and `tool_call.function_name` are
indistinguishable SGR sequences.
Resolving scopes in the sink would require every caller to tag each write with
its scope, making correctness depend on call-site discipline.

### Build a generic frontend abstraction now

While web, TUI, and native frontends can share `jp_style`, extracting a shared
view-model layer before a second frontend exists would force us to design the
abstraction without a concrete implementation to validate it.

### Generate or set the terminal's 256-color palette

The [color256] writeup demonstrates how to derive a terminal's 256-color palette
from a base16 theme.
JP adopts base16 palette roles in `jp_style`, but reprogramming the user's
terminal palette via escape sequences would alter their terminal state outside
JP.

## Non-Goals

- Building a web, TUI, or native frontend, or defining a generic frontend
  view-model abstraction before a second frontend exists.
- Routing non-CLI frontends through `jp_cli` or `jp_printer`.
- Moving `conversation.tools.*.style.parameters` into `style.tool_call`.
- Replacing `syntect` or moving `syntect::Theme` into `jp_style`.
- Reprogramming the terminal's 256-color palette.
- Defining the full trust policy for terminal control sequences emitted by
  tools.
  This RFD specifies the default filtering in `jp_printer`; the broader tool
  sandbox and trust model belongs to [RFD 075].

## Risks and Open Questions

- **Performance**: Parsing and adapting ANSI sequences in `jp_printer` adds CPU
  overhead.
  The adapter should cache RGB-to-ANSI conversions per `ColorProfile`.
- **Scheme detection reliability**: Terminal background queries can fail or time
  out.
  `ColorScheme` detection treats `None` as a first-class outcome and falls back
  to styles that remain readable on any background.
- **Migration timing**: `style.inline_code.background` and
  `style.markdown.theme` already appear in user config files and persisted
  conversation deltas.
  Both keys must remain supported as aliases before being removed from the
  schema.
- **Scope naming**: Markdown scopes follow [RFD 084], whereas CLI chrome scopes
  should start with a minimal set covering elements JP already renders.
- **Bootstrap output**: Errors emitted before configuration loads remain
  unthemed, using plain text or minimal ANSI that is readable on any terminal.

## Implementation Plan

### Phase 1: Terminal profile and sink adapter

- Add `ColorProfile`, `ColorDepth`, `ColorScheme`, and detection helpers to
  `jp_term`.
- Add `style.terminal.color_depth` and `style.terminal.background` to
  `jp_config`.
- Replace the raw-or-strip sinks in `jp_printer` with per-target ANSI adapters
  that use a persistent `vte::Parser` to handle escape sequences split across
  writes.
- Downgrade SGR colors to `Truecolor`, `Ansi256`, `Ansi16`, or `None` based on
  the target profile.
- Add tests for split CSI sequences, typewriter output, stdout redirection with
  styled stderr, and Apple Terminal-style `TERM=xterm-256color` detection.

This phase fixes truecolor output in 256-color terminals and can merge before
`jp_style` exists.

### Phase 2: Inline-code readability

- Pass the resolved `ColorScheme` to the CLI markdown renderer in `jp_md`.
- Ensure default styles that set a background color also set a foreground color
  (or omit the background) when `ColorScheme` is unknown.
- Add snapshot tests for inline code under light and unknown color schemes.

This phase fixes the black-on-dark inline-code failure.

### Phase 3: Shared style model and markdown elements

- Add `jp_style` with `Style`, `Color`, `Palette`, `Scope`, and stylesheet
  resolution.
- Align [RFD 084]'s markdown element tree with `jp_style`.
- Keep `syntect::Theme` selection in `jp_md` or CLI formatter construction
  rather than `jp_style`.
- Replace the standalone inline-code background setting with
  `style.markdown.elements.inline_code.body.bg`.
- Add config aliases for `style.inline_code.background` and
  `style.markdown.theme`.

This phase depends on Phase 2.

### Phase 4: CLI chrome scopes

- Define the initial CLI chrome scopes: role header label, role header suffix,
  tool call function name, progress label, and progress timer.
- Resolve these scopes in `jp_cli` render modules before writing to `Printer`.
- Leave behavioral settings (`style.tool_call.show`,
  `style.tool_call.progress.*`, and `conversation.tools.*.style.parameters`) at
  their existing config paths.

This phase depends on Phase 3.

### Phase 5: Follow-on frontend sharing

Extract a target-neutral view model or styled-span layer once a web, TUI, or
native frontend requires shared rendering.

## References

- [RFD 004]: the `jp_md` streaming markdown parser and terminal renderer.
- [RFD 048]: the four-channel output model that this RFD extends with per-target
  color decisions.
- [RFD 075]: tool sandbox and access policy.
  This RFD only defines the terminal sink's default escape handling.
- [RFD 084]: configurable markdown element coloring.
  This RFD reuses the markdown scope direction and moves shared style values
  into `jp_style`.
- [RFD 096]: terminal output sanitization for untrusted content, which sanitizes
  untrusted input at render-pipeline ingress upstream of the formatters and sink
  adapters described here.
- [color256]: an informative writeup on generating 256-color palettes from
  base16 themes.

[RFD 004]: ../004-streaming-markdown-parser-and-terminal-renderer-in-jp_md.md
[RFD 048]: ../048-four-channel-output-model.md
[RFD 075]: ../075-tool-sandbox-and-access-policy.md
[RFD 084]: ../084-configurable-markdown-element-coloring.md
[RFD 096]: ../096-terminal-output-sanitization-for-untrusted-content.md
[color256]: https://gist.github.com/jake-stewart/0a8ea46159a7da2c808e5be2177e1783
