# RFD 091: Printer-Owned Status Region

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-03
- **Extends**: [RFD 048]

## Summary

This RFD makes ephemeral terminal chrome — `⏱ Waiting… 9.2s (receiving
response data)` and the rows above it — a first-class concept owned by
`jp_printer`.
A **status region** is a block of terminal rows: a status row that ticks its own
elapsed time, and an optional window showing the last N lines of output from one
or more live sources.
The printer's worker thread draws the region, ticks it, and erases it before any
printer-managed write on any channel.

Nine bespoke timer and temp-line mechanisms across `jp_cli` become clients of
this one primitive, and two long-running child processes — MCP server startup
and tool execution — gain a live view of their progress that leaves no trace in
the final transcript.

## Motivation

### Nine hand-rolled mechanisms

JP renders ephemeral chrome — a single self-overwriting line on stderr — in
nine places, each with its own hand-rolled draw, tick, and clear logic:

| Site                                                      | Mechanism                                         |
| --------------------------------------------------------- | ------------------------------------------------- |
| Waiting indicator (`cmd/query/turn_loop.rs`)              | `LineTimer`                                       |
| Reasoning timer (`render/chat.rs`)                        | `LineTimer`                                       |
| Lock-wait countdown (`cmd/lock.rs`)                       | `LineTimer`                                       |
| MCP startup timer (`cmd/query.rs`)                        | `LineTimer`                                       |
| Background-task drain timers (`lib.rs`, two sites)        | `LineTimer`                                       |
| Tool "preparing" temp line (`render/tool.rs`)             | `spawn_tick_sender` + manual `\r…\x1b[K` rewrites |
| Tool execution progress (`cmd/query/tool/coordinator.rs`) | `spawn_tick_sender`                               |
| Stream-retry notice (`cmd/query/stream/retry.rs`)         | manual `\r\x1b[K` write + `clear_line`            |

All of them fight over the same invariant: **ephemeral chrome must be erased
before any persistent write, and must not disappear before the persistent write
arrives**.
Today that invariant is enforced by per-site discipline, and the codebase
carries the scars of getting it wrong: `clear_temp_line()` before the interrupt
menu, `cancel_reasoning_timer()` inside `flush_on_transition`, the "don't
pre-clear `line_active`" warning in `ToolRenderer::reset`, and the waiting
indicator's `finish().await` ordering dance in the turn loop.
Each was a bug fixed at one site; none of the fixes protects the next site.

The most user-visible instance was the waiting-indicator gap: the indicator was
torn down by the first provider event of any kind — including an SSE keep-alive
ping that renders nothing — leaving the user staring at a blank terminal for
many seconds while the model produced no visible output.
A user who sees a progress indicator vanish and *then* nothing happen reasonably
concludes the program crashed.
That instance is fixed: `LineTimer` (in `jp_cli::timer`) carries a status
channel, and the turn loop finishes the indicator only on events that render.
But the fix is one client-side patch on the missing abstraction; the bug class
remains open at every other site, and every future chrome feature reopens it.

The retry notice shows how far the discipline spreads.
`StreamRetryState::notify` parks a `\r\x1b[K` line on the cursor row and four
separate call sites are responsible for retiring it — on the first rendering
event, on a fatal error, on an interrupt, and on a refused rebuild.
It outlives the indicator that preceded it and is silently overwritten by the
next attempt's indicator, on the same row, by luck of ordering rather than by
any rule.

### Invisible long-running work

A single row is not enough for the two sites that wait on a child process.

`jp query` with an MCP server that builds from source on spawn renders `⏱
Starting MCP server bookworm… 300s` and nothing else, for five minutes.
The server is compiling; the compiler is reporting progress on its stderr; JP
discards it unless the user knew in advance to pass `-v`.
A user watching a number climb toward a `startup_timeout_secs` they did not set
cannot tell a slow build from a hang.

The same gap applies to tool execution.
`⏱ Running tool shell… 90s` says nothing about a tool that shells out to a
build, a test run, or a deploy.

The output already exists in both cases and is already read line by line.
`jp_mcp::client::spawn_stderr_forwarder` forwards each MCP server stderr line to
tracing and retains the last 100 in a ring buffer for error reporting;
`jp_llm::tool::forward_stderr` does the same for local command tools under
`tool::stderr`.
What is missing is a display surface: somewhere to show the last few lines while
the work runs, and a guarantee they are gone afterwards so the transcript is not
polluted with build noise.

Doing nothing means each new indicator re-implements draw/tick/clear, each new
combination of chrome and content is a fresh opportunity for a stale row, a
clobbered row, or a premature disappearance, and long-running child processes
stay opaque.

## Design

### Concept

A **status region** is a block of ephemeral chrome rows at the bottom of the
terminal.
It has two parts:

- A **status row**: a subject, an elapsed time, and an optional replaceable
  detail.
  Always present.
- An **output window**: zero to N rows showing the most recent lines pushed by
  the region's **sources**.
  Sized by configuration and terminal height.

<!-- end list -->

```
   Compiling serde v1.0.219              ┐
   Compiling tokio v1.47.1                │ output window (rolling, newest last)
   Compiling bookworm v0.1.0             ┘
⏱ Starting MCP server bookworm… 47.2s     ─ status row
```

The output window sits above the status row, so the status row stays adjacent to
where persistent content appears next.

The two counts are separate and this document keeps them apart, because erase
correctness depends on the physical one.
A **zero-window region** has no output rows and is therefore one physical row —
the status line as it exists today, and what every current client becomes.
A region is never zero physical rows.

A **source** is one named producer of lines feeding a region: an MCP server
starting up, a tool executing.
Several sources can feed one region concurrently; the region holds a single
rolling window across all of them, not one window per source.

