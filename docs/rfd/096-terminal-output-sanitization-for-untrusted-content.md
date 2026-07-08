# RFD 096: Terminal Output Sanitization for Untrusted Content

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-07

## Summary

JP writes conversation content — echoed user messages, streamed LLM output,
tool results — to the terminal without filtering control bytes, so content
containing escape sequences is executed by the terminal instead of displayed.
This RFD introduces render-time sanitization for untrusted content: benign
styling sequences (colors, bold) pass through, terminal-state-changing sequences
are neutralized, and stored data remains byte-for-byte verbatim.

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

If we do nothing, every render of hostile or accidental control bytes executes
them, and the stored conversation makes the problem permanent.

## Design

### Trust model

Output falls into two classes, following the channel taxonomy of [RFD 048]:

| Class       | Examples                                                                   | Policy              |
| ----------- | -------------------------------------------------------------------------- | ------------------- |
| **Chrome**  | Role headers, separators, status line, `jp_md` styling, prompt widgets     | Trusted, unmodified |
| **Content** | Echoed user messages, LLM message/reasoning text, tool result text, titles | Sanitized at render |

The boundary is authorship: bytes JP composes are chrome; bytes that originate
in conversation data, model output, or tool output are content.

### Sanitization policy

Sanitization is an allowlist over escape-sequence classes, not a blanket strip
— JP's local tools legitimately use color in their output, and that must keep
working.

| Class                                            | Action | Rationale                                         |
| ------------------------------------------------ | ------ | ------------------------------------------------- |
| Printable text, `\n`, `\t`                       | Keep   | Content.                                          |
| SGR (`CSI … m`), except conceal (SGR 8)          | Keep   | Colors/bold are display-only; tools rely on them. |
| SGR conceal (`CSI 8 m`)                          | Drop   | Hides text from the user.                         |
| All other CSI (cursor, erase, scroll, DEC modes) | Drop   | Rewrites or hides what the user sees.             |
| OSC (title, clipboard, hyperlinks)               | Drop   | Side effects beyond the character grid.           |
| DCS / APC / PM / SOS, other `ESC x`              | Drop   | Terminal-specific side effects.                   |
| Remaining C0 (including `\r`), DEL, C1           | Drop   | `\r` overwrite is the classic log-spoofing trick. |

Conceal detection operates on standalone parameters: an SGR sequence containing
a standalone `8` parameter is dropped whole (subject to the strip/visualize
mode, like any other dropped sequence).
Parameters consumed by the extended color introducers `38`, `48`, and `58` (e.g.
`38;5;8`, `48;2;…;8`, and their colon-separated variants) are color payload,
never conceal.

A dropped sequence is removed in `strip` mode or replaced by a single visible
`␛` (U+241B) marker in `visualize` mode.
Strip is the default: pasted logs render as clean text.
Strip mode preserves printable text outside disallowed control sequences;
payload bytes inside dropped sequences (an OSC title's text, OSC 52's clipboard
data) are not rendered — use `visualize` to mark their presence, or `off` to
inspect raw terminal behavior.

```toml
[style]
# How control sequences in untrusted content are rendered.
# strip     - remove disallowed sequences (default)
# visualize - replace disallowed sequences with a visible ␛ marker
# off       - disable content sanitization for pretty terminal rendering.
#             OSC embedding is still escaped, and non-pretty output still
#             strips ANSI.
sanitize = "strip"
```

The knob governs the render-pipeline sanitizer wherever it is wired — live
streaming and history re-rendering alike.
OSC embedding hardening (below) and the non-pretty ANSI stripping are
independent of it.

### The sanitizer

A streaming filter in `jp_term` (working name `sanitize::Sanitizer`), built on
the same `vte::Parser` foundation as the existing
`jp_printer::ansi::AnsiStripper`.
The parser state persists across chunks, so a sequence split over two stream
events is still recognized — the same property `AnsiStripper` already needs for
non-pretty output.
The difference is policy: `AnsiStripper` drops everything; `Sanitizer` applies
the allowlist above.

The API is a stateful chunk transformer (`fn push(&mut self, chunk: &str) ->
String` plus a `finish()` that settles any dangling partial sequence).
Instances are scoped per content kind per turn: reasoning and message chunks
interleave within a turn (`ChatRenderer::flush_on_transition`), and a shared
instance would join a partial sequence from one kind with bytes from the other.
`finish()` runs at kind transitions and turn boundaries, so a dangling
introducer never spans kinds.

On naming: `sanitize` already appears in JP with two other meanings — storage
sanitization ([RFD 052]'s `Workspace::sanitize` / `SanitizeReport`) and
conversation-stream repair (`ConversationStream::sanitize`).
This RFD adds a third: display sanitization, scoped to `jp_term` and the render
pipeline.
The implementation updates the ubiquitous-language documentation to define all
three.

### Placement

Sanitization happens where content enters the render pipeline, **before**
markdown parsing:

```
untrusted text ──▶ Sanitizer ──▶ jp_md Buffer ──▶ Formatter ──▶ Printer
                                    (chrome styling added here, trusted)
```

It cannot happen at the printer: by that point trusted `jp_md` styling and
untrusted content bytes are interleaved and indistinguishable.
Allowed SGR bytes still flow into `jp_md`'s parser and can sit mid-token (inside
emphasis or fence markers); that interaction is unchanged from today.

