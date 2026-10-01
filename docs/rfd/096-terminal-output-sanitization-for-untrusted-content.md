# RFD 096: Terminal Output Sanitization for Untrusted Content

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-07
- **Summary**: Sanitize untrusted terminal output: keep user and tool colors,
  strip model escapes, and reset styling where styled content ends.

## Summary

JP writes conversation content — echoed user messages, streamed LLM output,
tool results — to the terminal without filtering control bytes, so content
containing escape sequences is executed by the terminal instead of displayed.
This RFD introduces render-time sanitization for untrusted content, with a
policy per content class: user messages and tool output keep their styling
(colors, bold), model output, derived strings, and tool prompt text are plain,
and terminal-state-changing sequences are neutralized everywhere.
Stored data remains byte-for-byte verbatim.
Content that may carry styling is written in a content span that JP closes with
a styling reset, so it cannot bleed into JP's own output.
Beneath the per-class policy, the printer holds everything it writes to the
sequences JP's own output is made of, so a path that forgets to filter its
content still cannot take over the terminal, and conversation titles are a type
that cannot be printed without a decision about filtering it.

## Motivation

The triggering incident: a user pasted a raw tty log (captured with `script`)
into an editor-composed reply.
JP echoes editor-composed messages back to the terminal, and the log's escape
sequences — cursor positioning, line erasure, color state — executed on the
user's terminal and corrupted its display.
Because the message is stored verbatim, every re-render (`jp conversation
print`, `--replay`, scrolling back through history) replays the corruption.

The display glitch is the benign version of the problem.
The same render pipeline prints LLM output and tool results, and those are
untrusted by construction: a model — or a prompt-injected web page flowing
through a tool result — can emit sequences that:

- move the cursor and rewrite earlier lines, e.g. to spoof a tool-approval
  prompt or alter text the user already read;
- erase or scroll content out of view to hide it;
- switch terminal modes (alternate screen, bracketed paste, mouse reporting),
  breaking JP's own prompts;
- write the clipboard (OSC 52) or retitle the window (OSC 0/2) on supporting
  terminals.

This is a known attack class for LLM CLIs; several comparable tools have shipped
CVEs for exactly this.

Empirically, at least one supported provider passes functional ANSI escape
sequences through verbatim: a raw `ESC` byte from the model stream reaches the
client and executes if rendered to a terminal.
JP cannot assume providers strip them.

The LLM stream is not even the most exposed path.
Echoed user messages (the triggering incident) and tool results (fetched web
pages, file contents, colored build output) carry real escape bytes without
passing through a model at all.

There is also a narrower injection point: JP embeds the conversation title —
LLM-generated text — into an OSC 2 window-title sequence.
A title containing a `BEL` or `ESC \` terminator ends the OSC early and injects
whatever follows as raw terminal input.

Markdown opens a path around any byte filter.
CommonMark decodes numeric character references, so `&#27;[2J` in model output
parses to a real `ESC` followed by `[2J` in a text node, and JP's formatter
writes text nodes unfiltered.

Styling that is allowed can still bleed.
JP decides where content ends on screen, and that is not always where the
content itself ends: a truncated tool result keeps only its first lines.
A user-built tool's diff lost its closing reset that way, and its red ran into
JP's truncation note and the next reasoning block ([#1201]).
Content can open styling; only JP can guarantee it is closed before JP writes
again.

If we do nothing, every render of hostile or accidental control bytes executes
them, and the stored conversation makes the problem permanent.

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

Conceal is removed from the SGR parameter list, not the whole sequence:
`\x1b[8;31m` keeps its red.
Parameters consumed by the extended color introducers `38`, `48`, and `58` (e.g.
`38;5;8`, `48;2;…;8`, and their colon-separated variants) are color payload,
never conceal.
This is the parameter walk [RFD 091]'s status-region filter already performs
(`visible_sgr`), and both filters share it.