The region is chrome in the [RFD 048] sense — written to stderr, never part of
the persistent transcript — with one added contract: the printer guarantees it
is erased before any printer-managed write, on any channel, reaches the
terminal.
One stderr writer lives outside the printer: the optional tracing layer (`-v`,
`--log`, `--log-file=-`), which [RFD 048] deliberately keeps out of the printer.
The worker cannot erase before writes it never sees, so the enabling predicate
below disables regions while that layer is active.

### API

Callers acquire a region from the printer and hold an RAII handle:

```rust
// Claim a status region. Returns a no-op handle when regions are
// disabled (see "Enabling predicate" below).
let region = printer.status_region(RegionStyle {
    delay: Duration::from_secs(4),
    interval: Duration::from_millis(100),
    output: OutputLines::Auto,
    format: Box::new(|secs, detail| match detail {
        Some(d) => format!("⏱ Starting {d}… {secs:.1}s"),
        None => format!("⏱ Starting… {secs:.1}s"),
    }),
});

region.set_detail("MCP server bookworm");

// Attach a source and push lines as they arrive.
let sink = region.source("bookworm");
for line in backlog { sink.push(line); }
sink.push("   Compiling bookworm v0.1.0");

// Release: the printer erases every row. Dropping the handle does the same.
drop(region);
```

General callers holding a full `Printer` — the turn loop, lock acquisition, the
shutdown drain, the MCP startup wait — acquire from `Printer` as above;
chrome-only renderers acquire through `ErrChannel::status_region` (below),
keeping the stderr-only boundary.

The handle is `Send` and requires no async runtime: there is no
`finish().await`, because the caller no longer owns the clear-before-write
ordering — the printer does.
A client that renders content simply writes through the printer as it always
has; the worker erases the region first.

The owning handle is not `Clone`; it releases the region on drop.
Three cloneable capabilities split off it, so shared use never blurs who
releases:

- `StatusDetail` — updates the status row's detail.
  Used by the turn loop's status transitions.
