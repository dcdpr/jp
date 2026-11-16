# RFD 096: Terminal Output Sanitization for Untrusted Content

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-07
- **Summary**: Sanitizes untrusted terminal output at render time, stripping
  control sequences from model output while isolating user and tool SGR styling.

## Summary

This RFD introduces render-time terminal sanitization for conversation content
while keeping stored event data verbatim.
Each content class has its own policy: user messages and tool output retain
bounded SGR styling (closed with a trailing reset so styles cannot bleed into JP
chrome), while model output and tool prompt text are stripped of escape
sequences.
As a baseline defense, `jp_printer` also strips terminal-state-changing control
sequences from all output channels.

## Motivation

When a user pasted a raw tty log (captured with `script`) into an
editor-composed reply, JP echoed the message back to the terminal.
The log's escape sequences (cursor positioning, line erasure, color state)
executed on the user's terminal and corrupted its display.
Because JP stores the message verbatim, every re-render (`jp conversation
print`, `--replay`, scrolling back through history) replays the corruption.

Display corruption is the benign case.
The same render pipeline prints LLM output and tool results, which are untrusted
by construction.
A model (or a prompt-injected web page in a tool result) can emit sequences
that:

- move the cursor and overwrite earlier lines, e.g. to spoof a tool-approval
  prompt or alter text the user already read;
- erase or scroll content out of view to hide it;
- switch terminal modes (alternate screen, bracketed paste, mouse reporting),
  breaking JP's own prompts;
- write to the clipboard (OSC 52) or retitle the window (OSC 0/2) on supporting
  terminals.

This is a known attack class for LLM CLIs; several comparable tools have shipped
CVEs for it.

At least one supported provider passes functional ANSI escape sequences through
verbatim: a raw `ESC` byte from the model stream reaches the client and executes
if rendered to a terminal.
JP cannot assume providers strip escape sequences.

Echoed user messages (the triggering incident) and tool results (fetched web
pages, file contents, colored build output) carry raw escape bytes directly,
without passing through a model.

JP also embeds the conversation title, which is LLM-generated text, into an OSC
2 window-title sequence.
A title containing a `BEL` or `ESC \` terminator ends the OSC sequence early and
injects the remaining bytes as raw terminal output.

Markdown opens a path around a raw byte filter.
CommonMark decodes numeric character references, so `&#27;[2J` in model output
parses to a literal `ESC` byte followed by `[2J` in a text node, which JP's
formatter writes unfiltered.

