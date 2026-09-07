# Per-tool delay for the progress window

- **Status**: Todo
- **Kind**: Enhancement
- **Authors**: jp
- **Date**: 2026-09-07

`style.tool_call.progress.delay_secs` is one value for the whole execution
batch, so every running tool joins the window at the same moment.
It could be per-tool: a quick tool worth watching after one second and a slow
one worth watching after ten are a reasonable pair to want.

The merge rule is not invented, it is the one the region already uses for MCP
startup: the region becomes visible when the *first* source passes its delay,
and each source starts contributing when its own does.
Sources already join and leave the displayed set independently —
`await_mcp_servers` drops a server from the status row as it finishes while its
lines stay in the window.

## Why it is not already there

`LineSink` has no notion of time.
It holds a label and a shared buffer, and `WindowBuffer::push` appends
unconditionally.
Per-source delay means each sink carries its own start instant and gate, and
`RegionEntry::rows` renders only lines from sources past theirs — a third
concern in a struct that currently just holds lines.

That was judged out of scope while landing [RFD 091] phase 6, not impossible.

## Shape

- `LineSink` gains a start instant and a delay, both set when the client asks
  for the sink.
- `WindowBuffer` records the source's delay alongside each line, or the sink
  drops the line itself before it reaches the buffer.
  The second is simpler and keeps the buffer dumb; the first allows a line
  pushed early to appear once the delay passes, which is probably not wanted.
- The region's own `delay` becomes the minimum across open sources rather than a
  claim-time constant, so it appears when the first source is due.
- `conversation.tools.<name>.style` gains `progress_delay_secs`, inheriting
  field-by-field from the `'*'` block like the rest of that block.

## Watch for

The status row's elapsed time counts from the claim, not from any source's
delay, and should keep doing so — it measures the wait, not any one tool.

A per-tool delay interacts with the two existing gates
(`style.tool_call.progress.stderr_rows` and the per-tool `style.print_stderr`);
a tool that is off contributes nothing whatever its delay, and the delay should
not resurrect it.

[RFD 091]: https://jp.computer/rfd/091
