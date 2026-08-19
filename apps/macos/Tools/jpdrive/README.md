# jpdrive

Reads and acts on a running macOS app's accessibility tree, speaking JSON. The
`debug_app_*` tools shell out to it; the Rust side stays the presenter, parsing
the JSON and rendering markdown.

Swift rather than Rust because `AXUIElement` is CoreFoundation-shaped: ordinary
code here, unsafe bindings or a 784-download crate there.

External rather than an in-app automation socket, deliberately. Driving through
`AXUIElement` means a broken accessibility tree breaks the tooling, which is the
pressure that keeps the app's accessibility honest.

## Build

```sh
just build-drive
```

The binary lands at `.build/release/jpdrive` under this directory.

## The TCC question

Everything downstream depends on one unknown: does a binary launched as a child
of `just serve-tools` inherit the Accessibility grant given to the terminal?

macOS attributes TCC to the *responsible process*, which for a command-line tool
is normally the terminal rather than the tool. That is the same mechanism behind
the `sample(1)` note in `.config/jp/tools/src/debug_jp/profile_sampling.rs` about
granting Terminal *Developer Tools*. Apple documents neither the algorithm nor
its stability, so the answer has to be measured.

`jpdrive doctor` measures it. Run it three ways, with Accessibility granted to
the terminal application and the app running:

Check the target first. An empty `pgrep` means the app is not running, and a
run without a target reports the trust flag alone, which is the half of the
answer that can be wrong:

```sh
pgrep -f JP.app   # must print exactly one pid
```

```sh
# 1. Directly from the terminal.
.build/release/jpdrive doctor --pid $(pgrep -f JP.app)

# 2. Through just, which adds the process layer the tools will run under.
just drive-doctor $(pgrep -f JP.app)
```

The third case, a child of `jp-tools` under `just serve-tools`, needs a tool that
shells out to the driver. Reaching it means writing the first `debug_app_*` tool,
which is why cases 1 and 2 come first: if the grant already fails at case 2,
nothing is learned by going further.

Compare `trusted` and `probe.axError` across the runs. `trusted: true` with a
window count means the grant inherits. `trusted: false`, or `api_disabled` /
`cannot_complete` from the probe, means it does not, and the driver needs its own
signed bundle or its own grant.

The report lists the ancestor chain, so a `false` says which processes were
candidates for holding the grant.

The check never prompts. `AXIsProcessTrustedWithOptions` with
`kAXTrustedCheckOptionPrompt` would raise the system dialog and change the state
being measured.

### Result

**The grant inherits.** With Accessibility granted to Ghostty, case 2 reports
`trusted: true` and a window count from a chain of six:

```
ghostty → login → fish → just → sh → jpdrive
```

So `tree`, `windows`, `menu`, and `act` need no signed bundle and no grant of
their own. They can assume the terminal's. Case 3, a child of `jp-tools` under
`just serve-tools`, adds one more process of the same kind and is still
unmeasured.

Observations across the runs, on macOS with Ghostty as the terminal:

- Process depth is not the variable. Run directly from the shell (chain of four,
  up to `ghostty`) and through `just` (chain of six, adding `sh` and `just`), the
  report is identical. Whatever governs the grant, it is not the number of
  processes between the terminal and the driver.
- The trust flag and the probe agree. `trusted: false` came with
  `ax_error: api_disabled` from a real read against a running app, which is what
  the accessibility API returns to an untrusted caller; `trusted: true` came with
  a window count. No case has been seen where the two disagree.
- Untested: a terminal instance started *before* the grant. The `false` runs and
  the `true` run may differ by the grant alone, by a relaunch, or by both, so
  "the grant is not visible to this terminal instance" is not yet ruled out as a
  separate failure mode.

## Screen Recording is a second grant

`windowid` answers the window server rather than the accessibility API, and the
two are governed by different TCC grants. Enumerating windows needs neither, so
the command works with nothing granted at all; reading a window's *title*, and
capturing its content with `screencapture -l`, need Screen Recording.

