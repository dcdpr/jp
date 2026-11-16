# RFD 021: Printer Live Redirection

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-19
- **Summary**: Adds a `SwapWriters` command to `Printer` so output destinations
  can switch at runtime without replacing the printer instance.

## Summary

This RFD adds runtime output redirection to `jp_printer`.
A `Printer` can switch its underlying writers at any point during execution via
a `SwapWriters` command, without replacing the `Printer` instance or disrupting
renderer state.

## Motivation

`Printer` is constructed with fixed `out` and `err` writers that move into a
background worker thread.
Because all renderers (`ChatResponseRenderer`, `ToolRenderer`,
`StructuredRenderer`, `JsonEmitter`) share a single `Arc<Printer>`, the output
destination is fixed for the lifetime of the process.

This blocks use cases that redirect output at runtime, such as detaching a
conversation from the terminal, redirecting output to a file mid-stream, or
swapping to a memory buffer mid-test to capture a specific phase without setup
noise.
Without live redirection, callers must either replace the `Printer` instance
(rebuilding all renderers or introducing `ArcSwap` indirection) or maintain a
parallel output path that duplicates rendering logic.

> [!TIP]
> [RFD 027] uses `Printer::swap_writers()` in its client-server output model to
> redirect output between the server's in-memory buffer and an attached client's
> terminal at runtime.

## Design

### `Command::SwapWriters`

Add a `SwapWriters` variant to the `Printer` worker's command channel:

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

The worker processes `SwapWriters` in FIFO order alongside other commands,
making the swap a deterministic point in the command stream: `Print` commands
enqueued before `SwapWriters` write to the old destination, and commands
enqueued after it write to the new one.

When handling `SwapWriters`, the worker flushes both active writers, replaces
them, and signals completion:

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

`swap_writers` blocks until the worker completes the swap, so the new writers
are active as soon as it returns.
All existing `Printer` methods (`print`, `println`, `eprint`, `flush`,
`flush_instant`, `shutdown`) remain unchanged.

### Type-Erased Worker Writers

The `Worker` struct currently uses generic type parameters for its writers:

```rust
struct Worker<O, E> {
    out: O,
    err: E,
    // ...
}
```

To support runtime replacement, these fields become boxed trait objects:

```rust
struct Worker {
    out: Box<dyn io::Write + Send>,
    err: Box<dyn io::Write + Send>,
    // ...
}
```

Erasing the writer types gives up monomorphization, but the writers sit behind a
thread boundary where calls cannot be inlined across the channel.
The only cost is one vtable dispatch per `write()` call, which is negligible
compared to the I/O itself.

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

The `Printer::terminal()`, `Printer::sink()`, and `Printer::memory()`
constructors delegate to `Printer::new()` and remain unchanged
(`Printer::memory()` still captures all output from construction time for
tests).

### Interaction with `flush_instant`

For time-sensitive swaps, callers can pair `flush_instant()` with
`swap_writers()`:

```rust
printer.flush_instant();
printer.swap_writers(new_out, new_err);
```

`flush_instant()` cancels typewriter delays, drains pending commands
immediately, and blocks until completion, minimizing the delay before the swap
takes effect.
Any `Print` commands enqueued between the two calls race with the swap, though
this window is negligible when both calls run sequentially on the same thread.

## Drawbacks

**Type erasure for all printers.** Replacing `Worker<O, E>` with `Box<dyn
Write>` fields applies to every `Printer` instance, including ones that never
call `swap_writers()`.
Every write pays for a vtable dispatch, though the indirect call is negligible
compared to the underlying I/O.

**Blocking swap.** `swap_writers()` blocks until the worker reaches the command
in its queue.
If the worker is mid-typewriter on a long task, the caller waits unless it calls
`flush_instant()` first.

## Alternatives

### Writer-Level Swap (`SwitchableWriter`)

Wrapping the inner writers in `Arc<Mutex<Box<dyn Write>>>` swaps the destination
below the worker at the writer level, so the next `write()` call goes to the new
destination regardless of the command queue.

This makes the swap immediate even for commands already in the queue, but it
requires locking the mutex on every `write()` call and breaks FIFO ordering:
commands enqueued before the swap write to the new destination if the worker has
not processed them yet.
Avoiding that race requires the caller to manually `flush()` before every swap,
whereas `SwapWriters` preserves command ordering by construction.

If the narrow window between `flush_instant()` and `SwapWriters` becomes a
problem in practice, `SwitchableWriter` can be revisited.

### Replace `Arc<Printer>` with `Arc<ArcSwap<Printer>>`

Replacing the entire `Printer` atomically via `Arc<ArcSwap<Printer>>` makes the
new instance visible to all holders immediately, but it also replaces the
background worker thread.
Doing so discards the worker's command queue, pending typewriter state, and any
in-flight writes, while adding an `ArcSwap` load to every print call.

## Non-Goals

- **Per-renderer redirection.** All renderers share a single `Printer`;
  individual renderers cannot be redirected to separate destinations.

- **Changing `OutputFormat` at runtime.** The output format (text, pretty, JSON)
  is fixed at construction because switching formats mid-stream would require
  resetting renderer state.

## Implementation Plan

### Phase 1: Worker Type Change

Change `Worker<O, E>` to store `Box<dyn io::Write + Send>` for `out` and `err`,
and update `Printer::new()` to box both writers.
All existing constructors and tests continue to work without behavioral changes.
This phase can be merged independently.

### Phase 2: `SwapWriters` Command

Add `Command::SwapWriters` and `Printer::swap_writers()`, with tests covering:

- Swapping from one pair of memory writers to another and verifying that each
  buffer receives the expected output.
- Ordering across a swap: prints enqueued before the swap go to the old writers,
  and prints enqueued after go to the new writers.
- Calling `flush_instant()` immediately before `swap_writers()`.

This phase can be merged independently.

## References

- `jp_printer::Printer` (`crates/jp_printer/src/printer.rs`): the printer
  implementation.
- `jp_printer::Printer::memory()` (`crates/jp_printer/src/printer.rs`):
  memory-backed printer constructor using `SharedBuffer`.

[RFD 027]: 027-client-server-query-architecture.md
