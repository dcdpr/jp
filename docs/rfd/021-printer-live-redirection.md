# RFD 021: Printer Live Redirection

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-19

## Summary

This RFD adds runtime output redirection to `jp_printer`. A `Printer` can switch
its underlying writers at any point during execution via a new `SwapWriters`
command, without replacing the `Printer` instance or disrupting renderer state.

## Motivation

The `Printer` is constructed with fixed `out`/`err` writers that are moved into
a background worker thread. Once created, there is no way to change where output
goes. All renderers (`ChatResponseRenderer`, `ToolRenderer`,
`StructuredRenderer`, `JsonEmitter`) share a single `Arc<Printer>`, so the
output destination is locked in for the lifetime of the process.

This blocks use cases that need to redirect output at runtime — detaching a
conversation from the terminal, redirecting to a file mid-stream, or capturing
output from a specific phase during testing. Without live redirection, the only
options are replacing the `Printer` instance (which means either rebuilding all
renderers or introducing `ArcSwap` indirection) or maintaining a parallel output
path outside the printer (which duplicates rendering logic).

Live redirection is also useful for testing: swap to a memory buffer mid-test to
capture output from a specific phase without capturing setup noise.

## Design

### Command::SwapWriters

Add a new command variant to the `Printer` worker's command channel:

```rust
enum Command {
    Print(PrintTask),
    Flush(mpsc::Sender<()>),
    FlushInstant(mpsc::Sender<()>),
    SwapWriters {
        out: Box<dyn io::Write + Send>,
        err: Box<dyn io::Write + Send>,
        done: mpsc::Sender<()>,
    },
    Shutdown,
}
```

The worker processes `SwapWriters` in FIFO order with other commands. All
`Print` commands enqueued before `SwapWriters` write to the old destination. All
`Print` commands enqueued after write to the new one. The swap is a
deterministic point in the command stream.

Worker implementation:

```rust
Command::SwapWriters { out, err, done } => {
    let _ = self.out.flush();
    let _ = self.err.flush();
    self.out = out;
    self.err = err;
    let _ = done.send(());
}
```

### Public API

```rust
impl Printer {
    /// Replace the output writers, blocking until the swap completes.
    ///
    /// All print commands enqueued before this call write to the old
    /// destination. All commands enqueued after write to the new one.
    /// Call `flush()` or `flush_instant()` before swapping to ensure
    /// pending output reaches the old destination first.
    pub fn swap_writers(
        &self,
        out: impl io::Write + Send + 'static,
        err: impl io::Write + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        self.send(Command::SwapWriters {
            out: Box::new(out),
            err: Box::new(err),
            done: tx,
        });
        let _ = rx.recv();
    }
}
```

The method blocks until the worker has performed the swap. After it returns,
the caller knows the new writers are active.

### Worker Type Change

The `Worker` struct currently uses generic type parameters for its writers:

```rust
struct Worker<O, E> {
    out: O,
    err: E,
    // ...
}
```

To support runtime replacement, these change to trait objects:

```rust
struct Worker {
    out: Box<dyn io::Write + Send>,
    err: Box<dyn io::Write + Send>,
    // ...
}
```

This loses monomorphization, but the writers are behind a thread boundary —
there is no inlining benefit to preserve. The cost is one vtable dispatch per
`write()` call, which is negligible compared to the I/O itself.

`Printer::new()` boxes the writers at construction:

```rust
pub fn new(
    out: impl io::Write + Send + 'static,
    err: impl io::Write + Send + 'static,
    format: OutputFormat,
) -> Self {
    // ...
    thread::spawn(move || {
        let mut worker = Worker {
            out: Box::new(out),
            err: Box::new(err),
            // ...
        };
        worker.run();
    });
    // ...
}
```

The existing `Printer::terminal()`, `Printer::sink()`, and `Printer::memory()`
constructors continue to work unchanged — they call `Printer::new()` which
handles the boxing.