That is why the report pairs the list with a `screen_recording` flag rather than
refusing outright. Missing the grant, a capture succeeds and returns the desktop
where the window should be, so the caller has to know before it writes a file.
An untitled window in the list is the same fact seen from the other side.

The pane is System Settings ▸ Privacy & Security ▸ Screen & System Audio
Recording, and as with Accessibility it is the terminal application that needs
it, not the driver.

### Result

**The grant inherits.** Measured with Ghostty as the terminal, the driver run as
a child of `jp-tools` under `just serve-tools`: before the grant,
`screen_recording` came back `false` and `debug_app_screenshot` refused; after
granting Screen Recording to Ghostty and restarting it, the same call captured
the window.

So the flag is worth trusting, and this grant reaches a driver six processes
deep from the terminal, same as Accessibility does.

Untested: whether the restart was necessary. The grant and the restart happened
together, so nothing here separates them.

## What the sidebar looks like through accessibility

SwiftUI's `.accessibilityIdentifier` does not land on the element that owns
behaviour. For a `List` row it lands two levels below it:

```
AXOutline    AXIdentifier: sidebar.list      AXRows: 1065, AXVisibleRows: 9
  AXRow      AXSelected settable: true
    AXCell   AXSelected settable: false, AXScrollToVisible settable: true
      AXUnknown  AXIdentifier: sidebar.row.<conversation id>
                 AXAttributedDescription: "<title>, <n> events"
                 no actions, no children
```

So addressing an element and acting on it are two different steps. The identified
element has no actions at all: no `AXPress`, nothing. Selecting a row means
walking up to the `AXRow` and writing `AXSelected`.

That write is preferable to a synthesized click for a reason beyond determinism.
Every row exists as an accessibility element, but only nine are on screen: the
outline's frame is 41658pt tall against a 398pt viewport. A click at
`AXActivationPoint` would miss an off-screen row, or land on whichever row
occupies those coordinates instead. An `AXSelected` write is independent of
scroll position.

`AXScrollToVisible` appears as a settable *attribute* on a sidebar cell and as an
*action* on a transcript event, so scrolling has to try both forms.

The sidebar materialises every row; the transcript does not. Only one
`transcript.event.*` element exists at a time, so an identifier that names an
unrendered event cannot be waited for, only scrolled to.

Writing `AXSelected` on a row 690 places down a thousand-row list selects it and
brings it into view, so selecting a row needs no scrolling step of its own. The
transcript still does.

### The identified element cannot be walked upwards

The `AXUnknown` carrying the identifier reports no `AXParent`, and no
`AXTopLevelUIElement` either, unlike the cell and row above it. Climbing from it
arrives nowhere.

So resolving an identifier means keeping the chain the search descended through,
not finding the element and navigating from it afterwards. Anything that acts on
an ancestor of an identified element depends on this.

### Cost

An accessibility round-trip to this app costs roughly 3ms, and that number sets
every other budget:

- Reading the first few rows under `sidebar.` takes 250ms.
- Finding one row 690 places down takes 5.8s, because the search reads about two
  thousand elements to get there and cannot prune on the way: every identifier in
  the sidebar sits on a leaf.

Hence the batched reads and the match budget. Anything that polls should resolve
an element once and re-read that reference, rather than searching each time.

## Acting on an element

Each step names exactly one mechanism, because the mechanism depends on what the
element is and guessing hides regressions:

| step | addressed by | mechanism |
| --- | --- | --- |
| `select` | identifier | write `AXSelected` on the nearest ancestor accepting it |
| `press` | identifier | `AXPress` on the element itself |
| `type` | identifier | write `AXValue`, then `AXConfirm` |
| `perform` | identifier | a named action, for the verbs with no step of their own |
| `menu` | titled path | `AXPress` on the item the path resolves to |
| `click` | identifier | synthesized mouse event at `AXActivationPoint` |