A dropped sequence is removed in `strip` mode or replaced by a single visible
`␛` (U+241B) marker in `visualize` mode.
A removed conceal parameter is marked the same way.
Strip is the default: pasted logs render as clean text.
Strip mode preserves printable text outside disallowed control sequences;
payload bytes inside dropped sequences (an OSC title's text, OSC 52's clipboard
data) are not rendered — use `visualize` to mark their presence, or `off` to
inspect raw terminal behavior.

### Content classes

Whether SGR survives depends on where the content comes from:

| Class            | Examples                                                                     | SGR     | Content span                  |
| ---------------- | ---------------------------------------------------------------------------- | ------- | ----------------------------- |
| User messages    | Echoed editor-composed and replayed requests                                 | Kept    | Closed after formatting       |
| Tool output      | Tool results, custom-formatter output                                        | Kept    | Closed after the written part |
| Model output     | LLM message and reasoning text                                               | Dropped | None                          |
| Derived strings  | Conversation titles, `jp conversation grep` hits                             | Dropped | None                          |
| Tool prompt text | Question pre-amble, question text, select option labels, text-answer default | Dropped | None                          |

- **User messages** keep styling: the user wrote them, and a pasted colored log
  should look like the log.
  A message renders in one `format_terminal` call, so its span closes once, on
  the formatted output.
