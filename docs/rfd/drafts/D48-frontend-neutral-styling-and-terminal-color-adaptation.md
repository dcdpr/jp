# RFD D48: Frontend-Neutral Styling and Terminal Color Adaptation

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-02
- **Extends**: [RFD 048], [RFD 004]

## Summary

Introduce a frontend-neutral style model for JP and a terminal-specific color
adapter for the CLI.
Renderers resolve semantic styles into their own target's native representation,
while `jp_printer` adapts terminal color bytes to the active terminal profile.

## Motivation

JP currently mixes three concerns that need separate homes:

1. **Semantic style**: inline code, tool names, role headers, progress labels,
   markdown headings, and other UI concepts have meaning before they have a
   color.
2. **Frontend rendering**: the CLI renders ANSI, a web UI renders CSS, a TUI
   renders widget styles, and a native app renders platform text attributes.
3. **Terminal capability**: Apple Terminal may expose `TERM=xterm-256color` and
   reject truecolor even though JP currently emits truecolor unconditionally.

Two user-visible failures fall out of that entanglement:

- Inline code can render as black text on a near-black background on light
  terminals.
  `jp_md` sets a background for inline code but leaves the foreground inherited
  from the terminal.
- Syntax-highlighted output can render incorrectly in 256-color terminals.
  JP emits `38;2;r;g;b` and `48;2;r;g;b` truecolor sequences from multiple
  places without a terminal capability check.

If JP fixes these locally at each call site, every new styled element repeats
the same failure mode.
If JP puts semantic scope resolution in the terminal sink, the sink receives
only bytes and cannot tell whether `\x1b[38;2;...]` came from inline code, a
tool name, a progress line, or relayed tool output.

The design needs one clean rule:

> Renderers resolve semantic scopes into their frontend's native style
> representation.
> Terminal byte adaptation is a CLI sink concern.

## Design

### What users see

JP becomes readable on light terminals and correct on 256-color terminals by
default.
A user in Apple Terminal with `TERM=xterm-256color` gets 256-color output
instead of raw truecolor.
A user on a light terminal does not get inline code rendered as
dark-background-only text that depends on a dark foreground.

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

Visual styling lives under element trees.
Behavioral settings stay where they already belong.

```toml
[style.markdown.elements.inline_code.body]
bg = "base01"

[style.markdown.elements.inline_code.delim]
fg = "base04"

[style.tool_call.elements.function_name]
fg = "base0D"
intensity = "bold"
```

This RFD does not move `conversation.tools.*.style.parameters` to
`style.tool_call.parameters`.
That setting controls how a specific tool's arguments are displayed; it is not a
visual style scope.

### Architecture

The system has three layers.

#### 1\. Shared semantic style

A new pure `jp_style` crate owns the frontend-neutral vocabulary:

- `Style`: foreground, background, intensity, italic, underline, strikethrough,
  and related text attributes.
- `Color`: palette role, ANSI-256 index, 24-bit RGB, terminal default, or
  inherit.
- `Palette`: base16-style roles such as `base00` through `base0F`.
- `Scope`: stable semantic paths such as `markdown.inline_code.body` and
  `tool_call.function_name`.
- Stylesheet resolution from scope plus theme to a `Style` value.

`jp_style` does not depend on `syntect`, does not detect terminals, and does not
emit ANSI.
It may convert to and from `anstyle` color values, but it is not a terminal
rendering crate.

#### 2\. Frontend renderers

Each frontend resolves semantic style while it still has semantic context.

- The CLI markdown renderer in `jp_md` maps markdown nodes to markdown scopes.
  It owns or receives the `syntect::Theme` used for code highlighting because
  syntax highlighting belongs to markdown/code rendering, not to `jp_style`.
- CLI chrome renderers in `jp_cli` map tool call headers, role headers, progress
  labels, and similar concepts to chrome scopes.
- A web frontend maps the same scopes to CSS classes, CSS variables, or inline
  style objects.
- A TUI frontend maps the same scopes to widget styles.
- A native app maps the same scopes to platform text attributes.

Non-CLI frontends do not use `jp_printer` and do not receive ANSI strings from
`jp_cli`.
If frontend sharing becomes necessary, JP extracts a target-neutral view model
or styled-span representation; it does not route other frontends through
terminal bytes.

#### 3\. Terminal sink

`jp_printer` remains CLI-specific.
It receives bytes and adapts them to the terminal target.

The sink owns:

- per-target color enablement,
- SGR color downgrading,
- stripping color when disabled,
- stripping unsafe control sequences from normal content,
- preserving safe text attributes where allowed,
- correct handling of escape sequences split across writes.

The sink does not own:

- scope resolution,
- markdown semantics,
- tool-call semantics,
- syntect theme selection,
- web/TUI/native rendering decisions.

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

Color capability is terminal-wide.
Color emission is per target.
A redirected stdout must not disable styled stderr or `/dev/tty` prompt output.

```rust
pub struct TargetColorProfile {
    pub color_enabled: bool,
    pub profile: ColorProfile,
}
```

