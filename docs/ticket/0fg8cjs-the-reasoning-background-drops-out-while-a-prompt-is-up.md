# The reasoning background drops out while a prompt is up

- **Status**: Done
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-08

A tool call inside a reasoning block renders its chrome against the reasoning
background (RFD 095): every visual row shows the background to the right edge,
cursor-relative rewrites and `\x1b[K` erases included.

When that tool call prompts — `run = "ask"`, or a user-targeted question — the
background disappears for the prompt's duration and comes back afterwards:

```
<grey>  Calling tool window_alpha
<grey>  Calling tool window_asks
        > Run local window_asks tool? [y,Y,n,N,p,r,e,?] y   ← not grey
<grey>  asks done
```

The prompt widget is a visual row inside a reasoning region, so by RFD 095's
rule it should carry the background like the rows around it.

## Why it happens

Prompt output goes through `Printer::prompt_writer()` as `PrintTarget::Tty`
tasks.
Nothing in that path knows about the reasoning region: the background is applied
by `ShadedWriter` in `ToolRenderer::write_chrome` and by the chat renderer,
neither of which the prompt writer passes through.

So the gap is structural rather than an oversight in one call site — there is
no seam today where a prompt's writes could pick the background up.

## Not the same as

- The status region's `RowBackground`, which covers the rows the printer worker
  draws and erases.
  That works; it is the mechanism RFD 091 phase 6 added for exactly this
  invariant.
- A prompt suspending the status region, which is correct and deliberate.

## Shape of a fix

The printer already carries a per-region background as an opaque SGR parameter.
The same idea would work here: a background the *prompt writer* asserts before
each write and closes at row end, set by whoever owns the reasoning region.

`PromptWriter` is the natural place — it already exists, already wraps every
prompt's writes, and already carries a `SuspendGuard`, so its lifetime is
exactly the span that needs shading.

Worth checking before building: whether `inquire`'s own cursor handling survives
a background being asserted around its writes, since it redraws its line on
every keystroke.

## Watch for

Whatever asserts the background has to close it before the widget reads a
keypress, or the terminal's own echo inherits it.

The eight prompt sites in `T-0ffsv2r` bypass `Printer` entirely, so they would
not be covered by a `PromptWriter`-based fix either way.

## Comments

-----

- **From**: jp
- **Date**: 2026-09-08T10:53:04Z

Tool prompts are fixed on `visible-mcp-logs`; the `Ctrl+X` editor escape is not.

## What landed

`ToolPrompter` gained a `background: Mutex<Option<DefaultBackground>>` and a
`canvas()` helper returning `PromptCanvas`, an enum over a plain `PromptWriter`
or one wrapped in `jp_md::ShadedWriter`.
The coordinator sets it in `resolve_tool_call_decision`, the one place holding
both the prompter and the renderer that owns the region.

`ShadedWriter` rather than a hand-written escape pair, for the reason this
ticket flags under "worth checking": `inquire` rewrites its line with `\r\x1b[K`
on every keystroke and emits its own SGR resets mid-line.
Asserting the background once and resetting at the end is cleared by the
widget's first reset.
`ShadedWriter` tracks the content's own attribute state and re-asserts, which is
what it was built for.

The "watch for" note turned out not to apply: `inquire` reads keys in raw mode,
so there is no echo to inherit the background.

Covered: `prompt_permission`, `prompt_question`, `prompt_result_confirmation`.

## What remains

`ToolPrompter::inline_edit` uses `Printer::owned_prompt_writer()`, which returns
`Box<dyn io::Write + Send>`.
`ShadedWriter<W>` requires `W: fmt::Write`, and a boxed `io::Write` does not
implement it, so the same wrap does not apply.

Options, roughly in order of preference:

- An adapter implementing `fmt::Write` over `Box<dyn io::Write + Send>`, then
  the same `ShadedWriter` wrap.
  Smallest change; the adapter is a handful of lines and would live next to
  `PromptCanvas`.
- Have `owned_prompt_writer` return a type implementing both traits.
  Wider blast radius — `jp_inquire::InlineReply::prompt` takes `Box<dyn Write +
  Send>` by value, and the interrupt handler uses it too.

Reached by `Ctrl+X` from the inline reply widget during a tool prompt, so it is
the same bug in a rarer path.

## Note for whoever finishes this

The background must be closed on `Drop`, not by an explicit call at the end of
each prompt.
A widget can end by cancellation or by an error, and a background left open
paints everything printed afterwards — the same failure mode as the region
erase bug fixed in the same branch, where `\x1b[K` under an active background
painted the row rather than clearing it, and the shell prompt inherited it after
`jp` exited.

`prompter_tests.rs` has a test that abandons a prompt mid-session and asserts
the close still lands; worth extending to the editor path rather than writing a
new one.

-----

- **From**: jp
- **Date**: 2026-09-08T13:57:29Z

Done, including the `Ctrl+X` / inline-reply path this ticket left open.

## Where it landed

The background moved into `jp_printer`, which is where this ticket's "shape of a
fix" pointed: `Printer::set_prompt_background` holds it, and both
`prompt_writer` and `owned_prompt_writer` wrap their writer in a `ShadedWriter`
when one is set.
Every prompt taken from the printer is shaded without its call site knowing a
region exists, so the reply widget is covered by the same code as the approval
prompt rather than needing its own.

That required `ShadedWriter` to be reachable from `jp_printer`, which depends on
`jp_term` and deliberately not on `jp_md`.
`ansi` and `shade` moved to `jp_term` along with `DefaultBackground`,
`BackgroundFill` and `line_fill` (now `jp_term::background`).
None of them were markdown: `segments` tokenizes an escape stream, `AnsiState`
tracks what a stream left active.

The `owned_prompt_writer` trait problem this ticket recorded resolved itself in
the move.
`OwnedPrinterWriter` gained a `fmt::Write` impl and became the inner writer of a
shared `Canvas<W>`, so the adapter is a trait impl on the type that needs it
rather than a wrapper at the call site.

`ToolPrompter::set_background`, `canvas()`, and `PromptCanvas` are deleted.
The coordinator still names the background at the same point in
`resolve_tool_call_decision` — the region is per tool call, so the read has to
happen there — but it now hands it to the printer.

## Two things found on the way

The `PromptCanvas::flush` this ticket's earlier comment described was swallowing
the flush for the shaded variant, on the grounds that the printer holds nothing
back a flush could release.
True of buffering, false of ordering: on the prompt path a flush is a barrier,
and `inquire` flushes before it reads a key.
Inside a reasoning block that was the only difference between the two variants,
which is why prompt output staircased there and nowhere else.
Fixed separately, along with making prompt-writer acquisition quiesce the
terminal.

`MockPromptBackend::inline_reply` wrote nothing to the stream it was handed, so
a test asserting on what the reply widget renders passed against no prompt at
all.
It now writes its message, like the real widget does.

## Not covered

The eight prompts in `T-0ffsv2r` still build their own `io::stderr()` writer, so
they are still unshaded.
They pick this up for free once they go through the printer.