- **Tool output** keeps styling: JP's local tools and user-built tools color
  their output (a diff, in [#1201]).
- **Model output** is plain text.
  It streams in chunks that JP interleaves with kind transitions,
  reasoning-budget cuts, and code fences; with no content styling, none of those
  needs a reset.
  The model styles text through markdown, which JP renders.
- **Derived strings** are plain text: they are laid out against a display-width
  budget (see Placement), and JP styles them itself.
- **Tool prompt text** is plain text: `jp_tool::Question` documents its text as
  a single plain line, and the prompt is where the user approves tool calls.
  This is a display contract, not sanitization, and holds under every
  `style.sanitize` mode.

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

The knob governs the render-pipeline sanitizer wherever it is wired — live
streaming and history re-rendering alike.
Under `off` every class passes its sequences through, and classes without a span
behave as they do today.
OSC embedding hardening (below), the closing reset of content spans (below), the
plain-text rule for tool prompt text, and the non-pretty ANSI stripping are
independent of it.

### Content spans

The renderer writes user messages and tool output inside a *content span*: it
opens the span where it starts writing the content and closes it before it
writes chrome again.
Inside the span the class's policy applies.
Closing the span emits `\x1b[0m`.

JP does not control what content styling does inside the span, only that none of
it survives the span.
Content that opens a color and never closes it, or whose closing reset JP never
wrote, stays colored up to the end of the span and no further.

The span closes where JP stops writing the content, not where the content ends.

**Tool output** closes after the last kept line, before the closing fence and
any truncation note.
A tool result truncated to two lines is written as its kept lines, then the span
closes, then JP writes its truncation note:

```text
tool output: \x1b[31mline 1\nline 2\nline 3\x1b[0m
written:     \x1b[31mline 1\nline 2\x1b[0m\n _(truncated to 2 lines)_
```

The span closes before the final line break, so a background the content left
open does not paint the row below it.

**User messages** close after `format_terminal` returns: the reset is inserted
into the formatted output before its trailing line breaks, never into the
markdown source.
The formatter ends its output with a line break, and a break written under a
background the message left open paints the row below it.
A reset in the source could change how the message parses; after a closing fence
with no trailing newline, it turns the fence into an opening one.

Closing the span does not depend on `style.sanitize`.
Under `off` the span passes every sequence through and still closes with the
reset: the reset changes where content styling ends, never what content shows.

### The sanitizer

A `fmt::Write` decorator in `jp_term` (working name `sanitize::ContentWriter`),
the same shape as `jp_term::shade::ShadedWriter` and built on the same
`vte::Parser` foundation as the existing `jp_printer::ansi::AnsiStripper`.
It is constructed for a content class; for a class with a span, constructing it
opens the span, and `finish()` settles any dangling partial sequence and closes
the span.
The parser state persists across writes, so a sequence split over two stream
events is still recognized — the same property `AnsiStripper` already needs for
non-pretty output.
The difference is policy: `AnsiStripper` drops everything; `ContentWriter`
applies the allowlist above, keeping or dropping SGR by class.

The writer sits upstream of any decoration JP applies to the whole write.
In a reasoning region, tool-result bytes flow `ContentWriter` → `ShadedWriter`
→ channel: the region's fill and erase escapes are added after filtering, so
the allowlist never sees them, and the span's closing reset reaches
`ShadedWriter` as a content reset, which re-asserts the region background.

Model output uses one instance per content kind per turn: reasoning and message
chunks interleave within a turn (`ChatRenderer::flush_on_transition`), and a
shared instance would join a partial sequence from one kind with bytes from the
other.
`finish()` runs at kind transitions and turn boundaries, so a dangling
introducer never joins the next kind's bytes.
Model output has no span, so `finish()` emits no reset there.

### Markdown-decoded text

The byte filter sees markdown source.
`jp_md` parses it with comrak, which decodes character references in text nodes,
link URLs, and link titles: `&#27;` becomes a real `ESC` after the filter has
run.
The formatter therefore filters decoded text a second time, in `jp_md::render`
(`format_text`, `format_link`, `format_image`), before `TerminalWriter` adds
JP's styling:

- **Model output** removes every control character except `\n` and `\t`, the
  whitespace the policy table keeps.
  The byte filter already dropped all escapes, so any other control character
  left is one an entity produced.
  This is a per-character check, not a parse.
- **User messages** apply the allowlist again: a decoded `ESC` and a raw one
  that the byte filter kept are the same byte after parsing.
  This is a second vte pass, once per render, over text the user wrote.

The formatter receives the class's decoded-text policy through its options.
The upstream byte filter stays: on the streaming path, fenced code lines skip
comrak (`ChatRenderer` → `Formatter::render_code_line`).

On naming: `sanitize` already appears in JP with two other meanings — storage
sanitization ([RFD 052]'s `Workspace::sanitize` / `SanitizeReport`) and
conversation-stream repair (`ConversationStream::sanitize`).
This RFD adds a third: display sanitization, scoped to `jp_term` and the render
pipeline.
The implementation updates the ubiquitous-language documentation to define all
three.

### Placement

The byte filter runs where content enters the render pipeline, **before**
markdown parsing; the decoded-text filter runs inside the formatter, **before**
JP's styling:

```
untrusted text ──▶ ContentWriter ──▶ jp_md Buffer ──▶ Formatter ──▶ Printer
                    (byte filter)                     (decoded-text filter,
                                                       then chrome styling)
```

It cannot happen at the printer: by that point trusted `jp_md` styling and
untrusted content bytes are interleaved and indistinguishable.
SGR bytes kept in user messages still flow into `jp_md`'s parser and can sit
mid-token (inside emphasis or fence markers); that interaction is unchanged from
today.

Concretely:

1. `ChatRenderer::render_request` — the echo of editor-composed and replayed
   user messages (the triggering incident).
   SGR kept; the span closes on the formatted output.
2. `ChatRenderer::render_content` / `render_reasoning` — streamed LLM output,
   with one `ContentWriter` per content kind to handle chunk-split sequences
   (see above).
   SGR dropped; no span.
3. Tool-result rendering (`jp_cli::render::tool`) — audit for text paths that
   bypass `jp_md` and wrap them.
   SGR kept; the span closes after the kept lines.
4. History re-rendering (`jp conversation print/show`) — covered where it
   reuses the chat renderer; audit for direct prints.
5. Table, list, and hit rendering of conversation-derived strings — `jp
   conversation ls` prints LLM-generated titles into a table, and `jp
   conversation grep` prints matched lines drawn from message, reasoning, and
   tool-result text.
   Both lay that text out against a display-width budget computed with
   `jp_term::width`, so embedded control bytes corrupt the visible output and
   the width computation at once.
   SGR dropped, so the text reaches the truncator escape-free.
6. Tool prompts — `ToolPrompter::prompt_question` writes `question.pre_amble`
   straight to the prompt writer and hands `question.text` and select options to
   the prompt backend, along with a text question's default answer.
   Each is stripped of control characters for display (`\n` and `\t` kept in the
   pre-amble only, which carries multi-line content such as a diff under
   review).
   Display never changes the answer the tool receives: select options show
   stripped labels and the chosen index maps back to the original option, and a
   text question shows its stripped default but returns the original default
   when the user accepts it unchanged.

### Output floor

The per-class policy only protects the paths that apply it.
A command that prints a conversation title, or a line of a tool result, without
going through a `ContentWriter` writes whatever the bytes say.
The *output floor* is the least filtering every printed byte gets, whoever
printed it.

Under a pretty format the printer worker passes each print task through an
`OutputFloor` (`jp_term::sanitize`) on its way to its stream, one floor per
stream so a sequence split between two tasks is recognized whole.
The floor keeps what JP's own output is made of, and drops everything else:

- printable text, `\n`, `\t`, and `\r`;
- SGR (`CSI … m`), less conceal;
- erase to the end of the line (`\x1b[K`);
- OSC 8 hyperlinks, with control characters removed from the target.

What it drops follows `style.sanitize` like the content policy: removed under
`strip`, marked with `␛` under `visualize`, and passed through under `off`.
The mode is the one the run renders under: set when the run's context is
built, when a long-running host swaps configs, and by each turn's chat renderer,
so a conversation rendered with its own `style.sanitize` gets the floor that
setting asks for.

Two writes bypass it.
A prompt widget's own drawing (`PrintOrigin::Widget`) moves the cursor and
switches terminal modes on purpose, so whatever text a caller hands a widget
(picker rows, a confirmation's preamble) has to be filtered by that caller.
Status-region frames are written by the worker itself, under [RFD 091]'s
stricter filter.

The floor does not replace the per-class policy: it cannot tell content from
chrome (see Placement), so it lets through everything chrome needs, including
styling model output must not carry and the `\r` and erase that rewrite the
current line.
It bounds what a forgotten path can do to restyling text and rewriting its own
line.

### Stored strings are typed

The floor bounds a forgotten path; a type makes forgetting visible.
A conversation's title is `jp_conversation::Title`, stored and serialized as a
bare string, with no `Display` and no dereference to `str`.
Code reaches its text either through `raw()`, named for what it is and used for
storage, machine-readable output, plugins, and the macOS app, or through
`jp_cli`'s derived-text filter (`DerivedText::title`), which shows it the way
`jp conversation ls` shows titles.
A picker row is redrawn in place, so a title in a picker is always one line of
plain text, whatever `style.sanitize` says.

A new display site that formats a title does not compile until it picks one of
the two.
Only titles are typed; `jp conversation grep` hit lines and a message's author
name are still strings, and the floor is what bounds them.

### OSC embedding hardening

Independent of the configurable sanitizer, `jp_term::osc` escapes the dynamic
strings it splices into OSC sequences by removing all control characters — C0
(including `BEL` and `ESC`), DEL, and C1 code points — covering both the `BEL`-
and `ST`-terminated forms.
The module has exactly two embedding positions today:

- `set_title` — the conversation title (LLM-generated text) inside OSC 2.
- `hyperlink` — the URI inside OSC 8.
  Call sites splice model-influenceable strings into this position:
  `jp_cli::render::tool` builds `file://` / `copy://` URIs from tool-created
  paths, and `jp conversation ls` builds `jp://` URIs from IDs.
  Only the URI is escaped; the link *text* argument sits between the OSC
  open/close sequences in ordinary display space, legitimately carries SGR
  styling, and is covered by the general sanitizer instead.

Sanitization applies to content *strings* before they are spliced into chrome,
never to already-assembled chrome.
Raw OSC 8 arriving in content is dropped by the sanitizer; JP-authored OSC 8
built via `jp_term::osc::hyperlink` is trusted chrome after URI escaping.

This escaping is unconditional — there is no legitimate title or URI containing
control characters — and ships even when `sanitize = "off"`.

### What does not change

- **Stored data and LLM input.** Conversations keep the verbatim bytes, and the
  model receives them unmodified.
  Sanitization is a display concern; the debugging session that motivated this
  RFD depended on the model seeing raw escape bytes in a pasted log, and that
  must keep working.
- **Non-pretty output.** The `out`/`err` sinks already strip *all* ANSI via
  `AnsiStripper` for non-pretty formats; that behavior stays.
- **Chrome.** JP's own styling and widgets, and the controls the
  reedline/inquire prompts draw, look the same: the floor keeps everything
  chrome is made of, and a widget's drawing bypasses it.
  Tool-supplied strings shown inside a prompt are content (see Placement).

## Drawbacks

- A tool or user who deliberately emits cursor-control art loses it (until they
  set `sanitize = "off"`).
- Allowed SGR from tool output can clash with `jp_md`'s styling state: a `SGR 0`
  reset inside a code block resets the block's background fill until the
  formatter's next own write.
  This exists today; sanitization neither fixes nor worsens it.
- A vte parse per content byte on the streaming path — negligible next to
  network latency, but nonzero.
  User messages get a second vte pass over their decoded text nodes, once per
  render; model output's decoded text gets a per-character check, not a parse.
- Model output renders uncolored even when it carries SGR, e.g. a model quoting
  a colored log.
  The model still receives the raw bytes, and `sanitize = "off"` shows them.
- Character references in user messages that decode to SGR style the text: a
  user typing `&#27;[31m` gets red.
  It is the user's own message, and only SGR survives.
- Dropping `\r` turns tool progress redraws (`\r`-overwritten lines, common in
  build tools) into concatenated text in rendered tool results.
  CRLF line endings are unaffected, since `\n` survives.
- Allowing SGR keeps one residual hiding trick: matching foreground to
  background color.
  Blocking that requires tracking color state against the theme, which is not
  worth the complexity now (see Risks).
- The output floor parses everything printed in a pretty format a second time.
- What the floor allows, a forgotten path can still do: restyle text, rewrite
  its own line with `\r` or an erase, or show a link whose text misrepresents
  its target.

## Alternatives

- **Strip all escape sequences from content.** Simplest and safest, but breaks
  legitimate colored tool output — an explicit requirement.
- **Sanitize at the printer sink.** Rejected as the place for the per-class
  policy: trusted chrome and untrusted content are indistinguishable there.
  Kept as a floor beneath it, which needs no such distinction because it only
  drops what chrome never uses (see Output floor).
- **Filter titles once, on load.** The loaded title also feeds storage, JSON
  output, plugins, and the macOS app, so a filtered copy and a stored copy would
  share a type, and "remember to filter" would become "remember which copy".
  The live query path renders before anything is loaded at all.
- **One policy for every content class.** Keeping SGR in model output too means
  closing a span at every kind transition, reasoning-budget cut, and closing
  fence of a streamed response, and a reset fed through markdown turns a closing
  fence without a trailing newline into an opening one.
  Model output has no legitimate need for raw SGR; markdown is its styling.
- **Keep control-producing entities as source text.** Leaves `&#27;` visible
  instead of decoding it, but needs markdown context: references are not decoded
  inside code spans.
  Filtering decoded text needs none.
- **Reset at each truncation site.** Fixes [#1201] by repeating the reset at
  every site that cuts content; every new cut (head/tail views, width budgets, a
  future renderer) reopens the bug.
- **A third-party mode on the printer.** The renderer switches the printer into
  a mode that filters content and resets styling when switched back.
  Rejected: every exit path (Ctrl-C, an early return on error, a panic) must
  switch the mode back, and a missed one filters JP's own output from then on,
  where a writer's span ends with the writer.
  Within a tool result JP also adds its own escapes before the printer sees the
  bytes (the region fill), which a printer-side filter would drop.
  A decorator is also testable without a printer.
  Revisit if region shading moves into the printer.
- **Sanitize at ingestion (storage).** Rejected: corrupts data, blinds the LLM
  to bytes the user asked about, and cannot be revisited later (a policy change
  would not restore stripped bytes).
- **Caret-notation everything (`^[[48;5;236m`).** Honest but extremely noisy for
  the common pasted-log case; offered in spirit via `visualize`, which marks
  without expanding.

## Non-Goals

- Sanitizing content sent to the LLM or stored in conversations.
- Protecting the `serve-web` HTML view — it needs HTML-escaping, a different
  mechanism with its own existing handling.
- Defending against a hostile terminal emulator itself.
- Redesigning styling; the sanitizer is formatter-agnostic and slots in front of
  whatever formatter the render pipeline uses, today or after any future styling
  redesign.
- Capability adaptation of styling (color downgrading, `NO_COLOR`, non-TTY
  stripping); that is the terminal sink's job, downstream of rendering.
  The seam: this RFD neutralizes untrusted content where it enters the render
  pipeline; capability adaptation applies to already-trusted styling on the way
  out.
  The untrusted-content SGR allowlist is owned here, not at the sink, so the two
  policies cannot drift apart.
- The status-region filter of [RFD 091].
  Status rows need stricter guarantees (no tabs, a reset on every row), because
  cursor movement or tab expansion breaks the worker's row accounting, and they
  stay filtered under `sanitize = "off"`.
  The two filters share SGR parsing and conceal removal, not policy.

## Risks and Open Questions

- **Is `strip` the right default?** `visualize` makes tampering attempts loud;
  `strip` is cleaner for the common accidental case.
  The config knob keeps this a one-line decision to revisit.
- **OSC 8 hyperlinks.** Dropped for now (link text can misrepresent the target).
  Could be allowlisted later with a scheme filter.
- **SGR sub-policy.** Conceal is removed; blink and reverse-video pass.
  If fg==bg hiding shows up in practice, tighten to a parameter allowlist.
- **Where exactly tool results bypass `jp_md`** needs an implementation-time
  audit; the design assumes wrapping is mechanical.
- **Partial sequence at stream end.** `finish()` must decide between dropping
  and visualizing a dangling introducer; proposal: treat as dropped sequence.
- **Tab width on width-budgeted paths.** `\t` is kept as content, but
  `jp_term::width` measures it as zero columns while a terminal expands it to
  the next tab stop, so a tab-bearing line overruns any budget computed from it
  and soft-wraps.
  Keeping the character is right — tabs are legitimate in file contents and
  tool output, and dropping them would silently reflow indented text.
  The open choice is where to normalize: expand to a fixed tab stop during
  sanitization, or surface tabs the way `visualize` surfaces dropped sequences.
  The failure is cosmetic and bounded (ripgrep's `--max-columns` has the same
  property), so this does not block the steps above.

## Implementation Plan

1. **`jp_term::sanitize::ContentWriter`** — the vte-based allowlist filter with
   per-class SGR policy and the span's closing reset, with conceal removal
   shared with [RFD 091]'s `visible_sgr`, and unit tests (sequence classes,
   chunk-split sequences, `finish()` with styling open, `finish()` under `off`,
   a spanless class emitting no reset).
   Independent, no behavior change until wired.
2. **Close tool-output spans** ([#1201]): in `jp_cli::render::tool`,
   `render_result` and `render_formatted_arguments` write the tool's bytes
   through a `ContentWriter` under the `off` policy, inside `write_chrome` so
   region shading stays downstream, and finish it before the closing fence and
   truncation note.
   Pass-through policy, so no config knob.
   The regression test is a result that opens a color on its first line and
   closes it past the cut, asserted against the exact output.
3. **OSC embedding hardening** in `jp_term::osc`.
   Two-line change plus tests; independent and immediate.
4. **Config knob** `style.sanitize` in `jp_config` (default `strip`).
5. **Wire the chat paths**: `render_request` (SGR kept, reset inserted before
   the formatted output's trailing line breaks), `render_content` and
   `render_reasoning` (SGR dropped, no span), and the decoded-text filter in
   `jp_md::render`; snapshot tests with escape-laden fixtures, including
   `&#27;[2J` in a model message.
   For truncated reasoning display, sanitize before applying the
   visible-character budget, so control sequences neither consume the truncation
   limit nor get split by the truncator.
6. **Audit and wire tool-result, history, and table/list rendering** (`jp
   conversation ls` titles, `jp conversation grep` hits).
   In `jp_cli::render::tool`, `write_chrome` is an output helper, not a trust
   marker: it emits both JP-authored headers and relayed tool-result text.
   Sanitize the untrusted input (custom-formatter output, and `inner_content`
   under the configured policy instead of step 2's pass-through) before it is
   formatted — code-block highlighting adds trusted styling to those bytes
   ahead of the write — and do not sanitize JP-authored headers, separators,
   temp lines, or assembled `jp_term::osc::hyperlink` chrome.
   Close the span after the kept lines, as in step 2: filtering the whole result
   and closing at its end would put the reset in the part truncation drops.
   The width-budgeted paths carry step 5's ordering requirement too:
   `jp_term::width::truncate_to_width` documents an escape-free precondition,
   since it spends the budget per grapheme cluster and would let escape bytes
   both consume columns and be split mid-sequence.
   Titles and grep hits drop SGR by class, so they reach the truncator
   escape-free; JP styles them itself.
7. **Plain-text tool prompts**: document plain text on `jp_tool::Question`'s
   `text`, `pre_amble`, select options, and text default; strip control
   characters for display in `ToolPrompter` (keeping `\n` and `\t` in the
   pre-amble), map the chosen select index back to the original option, and
   return the original default when a text question's default is accepted
   unchanged.
   Independent of the config knob.
8. **Docs**: `docs/configuration.md` entry; note in the security section of the
   README docs; ubiquitous-language entry disambiguating display sanitization
   from storage sanitization ([RFD 052]) and stream repair.
9. **Output floor**: `jp_term::sanitize::OutputFloor`, one per stream in the
   printer worker under a pretty format; `PrintOrigin::Widget` for a widget's
   own drawing, which bypasses it; the mode set from `style.sanitize` by the
   run's context, a config swap, and each chat renderer.
10. **Typed titles**: `jp_conversation::Title`, and every display site moved to
    `DerivedText::title` or, in a picker, one plain line.

Steps 1–3 and 7 merge ahead of the config knob (step 2 after step 1); 4–6 land
together behind the default.
Steps 9 and 10 came out of review of the first eight: they keep a command
written later from bringing the problem back.

## References

- [RFD 048]: Four-Channel Output Model — the channel taxonomy this RFD's trust
  classes build on.
- [RFD 052]: Workspace Data Store Sanitization — the *other* `sanitize` in JP;
  storage-level, unrelated to display sanitization.
- [RFD 091]: Printer-Owned Status Region — the status-region filter that shares
  this RFD's SGR parsing and conceal removal under a stricter policy.
- `jp_printer::ansi::AnsiStripper` — existing vte-based full strip for
  non-pretty output; the sanitizer generalizes its approach.
- [Terminal escape injection] — survey of the attack class, including OSC 52
  clipboard writes.

[#1201]: https://github.com/dcdpr/jp/issues/1201
[RFD 048]: 048-four-channel-output-model.md
[RFD 052]: 052-workspace-data-store-sanitization.md
[RFD 091]: 091-printer-owned-status-region.md
[Terminal escape injection]: https://dgl.cx/2023/09/ansi-terminal-security#vulnerabilities
