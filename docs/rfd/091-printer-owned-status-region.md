# RFD 091: Printer-Owned Status Region

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-03
- **Extends**: [RFD 048]
- **Extended by**: [RFD 120]
- **Summary**: Consolidates ephemeral terminal timers and live subprocess output
  windows into a single status-region primitive owned by `jp_printer`.

## Summary

This RFD makes ephemeral terminal chrome a first-class **status region** owned
by `jp_printer`: a self-ticking status line paired with an optional rolling
window of live output lines.
The printer's worker thread draws, ticks, and erases the region before any
persistent write on any channel.
Nine ad-hoc timer and temporary-line mechanisms across `jp_cli`, along with live
output for MCP server startup and tool execution, share this single primitive.

## Motivation

### Nine hand-rolled mechanisms

JP renders ephemeral chrome (a single self-overwriting line on stderr) in nine
places, each with its own draw, tick, and clear logic:

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

All nine must uphold the same invariant: **erase ephemeral chrome before any
persistent write, and keep it visible until that write arrives**.
Today each call site enforces this invariant by hand, which has produced a trail
of point fixes: `clear_temp_line()` before the interrupt menu,
`cancel_reasoning_timer()` inside `flush_on_transition`, the warning against
pre-clearing `line_active` in `ToolRenderer::reset`, and the waiting indicator's
`finish().await` ordering in the turn loop.
Each fix repaired one call site without protecting the others.

The most visible failure was the waiting-indicator gap.
The first provider event of any kind tore down the indicator, including SSE
keep-alive pings that render nothing.
That left the terminal blank for seconds while the model produced no visible
output, making JP look hung.
`LineTimer` (in `jp_cli::timer`) fixes that one site by carrying a status
channel so the turn loop finishes the indicator only on events that render.
The underlying bug class remains open at every other site.

The retry notice shows how far this manual coordination spreads.
`StreamRetryState::notify` writes a `\r\x1b[K` line on the cursor row, and four
separate call sites must clear it: on the first rendering event, on a fatal
error, on an interrupt, and on a refused rebuild.
It outlives the preceding indicator and is overwritten on the same row by the
next attempt's indicator only because of call ordering.

### Invisible long-running subprocesses

A single status row is also not enough when waiting on a child process.

When an MCP server builds from source on spawn, `jp query` renders `⏱ Starting
MCP server bookworm… 300s` and nothing else for five minutes.
The compiler reports progress on stderr, but JP discards those lines unless the
user passed `-v`.
As the timer climbs toward a `startup_timeout_secs` threshold the user did not
configure, a slow build is indistinguishable from a hang.

Tool execution has the same blind spot.
`⏱ Running tool shell… 90s` shows nothing while a tool runs a build, test
suite, or deploy.

JP already reads both stderr streams line by line.
`jp_mcp::client::spawn_stderr_forwarder` forwards each MCP server stderr line to
tracing and retains the last 100 lines in a ring buffer for error reporting.
`jp_llm::tool::forward_stderr` does the same for local command tools under
`tool::stderr`.
What is missing is a display surface that shows the latest lines while the
process runs and erases them afterward so build output does not pollute the
transcript.

Without a shared primitive, every new indicator re-implements draw, tick, and
clear logic, every combination of chrome and content risks stale or clobbered
rows, and long-running child processes remain opaque.

## Design

### Concept

A **status region** is a block of ephemeral chrome rows at the bottom of the
terminal.
It has two parts:

- A **status row**: a subject, an elapsed time, and an optional replaceable
  detail.
  Always present.
- An **output window**: zero to N rows showing the most recent lines pushed by
  the region's **sources**, sized by configuration and terminal height.

<!-- end list -->

```
   Compiling serde v1.0.219              ┐
   Compiling tokio v1.47.1                │ output window (rolling, newest last)
   Compiling bookworm v0.1.0             ┘
⏱ Starting MCP server bookworm… 47.2s     ─ status row
```

The output window sits above the status row, keeping the status row adjacent to
where persistent content appears next.

Window rows and physical rows are tracked separately because erase correctness
depends on the physical row count.
A **zero-window region** has zero output rows and one physical row: the status
line as it exists today, which is what every existing timer becomes.
A region never has zero physical rows.

A **source** is a named line producer feeding a region, such as an MCP server
starting up or a tool executing.
Multiple sources can feed one region concurrently; the region maintains a single
rolling window across all of them rather than one window per source.

In [RFD 048] terms, the region is chrome (written to stderr, never part of the
persistent transcript) with one added contract: the printer erases the region
before any printer-managed write on any channel reaches the terminal.
Only one stderr writer lives outside the printer: the optional tracing layer
(`-v`, `--log`, `--log-file=-`), which [RFD 048] keeps separate.
Because the printer worker cannot erase before writes it never sees, the
enabling predicate below disables status regions while stderr tracing is active.

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

Callers holding a full `Printer` (the turn loop, lock acquisition, the shutdown
drain, and the MCP startup wait) call `Printer::status_region` as shown above.
Chrome-only renderers acquire a region through `ErrChannel::status_region`
(below), preserving the stderr-only boundary.

The handle is `Send` and needs no async runtime.
There is no `finish().await` because the printer, not the caller, orders region
erasure before persistent writes.
A client rendering content writes through the printer as usual, and the worker
erases the region first.

The owning `StatusRegion` handle is not `Clone` and releases the region on drop.
Three cloneable handles split off from it so shared access never obscures
ownership:

- `StatusDetail`: updates the status row's detail text (used by the turn loop's
  status transitions).