- `RowBackground` — sets the background the worker draws every row against.
  Used by the tool renderer to keep the reasoning shading (see "Region
  background").
- `LineSink` — pushes lines for one source.
  Dropping every clone of a sink closes that source; the lines it already pushed
  stay in the window, labelled, until they are evicted.

The format closure (rather than a fixed template) is required by existing
clients: the lock-wait and drain timers render a *countdown*, not an elapsed
time.

### Source lifetime is the client's, not the producer's

A source is open from the moment the client asks for a sink until the client
drops it.
It is not tied to the lifetime of whatever process produces the lines, and the
region never infers one from the other.

The distinction is invisible for tool execution, where the two coincide:
`jp_llm::tool::forward_stderr` returns when the tool's pipe closes, which is
when the tool call ends.
It is load-bearing for MCP.
`spawn_stderr_forwarder` runs until the *server* exits, but a server is only
interesting to the region until it finishes *starting* — and
`StartupSet::joins` completes at the `initialize` handshake, with the child
still alive and still logging.
A sink owned by the forwarder would keep a started server open in the region for
the rest of the process, letting its operational logging evict the output of the
server still compiling.

So the client owns the sink:

- `jp_mcp` sends `(McpServerId, String)` on the channel `StartupSet` carries and
  knows nothing else.
- `await_mcp_servers` holds one sink per pending server, routes each received
  line to the matching sink, and drops that sink when the server's join
  completes.
- The forwarder keeps draining for tracing and the ring buffer, unchanged.

**The status row's subject is also the client's.** Closing a source does not
rewrite it: the format closure receives elapsed time and `detail`, never the
open-source set, so a client that names its pending work in the status row keeps
naming it through `StatusDetail`.
`await_mcp_servers` already does exactly this — `mcp_startup_status(&pending)`
feeding `set_status` — and that call becomes `set_detail` unchanged.

Chrome renderers hold an [`ErrChannel`], not a `Printer` — the stderr-only view
exists precisely so tool chrome cannot reach stdout.
Regions are chrome, so acquisition is part of the chrome-facing surface:

```rust
impl ErrChannel {
    pub fn status_region(&self, style: RegionStyle) -> StatusRegion;
}
```

`ToolRenderer` migrates through this method and keeps its `ErrChannel`; it does
not regain full-printer access.

### What may enter the output window

Region content is untrusted by construction — it comes from a child process JP
did not write.
Three rules constrain it, all enforced by the primitive rather than by each
client.

**Only stderr, never stdout.** For both child-process clients, stdout is the
machine channel: MCP servers speak JSON-RPC on it, and local tools return a
`jp_tool::Outcome` payload on it.
Stderr is the human channel.
A client pushing stdout into a region would be rendering a wire protocol; the
primitive does not prevent it, but no client does it and the distinction is
named here because it is the reason the feature is safe to enable by default.

**Styling passes, control does not.** Each pushed line is filtered to an SGR
allowlist: `\x1b[…m` sequences (colors, bold, italic, underline) are forwarded,
except conceal (`SGR 8`), which hides text from the reader and has no place in a
preview; every other escape family — CSI cursor movement and erasure, OSC, DCS
— and every bare control byte other than the line terminator is dropped.
A line that leaves an attribute open is terminated with a reset, so child state
cannot bleed into JP's own chrome below it.

This is the policy `jp_md::table` already applies when truncating cells (retain
SGR, drop the rest, close with a reset), and `jp_md::ansi::is_sgr` is the same
predicate.
In `jp_printer` it is a second policy over the existing `vte` parser that backs
`AnsiStripper` — same crate, same parser, no new dependency.
Without the filter a child emitting `\x1b[2J` erases the screen and a child
emitting cursor movement corrupts the region's own row accounting; [RFD 096]
covers the same class of problem for conversation content and subsumes this
narrow case when it lands.

**Every row is truncated to the terminal width.** A row wider than the terminal
soft-wraps, and a wrapped row puts the worker's count out of step with the
screen: the erase then walks up through content that was never part of the
region.
The rule covers every physical row the worker emits — the status row and its
detail as much as a pushed line — rather than being left to each format
closure, which is the per-caller discipline this RFD exists to remove.
(`mcp_startup_line` does its own bounding today, and is the only status line
that does.)

Truncation measures visible columns, keeps SGR sequences whole, and closes any
state it cut through.
`jp_term::width::truncate_to_width` is not that function: it documents that its
input carries no escapes, because escape bytes otherwise consume the budget and
can be split mid-sequence — which would leave a half-written sequence to style
whatever follows.
A filtered line, by construction, carries escapes.
`jp_md::table::truncate_to_visual_width` demonstrates the behavior needed,
privately and in the wrong crate; the primitive needs an equivalent reachable
from `jp_printer`.

The order per row is fixed: filter controls, apply the source label, truncate by
visible columns, close any open SGR state, then draw.
[RFD 096] step 5 carries the same ordering requirement for its own
width-budgeted paths.

### Source labelling

When the window holds lines from a single source, they render verbatim.
When it holds lines from two or more, every line is prefixed with its source
label:

```
[bookworm]    Compiling serde v1.0.219
[grizzly]     Compiling tantivy v0.24.2
[bookworm]    Compiling bookworm v0.1.0
⏱ Starting 2 MCP servers (bookworm, grizzly)… 47.2s
```

Interleaved unlabelled lines from concurrent sources are worse than no lines:
they misattribute progress.
The label is applied even when the window is one row tall, where it costs most
of the width — a single unlabelled row alternating between two sources is
actively misleading, and the status row above already names which sources are
pending.

The rule keys on the sources represented in the window, not on the set of open
ones.
A source that finishes drops out of the status row's subject while its lines are
still on screen; keying on open sources would strip the labels at that moment
and hand the finished server's output to the one still running.
Retained lines keep their label until they are evicted.

### Region background

A *reasoning* region ([RFD 095]) is a different thing from the status region
this RFD defines — a span of an assistant turn, not a block of terminal rows —
and they meet at exactly one point: the background.

RFD 095 extends the reasoning background across tool chrome and names the tool
temp and progress rows as part of it: while a reasoning region with background
`B` is active, every visual row shows `B` to the right edge — including rows
produced by cursor-relative rewrites, and including the `\x1b[K` that erases
them, which fills with whatever background is active when it runs.
`ToolRenderer` holds that invariant today by routing its writes through
`jp_md::shade::ShadedWriter`.
A worker that draws and erases those rows itself, knowing nothing about the
reasoning region, would punch an unshaded hole in the middle of a shaded one —
the exact gap RFD 095 closed.

A status region therefore carries an optional row background.
The worker applies it to every row it draws, asserts it before its own erase,
and clears it at row end so it never leaks below the region.

The background is a cloneable capability like `StatusDetail`, not a claim-time
constant: the temp/progress line is a live aggregate over the tools pending now,
so it follows whichever region is active, and the client updates it as tool
calls enter and leave the reasoning region.

The region takes the background as an opaque SGR parameter — the pre-built
string `DefaultBackground` already wraps — which keeps `jp_printer` free of a
`jp_md` dependency, as the SGR filter does.
If [RFD 084] lands first and `DefaultBackground` becomes a logical color, the
client renders it to a parameter at the boundary.

### Erasure and durability

**The region is always erased on release**, whether the client succeeded or
failed.
A client never has to reason about whether prior rows survived, and code running
after a release never has to account for two possible screen states.

This is sound because a region is additive: it shows content that is recorded
elsewhere, or content that is recorded nowhere and would otherwise never be
seen.
What it may never do is take a record away.
That is the precondition on becoming a client:

> Feeding a region must not reduce what survives the erase.
> A region may be the only place output appears live; it may never be the reason
> output stops being recorded.

The scope is deliberate.
A successful build's progress lines are worth watching and not worth keeping;
neither client retains them, and neither should.

Both clients preserve their existing records untouched.
MCP server stderr stays in the 100-line ring buffer that `jp_mcp::client`
attaches to `InitializeError` and `InitializeTimeout`, so a failed *required*
startup still reports the build output that explains it.
Tool stderr is still accumulated in full by `jp_llm::tool::forward_stderr` and
— for tools that do not emit an `Outcome` payload — still reaches the model as
part of `CommandResult::RawOutput`.
Both remain on the `mcp::stderr` and `tool::stderr` tracing targets, though that
log goes to a delete-on-drop temp file unless the run itself fails: a
post-mortem aid, not a record the user will find.

Two paths carry output that is already recorded nowhere the user will see: an
optional MCP server that fails to start, and a tool that emits a valid
`Outcome::Error` with the detail on stderr.
Rendering those in a region and erasing them leaves the user no worse off than
today and briefly better informed, so the precondition permits it.
Neither is thereby fixed — the first is closed by phase 5, the second is
recorded under Risks — but neither blocks a client.

### The push path is lossy

A tool shelling out to a verbose test run emits stderr faster than a terminal
can usefully show it, and the rolling window bounds only what has already
reached the worker.

The path from child to window is more than one hop.
For MCP it is four:

```text
spawn_stderr_forwarder → tagged channel → await_mcp_servers → LineSink → worker
```

The rules below hold at **every** hop, not just at `LineSink`.
A bound enforced only at the last one is no bound at all: the queue simply backs
up in front of it.

**Child drainage never waits on rendering.** `forward_stderr` and
`spawn_stderr_forwarder` have to keep reading, or the child blocks on a full OS
pipe and the tool call or the MCP handshake hangs.
`run_tool_command` makes this concrete — it joins `forward_stderr` with
`child.wait()`, so a forwarder parked on a send never lets the invocation
complete.
No send on any hop awaits display capacity.

**Pressure is absorbed by dropping, not by queueing.** Every hop is bounded and
drops its oldest entries when full, which is invisible: the window shows the
most recent lines by definition.
A push into the worker raises at most one pending redraw command, so a burst of
a thousand lines costs one wakeup rather than a thousand commands sitting in
front of the next persistent write.
The concrete channel type is an implementation choice; bounded, non-blocking,
and drop-oldest are not.

**The channel is the only backlog.** A server can spend minutes compiling before
anyone drains it: `configure_active_mcp_servers` spawns the startups, and
`await_mcp_servers` — which holds the receiver — is not reached until later in
the same command, with `delay_secs` on top of that before the region is visible.
Whatever is queued when the region opens *is* the backlog, drained in order, and
the drop-oldest bound is what keeps it to the last few lines rather than the
whole build.
The diagnostic ring buffer in `jp_mcp::client` is not a second source: it keeps
doing its existing job of attaching stderr to initialization errors, and the
region never reads it.
Seeding from both would render the same lines twice.

With regions disabled by the predicate below, the sink is a no-op and the
producer side is never created, so nothing is left holding a receiver that no
one drains.

### Release contract

- Releasing (dropping the owner) *enqueues* a release command.
  Commands enqueued from one thread stay ordered: a release followed by a print
  from the same thread is processed in that order.
- Every `Print` with non-empty content erases a drawn region before writing,
  whether or not a pending release has been processed.
  A stale region can never sit above content.
  "Non-empty" is byte-level, not glyph-level: newline-only and
  control-sequence-only writes erase too.
  Empty-content tasks are no-ops (no erase, no redraw), and the region's own
  draw and erase writes are exempt — the worker does not recurse.
- A released entry is never redrawn once its release command is processed.
- Across threads, drop is eventual cleanup only: a released entry may be redrawn
  once more if another thread's print is processed before the release command
  drains.
  The stale window is bounded by the queue, and the second rule keeps the stale
  region below content.
- Pushing a line is not a `Print`: it mutates region state and raises at most
  one coalesced redraw, never a persistent write.
  Pushes on a released region are dropped.
- The worker erases before it exits.
  Processing `Shutdown`, or finding the command channel disconnected, erases any
  drawn region first and does not redraw the entry below it.
  `Ctx::drop` calls `Printer::shutdown`, so any exit that runs destructors —
  including one that ends a turn with a region still drawn — leaves the
  terminal clean; hard termination stays best effort.

The design deliberately provides no blocking release.
Same-thread ordering covers the release-then-render pattern used by every
current client, and a blocking release would reintroduce the async ordering
surface (`finish().await`) this design removes.

Suspension is the exception, and for a reason that does not apply to release:
its whole purpose is to hand the terminal to a writer that is *not* in the
queue, so there is no ordering to inherit (see "Interactive sessions").

### Enabling predicate

A region renders iff:

1. the resolved output format permits terminal control (`text-pretty`),
2. the chrome channel (stderr) is an interactive terminal, and
3. no tracing layer writes to stderr (`-v`, `--log`, or `--log-file=-`, absent
   `--quiet`).

| Situation                                     | Region                                                                                              |
| --------------------------------------------- | --------------------------------------------------------------------------------------------------- |
| `--format json` / `json-pretty`               | off (chrome is NDJSON)                                                                              |
| `--format text`                               | off (the non-pretty `Sink` strips `\r` and escape sequences; the rows would smear as repeated text) |
| `text-pretty`, stderr is a terminal           | on                                                                                                  |
| stderr is not a terminal                      | off, regardless of format                                                                           |
| `-v` / `--log` / `--log-file=-` on a terminal | off (stderr carries live logs, a persistent stream outside the printer)                             |

The output window has one further condition: it renders only when the terminal
height is known.
With an unknown height the region falls back to a bare status row, since the
worker cannot bound a window it cannot size.

Condition 2 changes the tty *source* for this chrome from stdout to stderr:
today chrome gating keys off stdout (`ctx.term.is_tty`), so `jp query 2>file`
with stdout on a terminal writes `\r\x1b[K` bytes into the file — under this
predicate it does not.
`--format auto` continues to resolve by stdout tty-ness per [RFD 048]; this RFD
does not change format resolution.

Condition 3 mirrors the guarantee's scope: tracing writes to stderr directly,
behind the worker's back, and a user opting into live logs has chosen stderr as
a persistent stream where an ephemeral line cannot survive.
The shell knows this at logging setup, before the printer is constructed, so it
feeds the same constructor input as condition 2.

To make condition 2 explicit and testable, terminal capability is a constructor
input: `Printer::terminal` captures stderr's tty-ness *and the terminal's row
and column count* at construction, and the memory constructor accepts an
explicit capability override so tests can exercise draw, erase, and redraw at
any geometry without a real terminal.
The exact constructor shape is an implementation detail.

Both dimensions are capabilities of the chrome channel, so they belong next to
tty-ness in the printer rather than in `jp_cli::ctx::Term`, which carries
`width` for layout and nothing else.
The existing `OutputWidth` cannot serve: it describes stdout, and
`detect_output_width` returns `Unknown` as soon as stdout is not a terminal,
before it ever measures one.
`jp --format text-pretty query > answer.txt` satisfies the predicate above —
the format is pinned rather than resolved, and stderr is still a terminal —
while stdout's width is `Unknown`.
A region drawn against that width would be drawn against no width at all, and a
wrapped row is exactly what the erase cannot survive.
(Plain `jp query > answer.txt` resolves `auto` to `text` and disables the region
outright, so it is not the case that exposes this.)

### Worker integration

The printer already serializes every write — stdout, stderr, and `/dev/tty` —
through one background worker thread (`Worker::run` in `jp_printer::printer`).
That choke point is the entire architectural argument for this design: it is the
only place in the process where "clear chrome, then write content" can be made
atomic with respect to all three channels.

Four changes to the worker:

1. **State.** The worker holds a stack of active region entries (claim
   `Instant`, format closure, current detail, resolved window budget, the set of
   open sources, and a rolling `VecDeque` of filtered lines capped at the
   budget).
   The top entry is the one rendered.
   Claim, detail-update, background-update, source-close, and release arrive as
   new `Command` variants; line pushes go to the bounded buffer above and raise
   a single coalesced redraw.
2. **Ticking.** The worker's `rx.recv()` becomes `rx.recv_timeout(interval)`
   while a region is active; on timeout it redraws the top entry with updated
   elapsed time.
   With no active entry, it blocks as today.
   This removes the tokio timer tasks entirely — timing moves to the thread
   that owns the terminal.
3. **Erase-before-write, redraw-after.** Before processing any `Print` task with
   non-empty content, the worker erases the drawn region; after the task
   completes, it redraws it.
   Coexistence with streaming content is therefore at *task* granularity: the
   region redraws between print tasks, and a long typewriter task — one
   blocking loop inside the worker — hides it until the task completes.
   For instant prints (tool chrome, block-at-a-time streaming) this approximates
   the cargo/indicatif model.
   Clients that want disappear-on-content behavior (the waiting indicator)
   simply release the handle at that moment, as they already do.
4. **Row accounting.** The worker tracks how many rows it last drew and moves
   only in relative terms — it never queries the terminal for the cursor
   position and never uses absolute positioning.
   Drawing a region taller than one row first emits that many newlines to force
   the terminal to scroll and reserve the space, then moves back up to fill it;
   otherwise a region claimed while the cursor sits on the last row scrolls the
   content it is about to overwrite and every subsequent cursor-up is off by the
   scrolled amount.
   Erasing walks the same count back up, clearing each row.
   This is the mechanism `indicatif` uses for multi-row progress, and it is the
   part of the design most likely to need a spike before the code lands.

### Concurrent claimants

Claims form a stack (LIFO): the most recent claim is rendered; releasing it
re-exposes the one below; releasing a non-top entry removes it from the middle.
A stack matches the actual nesting in the code — a tool "preparing" line
claimed during a streaming cycle sits on top of nothing today, but the moment
two indicators overlap (e.g. reasoning timer active when a tool call starts
streaming), LIFO produces the intuitive result without either site knowing about
the other.

### Interactive sessions

JP's prompts do not bypass the printer: prompt output flows through
`Printer::prompt_writer()` / `owned_prompt_writer()` as `PrintTarget::Tty`
tasks, serialized by the same worker.
The erase-before-write rule covers prompts exactly as it covers stdout and
stderr.

Erase-before-write is not sufficient for them, though.
A prompt session is a sequence of small `Tty` writes with the widget owning the
cursor in between; a region redraw landing between those writes corrupts the
widget.
Suspension is therefore tied to the prompt-writer boundary, not to call sites:
acquiring a prompt writer (`prompt_writer()` or `owned_prompt_writer()`)
suspends region rendering — the rows are erased and redraws are blocked — for
the writer's lifetime.
Prompt code carries no guard obligation; the prompt sites spread across `jp_cli`
(`ToolPrompter`, the interrupt handler, `cmd/init.rs`, `cmd/target.rs`, the
`conversation` subcommands) need no changes.

Two consequences for the implementation:

- `PrinterWriter` is currently `Copy`; a suspension-carrying writer needs a
  guard type.
  The concrete shape is an implementation detail.
- The lock-contention prompt renders via `err_writer()` today, outside the
  boundary; it migrates to `prompt_writer()` alongside the lock-wait countdown
  (phase 3).

One explicit guard remains, for the single writer genuinely outside the printer:
the external `$EDITOR`, which takes over the terminal as a child process and
touches no prompt writer.

```rust
let _pause = printer.suspend_status(); // erases the region, blocks redraws
// ... run $EDITOR ...
// guard drop: redraw resumes
```

**Acquiring the guard blocks.** `suspend_status` returns only once the worker
has erased any drawn region, entered the suspended state, and flushed the chrome
writer.
Prompt writers can settle for an enqueued suspension because their writes are
`Tty` tasks behind it in the same queue; `TerminalEditorBackend` spawns a child
that writes straight to the terminal, so an enqueued suspension races it — the
editor paints, then the worker's pending redraw lands inside the editor's
screen.
The printer already blocks on an acknowledgement for `flush`, `flush_instant`,
and `shutdown`; this is the same barrier.

Guard drop stays asynchronous: whatever the caller renders next is a printer
write, and the queue orders it behind the resume.

### Configuration

The four config blocks that gate today's timers — `style.mcp_startup`,
`style.lock_wait`, `style.tool_call.progress`, `style.streaming.progress` —
keep their `show`, `delay_secs`, and `interval_ms` keys and their current
defaults.
They map onto `RegionStyle` unchanged.

`delay_secs` gains a second meaning rather than a sibling key: it is the point
at which the region becomes visible, output window included.
One threshold, one concept — "this has taken long enough to be worth showing".

The two blocks whose subject is a child process gain one key:

```toml
[style.mcp_startup]
delay_secs = 4
print_stderr = true   # false | true | N
```

- `false` or `0` — no output window.
  Today's behavior.
- `true` — window sized from the terminal height (height / 10, so a 40-row
  terminal shows 4 rows), and at least one row whenever the terminal has a row
  to spare above the status row.
- `N` — window of exactly `N` rows, capped by the same height budget.

The count is total, not per source: `print_stderr = 1` is a single row that
every source swaps for its latest line.
An unknown terminal height falls back to a bare status row whatever the value
is, per the enabling predicate.

This is the shape [`InlineResults`] already uses for
`conversation.tools.*.style.inline_results` (`off` / `full` / a number, with a
bool accepted as a synonym for the outer two), including the hand-written
`Deserialize` visitor and the `#[variant(fallback)]` numeric case.
Following it keeps one idiom for "off, automatic, or a count" across the config
tree, and gives both keys the same accepted values by construction.

The defaults differ by block, because the two waits differ:

| Key                                     | Default | Why                                                                                                                      |
| --------------------------------------- | ------- | ------------------------------------------------------------------------------------------------------------------------ |
| `style.mcp_startup.print_stderr`        | `true`  | The wait owns the screen: nothing else is streaming, and a silent five-minute build is the case that motivates this RFD. |
| `style.tool_call.progress.print_stderr` | `false` | A tool region can be live while assistant output streams, where extra rows cost the most and flicker scales with them.   |

Defaulting `style.mcp_startup.print_stderr` to `true` is a deliberate
compatibility change: a user with no config sees a build preview where a one-row
timer stood.
Setting it to `false` restores the one-row timer.

The key belongs only on the two blocks that have a child process.
`style.lock_wait` waits on a file lock and `style.streaming.progress` waits on
an HTTP response; neither has a stderr to show.
That matters because `ProgressConfig` is currently shared — `style.streaming`
imports it from `style::tool_call` — so `style.tool_call.progress` needs its
own type to carry the key without leaking a meaningless
`style.streaming.progress.print_stderr` into the schema.
The two blocks were identical by coincidence, not by design, and this is where
they diverge.

### Migration

All nine sites become clients, seven of them as zero-window regions.
`jp_cli::timer` (`LineTimer`, `spawn_line_timer`, `spawn_tick_sender`) is
deleted once the last client migrates.
The tool "preparing" temp line keeps its separator bookkeeping and its
temp-to-permanent header conversion in `ToolRenderer`; only the draw, tick, and
erase mechanics move to the printer.

The stream-retry notice becomes a zero-window region whose owner spans the
backoff and is released on the first rendering event, an interrupt, or a fatal
error; `clear_line` and its four call sites go with it.
Its non-terminal behavior is unchanged: with regions disabled the client prints
each notice as a persistent line, which is what its `is_tty` branch does today.

[RFD 092] replaces both background-drain timers with a single shutdown watchdog
and says its countdown absorbs their line.
If it lands first, that countdown is the client and the two drain-timer rows are
already gone; if this RFD lands first, the watchdog claims a region instead of
spawning a timer.

Two clients gain an output window, and each needs a line channel out of the
crate that owns the child:

- **MCP startup.** `jp_mcp::client::spawn_stderr_forwarder` gains a channel
  alongside tracing and the ring buffer, tagging each line with its
  `McpServerId`.
  `StartupSet` — already the handoff type between `run_services` and
  `await_mcp_servers` — carries the receiving end.
  `await_mcp_servers` owns the sinks and their lifetimes, per "Source lifetime
  is the client's, not the producer's".
- **Tool execution.** `jp_llm::tool::forward_stderr` gains the same channel.
  It already reads line by line and accumulates in full, so the captured bytes
  are unaffected.

Neither producer filters, truncates, labels, or decides when a source closes;
all four are the region's or the client's job.

## Drawbacks

- **The printer becomes stateful across writes.** Today each `Print` task is
  independent; with this change the worker carries cross-task state (the claim
  stack, drawn/not-drawn) that every write path implicitly interacts with.
  Complexity is conserved ([Tesler]): it moves out of nine call sites into one
  primitive — but bugs in that primitive now affect all chrome at once.
- **`jp_printer` is foundational.** Every crate that prints depends on it; its
  API surface grows, and per Hyrum's Law the rendered chrome format becomes
  something users' scripts may match on.
- **The worker gains a timing loop.** `recv_timeout` polling is cheap but makes
  the worker's behavior time-dependent, which is harder to test than the current
  pure command-processing loop.
- **Multi-row erasure is strictly riskier than one-row erasure.** A single line
  is corrected by `\r\x1b[K` from wherever the cursor happens to be; an N-row
  block depends on the worker's own row count being right.
  When it is wrong the failure is visible and ugly — duplicated rows, eaten
  content — rather than a subtly stale line.
- **The printer learns about terminal geometry.** Rows and columns join tty-ness
  as constructor inputs, which is two more things that can be stale after a
  resize.
- **The printer carries a styling decision it cannot read.** The row background
  exists only so that an assistant-output setting (`style.reasoning.background`)
  survives across chrome the printer now draws.
  `jp_printer` gains a parameter it forwards without interpreting, and the
  invariant that makes it correct is documented in another RFD.
- **Two crates gain a chrome-shaped output.** `jp_mcp` and `jp_llm` grow a line
  channel whose only consumer is the terminal.
  It keeps them ignorant of rendering, but the plumbing exists for a display
  concern, and a crate that previously only logged now also feeds a UI.
- **Child output reaches the screen without the user asking for it.** With
  `style.mcp_startup.print_stderr` defaulting to `true`, a startup wrapper that
  echoes a resolved token or an expanded environment variable puts it on the
  terminal and in scrollback — during a screen share, a recording, or a
  captured support session.
  The SGR filter neutralizes control sequences; it does not and cannot identify
  secrets.
  The same bytes already reach the terminal on the failure path, where
  `render_stderr_tail` embeds up to 100 lines of child stderr in
  `InitializeError`, so this widens an existing surface rather than opening one.
  Users running sensitive startup wrappers set `print_stderr = false`.

## Alternatives

- **Keep the status quo (`LineTimer` + per-site discipline).** The
  waiting-indicator fix shipped this way and works.
  Rejected as the end state: it fixes one site per bug, the clear-ordering
  guarantee needs an async context (`finish().await`) that synchronous renderers
  don't have (see `cancel_reasoning_timer`'s workaround), and the tool temp-line
  machinery remains bespoke.