`press` and `menu` end in the same call and are not redundant: they differ in what
they address by, and that is what a test pins. `closeAll:` is an `AppKit` selector
name that survives the item moving to another menu, so a script keyed on it cannot
notice the menu bar being rearranged. `["File", "Close All"]` names the structure
the user sees, and a path that stops resolving reports how far it got and what that
level holds instead — which is the assertion failure a layout test wants to read.

A step that names the wrong mechanism fails and says which actions the element
does accept. There is no fallback chain: if a sidebar row stopped accepting
`AXSelected`, a driver that quietly fell back to a synthesized click would keep
every script green while the app's accessibility rotted, which is the failure this
tool exists to prevent.

`select` and `type` read the attribute back afterwards, because a write can be
accepted and discarded. `press` cannot: nothing observable says a button did
anything, so its result reports no confirmation rather than claiming one.

### A menu step has to bring the app forward

`menu` writes `AXFrontmost` on the application and waits for it to take, which
makes it the one step that takes focus from whatever had it.

Without it almost nothing in the menu bar can be pressed. AppKit disables every
item that acts on the front window or the responder chain while the application
is in the background, and against a driven instance that is most of the bar:
`Close`, `Copy`, `Select All`, `Show Sidebar`, and every `SwiftUI` command
reading a `@FocusedValue` all report `AXEnabled: 0`. `New Window` and `Close
All` do not, which is what makes the difference easy to miss — a first menu step
against an app-level item works, and the next one silently does nothing.

So the item's enabled state is checked before it is pressed, rather than
trusting `AXPress` to report a refusal. A disabled item accepts the press and
answers success.

An element that reports no `AXEnabled` at all is not disabled. Plenty carry no
such attribute, and reading its absence as a refusal would reject them all.

### Typing writes the value, and then has to commit it

`type` writes `AXValue` and performs `AXConfirm`. Both are needed, and the second
one is the part that was not obvious.

Writing `AXValue` on a `SwiftUI` `TextField` changes the text the field displays
and leaves the binding behind it untouched. Measured against the conversation
filter: after the write the field read back `"accessibility"` and the list still
showed all 1,066 rows. Deleting one character by hand then filtered on
`"accessibilit"` — the keystroke made the binding resync from whatever the field
held by then. So a `type` that only wrote the value would report success while the
application carried on as though nothing had been typed.

`AXConfirm` commits through the path the binding observes. A field advertising no
confirm action is not a failure — some publish every change as it happens — so the
result reports `committed` separately from `confirmed`: the text being in the field
and the application having seen it are different facts.

Synthesizing key events was rejected on three counts: the events go wherever focus
is, so a window activating mid-sequence types into it instead; posting them fast
enough to be useful means pauses between characters, which makes the step flaky
rather than deterministic; and event posting is global process state, so it could
not sit behind the element abstraction the rest of the driver is tested through.

The remaining cost is that per-character behaviour never runs. A field that
validates each keystroke, or completes as you type, sees one change rather than a
dozen.

### Clicking is the last resort

`click` raises the element's window and posts a mouse event at its
`AXActivationPoint`. It is the only step whose effect is not addressed to an
element: the event goes to whatever occupies that screen coordinate, which is why
the window is raised first and why an occluding window from another application
will still swallow it.

An element reporting no activation point is refused rather than clicked at the
origin. A sidebar row is exactly that case, and it wants `select`.

Posting is behind an `EventPoster`, so where the driver aimed can be asserted in a
test even though where the event lands cannot.

### Apple Events are a separate pathway, and that one does not inherit

Reading the same tree through AppleScript fails from the terminal the driver
succeeds from:

```
System Events got an error: osascript is not allowed assistive access. (-1719)
```

Two different checks. `AXIsProcessTrusted`, which the driver calls, resolves to
the responsible process and finds the terminal. `System Events` requires the
calling binary itself to be listed, and the calling binary is `/usr/bin/osascript`
— shared by everything on the machine, so granting it grants far more than the
driver needs.

This is the second reason the driver is a binary of its own rather than a shell
script over `osascript`, alongside the one at the top of this file. It also means
AppleScript is not a fallback when the driver is missing a verb: the verb has to
be added here.