`PrintTarget::Out`, `PrintTarget::Err`, and `PrintTarget::Tty` each receive
their own target profile.

### Environment and config precedence

Color enablement for each target is resolved in this order:

1. Explicit JP config or CLI override.
2. `NO_COLOR` disables foreground and background color.
3. `CLICOLOR_FORCE` enables color even when the target is not a TTY, unless
   `NO_COLOR` is set.
4. `CLICOLOR=0` disables color.
5. The target's own TTY state enables color.
6. Color is disabled.

Color depth is resolved terminal-wide in this order:

1. Explicit `style.terminal.color_depth`.
2. `COLORTERM=truecolor` or `COLORTERM=24bit` selects truecolor.
3. `TERM` containing `256color` selects ANSI 256.
4. Other color-capable `TERM` values select ANSI 16.
5. Otherwise, no color.

Background scheme is resolved terminal-wide in this order:

1. Explicit `style.terminal.background`.
2. `COLORFGBG`, when present and parseable.
3. A short-timeout query against the controlling terminal, when available.
4. `None`.

`None` is not dark.
If the scheme is unknown, any default style that sets a background must either
set a matching foreground or skip the background.
This is the readability rule that prevents dark inline-code backgrounds from
inheriting black terminal text.

### Startup order

The configured printer cannot be fully constructed before config is loaded, but
JP still needs sane behavior for bootstrap errors.

Startup proceeds as follows:

1. Early bootstrap output uses plain text or minimal unthemed ANSI only.
2. Load config.
3. Detect per-target TTY state for stdout, stderr, and the controlling terminal.
4. Resolve `ColorProfile` from config, environment, and terminal detection.
5. Construct or update `Printer` with per-target profiles.
6. Run command rendering.

Background scheme detection queries the controlling terminal when available.
It does not blindly query stdout, because stdout may be redirected while
`/dev/tty` still exists for prompts.

### Terminal byte adaptation

`jp_printer` extends the current `AnsiStripper` model into an ANSI adapter.
The adapter keeps a persistent `vte::Parser` per sink so split escape sequences
are handled correctly.

For normal content writes:

| Sequence category                             | Styled terminal target            | Non-styled target           |
| --------------------------------------------- | --------------------------------- | --------------------------- |
| SGR foreground/background color               | Adapt to target depth or strip    | Strip                       |
| SGR text attributes                           | Preserve where supported          | Strip for structured output |
| OSC 8 hyperlinks                              | Preserve for terminal text output | Strip                       |
| Cursor movement, erase, title, clipboard, etc | Strip from normal content         | Strip                       |

JP-owned cursor and erase controls, such as progress-line redraws, use a trusted
control path on the printer rather than being written as normal content.
The safe failure mode is losing a progress redraw, not leaking relayed control
sequences from tool output.

### Relationship to RFD 084

[RFD 084] defines configurable markdown element coloring inside `jp_md`.
This RFD keeps that direction but changes the boundary:

- Markdown element scopes resolve in `jp_md`, where markdown context exists.
- `jp_md` may emit canonical terminal styles for the CLI, but terminal depth
  adaptation happens in `jp_printer`.
- `jp_style` owns shared style values and palettes, not `syntect::Theme`.
- The deprecated `style.inline_code.background` key maps to
  `style.markdown.elements.inline_code.body.bg` during config loading.

## Drawbacks

- The scope taxonomy becomes user-facing config API.
  Scope names must be stable once shipped.
- The terminal sink parses output that it writes.
  This adds runtime overhead, especially for syntax-highlighted code with many
  color changes.
- The implementation crosses several crates: `jp_style`, `jp_term`,
  `jp_printer`, `jp_md`, `jp_cli`, and `jp_config`.
- Light and dark default themes require curation.
  Automatic downgrade alone does not guarantee pleasant colors.
- Configuration migration touches persisted conversation config and deltas, not
  only user TOML files.

## Alternatives

### Set an inline-code foreground only

JP could fix the reported inline-code failure by setting a foreground whenever
inline code sets a background.
This is a useful emergency patch, but it does not fix truecolor output in
256-color terminals and leaves the one-off style plumbing in place.

### Emit ANSI 256 colors everywhere

JP could avoid truecolor and use ANSI 256 by default.
This fixes Apple Terminal at the cost of worse output on terminals that support
truecolor.
It also does not solve light/dark readability or frontend-neutral styling.

### Put scope resolution in `jp_printer`

The printer receives bytes, not semantic nodes.
By the time output reaches the sink, `markdown.inline_code.body` and
`tool_call.function_name` have both become SGR sequences plus text.
Putting scope resolution there would require callers to annotate every print
with provenance, which makes correctness depend on every call site remembering
to classify output.

### Build a generic frontend abstraction now

A web UI, TUI, and native app can share `jp_style`, but they should not share a
terminal printer.
A generic view-model layer can be extracted when a second frontend needs shared
rendering.
Defining it before that creates abstraction without a second implementation to
validate it.

### Generate or set the terminal's 256-color palette