- **A region actor outside the printer.** A separate task owning the rows, with
  renderers notifying it before writes.
  Rejected: it recreates the ordering problem it is meant to solve —
  notifications race with writes unless every write goes through the actor, at
  which point it *is* the printer.
- **Last-writer-wins instead of a claim stack.** Simpler, but a released
  claimant would leave the screen blank even when an earlier claimant is still
  logically active.
- **The alternate screen for the output window.** Switching to the alternate
  buffer for the duration of a wait gives an unbounded scrolling log and a free
  erase, since the alternate buffer has no scrollback.
  Rejected on three counts.
  It hides content the user is already reading — `jp query` echoes the user's
  request before the MCP startup wait, so the prompt would vanish for the
  duration.
  It discards anything written to the real terminal outside the printer during
  the window, including a panic message or a `warn!` from an optional server
  that failed, which is precisely the output worth keeping.
  And it introduces a restore invariant that has to hold across Ctrl-C, SIGTERM,
  and panic; JP installs no panic hook today, so the guarantee would be new
  machinery in the interrupt path rather than a rendering detail.
  A bounded region on the primary screen needs none of it.
- **A per-source window instead of one shared window.** Giving each source its
  own rows keeps concurrent output separated without labels, but the row budget
  then scales with the number of sources, and a wait on six servers would own
  the whole screen.
  A shared window with labels keeps the budget fixed.