### Interaction with FlushInstant

A common pattern for time-sensitive swaps is `flush_instant()` followed by
`swap_writers()`:

```rust
printer.flush_instant();
printer.swap_writers(new_out, new_err);
```

`flush_instant()` cancels typewriter delays, drains pending commands
immediately, and blocks until complete. This minimizes the time between the
caller's intent to swap and the swap taking effect. Any `Print` commands
enqueued between the two calls race with the worker — in practice this window
is negligible since both calls happen on the same thread in sequence.

### Existing API Unchanged

The public `Printer` API does not change beyond the addition of `swap_writers`.
All existing methods (`print`, `println`, `eprint`, `flush`, `flush_instant`,
`shutdown`) work exactly as before. The `Printer::memory()` constructor also
remains unchanged — it serves a different purpose (capturing all output for
testing from construction time).

## Drawbacks

**Type erasure for all printers.** Changing `Worker<O, E>` to `Worker { out:
Box<dyn Write>, ... }` applies to all `Printer` instances, not just ones that
use `swap_writers()`. Printers that never swap still pay for the vtable
dispatch. The cost is negligible (one indirect call per write, dwarfed by actual
I/O) but it is a universal change.

**Blocking call.** `swap_writers()` blocks until the worker processes the
command. If the worker is mid-typewriter on a long task, the caller waits.
Callers who need a faster swap should call `flush_instant()` first.

## Alternatives

### SwitchableWriter (writer-level swap)

Wrap the inner writers in `Arc<Mutex<Box<dyn Write>>>`. The swap happens below
the worker, at the writer level. The next `write()` call goes to the new
destination regardless of the command queue.

This has one advantage: the swap is immediate, even for commands already in the
queue. But it introduces a mutex acquisition on every `write()` call (not just
during swaps), and it creates ordering confusion — commands enqueued before the
swap may write to the new destination if the worker hasn't processed them yet.
The caller must manually `flush()` before swapping to avoid this, which is a
footgun.

The command-based approach (`SwapWriters`) has correct ordering by construction.
Every command before the swap goes to the old destination, every command after
goes to the new one.

If the small window between `flush_instant()` and `SwapWriters` processing turns
out to be a problem in practice, `SwitchableWriter` can be revisited as the
immediate-swap alternative.

### Replace `Arc<Printer>` with `Arc<ArcSwap<Printer>>`

Replace the entire `Printer` atomically. All holders see the new printer
immediately.

Rejected because it replaces the entire printer (including the background worker
thread), not just the output destination. This loses the worker's queue, pending
typewriter state, and any in-flight writes. It also requires every call site to
load from the `ArcSwap`, adding overhead to every print.

## Non-Goals

- **Per-renderer redirection.** All renderers share a single `Printer`. This RFD
  does not add the ability to redirect individual renderers to different
  destinations.

- **Changing `OutputFormat` at runtime.** The format (text, pretty, JSON) is set
  at construction and does not change. Switching from pretty to plain mid-stream
  would require renderer resets.

## Implementation Plan

### Phase 1: Worker Type Change

Change `Worker<O, E>` to use `Box<dyn io::Write + Send>` for `out` and `err`.
Update `Printer::new()` to box the writers. All existing constructors and tests
continue to work.

Can be merged independently. No behavioral change.

### Phase 2: SwapWriters Command

Add `Command::SwapWriters` and `Printer::swap_writers()`. Add tests:

- Create a printer with memory writers, swap to different memory writers, verify
  each buffer received the correct output.
- Verify ordering: print before swap goes to old, print after goes to new.
- Verify `flush_instant()` + `swap_writers()` works as expected.

Can be merged independently.

## References

- [`jp_printer::Printer`](../../crates/jp_printer/src/printer.rs) — the
  current printer implementation.
- [`jp_printer::Printer::memory()`](../../crates/jp_printer/src/printer.rs) —
  existing memory-backed printer pattern using `SharedBuffer`.