The [color256] writeup shows how a terminal can generate its 256-color palette
from base16 colors.
JP adopts the base16 role vocabulary as a style model, but it must not reprogram
a user's terminal palette as part of normal output.

## Non-Goals

- Build the web, TUI, or native frontend.
- Route non-CLI frontends through `jp_cli` or `jp_printer`.
- Define a generic frontend view-model abstraction before a second frontend
  needs it.
- Move `conversation.tools.*.style.parameters` into `style.tool_call`.
- Replace `syntect` or move `syntect::Theme` into `jp_style`.
- Reprogram the terminal's 256-color palette.
- Define the full trust policy for tool-emitted terminal controls.
  This RFD sets the CLI sink default; the broader tool trust model belongs with
  [RFD 075].

## Risks and Open Questions

- **Performance:** parsing and adapting ANSI in the sink costs CPU.
  The adapter should cache RGB-to-ANSI conversions per `ColorProfile`.
- **Scheme detection reliability:** terminal background queries can fail or time
  out.
  `None` remains a first-class result with a readability fallback.
- **Migration timing:** `style.inline_code.background` and
  `style.markdown.theme` are already serialized in configs and conversation
  deltas.
  They need explicit aliases before any field is removed from the schema.
- **Scope naming:** markdown scopes can follow [RFD 084], but chrome scopes need
  a smaller first set based on existing rendered elements.
- **Bootstrap output:** errors before config loading stay unthemed.
  The user experience must remain readable but does not need the full style
  system.

## Implementation Plan

### Phase 1: Terminal profile and sink adapter

- Add `ColorProfile`, `ColorDepth`, `ColorScheme`, and detection helpers to
  `jp_term`.
- Add `style.terminal.color_depth` and `style.terminal.background` to
  `jp_config`.
- Extend `jp_printer` from raw-or-strip sinks to per-target ANSI adapters.
- Downgrade SGR colors to truecolor, ANSI 256, ANSI 16, or no color based on the
  target profile.
- Preserve split-escape correctness with a persistent parser per sink.
- Add tests for split CSI sequences, typewriter output, stdout redirection with
  styled stderr, and Apple Terminal-style `TERM=xterm-256color` detection.

This phase fixes truecolor output in 256-color terminals and can merge before
`jp_style` exists.

### Phase 2: Inline-code readability

- Teach the CLI markdown renderer about the resolved `ColorScheme`.
- Apply the rule that unknown-scheme background-bearing defaults must set both
  foreground and background or set neither.
- Add light-scheme and unknown-scheme snapshot tests for inline code.

This phase fixes the black-on-dark inline-code failure.

### Phase 3: Shared style model and markdown elements

- Add `jp_style` with `Style`, `Color`, `Palette`, `Scope`, and stylesheet
  resolution.
- Align [RFD 084]'s markdown element tree with `jp_style`.
- Keep `syntect::Theme` selection in `jp_md` or the CLI formatter construction
  path, not in `jp_style`.
- Replace one-off inline-code background plumbing with
  `style.markdown.elements.inline_code.body.bg`.
- Add config aliases for `style.inline_code.background` and
  `style.markdown.theme`.

This phase depends on Phase 2.

### Phase 4: CLI chrome scopes

- Add a small first set of chrome scopes where semantic context is still
  present: role header label, role header suffix, tool call function name,
  progress label, and progress timer.
- Resolve those scopes in `jp_cli` render modules before writing to the printer.
- Keep behavioral settings such as `style.tool_call.show`,
  `style.tool_call.progress.*`, and `conversation.tools.*.style.parameters` in
  their current config homes.

This phase depends on Phase 3.

### Phase 5: Follow-on frontend sharing

When a second frontend needs shared rendering, extract a target-neutral view
model or styled-span layer.
This phase is intentionally deferred until the web, TUI, or native frontend
exercises the shared boundary.

## References

- [RFD 004]: the `jp_md` streaming markdown parser and terminal renderer.
- [RFD 048]: the four-channel output model that this RFD extends with per-target
  color decisions.
- [RFD 075]: tool sandbox and access policy.
  This RFD only defines the terminal sink's default escape handling.
- [RFD 084]: configurable markdown element coloring.
  This RFD reuses the markdown scope direction and moves shared style values
  into `jp_style`.
- [RFD 096]: terminal output sanitization for untrusted content.
  Its sanitizer neutralizes untrusted content at render-pipeline ingress,
  upstream of any formatter this RFD proposes; this RFD's design owes it no
  changes.
- [color256]: an informative writeup on generating 256-color palettes from
  base16 themes.

[RFD 004]: ../004-streaming-md-parser-renderer.md
[RFD 048]: ../048-four-channel-output-model.md
[RFD 075]: ../075-tool-sandbox-and-access-policy.md
[RFD 084]: ../084-configurable-markdown-element-coloring.md
[RFD 096]: ../096-terminal-output-sanitization-for-untrusted-content.md
[color256]: https://gist.github.com/jake-stewart/0a8ea46159a7da2c808e5be2177e1783