- `RowBackground`: sets the background color the worker applies to every row
  (used by the tool renderer to preserve reasoning shading; see "Region
  background").
- `LineSink`: pushes lines for a single source.
  Dropping all clones of a sink closes that source; lines it already pushed
  remain in the window with their label until evicted.

`RegionStyle` takes a `format` closure rather than a fixed template because the
lock-wait and drain timers render a countdown instead of elapsed time.

### Source lifetime is the client's, not the producer's

A source is open from the moment the client creates a `LineSink` until the
client drops it.
Source lifetime is not tied to the child process producing the lines, and the
region never infers one from the other.

For tool execution the two lifetimes coincide: `jp_llm::tool::forward_stderr`
returns when the tool's stderr pipe closes at the end of the tool call.
For MCP startup they diverge.
`spawn_stderr_forwarder` runs until the server process exits, whereas the
startup region only cares about the server until `StartupSet::joins` completes
at the `initialize` handshake.
After initialization, the server process stays alive and continues logging.
If the forwarder owned the sink, an initialized server would remain an active
source for the rest of the session, and its operational logs would evict output
from servers still compiling.

The client therefore owns the sink:

- `jp_mcp` sends `(McpServerId, String)` on the channel carried by `StartupSet`
  and knows nothing about status regions.
- `await_mcp_servers` holds one sink per pending server, routes each incoming
  line to the matching sink, and drops that sink when the server's join
  completes.
- `spawn_stderr_forwarder` continues draining stderr for tracing and the error
  ring buffer unchanged.

**The client also owns the status row's subject.** Closing a source does not
rewrite the status row: the format closure receives only elapsed time and
`detail`, never the set of open sources.
A client that lists pending work in the status row updates it through
`StatusDetail`.
`await_mcp_servers` already passes `mcp_startup_status(&pending)` to
`set_status`; that call becomes `set_detail`.

Chrome renderers hold an [`ErrChannel`] rather than a `Printer` so tool chrome
cannot write to stdout.
Because status regions are chrome, [`ErrChannel`] exposes the same entry point:

```rust
impl ErrChannel {
    pub fn status_region(&self, style: RegionStyle) -> StatusRegion;
}
```

`ToolRenderer` acquires its region through `ErrChannel::status_region` without
gaining access to the full `Printer`.

### What may enter the output window

Region content comes from child processes JP does not control and is untrusted
by construction.
The status-region primitive enforces three constraints on behalf of all clients.

**Only stderr, never stdout.** For both child-process clients, stdout is the
machine channel: MCP servers send JSON-RPC messages on stdout, and local tools
return a `jp_tool::Outcome` payload on stdout.
Stderr is the human channel.
Pushing stdout into a region would render a wire protocol; no client does so,
and this separation is why the output window is safe to enable by default.

**Styling passes, control sequences do not.** Each pushed line passes through an
SGR allowlist.
`\x1b[…m` sequences (colors, bold, italic, underline) are forwarded except
conceal (`SGR 8`), which hides text and has no place in a preview.
Every other escape family (CSI cursor movement and erasure, OSC, DCS) and every
bare control byte other than the line terminator is dropped.
Lines that leave an SGR attribute open end with a reset sequence so child
styling cannot bleed into JP's chrome below.

`jp_md::table` already applies this policy when truncating table cells (retain
SGR, drop other escapes, close with a reset), and `jp_term::ansi::is_sgr`
provides the same predicate.
In `jp_printer`, this filter is a second policy over the `vte` parser that backs
`AnsiStripper`, requiring no new dependency.
Without the filter, a child emitting `\x1b[2J` would clear the screen and cursor
movement sequences would corrupt the region's row count.
[RFD 096] addresses the same class of problem for conversation content and
subsumes this narrow filter once it lands.

**Every row is truncated to the terminal width.** A row wider than the terminal
soft-wraps, putting the worker's row count out of step with the screen and
causing erasure to walk up into persistent content above the region.
The worker truncates every physical row it emits, including the status row and
its detail, rather than relying on each format closure to bound its own output
(today, only `mcp_startup_line` bounds its width).

Truncation measures visible columns, keeps SGR sequences intact, and resets any
SGR state open at the truncation point.
`jp_term::width::truncate_to_width` cannot be used here: it assumes plain text
without escape sequences, so escape bytes would count against the column budget
and could be split mid-sequence.
Because filtered lines still carry SGR escapes, `jp_printer` needs a
visual-width truncation helper equivalent to
`jp_md::table::truncate_to_visual_width`.

Each row is processed in a fixed order: filter control sequences, prepend the
source label, truncate by visible column width, close any open SGR state, and
draw.
[RFD 096] step 5 specifies the same ordering for its width-budgeted paths.

### Source labelling

When the window contains lines from a single source, they render without a
prefix.
When it contains lines from two or more sources, every line is prefixed with its
source label:

```
[bookworm]    Compiling serde v1.0.219
[grizzly]     Compiling tantivy v0.24.2
[bookworm]    Compiling bookworm v0.1.0
⏱ Starting 2 MCP servers (bookworm, grizzly)… 47.2s
```

Interleaved unlabelled lines from concurrent sources misattribute progress.
Labels apply even in a single-row window: an unlabelled row alternating between
two sources is misleading, even when the status row names the pending sources.

Labelling depends on the sources currently represented in the window, not the
set of open sources.
When a source finishes, it leaves the status row's subject while its lines are
still visible in the window; keying on open sources would strip the labels from
those remaining lines and attribute the finished server's output to the server
still running.
Retained lines keep their label until evicted.

### Region background

A *reasoning* region ([RFD 095]) is a span of an assistant turn rather than a
block of terminal rows, and it intersects the status region at one point: the
row background.

[RFD 095] extends the reasoning background across tool chrome, including the
tool temporary and progress rows.
While a reasoning region with background `B` is active, every visual row must
show `B` to the right edge of the terminal, including cursor-relative rewrites
and the `\x1b[K` erase sequence (which fills with the active background).
`ToolRenderer` upholds that invariant today by writing through
`jp_term::shade::ShadedWriter`.
If the printer worker drew and erased those rows without the reasoning
background, it would leave unshaded gaps inside a shaded reasoning block.

A status region therefore accepts an optional row background via the cloneable
`RowBackground` handle.
The worker applies the background to every row it draws, re-applies it before
erasing, and resets it at the end of each row so it never leaks below the
region.
Because the tool progress line aggregates currently pending tools, the client
updates `RowBackground` dynamically as tool calls enter and leave a reasoning
region.

The region receives the background as an opaque SGR parameter string (the value
`DefaultBackground` already wraps), keeping `jp_printer` free of a `jp_md`
dependency.
If [RFD 084] lands first and turns `DefaultBackground` into a logical color, the
client converts it to an SGR parameter at the boundary.

### Erasure and durability

**The region is always erased on release**, whether the client succeeded or
failed.
Callers never have to check whether prior rows survived, and subsequent code
never has to handle two possible screen states.

Unconditional erasure is safe because a region is purely additive: it previews
output that is either recorded elsewhere or not worth keeping once the wait
completes.
A region must never replace durable error reporting:

> Feeding a region must not reduce what survives the erase.
> A region may be the only place output appears live; it may never be the reason
> output stops being recorded.

Progress output from a successful build is useful while waiting and disposable
afterward; neither client retains it today.

On failure, both clients keep their existing records intact.
MCP server stderr remains in the 100-line ring buffer that `jp_mcp::client`
attaches to `InitializeError` and `InitializeTimeout`, so a failed required
server still reports its build output.
`jp_llm::tool::forward_stderr` still accumulates full tool stderr and, for tools
that do not emit an `Outcome` payload, includes it in `CommandResult::RawOutput`
for the model.
Both streams also continue logging to the `mcp::stderr` and `tool::stderr`
tracing targets (written to a temporary file that is deleted on drop unless the
run fails).

Two failure paths currently discard stderr before the user can see it: an
optional MCP server that fails to start, and a tool that returns a valid
`Outcome::Error` on stdout while writing the failure details to stderr.
Showing those lines in the region and erasing them on completion leaves durable
output unchanged while making the failure visible while it runs.
Phase 5 adds persistent reporting for failed optional MCP servers, and the Risks
section covers the `Outcome::Error` stderr gap.

### The push path is lossy

A verbose test suite can emit stderr much faster than a terminal can render it,
and the rolling window only bounds lines that have already reached the printer
worker.

For MCP startup, each line crosses four hops from the child process to the
worker:

```text
spawn_stderr_forwarder → tagged channel → await_mcp_servers → LineSink → worker
```

Every hop in this pipeline must obey three rules; bounding only the final hop
would let earlier queues back up in front of it.

**Child drainage never waits on rendering.** `forward_stderr` and
`spawn_stderr_forwarder` must keep reading from the child's stderr pipe so the
child never blocks on a full OS pipe buffer during a tool call or MCP handshake.
In `run_tool_command`, which joins `forward_stderr` with `child.wait()`, a
forwarder blocked on a channel send would prevent the tool invocation from
completing.
No send at any hop awaits display capacity.

**Backpressure drops the oldest lines rather than queueing.** Every hop is
bounded and overwrites its oldest entries when full, matching the rolling
window's newest-last display.
Pushing a line into the worker queues at most one pending redraw command, so a
burst of a thousand lines triggers one wakeup rather than a thousand redraws
ahead of the next persistent write.
The exact channel type is an implementation detail, provided it is bounded,
non-blocking, and drop-oldest.

**The channel is the only backlog.** An MCP server can compile for minutes
before `await_mcp_servers` begins waiting on it: `configure_active_mcp_servers`
spawns the server tasks early, `await_mcp_servers` is called later in the
command, and `delay_secs` elapses before the region first renders.
Lines queued in the bounded channel when the region opens form the initial
backlog and drain in order.
The diagnostic ring buffer in `jp_mcp::client` remains dedicated to
initialization errors; the region never reads from it, avoiding duplicate lines.

When the enabling predicate disables status regions, `LineSink` is a no-op and
the producer channel is not created, leaving no undrained receiver.

### Release contract

- Dropping the owning `StatusRegion` handle enqueues a release command.
  Commands from a single thread are processed in FIFO order, so a release
  followed by a print on the same thread always erases before printing.
- Every `Print` task with non-empty byte content erases a drawn region before
  writing, even if a release command is still queued.
  A stale region can never remain above persistent content.
  Newline-only and control-sequence-only writes count as non-empty; zero-byte
  writes are no-ops, and the worker's own region draw and erase writes do not
  recurse.
- Once a release command is processed, that region entry is never redrawn.
- Across threads, dropping a handle is eventual cleanup: if thread A drops its
  handle while thread B enqueues a `Print`, the worker may process B's `Print`
  (erasing and redrawing the region once) before draining A's release command.
  The queue bounds that window, and the erase-before-write rule keeps the region
  below persistent output.
- Pushing a line to `LineSink` is not a `Print`: it updates the region's line
  buffer and schedules at most one coalesced redraw.
  Pushes to a released region are dropped.
- The worker erases any drawn region before exiting.
  When processing `Shutdown` or observing a closed command channel, the worker
  erases the active region without redrawing lower entries on the stack.
  Because `Ctx::drop` calls `Printer::shutdown`, any clean exit leaves the
  terminal clear; hard termination remains best-effort.

Releasing a region does not block the caller.
Same-thread command ordering already guarantees that releasing a handle and then
printing on the same thread erases the region first, without reintroducing an
async `finish().await` step.

Terminal suspension is the one exception because it hands the terminal to an
external process that writes outside the printer queue (see "Interactive
sessions").

### Enabling predicate

A region renders if and only if:

1. the resolved output format permits terminal control (`text-pretty`),
2. the chrome channel (stderr) is an interactive terminal, and
3. no tracing layer writes to stderr (`-v`, `--log`, or `--log-file=-` without
   `--quiet`).

| Situation                                     | Region                                                                                              |
| --------------------------------------------- | --------------------------------------------------------------------------------------------------- |
| `--format json` / `json-pretty`               | off (chrome is NDJSON)                                                                              |
| `--format text`                               | off (the non-pretty `Sink` strips `\r` and escape sequences; the rows would smear as repeated text) |
| `text-pretty`, stderr is a terminal           | on                                                                                                  |
| stderr is not a terminal                      | off, regardless of format                                                                           |
| `-v` / `--log` / `--log-file=-` on a terminal | off (stderr carries live logs, a persistent stream outside the printer)                             |

The output window requires one additional condition: a known terminal height.
When the terminal height is unknown, the region falls back to a single status
row because the worker cannot bound the window size.

Condition 2 checks whether stderr is a terminal rather than stdout.
Today chrome gating checks stdout (`ctx.term.is_tty`), so `jp query 2>file` with
stdout on a terminal writes `\r\x1b[K` sequences into `file`.
Under condition 2 it does not.
`--format auto` still resolves from stdout per [RFD 048]; this RFD does not
change format resolution.

Condition 3 disables status regions when tracing writes directly to stderr
outside the printer worker.
Logging setup runs before printer construction and passes this flag into the
printer constructor alongside condition 2.

To make condition 2 explicit and testable, `Printer::terminal` captures stderr
tty status and the terminal's row and column counts at construction.
`Printer::memory` accepts an explicit capability override so tests can exercise
draw, erase, and redraw at arbitrary terminal dimensions without a real PTY.

Terminal width and height belong in `jp_printer` alongside stderr tty status
rather than in `jp_cli::ctx::Term`, which tracks only stdout `width` for layout.
The existing `OutputWidth` type cannot be reused here because it describes
stdout and returns `Unknown` whenever stdout is redirected.
For example, `jp --format text-pretty query > answer.txt` satisfies all three
enabling conditions because the format is pinned to `text-pretty` and stderr is
still a terminal, even though stdout's `OutputWidth` is `Unknown`.
Using `OutputWidth` there would leave region rows untruncated, allowing soft
wraps that break multi-row erasure.
(Plain `jp query > answer.txt` resolves `auto` to `text` and disables the region
outright.)

### Worker integration

The printer serializes every write to stdout, stderr, and `/dev/tty` through a
single background worker thread (`Worker::run` in `jp_printer::printer`).
That single serialization point is what makes "erase chrome, then write content"
atomic across all three channels.

The worker changes in four places:

1. **State.** The worker maintains a stack of active region entries.
   Each entry holds its claim `Instant`, format closure, current detail,
   resolved window budget, open source set, and a rolling `VecDeque` of filtered
   lines capped at the window budget.
   Only the top entry on the stack is rendered.
   Claim, detail update, background update, source close, and release arrive as
   `Command` variants; line pushes append to the bounded buffer and schedule a
   single coalesced redraw.
2. **Ticking.** While a region is active, the worker calls
   `rx.recv_timeout(interval)` instead of `rx.recv()`, redrawing the top entry
   with the updated elapsed time on each timeout.
   When the stack is empty, `rx.recv()` blocks as before.
   Moving ticking into the printer worker removes the separate tokio timer
   tasks.
3. **Erase-before-write, redraw-after.** Before executing any `Print` task with
   non-empty content, the worker erases the drawn region; after the task
   completes, it redraws the region.
   Coexistence with streaming content therefore operates at task granularity:
   the region redraws between `Print` tasks, while a multi-batch typewriter task
   hides the region until that task finishes.
   For instant prints (tool chrome and block-buffered output), this matches the
   `cargo` / `indicatif` model.
   Callers that want the indicator to disappear permanently on first content
   (such as the waiting indicator) drop their handle before printing.
4. **Row accounting.** The worker records how many physical rows it last drew
   and moves the cursor using only relative CSI sequences, never querying cursor
   position or using absolute coordinates.
   To draw a region of N rows (for N > 1), the worker first emits N newlines to
   scroll the terminal if the cursor is at the bottom, then moves up N rows to
   write each line.
   Without that initial scroll reservation, claiming a multi-row region on the
   bottom row would scroll the screen mid-draw and offset every subsequent
   upward cursor move.
   Erasing walks back up the recorded row count, clearing each row.
   This is the same multi-row technique `indicatif` uses, and phase 4 validates
   it with a terminal spike before implementation.

### Concurrent claimants

Active regions form a LIFO stack: the worker renders only the most recent claim,
exposes the previous entry when the top entry is released, and removes non-top
entries in place if they are released early.
A stack matches how indicators nest in practice: if a reasoning timer is active
when a tool call begins streaming and claims a "preparing" region, the tool
region takes over the status area and yields back to the reasoning timer on
release without either call site coordinating with the other.

### Interactive sessions

Interactive prompts write through `Printer::prompt_writer()` and
`owned_prompt_writer()` as `PrintTarget::Tty` tasks serialized by the worker, so
the erase-before-write rule applies to them automatically.

Per-write erasure is not enough for interactive prompts, however.
A prompt session issues a sequence of small `Tty` writes while the prompt widget
manages cursor position between them; redrawing the status region between those
writes would corrupt the prompt display.
Acquiring a prompt writer (`prompt_writer()` or `owned_prompt_writer()`)
therefore suspends status regions for the writer's lifetime, erasing any drawn
rows and blocking redraws until the writer is dropped.
Individual prompt call sites (`ToolPrompter`, the interrupt menu, `cmd/init.rs`,
`cmd/target.rs`, and the `conversation` subcommands) require no changes.

This design has two implementation consequences:

- `PrinterWriter` is currently `Copy`; carrying suspension lifetime requires a
  drop guard type.
- The lock-contention prompt currently writes via `err_writer()` and moves to
  `prompt_writer()` alongside the lock-wait countdown in phase 3.

One explicit suspension guard is still needed for external `$EDITOR` sessions,
which spawn a child process that writes directly to the terminal without
acquiring a prompt writer:

```rust
let _pause = printer.suspend_status(); // erases the region, blocks redraws
// ... run $EDITOR ...
// guard drop: redraw resumes
```

**Acquiring `suspend_status` blocks.** The call returns only after the worker
erases any drawn region, enters the suspended state, and flushes stderr.
Prompt writers can rely on an asynchronous queued command because their `Tty`
writes sit behind that command in the same worker queue.
`TerminalEditorBackend`, by contrast, spawns a child process that writes
directly to the terminal; an asynchronous suspend would race the editor's
initial paint.
`suspend_status` uses the same acknowledgement barrier as `flush`,
`flush_instant`, and `shutdown`.

Dropping the suspension guard is asynchronous: any subsequent printer write is
queued after the resume command.

### Configuration

The four config blocks that gate existing timers (`style.mcp_startup`,
`style.lock_wait`, `style.tool_call.progress`, and `style.streaming.progress`)
keep their `show`, `delay_secs`, and `interval_ms` keys and defaults.
They map directly onto `RegionStyle`.

`delay_secs` governs when the entire region becomes visible, output window
included, rather than introducing a separate delay key for the window.

The two config blocks that wait on a child process gain a `stderr_rows` key:

```toml
[style.mcp_startup]
delay_secs = 4
stderr_rows = true # false | true | N
```

- `false` or `0`: no output window (single status row only).
- `true`: window height scaled to terminal height (`height / 10`, so a 40-row
  terminal shows 4 rows), with a minimum of 1 row whenever the terminal has at
  least one row above the status row.
- `N`: window of `N` rows, capped at `height / 10`.

The row count is shared across all sources: `stderr_rows = 1` allocates a single
output row that displays the latest line from any source.
When the terminal height is unknown, the region falls back to a single status
row regardless of `stderr_rows`.

`stderr_rows` uses the same schema pattern as [`InlineResults`] on
`conversation.tools.*.style.inline_results` (`off` / `full` / a line count, with
booleans accepted as aliases for `off` and `full`), including the custom
`Deserialize` visitor and `#[variant(fallback)]` numeric variant.

Both keys default to `true`.
Once a wait exceeds `delay_secs`, showing the child process's stderr is the most
useful context JP can provide:

| Key                                    | Default | Why                                                                                                                      |
| -------------------------------------- | ------- | ------------------------------------------------------------------------------------------------------------------------ |
| `style.mcp_startup.stderr_rows`        | `true`  | The wait owns the screen: nothing else is streaming, and a silent five-minute build is the case that motivates this RFD. |
| `style.tool_call.progress.stderr_rows` | `true`  | A tool that outlives `delay_secs` is exactly the one whose output the user wants; the delay is the filter.               |

**Window size and source membership are configured separately.** Because
`stderr_rows` sizes a single shared window, it cannot express per-tool opt-out;
a per-tool row count would leave ambiguous how two concurrent tools with
different counts share one window.
Instead, tool output passes through two gates:

```toml
[style.tool_call.progress]
stderr_rows = "auto" # is there a window, and how tall

[conversation.tools.'*'.style]
print_stderr = true # does this tool feed it

[conversation.tools.cargo_test.style]
print_stderr = false # … this one does not
```

This matches how `style.tool_call.show` and
`conversation.tools.<name>.style.hidden` gate tool chrome: a global setting
combined with a per-tool override.
`print_stderr` inherits from `conversation.tools.'*'.style` like every other
tool `style` field.
A tool with `print_stderr = false` receives no `LineSink`, avoiding channel
overhead when a noisy tool opts out.

MCP startup has no per-server toggle because all servers starting concurrently
belong to the same startup wait.

**A tool region never coexists with streaming assistant output.** The turn loop
separates streaming from tool execution: it drains and closes the provider
stream before spawning any tools, and waits for all tools in the batch to finish
before starting the next turn.
A tool region only shares the screen with other tool chrome (for example, one
tool rendering its result while a sibling tool continues running), which the
erase-before-write rule handles directly.

Defaulting both `stderr_rows` keys to `true` changes default behavior: users
with no custom config see an output window where a single-row timer previously
appeared.
Setting either key to `false` restores the single-row timer.

Only `style.mcp_startup` and `style.tool_call.progress` carry `stderr_rows`.
`style.lock_wait` waits on a file lock and `style.streaming.progress` waits on
an HTTP response; neither has child stderr to display.
Because `style.streaming` currently reuses `ProgressConfig` from
`style::tool_call`, `style.tool_call.progress` gets its own config struct so
`style.streaming.progress.stderr_rows` does not appear in the schema.

### Migration

All nine existing sites migrate to status regions, seven of them as zero-window
regions.
Once the last site migrates, `jp_cli::timer` (`LineTimer`, `spawn_line_timer`,
and `spawn_tick_sender`) is deleted.
`ToolRenderer` retains its separator tracking and temporary-to-permanent header
promotion for the tool "preparing" line; only the draw, tick, and erase
mechanics move to `jp_printer`.

The stream-retry notice becomes a zero-window region held across the retry
backoff and released on the first rendering event, an interrupt, or a fatal
error, eliminating `clear_line` and its four call sites.
When status regions are disabled, `StreamRetryState` prints each retry notice as
a persistent line, matching its existing non-TTY branch.

[RFD 092] replaces both background-drain timers with a single shutdown watchdog
countdown.
If [RFD 092] lands first, that watchdog countdown migrates to a status region;
if this RFD lands first, the two drain timers migrate first and the watchdog
later claims a region in their place.

The two child-process clients gain a line channel from the crate that spawns the
child:

- **MCP startup.** `jp_mcp::client::spawn_stderr_forwarder` sends `(McpServerId,
  String)` lines on a bounded channel alongside tracing and the error ring
  buffer.
  `StartupSet`, which already hands startup state from `run_services` to
  `await_mcp_servers`, carries the receiver.
  `await_mcp_servers` owns the per-server sinks and drops each sink when its
  server finishes initializing (see "Source lifetime is the client's, not the
  producer's").
- **Tool execution.** `jp_llm::tool::forward_stderr` gains an equivalent bounded
  line channel while continuing to accumulate full stderr for the tool result.

Neither producer filters, truncates, labels, or closes sources; those
responsibilities belong to the status region and the client.

## Drawbacks

- **The printer worker becomes stateful across writes.** Today each `Print` task
  is independent; with status regions, the worker maintains cross-task state
  (the claim stack and drawn row count) that every write path interacts with.
  Complexity is conserved ([Tesler]): moving it out of nine call sites into one
  primitive means a bug in that primitive affects all ephemeral chrome at once.
- **`jp_printer` grows in scope.** Every crate that prints depends on
  `jp_printer`; its API surface expands, and per Hyrum's Law scripts may come to
  depend on the rendered chrome format.
- **The worker loop becomes time-dependent.** Polling with `recv_timeout` is
  inexpensive, but time-driven redraws are harder to test deterministically than
  a pure command-processing loop.
- **Multi-row erasure is more fragile than single-row erasure.** A single line
  can be cleared with `\r\x1b[K` from the current row, whereas erasing an N-row
  block requires the worker's tracked row count to match the terminal screen.
  Any mismatch leaves duplicated rows or erases persistent content above the
  region.
- **The printer must track terminal dimensions.** Row and column counts join
  stderr tty status as constructor inputs and can become stale if the terminal
  is resized while a region is drawn.
- **The printer forwards a styling parameter it does not interpret.** The
  optional row background exists solely so `style.reasoning.background` ([RFD
  095]) is preserved across tool chrome drawn by the worker.
- **Two non-UI crates gain display plumbing.** `jp_mcp` and `jp_llm` expose line
  channels consumed only by the terminal status region.
- **Child stderr is displayed by default.** With `style.mcp_startup.stderr_rows`
  defaulting to `true`, a startup wrapper that echoes a secret or environment
  variable to stderr displays it on screen and in scrollback during screen
  shares or recordings.
  The SGR filter strips control sequences but cannot redact secrets.
  Because `render_stderr_tail` already includes up to 100 lines of child stderr
  in `InitializeError` on failure, this change widens an existing exposure
  surface rather than creating a new one; users with sensitive wrappers can set
  `stderr_rows = false`.

## Alternatives

- **Keep `LineTimer` and per-site discipline.** The waiting-indicator fix
  shipped this way and works for that single site.
  Rejected because each bug fix protects only one site, clear-before-write
  ordering requires an async call (`finish().await`) unavailable to synchronous
  renderers (forcing workarounds such as `cancel_reasoning_timer`), and
  `ToolRenderer`'s temporary-line rewrites remain bespoke.
- **Run a separate region actor outside the printer.** A dedicated task could
  own the status rows and receive notifications from renderers before writes.
  Rejected because notifications race with writes unless every write passes
  through the actor, at which point the actor duplicates the printer worker.
- **Use last-writer-wins instead of a claim stack.** Replacing the active region
  without a stack is simpler, but releasing an inner indicator would leave the
  status area blank even when an outer indicator is still active.
- **Render the output window on the alternate screen.** Switching to the
  alternate screen buffer during a wait provides an unbounded scrolling log and
  instant cleanup on exit.
  Rejected for three reasons:
  1. It hides the conversation transcript (`jp query` echoes the user's prompt
     before waiting on MCP servers, so the prompt would disappear).
  2. It discards output written directly to the terminal during the wait,
     including panic messages and warnings from failed optional servers.
  3. It requires restoring the primary screen on `Ctrl-C`, `SIGTERM`, and panic,
     requiring a panic hook and extra signal-path cleanup that a bounded region
     on the primary screen avoids.
- **Allocate one output window per source.** Separate windows avoid per-line
  source labels, but total row usage scales with the number of concurrent
  sources; waiting on six MCP servers would consume most of the terminal.
  A single shared window with source labels keeps height bounded.

## Non-Goals

- **Rendering child stdout.** Stdout carries machine-readable payloads (JSON-RPC
  for MCP servers and `Outcome` JSON for local tools) and never enters a status
  region.
- **Progress bars, spinner groups, or per-source status rows.** The output
  window displays the last N lines emitted by its sources after SGR filtering;
  JP does not parse child output to estimate progress.
- **A general-purpose TUI.** The status region is a single block of rows at the
  bottom of the terminal with no input handling or layout engine.
- **Changing indicator wording or claim lifetimes.** Reasoning display modes,
  waiting-indicator messages, and lock-wait countdown behavior remain unchanged;
  only the rendering mechanism moves.
- **Rendering regions alongside interactive prompts.** Acquiring a prompt writer
  suspends the status region rather than sharing the terminal with an active
  prompt widget.

## Risks and Open Questions

- **Row accounting under resize.** The window budget is computed from the
  terminal height captured at construction.
  If the terminal shrinks below the drawn row count while a region is visible,
  erasing walks past the remaining region rows and clears persistent content
  above them.
  A conservative height budget re-evaluated on each redraw is the simpler
  mitigation; reacting to terminal resize events is the exact fix.
  Choosing between the two is the main open question for the phase 4 spike.

  Reacting to resize events requires mutable terminal geometry in `jp_printer`,
  updated via a worker command from outside the crate.
  Resize notification also differs by platform: Unix can subscribe to `SIGWINCH`
  in `SignalRouter` alongside `SIGINT`, `SIGTERM`, and `SIGQUIT`, whereas
  Windows has no equivalent signal and requires polling.
  If the conservative budget suffices in the spike, terminal geometry remains
  immutable.

- **Typewriter granularity.** Because coexistence with streaming output happens
  at `Print` task boundaries, a long typewriter task hides the status region and
  pauses its elapsed timer until the task finishes.
  Regions rarely coexist with typewriter output, so this limitation is accepted.
  If needed later, the worker loop can yield redraws between typewriter batches.

- **Flicker scales with row count.** Erasing and redrawing around every `Print`
  task can flicker on slow terminals, and an N-row region writes N times as many
  bytes as a single status line.
  Parallel tool execution is the main exposure: assistant output does not stream
  during tool execution, but each completing tool renders its result while
  sibling tools continue running.
  If flicker is noticeable, the worker can skip redrawing when another `Print`
  command is already queued, coalescing redraws across a burst of writes.

- **Window height heuristic.** Sizing the output window to `height / 10` keeps
  it compact beneath tool results that render while a region is active.
  MCP startup runs before any tool chrome and may benefit from a larger fraction
  once both clients are in use.

- **Windows console behavior.** `\r\x1b[K` erasure, relative cursor movement
  across N rows, and `recv_timeout` tick resolution (~15.6 ms scheduler quantum)
  require verification on Windows, just as typewriter batching did.

- **Tool errors can lose explanatory stderr.** `parse_command_output` attaches
  stderr only to `CommandResult::RawOutput`.
  When a tool emits a valid `Outcome::Error` on stdout and writes the failure
  details to stderr, `parse_command_output` drops stderr before the result
  reaches the model, and the trace log is deleted on drop unless the JP command
  itself fails.
  The output window shows those stderr lines while the tool runs and erases them
  when it finishes, matching current durable behavior while still losing the
  error context afterward.
  Attaching bounded stderr to `Outcome::Error` changes tool-result semantics
  rather than the printer and belongs in a separate change.

- **Overlap with [RFD 096].** The SGR allowlist here and the content sanitizer
  in [RFD 096] enforce the same policy at two boundaries, including dropping
  conceal (`SGR 8`).
  This RFD does not depend on [RFD 096]: the filter is scoped to status-region
  input inside `jp_printer`, so the two RFDs can land in either order.
  If [RFD 096] lands first, the status region can reuse its filter in `jp_term`;
  if this RFD lands first, its filter can move to `jp_term` when [RFD 096]
  lands.

  > [!TIP]
  > [RFD 096] keeps this filter separate from its content sanitizer: status rows
  > keep their stricter policy and ignore `style.sanitize`, and the two share
  > only SGR parsing and conceal removal.

## Implementation Plan

Each phase is independently reviewable and mergeable.
Phases 1 through 3 ship a zero-window region (a single physical status row) and
keep all existing indicators visually unchanged.
Phase 4 adds the multi-row output window.

Both halves belong in one RFD because the output window in phase 4 shapes the
core primitive: relative row accounting, terminal geometry as a constructor
capability, and unconditional erasure on release.
Designing a single-row API in isolation would force phase 4 to redesign it.
Splitting the work across phases still isolates risk: phases 1 through 3 do not
depend on "What may enter the output window", "The push path is lossy", or
"Configuration", and stand on their own if the phase 4 spike requires adjusting
multi-row cursor movement; phases 4 through 6 implement the output window
contracts.

1. **Printer primitive.** Add the claim stack, `Command` variants,
   `recv_timeout` ticking, erase-before-write and redraw-after-write, the
   enabling predicate, the row background, erasure on shutdown, prompt-writer
   suspension, and the explicit `suspend_status` guard to `jp_printer`.
   Regions are a single row truncated to the captured column count, with the
   window budget fixed at zero.
   Add unit tests against `Printer::memory` with an explicit terminal-capability
   override.

2. **Waiting indicator.** Migrate the turn loop's waiting indicator and its
   status transitions from `LineTimer` to `StatusRegion`.
   Keep the `turn_loop_tests` waiting-indicator suite as characterization tests.

3. **Simple timers.** Migrate the reasoning timer, lock-wait countdown, MCP
   startup timer, both background-drain timers, and the stream-retry notice.
   Move the lock-contention prompt from `err_writer()` to `prompt_writer()`.
   Delete `spawn_line_timer` and `LineTimer`.
   Preserve the retry notice's non-terminal branch and its `retry_tests`
   coverage.

4. **Output window.** Add the bounded line buffer and coalesced redraws, source
   registration and labelling, the SGR allowlist filter, ANSI-aware visual-width
   truncation, terminal height as a capability input, multi-row draw and erase
   with relative cursor movement, the `stderr_rows` config option (splitting
   `ProgressConfig`), and the per-tool `print_stderr` toggle.

   Begin with a spike against a real terminal, where scrolling, deferred wrap,
   and resize actually occur (`Printer::memory` records raw bytes without a
   terminal emulator, and JP has no PTY test harness; see [issue 392]).
   The spike validates four cases: claiming a region when the cursor is on the
   bottom row, printing persistent content while a multi-row region is drawn,
   shrinking the terminal below the drawn row count, and running the same
   sequences on Windows.
   Once the spike settles the draw and erase sequences, pin those byte sequences
   in `Printer::memory` regression tests at a fixed terminal height.
   No client enables a non-zero window in this phase, keeping all changes inside
   `jp_printer` and `jp_config`.

5. **MCP startup client.** Add the tagged stderr line channel to
   `jp_mcp::client::spawn_stderr_forwarder` and its receiver to `StartupSet`.
   In `await_mcp_servers`, create one `LineSink` per pending server and drop it
   when that server's join completes.
   Drain the pre-open backlog from the channel without reading the diagnostic
   ring buffer.

   Also report failed optional MCP servers.
   Today `SpawnOutcome::OptionalFailed` returns `Ok` and logs a `warn!` that is
   discarded with the temporary trace file, so queries silently lose tools
   without explanation.
   Emit one persistent stderr chrome line per failed optional server naming the
   server and the unavailable tools, and point the user to `-v` (`mcp::stderr`)
   for the captured stderr output.
   Follow [RFD 048] formatting rules (including NDJSON under `--format json`),
   and emit the notice regardless of `show` and `stderr_rows`.

6. **Tool temporary line and tool output.** Migrate `ToolRenderer`'s "preparing"
   line and the tool execution progress ticker to status regions, and delete
   `spawn_tick_sender` along with the manual rewrite and clear helpers
   (`clear_temp_line`, `rewrite_temp_line`, and `line_active`).
   Add the bounded line channel to `jp_llm::tool::forward_stderr` so the tool
   coordinator owns one `LineSink` per running tool and drops it when the tool
   call completes.
   Thread the [RFD 095] reasoning-region background through `RowBackground` so
   tools called inside a reasoning block preserve the shaded background across
   their temporary, progress, and erased rows.

## References

- [RFD 048]: the four-channel output model; defines "chrome" and the printer's
  ownership of stdout, stderr, and tty.
- [RFD 088]: the unified editor service and inline reply widget; its
  cursor-owning prompt sessions are protected by prompt-writer suspension, and
  its open widget/printer coordination risk overlaps with the problem solved
  here.
- [RFD 092]: the interrupt escalation ladder; its shutdown watchdog absorbs the
  two background-drain timers migrated here.
- [RFD 095]: reasoning-region shading across tool calls; defines the background
  invariant that migrated tool rows must preserve.
- [RFD 096]: terminal output sanitization for untrusted content; addresses the
  same escape-filtering problem at the conversation-content boundary.
- `crates/jp_cli/src/timer.rs`: the `LineTimer` interim implementation replaced
  by this RFD.
- `crates/jp_cli/src/cmd/query/stream/retry.rs`: `notify` and `clear_line`, the
  ninth hand-rolled ephemeral line mechanism.
- `crates/jp_term/src/shade.rs`: `ShadedWriter`, which maintains the background
  invariant for tool chrome today.
- `crates/jp_printer/src/printer.rs`: the printer worker loop extended by this
  RFD.
- `crates/jp_printer/src/ansi.rs`: the `vte`-based `AnsiStripper` extended by
  the SGR allowlist.
- `crates/jp_term/src/ansi.rs`: `is_sgr`, reused by the SGR allowlist, and the
  retain-SGR-drop-the-rest precedent in `jp_md/src/table.rs`.
- `crates/jp_mcp/src/client.rs`: `spawn_stderr_forwarder`, the stderr ring
  buffer, and `StartupSet`.
- `crates/jp_llm/src/tool.rs`: `forward_stderr` and the stdout/stderr split in
  `parse_command_output`.

[RFD 048]: 048-four-channel-output-model.md
[RFD 084]: 084-configurable-markdown-element-coloring.md
[RFD 088]: 088-unified-editor-service-and-inline-reply-widget.md
[RFD 092]: 092-predictable-and-responsive-interrupt-escalation.md
[RFD 095]: 095-reasoning-region-shading-across-tool-calls.md
[RFD 096]: 096-terminal-output-sanitization-for-untrusted-content.md
[RFD 120]: 120-ephemeral-reasoning-window.md
[Tesler]: https://en.wikipedia.org/wiki/Law_of_conservation_of_complexity
[`ErrChannel`]: https://github.com/dcdpr/jp/blob/main/crates/jp_printer/src/printer.rs
[`InlineResults`]: https://github.com/dcdpr/jp/blob/main/crates/jp_config/src/conversation/tool/style.rs
[issue 392]: https://github.com/dcdpr/jp/issues/392