## Non-Goals

- **Rendering stdout.** Stdout is the machine channel for both child-process
  clients — JSON-RPC for MCP servers, `Outcome` payloads for local tools — and
  is never region content.
- **Progress bars, spinner groups, or per-source rows.** The output window shows
  the last N lines a source emitted, verbatim modulo the SGR filter.
  JP does not parse child output to derive progress.
- **A general-purpose TUI.** The region is one block of rows at the bottom of
  the screen, owned by the printer worker, with no input handling and no layout.
  Anything needing more belongs behind a different abstraction.
- **Changing what any indicator says or when clients claim/release.** Reasoning
  display modes, waiting-indicator status wording, and lock-wait countdown
  semantics are unchanged; only the mechanics move.
- **Regions during interactive sessions.** A prompt session suspends the region
  rather than coexisting with it; rendering chrome alongside an active prompt
  widget is out of scope.

## Risks and Open Questions

- **Row accounting under resize.** The window budget is resolved from a height
  captured at construction.
  If the terminal shrinks below the drawn row count mid-region, the erase walks
  back further than the rows that are still on screen and eats content above.
  The cheap mitigation is a conservative budget and re-resolving it on each
  redraw; the correct one is reacting to a resize event.
  Which of the two is needed is the largest open question in this design, and
  the reason phase 4 carries a spike.

  Either way the second one costs more than it looks: geometry is a constructor
  input above, and a printer that reacts to resize has to hold it as mutable
  state that something outside `jp_printer` updates through a command.
  The sources differ by platform — `SignalRouter` handles SIGINT, SIGTERM, and
  SIGQUIT under `#[cfg(unix)]` and would gain SIGWINCH, while Windows has no
  such signal and would poll.
  If the spike settles on the conservative budget, none of that is needed and
  geometry stays immutable.