Allowed styling can also bleed into surrounding output.
JP truncates tool results to a line budget, which can cut off a result's closing
reset sequence.
In [#1201], a truncated diff from a user-built tool lost its reset and bled red
text into JP's truncation note and the next reasoning block.
Because content can open styling that truncation cuts off, JP must emit a
closing reset before writing its own output again.

Without sanitization, every render of hostile or accidental control bytes
executes them on the terminal, and storing conversations verbatim makes the
corruption permanent.

## Design

### Trust model

Output falls into two classes, following the channel taxonomy of [RFD 048]:

| Class       | Examples                                                                                     | Policy              |
| ----------- | -------------------------------------------------------------------------------------------- | ------------------- |
| **Chrome**  | Role headers, separators, status line, `jp_md` styling, prompt widget controls               | Trusted, unmodified |
| **Content** | Echoed user messages, LLM message/reasoning text, tool result text, titles, tool prompt text | Sanitized at render |

The boundary is authorship: bytes JP composes are chrome; bytes that originate
in conversation data, model output, or tool output are content.

### Sanitization policy

Sanitization is an allowlist over escape-sequence classes, not a blanket strip.

| Class                                            | Action    | Rationale                                         |
| ------------------------------------------------ | --------- | ------------------------------------------------- |
| Printable text, `\n`, `\t`                       | Keep      | Content (`\t` width is open, see Risks).          |
| SGR (`CSI … m`)                                  | Per class | Display-only; see content classes below.          |
| SGR conceal parameter (`8`)                      | Remove    | Hides text from the user.                         |
| All other CSI (cursor, erase, scroll, DEC modes) | Drop      | Rewrites or hides what the user sees.             |
| OSC (title, clipboard, hyperlinks)               | Drop      | Side effects beyond the character grid.           |
| DCS / APC / PM / SOS, other `ESC x`              | Drop      | Terminal-specific side effects.                   |
| Remaining C0 (including `\r`), DEL, C1           | Drop      | `\r` overwrite is the classic log-spoofing trick. |

Conceal (`8`) is removed from the SGR parameter list rather than dropping the
entire sequence, so `\x1b[8;31m` retains its red foreground.
Parameters consumed by the extended color introducers `38`, `48`, and `58` (e.g.
`38;5;8`, `48;2;…;8`, and their colon-separated variants) are color payload,
not conceal.
[RFD 091]'s status-region filter already performs this parameter walk
(`visible_sgr`), and both filters share it.

Disallowed sequences and conceal parameters are removed in `strip` mode (the
default, so pasted logs render as clean text) or replaced with a visible `␛`
(U+241B) marker in `visualize` mode.
`strip` mode preserves printable text outside disallowed control sequences, but
drops payload bytes inside them (such as an OSC title's text or OSC 52's
clipboard data).
Use `visualize` to mark their presence, or `off` to inspect raw terminal
behavior.

### Content classes

Whether SGR survives depends on where the content comes from:

| Class            | Examples                                                                     | SGR     | Content span               |
| ---------------- | ---------------------------------------------------------------------------- | ------- | -------------------------- |
| User messages    | Echoed editor-composed and replayed requests                                 | Kept    | Closed after formatting    |
| Tool output      | Tool results, custom-formatter output                                        | Kept    | Closed after written lines |
| Model output     | LLM message and reasoning text                                               | Dropped | None                       |
| Tool prompt text | Question pre-amble, question text, select option labels, text-answer default | Dropped | None                       |

- **User messages** keep styling because the user wrote them and a pasted
  colored log should retain its colors.
  Each message renders in a single `format_terminal` call, so its span closes
  once on the formatted output.
- **Tool output** keeps styling so local and user-built tools can color their
  output (such as the diff in [#1201]).
- **Model output** is plain text.
  It streams in chunks that JP interleaves with kind transitions,
  reasoning-budget truncations, and code fences; dropping SGR avoids emitting a
  reset at each boundary.
  The model styles text through markdown, which JP renders.
- **Tool prompt text** is plain text.
  `jp_tool::Question` documents its text as a single plain line, and the prompt
  is where the user approves tool calls.
  This display contract applies under every `style.sanitize` mode.

<!-- end list -->

```toml
[style]
# How control sequences in untrusted content are rendered.
# strip     - remove disallowed sequences (default)
# visualize - replace disallowed sequences with a visible ␛ marker
# off       - disable content sanitization for pretty terminal rendering.
#             OSC embedding is still escaped, tool prompt text is still
#             plain, content spans still close with a reset, and non-pretty
#             output still strips ANSI.
sanitize = "strip"
```

This setting governs the render-pipeline sanitizer across both live streaming
and history re-rendering.
When `style.sanitize` is `off`, every content class passes control sequences
through unmodified.
OSC embedding hardening, the closing reset on content spans, the plain-text rule
for tool prompt text, and non-pretty ANSI stripping apply regardless of
`style.sanitize`.

### Content spans

The renderer wraps user messages and tool output in a *content span*, opening
the span before writing content and emitting `\x1b[0m` before writing chrome
again.
Content can open arbitrary SGR styles inside the span, and the closing reset
prevents unclosed styles from bleeding past the span boundary.

The span closes where JP stops writing content, even when truncation cuts the
content short.

**Tool output** closes after the last written line, before the closing fence and
any truncation note:

```text
tool output: \x1b[31mline 1\nline 2\nline 3\x1b[0m
written:     \x1b[31mline 1\nline 2\x1b[0m\n _(truncated to 2 lines)_
```

Closing the span before the final line break prevents an open background color
from painting the row below.

**User messages** close after `format_terminal` returns.
JP inserts the reset into the formatted output before its trailing line breaks,
rather than into the markdown source.
Inserting before the trailing line breaks prevents an open background color from
painting the next row, and inserting after markdown parsing prevents the reset
from altering the parse (for example, turning a closing code fence with no
trailing newline into an opening fence).

Content spans always close with `\x1b[0m`, even when `style.sanitize` is `off`.

### The sanitizer

`sanitize::ContentWriter` in `jp_term` is a `fmt::Write` decorator with the same
structure as `jp_term::shade::ShadedWriter`, built on the `vte::Parser`
foundation used by `jp_printer::ansi::AnsiStripper`.
Constructing a `ContentWriter` for a content class opens its span (if the class
has one), and `finish()` resolves any trailing partial sequence and closes the
span.
Parser state persists across writes so sequences split across stream events are
recognized as a unit, matching `AnsiStripper`.
While `AnsiStripper` drops all escape sequences for non-pretty output,
`ContentWriter` applies the allowlist above and keeps or drops SGR by class.

The writer sits upstream of any decoration JP applies to the write.
In a reasoning region, tool-result bytes flow from `ContentWriter` to
`ShadedWriter` to the output channel.
Because `ShadedWriter` adds the region's fill and erase escapes downstream of
`ContentWriter`, the allowlist never sees them, and the span's closing reset
triggers `ShadedWriter` to re-assert the region background.

Model output uses one `ContentWriter` instance per content kind per turn.
Reasoning and message chunks can interleave within a turn
(`ChatRenderer::flush_on_transition`), and sharing an instance would join a
partial sequence from one kind with bytes from the other.
Calling `finish()` at kind transitions and turn boundaries prevents a dangling
introducer from joining the next kind's bytes.
Because model output has no span, `finish()` emits no reset.

### Markdown-decoded text

`ContentWriter` filters raw markdown source before `jp_md` parses it with
comrak.
Because comrak decodes character references in text nodes, link URLs, and link
titles, an entity such as `&#27;` becomes a literal `ESC` byte after the initial
byte filter has run.
The formatter therefore filters decoded text a second time in `jp_md::render`
(`format_text`, `format_link`, `format_image`) before `TerminalWriter` adds JP's
styling:

- **Model output** strips every control character except `\n` and `\t` with a
  per-character check.
  Because the upstream byte filter already dropped raw escape sequences, any
  remaining control character came from a decoded entity.
- **User messages** run the `vte` allowlist pass a second time over the decoded
  text nodes, since comrak outputs raw and entity-decoded `ESC` bytes
  identically.

The formatter receives the content class's decoded-text policy through its
options.
The upstream byte filter remains necessary because streamed fenced code lines
bypass comrak (`ChatRenderer` → `Formatter::render_code_line`).

Note on naming: JP already uses `sanitize` for storage sanitization ([RFD 052]'s
`Workspace::sanitize` / `SanitizeReport`) and conversation-stream repair
(`ConversationStream::sanitize`).
This RFD adds display sanitization, scoped to `jp_term` and the render pipeline.
The ubiquitous-language documentation will define all three.

### Placement

The byte filter runs where content enters the render pipeline, **before**
markdown parsing; the decoded-text filter runs inside the formatter, **before**
JP's styling:

```
untrusted text ──▶ ContentWriter ──▶ jp_md Buffer ──▶ Formatter ──▶ Printer
                    (byte filter)                     (decoded-text filter,
                                                       then chrome styling)
```

Per-class filtering cannot happen at the printer sink because trusted `jp_md`
styling and untrusted content bytes are already interleaved there.
SGR bytes kept in user messages still flow into `jp_md`'s parser and may appear
mid-token (inside emphasis or fence markers), preserving existing behavior.

Concretely:

1. `ChatRenderer::render_request`: echoes editor-composed and replayed user
   messages (the triggering incident).
   Keeps SGR and closes the span on the formatted output.
2. `ChatRenderer::render_content` / `render_reasoning`: renders streamed LLM
   output with one `ContentWriter` per content kind to handle chunk-split
   sequences.
   Drops SGR with no span.
3. Tool-result rendering (`jp_cli::render::tool`): wraps text paths that bypass
   `jp_md`.
   Keeps SGR and closes the span after the written lines.
4. History re-rendering (`jp conversation print/show`): uses the chat renderer
   for message history, with direct print paths audited and wrapped.
5. Table, list, and match rendering of conversation-derived strings: `jp
   conversation ls` prints LLM-generated titles in a table, and `jp conversation
   grep` prints matching lines from message, reasoning, and tool-result text.
   These derived strings (see below) are printed as stored, truncated to visible
   width, and bounded by the output floor.
6. Tool prompts: `ToolPrompter::prompt_question` writes `question.pre_amble` to
   the prompt writer and passes `question.text`, select options, and any default
   text answer to the prompt backend.
   Each string is stripped of control characters for display, except `\n` and
   `\t` in `pre_amble` (which carries multi-line content such as a diff under
   review).
   Stripping applies only to display: select options map the chosen index back
   to the original option, and accepting a text question's default unchanged
   returns the original default string.

### Output floor

The per-class policy only protects paths that explicitly wrap content in a
`ContentWriter`.
Any command that prints a conversation title or tool-result line directly would
otherwise write raw bytes to the terminal.
The *output floor* provides baseline filtering for every byte written in pretty
output mode.

Under a pretty format, the printer worker passes each print task through a
per-stream `OutputFloor` (`jp_term::sanitize`), preserving parser state across
tasks so sequences split between two tasks are recognized as a unit.
The floor allows only the sequences JP's own output uses and drops everything
else:

- printable text, `\n`, and `\t`;
- SGR (`CSI … m`), minus conceal (`8`);
- erase to the end of the line (`\x1b[K`);
- OSC 8 hyperlinks, with control characters stripped from the target URI.

Carriage returns (`\r`) are dropped along with other C0 control characters.
Because JP's own redraws (status regions and prompt line clears) bypass the
floor, output passing through the floor never needs `\r`, and `\x1b[K` without
`\r` can only clear to the end of the current line rather than overwriting
earlier text.
When the printer shuts down, each floor emits `\x1b[0m` if styling is still
active, preventing unclosed colors from bleeding into the shell prompt.

Disallowed sequences follow `style.sanitize`: removed under `strip`, replaced
with `␛` under `visualize`, and passed through under `off`.
The floor's mode is updated when the run context is built, when a long-running
host swaps configs, and by each turn's chat renderer so per-conversation
`style.sanitize` overrides take effect.

Two write paths bypass the floor.
Prompt widget drawing (`PrintOrigin::Widget`) intentionally moves the cursor and
switches terminal modes, so callers that pass untrusted text to a widget (such
as picker rows or confirmation preambles) must sanitize that text first.
The inline reply widget filters its own display buffer, rendering control
characters as `?` while returning the unmodified tool result if submitted
unchanged.
Status-region frames also bypass the floor because the printer worker writes
them directly under [RFD 091]'s stricter filter.

The output floor complements rather than replaces the per-class policy.
Because it sits downstream of formatting, it must allow SGR and OSC 8 hyperlinks
that chrome uses, which model output is not allowed to emit.
Its role is to bound the impact of any code path that prints derived strings
without a `ContentWriter`.

### Derived strings

Strings derived from conversation data (conversation titles, `jp conversation
grep` matches, message author names) do not belong to a content class.
Commands print them as stored, relying on the output floor to strip
terminal-control sequences while preserving any SGR styling.

`jp_term::width` measures and truncates strings by visible character width:
escape sequences consume zero columns and are never split mid-sequence.

Because prompt widgets bypass the output floor, the three call sites that pass a
conversation title to a widget (the conversation picker, the `jp conversation
title` picker, and the confirmation details for `rm` and `archive`) strip all
control characters and render the title as a single plain line regardless of
`style.sanitize`.

### OSC embedding hardening

Regardless of `style.sanitize`, `jp_term::osc` escapes dynamic strings spliced
into OSC sequences by stripping all C0 (including `BEL` and `ESC`), DEL, and C1
control characters.
This prevents early termination of both `BEL`- and `ST`-terminated OSC
sequences.
The module has two embedding functions:

- `set_title`: embeds the LLM-generated conversation title inside OSC 2.
- `hyperlink`: embeds the target URI inside OSC 8.
  Call sites splice model-influenced strings into this URI
  (`jp_cli::render::tool` builds `file://` and `copy://` URIs from tool-created
  paths, and `jp conversation ls` builds `jp://` URIs from conversation IDs).
  Only the URI is escaped here; the visible link text sits between the OSC open
  and close sequences in normal display text, where it can legitimately carry
  SGR styling and is covered by the content sanitizer.

Sanitization applies to content strings before they are spliced into chrome.
Raw OSC 8 sequences in untrusted content are dropped by the content sanitizer,
whereas OSC 8 sequences constructed via `jp_term::osc::hyperlink` are trusted
chrome with an escaped URI.

Because legitimate titles and URIs never contain control characters, this
escaping is unconditional and remains active when `sanitize = "off"`.

### What does not change

- **Stored data and LLM input.** Conversations store verbatim bytes, and the
  model receives them unmodified.
  Sanitization is strictly a display concern: the debugging session that
  motivated this RFD depended on the model seeing raw escape bytes in a pasted
  log.
- **Non-pretty output.** The `out` and `err` sinks already strip all ANSI
  sequences via `AnsiStripper` for non-pretty formats.
- **Chrome.** JP's own styling, widgets, and `reedline`/`inquire` prompt
  controls are unchanged: the output floor allows every sequence chrome emits,
  and widget drawing bypasses the floor.
  Tool-supplied strings displayed inside a prompt are sanitized as content (see
  Placement).

## Drawbacks

- Tools or users that deliberately emit cursor-control art lose it unless they
  set `sanitize = "off"`.
- Allowed SGR in tool output can clash with `jp_md`'s styling state: an `SGR 0`
  reset inside a code block clears the block's background fill until the
  formatter's next styled write.
  This behavior already exists; sanitization neither fixes nor worsens it.
- Streaming content incurs a `vte` parse per byte (negligible next to network
  latency, but nonzero).
  User messages incur a second `vte` pass over decoded text nodes once per
  render, while model output uses a per-character check on decoded text.
- Model output renders uncolored even when it contains raw SGR (for example,
  when quoting a colored log).
  The model still sees the raw bytes, and setting `sanitize = "off"` displays
  them.
- Character references in user messages that decode to SGR (such as `&#27;[31m`)
  style the rendered message.
  Because it is the user's own message and only SGR survives, this is harmless.
- Dropping `\r` turns carriage-return progress redraws (common in build tools)
  into concatenated lines in rendered tool results.
  CRLF line endings remain intact because `\n` is preserved.
- Allowing SGR leaves one hiding vector: matching foreground and background
  colors.
  Blocking this would require tracking color state against the terminal theme,
  which is not worth the complexity (see Risks).
- Pretty output parses every printed byte a second time at the output floor.
- Derived strings can still use sequences allowed by the output floor: they can
  restyle subsequent text on the same line, reset the color JP applied to a
  title early, or display an OSC 8 link whose text misrepresents its target URI.
- Titles and `grep` matches retain stored SGR styling, so a model-generated
  title can appear colored.

## Alternatives

- **Strip all escape sequences from content.** Simplest and safest, but breaks
  legitimate colored tool output, which is an explicit requirement.
- **Sanitize only at the printer sink.** Rejected for the per-class policy
  because trusted chrome and untrusted content are already interleaved at the
  sink.
  Retained as the output floor beneath the per-class policy, which only drops
  sequences that chrome never emits (see Output floor).
- **Filter titles once on load.** Loaded titles also feed storage, JSON output,
  plugins, and the macOS app.
  Maintaining both a filtered and a stored copy on the same type shifts the
  burden from remembering to filter to remembering which copy to use, and live
  queries render titles before loading from storage.
- **Filter derived strings by class in each command.** Having `ls`, `show`,
  `grep`, and every conversation notice filter titles individually requires
  every future command to remember to do the same.
  The output floor enforces terminal safety in one place; per-command filtering
  would only strip SGR colors from titles.
- **Dedicated type for stored titles.** A `Title` newtype without `Display`
  would make unescaped title printing a compile error, but `grep` matches and
  author names would remain plain strings.
  Most of the type-conversion churn would land on crates that never display
  titles (storage, workspace, FFI, background tasks), while only three widget
  call sites need explicit plain-text stripping.
- **Single policy for all content classes.** Allowing SGR in model output would
  require closing a span at every kind transition, reasoning-budget truncation,
  and closing code fence during streaming.
  Furthermore, feeding a reset through markdown after a closing fence without a
  trailing newline turns the closing fence into an opening one.
  Model output uses markdown for styling and has no need for raw SGR.
- **Preserve control-producing character references as literal text.** Keeping
  `&#27;` unexpanded requires markdown-context awareness because character
  references inside code spans are already literal.
  Filtering decoded text in the formatter requires no extra context.
- **Emit a reset at each truncation site.** Fixing [#1201] at individual
  truncation call sites would require every future truncation path (head/tail
  views, width budgets, new renderers) to repeat the reset.
- **Stateful content mode on the printer.** Having the renderer toggle a
  content-filtering mode on the printer requires every exit path (Ctrl-C, early
  error returns, panics) to restore the mode; a missed exit would filter JP's
  own chrome from then on.
  By contrast, a `ContentWriter` span is scoped to the writer's lifetime, sits
  upstream of region-shading escapes that a printer-side filter would strip, and
  can be unit-tested without a printer.
- **Sanitize at ingestion (storage).** Rejected because it corrupts stored data,
  hides raw bytes the user asked the model to inspect, and makes policy changes
  irreversible.
- **Render all escapes in caret notation (`^[[48;5;236m`).** Too noisy for
  pasted logs; `visualize` mode achieves the same goal more compactly with a
  single `␛` marker per sequence.

## Non-Goals

- Sanitizing content sent to the LLM or stored in conversations.
- Protecting the `serve-web` HTML view, which uses HTML escaping.
- Defending against vulnerabilities in the terminal emulator itself.
- Redesigning terminal styling; the sanitizer is formatter-agnostic and sits
  upstream of whatever formatter the render pipeline uses.
- Terminal capability adaptation (color downgrading, `NO_COLOR`, non-TTY
  stripping), which belongs to the terminal sink downstream of rendering.
  This RFD neutralizes untrusted content at the start of the render pipeline,
  whereas capability adaptation transforms trusted styling on output.
  Keeping the untrusted-content SGR allowlist in `jp_term::sanitize` prevents
  the two policies from drifting apart.
- Replacing the status-region filter of [RFD 091].
  Status rows require stricter guarantees (no tabs, a reset on every row) so
  cursor movement or tab expansion cannot break the printer worker's row
  accounting, and they remain filtered when `sanitize = "off"`.
  The two filters share SGR parsing and conceal removal (`visible_sgr`), not
  policy.

## Risks and Open Questions

- **Default mode (`strip` vs. `visualize`).** `visualize` makes tampering
  attempts visible, while `strip` produces cleaner output for accidentally
  pasted logs.
  The `style.sanitize` setting makes this default easy to revisit.
- **OSC 8 hyperlinks in content.** Dropped because link text can misrepresent
  the target URL.
  Future work could allowlist OSC 8 with a URL scheme filter.
- **SGR parameter allowlist.** Conceal (`8`) is removed while blink and reverse
  video pass through.
  If foreground-equals-background hiding becomes an issue in practice, SGR
  filtering can be tightened to an explicit parameter allowlist.
- **Tool-result paths bypassing `jp_md`.** Implementation requires auditing all
  tool-result rendering paths to ensure none bypass `ContentWriter`.
- **Dangling escape sequence at stream end.** When `finish()` encounters an
  incomplete sequence at the end of a stream, it treats the dangling introducer
  as a dropped sequence (removed in `strip` mode, marked with `␛` in
  `visualize` mode).
- **Tab width on width-budgeted paths.** `\t` is preserved as content, but
  `jp_term::width` measures it as zero columns while terminals expand it to the
  next tab stop, allowing lines with tabs to exceed width budgets and soft-wrap.
  Preserving `\t` is necessary because tabs are common in file contents and tool
  output, and stripping them would break indentation.
  If normalization is needed later, sanitization can either expand tabs to fixed
  tab stops or render them visibly in `visualize` mode.
  Because the effect is cosmetic and bounded (matching `ripgrep --max-columns`),
  it does not block implementation.

## Implementation Plan

1. **`jp_term::sanitize::ContentWriter`**: implement the `vte`-based allowlist
   filter with per-class SGR policy, closing span resets, and conceal removal
   shared with [RFD 091]'s `visible_sgr`.
   Add unit tests covering sequence classes, chunk-split sequences, `finish()`
   with active styling, `finish()` under `off`, and spanless classes.
   Lands independently with no behavior change until wired.
2. **Close tool-output spans** ([#1201]): in `jp_cli::render::tool`, wrap tool
   bytes in `render_result` and `render_formatted_arguments` with a
   `ContentWriter` under the `off` (pass-through) policy inside `write_chrome`
   so region shading remains downstream, and call `finish()` before the closing
   fence and truncation note.
   Add a regression test where a truncated result opens a color on line 1 and
   closes it past the truncation cut.
3. **OSC embedding hardening**: strip control characters from dynamic inputs in
   `jp_term::osc` with unit tests.
4. **Config setting**: add `style.sanitize` in `jp_config` (defaulting to
   `strip`).
5. **Wire chat rendering paths**: wrap `render_request` (SGR kept, reset
   inserted before trailing line breaks), `render_content`, and
   `render_reasoning` (SGR dropped, no span), and add the decoded-text filter in
   `jp_md::render`.
   Add snapshot tests with escape-sequence fixtures, including `&#27;[2J` in a
   model message.
   For truncated reasoning output, sanitize before applying the
   visible-character budget so escape sequences neither consume the budget nor
   get split by truncation.
6. **Audit and wire tool-result, history, and table/list rendering** (`jp
   conversation ls` titles, `jp conversation grep` matches): in
   `jp_cli::render::tool`, `write_chrome` is an output helper that emits both
   JP-authored headers and tool-result text.
   Sanitize untrusted input (custom-formatter output and `inner_content` under
   the configured `style.sanitize` policy rather than step 2's pass-through)
   before syntax highlighting adds trusted styling, while leaving JP-authored
   headers, separators, temporary lines, and `jp_term::osc::hyperlink` chrome
   unfiltered.
   Close the span after the kept lines as in step 2 so truncation cannot drop
   the closing reset.
   Leave titles and `grep` matches to the output floor and step 10.
7. **Plain-text tool prompts**: document the plain-text contract on
   `jp_tool::Question` (`text`, `pre_amble`, select options, and text default).
   In `ToolPrompter`, strip control characters for display (preserving `\n` and
   `\t` in `pre_amble`), map the selected option index back to the original
   option, and return the original default string when accepted unchanged.
8. **Documentation**: document `style.sanitize` in `docs/configuration.md`, add
   a note to the README security section, and update the ubiquitous-language
   glossary to distinguish display sanitization from storage sanitization ([RFD
   052]) and stream repair.
9. **Output floor**: add `jp_term::sanitize::OutputFloor` (one per stream in the
   printer worker under pretty formats), dropping `\r` and emitting `\x1b[0m` at
   shutdown.
   Add `PrintOrigin::Widget` so widget drawing bypasses the floor, and update
   the floor's mode from `style.sanitize` when building the run context,
   swapping configs, or initializing a chat renderer.
10. **Derived strings**: update `jp_term::width` to measure and wrap around
    escape sequences, print titles and `grep` matches as stored, and strip
    control characters when passing titles to pickers and confirmation prompts.

Steps 1 to 3 and step 7 can merge before the config setting (step 2 after step
1); steps 4 to 6 land together behind the default.
Steps 9 and 10 were added during review of the first eight steps to ensure new
commands remain protected by default.

## References

- [RFD 048]: Four-Channel Output Model, the channel taxonomy that this RFD's
  trust classes build on.
- [RFD 052]: Workspace Data Store Sanitization, the storage-level `sanitize`
  operation in JP.
- [RFD 091]: Printer-Owned Status Region, the status-region filter that shares
  this RFD's SGR parsing and conceal removal under a stricter policy.
- `jp_printer::ansi::AnsiStripper`: the existing `vte`-based full ANSI stripper
  for non-pretty output.
- [Terminal escape injection]: survey of terminal escape-sequence
  vulnerabilities, including OSC 52 clipboard writes.

[#1201]: https://github.com/dcdpr/jp/issues/1201
[RFD 048]: 048-four-channel-output-model.md
[RFD 052]: 052-workspace-data-store-sanitization.md
[RFD 091]: 091-printer-owned-status-region.md
[Terminal escape injection]: https://dgl.cx/2023/09/ansi-terminal-security#vulnerabilities
