# Tool chrome visibility uses two names and two polarities

- **Status**: Todo
- **Kind**: Enhancement
- **Authors**: jp
- **Date**: 2026-09-07

The same question is asked twice with different words pointing in opposite
directions:

```toml
[style.tool_call]
show = false                          # global: hide tool chrome

[conversation.tools.cargo_test.style]
hidden = true                         # per-tool: hide tool chrome
```

`show = false` and `hidden = true` mean the same thing.
A reader has to remember which level uses which word *and* which way round it
points, and the two are ANDed at `turn_loop.rs`:

```rust
let tool_chrome_visible = cfg.style.tool_call.show && !tool_style.hidden;
```

The two-level gate itself is right and [RFD 095] documents it deliberately — a
global switch with a per-tool exemption.
Only the naming is accidental.

## Why it matters now

[RFD 091] adds a second two-level gate for the progress window, and it had to
pick a side:

```toml
[style.tool_call.progress]
stderr_rows = "auto"        # global: is there a window, how tall

[conversation.tools.cargo_test.style]
print_stderr = false        # per-tool: does this tool feed it
```

That pair is positive at both levels, which is the convention this ticket
proposes to settle on.
Left alone, the config tree has one gate reading `show`/`hidden` and another
reading `stderr_rows`/`print_stderr`, and neither tells a reader which is the
house style.

## Fix

Settle on the positive form at both levels, so the per-tool key becomes `show`
and inverts:

```toml
[conversation.tools.cargo_test.style]
show = false                          # was: hidden = true
```

Double negatives in config are worse than the inconsistency, which is the
argument for positive over renaming the global to `hidden`.

Both keys are released and in use — `.jp/config/personas/committer.toml` sets
`tool_call.show = false`, and the RFD-pipeline personas set per-tool `hidden` —
so this needs a deprecation path, not a rename in place: accept `hidden` as an
alias that warns, and drop it a release later.

## Watch for

`DisplayStyleConfig::hidden` is read in six places in `jp_cli`
(`coordinator.rs`, `print.rs`, `turn.rs`) and the polarity flips at each.

`conversation.tools.'*'.style.hidden` fills field-by-field into each tool
(ticket `042d6hq`), and `ToolsConfig::to_partial` subtracts the `'*'` block from
each tool's style using equality.
An alias has to resolve before that subtraction or a tool setting `hidden` and a
`'*'` block setting `show` will not compare equal.

[RFD 091]: https://jp.computer/rfd/091
[RFD 095]: https://jp.computer/rfd/095