Concretely:

1. `ChatRenderer::render_request` — the echo of editor-composed and replayed
   user messages (the triggering incident).
2. `ChatRenderer::render_content` / `render_reasoning` — streamed LLM output,
   with one `Sanitizer` per content kind to handle chunk-split sequences (see
   above).
3. Tool-result rendering (`jp_cli::render::tool`) — audit for text paths that
   bypass `jp_md` and wrap them.
4. History re-rendering (`jp conversation print/show`) — covered where it
   reuses the chat renderer; audit for direct prints.
5. Table and list rendering of conversation-derived strings — `jp conversation
   ls` prints LLM-generated titles into a table with its own display-width math;
   embedded control bytes corrupt both the cells and the width computation.

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
- **Chrome.** JP's own styling, widgets, and the reedline/inquire prompts are
  untouched.

## Drawbacks

- A tool or user who deliberately emits cursor-control art loses it (until they
  set `sanitize = "off"`).
- Allowed SGR from tool output can clash with `jp_md`'s styling state: a `SGR 0`
  reset inside a code block resets the block's background fill until the
  formatter's next own write.
  This exists today; sanitization neither fixes nor worsens it.
- A vte parse per content byte on the streaming path — negligible next to
  network latency, but nonzero.
- Dropping `\r` turns tool progress redraws (`\r`-overwritten lines, common in
  build tools) into concatenated text in rendered tool results.
  CRLF line endings are unaffected, since `\n` survives.
- Allowing SGR keeps one residual hiding trick: matching foreground to
  background color.
  Blocking that requires tracking color state against the theme, which is not
  worth the complexity now (see Risks).

## Alternatives

- **Strip all escape sequences from content.** Simplest and safest, but breaks
  legitimate colored tool output — an explicit requirement.
- **Sanitize at the printer sink.** Rejected: trusted chrome and untrusted
  content are indistinguishable there.
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

## Risks and Open Questions

- **Is `strip` the right default?** `visualize` makes tampering attempts loud;
  `strip` is cleaner for the common accidental case.
  The config knob keeps this a one-line decision to revisit.
- **OSC 8 hyperlinks.** Dropped for now (link text can misrepresent the target).
  Could be allowlisted later with a scheme filter.
- **SGR sub-policy.** Conceal is dropped; blink and reverse-video pass.
  If fg==bg hiding shows up in practice, tighten to a parameter allowlist.
- **Where exactly tool results bypass `jp_md`** needs an implementation-time
  audit; the design assumes wrapping is mechanical.
- **Partial sequence at stream end.** `finish()` must decide between dropping
  and visualizing a dangling introducer; proposal: treat as dropped sequence.

## Implementation Plan

1. **`jp_term::sanitize`** — the vte-based allowlist filter with unit tests
   (sequence classes, chunk-split sequences, `finish()`).
   Independent, no behavior change until wired.
2. **OSC embedding hardening** in `jp_term::osc`.
   Two-line change plus tests; independent and immediate.
3. **Config knob** `style.sanitize` in `jp_config` (default `strip`).
4. **Wire the chat paths**: `render_request`, `render_content`,
   `render_reasoning`; snapshot tests with escape-laden fixtures.
   For truncated reasoning display, sanitize before applying the
   visible-character budget, so control sequences neither consume the truncation
   limit nor get split by the truncator.
5. **Audit and wire tool-result, history, and table/list rendering** (`jp
   conversation ls` titles).
   In `jp_cli::render::tool`, `write_chrome` is an output helper, not a trust
   marker: it emits both JP-authored headers and relayed tool-result text.
   Sanitize the untrusted input (`inner_content`, custom-formatter output)
   before it is formatted — code-block highlighting adds trusted styling to
   those bytes ahead of the write — and do not sanitize JP-authored headers,
   separators, temp lines, or assembled `jp_term::osc::hyperlink` chrome.
6. **Docs**: `docs/configuration.md` entry; note in the security section of the
   README docs; ubiquitous-language entry disambiguating display sanitization
   from storage sanitization ([RFD 052]) and stream repair.

Steps 1–2 are mergeable independently; 3–5 land together behind the default.

## References

- [RFD 048]: Four-Channel Output Model — the channel taxonomy this RFD's trust
  classes build on.
- [RFD 052]: Workspace Data Store Sanitization — the *other* `sanitize` in JP;
  storage-level, unrelated to display sanitization.
- `jp_printer::ansi::AnsiStripper` — existing vte-based full strip for
  non-pretty output; the sanitizer generalizes its approach.
- [Terminal escape injection] — survey of the attack class, including OSC 52
  clipboard writes.

[RFD 048]: 048-four-channel-output-model.md
[RFD 052]: 052-workspace-data-store-sanitization.md
[Terminal escape injection]: https://dgl.cx/2023/09/ansi-terminal-security#vulnerabilities