- **Typewriter granularity.** Under the task-boundary coexistence contract, a
  long typewriter print hides the region (and freezes its elapsed display) for
  the task's duration.
  Accepted as a limitation: regions rarely coexist with typewriter output.
  The known refinement, if it matters in practice, is yielding redraws between
  typewriter batches inside the worker loop.

- **Flicker scales with row count.** Erase-redraw around every write during
  heavy streaming could flicker on slow terminals, and an N-row region is N
  times the bytes of a status line.
  Tool execution is the exposed case, since a region can be active while
  assistant output streams.
  Mitigation if observed: skip the redraw when another write is already queued,
  coalescing to one redraw per batch.

- **Window sizing is a guess.** `height / 10` is chosen to stay unobtrusive
  while assistant output streams beneath a tool's region.
  Whether the same divisor suits MCP startup — which runs before any streaming
  and could afford more rows — is worth revisiting once both clients exist.

- **Windows console.** `\r\x1b[K` handling, relative cursor movement across N
  rows, and the worker's `recv_timeout` resolution (~15.6ms scheduler tick) need
  verification on Windows, same as the existing typewriter batching did.

- **Tool errors can lose the stderr that explains them.** `parse_command_output`
  carries stderr only on `CommandResult::RawOutput`; a tool that emits a valid
  `Outcome::Error` on stdout with its detail on stderr has that detail dropped
  before the result reaches the model, and the trace log holding it is discarded
  unless the run itself fails.
  The region would show that detail and then erase it, leaving the user where
  they already are today — the precondition permits that, but it is a poor
  outcome for the case where a tool fails and the reason was on screen a moment
  ago.
  The gap predates this RFD and the fix — carrying bounded stderr on the tool
  error variants — changes tool-result semantics rather than the printer, so it
  belongs in its own change.

