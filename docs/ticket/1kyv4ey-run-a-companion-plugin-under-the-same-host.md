# Run a companion plugin under the same host

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-16
- **Label**: client=cli
- **Label**: domain=plugins
- **Label**: package=jp_cli
- **Label**: package=jp_plugin
- **Label**: type=pre-rfd

A `jp` invocation runs a single command plugin.
When one plugin needs a companion plugin running alongside it (for example, `jp
vite serve` running `jp review serve` to serve review endpoints on the docs
site), the outer plugin currently has to spawn a second `jp` child process.

## Cost of a nested host process

- The outer plugin must translate its own `shutdown` message into an OS signal
  for the child `jp` process and wait for it to exit, pushing process and signal
  management into the plugin.
- Shutdown timeouts stack: the outer plugin's `shutdown_timeout_secs` must
  exceed the inner plugin's timeout, or the outer host will kill the process
  group while the inner host is still stopping an in-flight turn.
- Pressing Ctrl-C reaches only the outer host, where the topmost interrupt
  handler belongs to the outer plugin rather than the inner host's running turn.
  As a result, Ctrl-C cannot stop just the active turn the way it does under a
  single host.
- Both hosts load the workspace and resolve configuration independently.

## Current host architecture

All references below are in `crates/jp_cli/src/cmd/plugin/dispatch.rs` unless
noted otherwise:

- `run_external` resolves and admits a single plugin, then calls
  `run_plugin(name, binary, args, &mut Ctx)`, which holds `&mut Ctx` until the
  plugin exits.
- Per-plugin runtime state is already local to `run_plugin`: the child process
  tree, stdin writer, stdout reader channel, `JoinSet` of delegated turns,
  `RunningTurns`, `LateWrites`, interrupt handler registration, and shutdown
  budget claim.
- The process-wide services already support multiple registrants:
  `ShutdownBudgets` (`shutdown.rs`) waits for the maximum claimed timeout, and
  `SignalRouter` (`signals.rs`) maintains a stack of interrupt handlers.
- The plugin message loop is sequential.
  It requires `&mut Ctx` for `handle_request` (`&mut ctx.workspace`) and for
  `prepare_turn`, which calls `ctx.mcp_client.set_servers`,
  `configure_active_mcp_servers`, and `swap_config`.
  Delegated turns then execute on background tasks.
- `edit_files` runs synchronously inside the message loop and blocks it while
  the user's editor is open.
- RFD 113 Phase 1 (Accepted, tracked by T-0syqmp1) refactors `run_plugin`,
  `message_loop`, and the request handlers to take `CliCtx` and `WorkspaceCtx`
  and removes `swap_config`.

## Candidate designs

1. **Single event loop multiplexing both plugins.** The host selects across both
   plugins' reader channels and dispatches requests sequentially, keeping
   per-plugin state in a struct per plugin instance.
   `Ctx` ownership remains unchanged, though a synchronous request like
   `edit_files` blocks both plugins while the editor is open.
2. **Concurrent `run_plugin` tasks.** Each plugin runs its own message loop,
   which requires sharing workspace and MCP state without an exclusive `&mut
   Ctx` borrow across the entire run.
   RFD 113 Phase 1 performs most of that context decomposition.

## Open questions for the RFD

- **Discovery and startup**: whether a companion plugin is started via a
  protocol request from the primary plugin (`start_plugin` with command and
  args), declared in the plugin manifest, or configured in `.jp/config.toml`.
  A protocol request lets the primary plugin pass runtime arguments such as a
  dynamically bound port.
- **Admission policy**: the companion plugin must pass `admit` and its own `run`
  policy, and an interactive approval prompt requires a terminal that the
  primary plugin may already be using.
- **Lifecycle notifications**: how the primary plugin learns that the companion
  is ready, which address or port it bound, and when it exits.
- **Failure coupling**: whether the companion exiting terminates the primary
  plugin, and vice versa.
- **Shutdown ordering**: in what order the two plugins receive `shutdown`, and
  how their `shutdown_timeout_secs` budgets combine.
- **Idle Ctrl-C routing**: when no turn is running, Ctrl-C reaches only the top
  plugin handler on the signal stack; stopping one plugin should cleanly shut
  down both.
- **Turn isolation**: an `interrupt` request from one plugin must not stop a
  turn started by the other plugin (`RunningTurns` is currently keyed only by
  `ConversationId`).
- **Terminal output**: both plugins share a single `Printer`.

Filed while designing RFD 136 (`jp-review`), which initially spawns a nested
`jp` host process.