- **Overlap with [RFD 096].** The SGR allowlist here and 096's content
  sanitization are the same policy at two boundaries, down to the conceal
  exclusion.
  This RFD does not depend on 096: the filter is scoped to region input and
  lives in `jp_printer`, so both can land in either order.
  If 096 lands first, the region reuses its filter — which 096 places in
  `jp_term` — instead of defining one; if this RFD lands first, its narrow
  filter is a candidate for replacement once that shared owner exists.

## Implementation Plan

Each phase is independently reviewable and mergeable.
Phases 1–3 ship a zero-window region — one physical row, the status line as it
behaves today — and leave every current indicator looking unchanged.
The output window arrives in phase 4.

The two halves are one RFD because the primitive's shape — relative row
accounting, terminal geometry as a capability, exact erasure — is justified
only by the window phase 4 adds; designing the one-row primitive alone would
settle an API that phase 4 immediately reopens.
The split is in the phases instead.
Phases 1–3 depend on nothing in "What may enter the output window", "The push
path is lossy", or "Configuration", and stand on their own if the phase-4 spike
finds relative row accounting unworkable; phases 4–6 are what accepting those
contracts commits to.

1. **Printer primitive.** Add the claim stack, `Command` variants,
   `recv_timeout` ticking, erase-before-write/redraw-after, the enabling
   predicate, the row background, erasure on shutdown, prompt-writer suspension,
   and the explicit `suspend_status` guard to `jp_printer`.
   Regions are one row, truncated to the captured column count; the window
   budget is fixed at zero.
   Unit tests against `Printer::memory` with an explicit terminal-capability
   override.

2. **Waiting indicator.** Migrate the turn loop's indicator (including its
   status transitions) from `LineTimer` to the printer handle.
   The `turn_loop_tests` waiting-indicator suite carries over as the
   characterization tests.

3. **Simple timers.** Migrate the reasoning timer, lock-wait countdown, MCP
   startup timer, both drain timers, and the stream-retry notice; move the
   lock-contention prompt from `err_writer()` to `prompt_writer()`.
   Delete `spawn_line_timer` and `LineTimer`.
   The retry notice keeps its non-terminal branch, so its `retry_tests` coverage
   carries over unchanged.

4. **Output window.** Add the bounded line buffer and its coalesced redraw,
   source registration and labelling, the SGR filter, ANSI-aware truncation,
   terminal height as a capability input, multi-row draw and erase with relative
   row accounting, and the `print_stderr` config shape (including splitting
   `ProgressConfig`).

   Opens with a spike against a real terminal, because that is the only place
   scrolling, deferred wrap, and resize exist — `Printer::memory` records
   emitted bytes and models none of them, and JP has no PTY harness ([issue
   392]).
   The spike settles four cases: claiming while the cursor sits on the last row,
   a persistent write landing while the region is drawn, the terminal shrinking
   below the drawn row count, and the same on Windows.
   Its output is the draw/erase sequence; once that is fixed, `Printer::memory`
   at a declared height pins those bytes as the regression tests.
   No client uses a non-zero window yet, so nothing outside `jp_printer` moves
   in this phase.

5. **MCP startup client.** Add the tagged line channel to
   `jp_mcp::client::spawn_stderr_forwarder` and the receiver to `StartupSet`;
   own one sink per pending server in `await_mcp_servers`, and drop it when that
   server's join completes.
   The channel carries the pre-open backlog, so nothing seeds from the
   diagnostic ring buffer.
   First client with a visible window.

   Also report optional-server failures.
   `SpawnOutcome::OptionalFailed` completes as `Ok` and the `warn!` explaining
   it dies with the trace file, so a query silently loses tools with no account
   of why.
   The phase adds one persistent line per failed optional server, naming the
   server and the tools that become unavailable, with the retained stderr
   reachable through `-v` (`mcp::stderr`) rather than dumped inline.
   It is persistent stderr chrome and follows [RFD 048]'s format rules like any
   other chrome, including the NDJSON form under `--format json`.
   It is emitted regardless of `show` and `print_stderr`: those gate progress
   display, and gating a failure report behind them would reproduce the silent
   failure this closes.

6. **Tool temp line and tool output.** Migrate `ToolRenderer`'s preparing line
   and the execution-progress ticker; delete `spawn_tick_sender` and the manual
   rewrite/clear paths (`clear_temp_line`, `rewrite_temp_line`, the
   `line_active` bookkeeping).
   Add the line channel to `jp_llm::tool::forward_stderr`; the coordinator owns
   one sink per executing tool and drops it when that tool call ends.
   Carry the [RFD 095] reasoning-region background onto the migrated rows: the
   phase is not done until a tool called from a reasoning block still shows the
   background across its temp and progress rows, the worker's own erases
   included.
   Largest phase; depends on phases 1–4 only for the primitive's API having
   settled.

## References

- [RFD 048] — the four-channel output model; defines "chrome" and the printer's
  ownership of stdout/stderr/tty.
- [RFD 088] — the unified editor service and inline reply widget; its
  cursor-owning prompt sessions are what prompt-writer suspension protects, and
  its open widget/printer coordination risk overlaps with the problem addressed
  here.
- [RFD 092] — the interrupt escalation ladder; its shutdown watchdog absorbs
  the two background-drain timers this RFD migrates.
- [RFD 095] — reasoning-region shading across tool calls; the background
  invariant the migrated tool rows have to keep.
- [RFD 096] — terminal output sanitization for untrusted content; the same
  escape-filtering problem at the conversation-content boundary.
- `crates/jp_cli/src/timer.rs` — the `LineTimer` interim solution this RFD
  replaces.
- `crates/jp_cli/src/cmd/query/stream/retry.rs` — `notify` and `clear_line`,
  the ninth hand-rolled mechanism.
- `crates/jp_md/src/shade.rs` — `ShadedWriter`, which holds the background
  invariant for tool chrome today.
- `crates/jp_printer/src/printer.rs` — the worker loop this RFD extends.
- `crates/jp_printer/src/ansi.rs` — the `vte`-based `AnsiStripper` the SGR
  allowlist extends.
- `crates/jp_md/src/ansi.rs` — `is_sgr`, the predicate the allowlist reuses,
  and the retain-SGR-drop-the-rest precedent in `jp_md/src/table.rs`.
- `crates/jp_mcp/src/client.rs` — `spawn_stderr_forwarder`, the stderr ring
  buffer, and `StartupSet`.
- `crates/jp_llm/src/tool.rs` — `forward_stderr` and the stdout/stderr split in
  `parse_command_output`.

[RFD 048]: 048-four-channel-output-model.md
[RFD 084]: 084-configurable-markdown-element-coloring.md
[RFD 088]: 088-unified-editor-service-and-inline-reply-widget.md
[RFD 092]: 092-predictable-and-responsive-interrupt-escalation.md
[RFD 095]: 095-reasoning-region-shading-across-tool-calls.md
[RFD 096]: 096-terminal-output-sanitization-for-untrusted-content.md
[Tesler]: https://en.wikipedia.org/wiki/Law_of_conservation_of_complexity
[`ErrChannel`]: https://github.com/dcdpr/jp/blob/main/crates/jp_printer/src/printer.rs
[`InlineResults`]: https://github.com/dcdpr/jp/blob/main/crates/jp_config/src/conversation/tool/style.rs
[issue 392]: https://github.com/dcdpr/jp/issues/392
