# Affordance map

What the JP reader responds to, and what each response is meant to do.
This is the contract phase 4 of [RFD 099] holds the app to; the QA checklist
beside it ([`QA.md`]) is how it gets checked by hand.

Anything listed here that is not implemented says so.

## Menus

| Menu   | Item                | Shortcut | Does                                                          |
| ------ | ------------------- | -------- | ------------------------------------------------------------- |
| JP     | About JP            |          | Standard.                                                     |
| JP     | Quit JP             | ⌘Q       | Standard.                                                     |
| File   | New Window          | ⌘N       | Opens a window on the most recently opened workspace.         |
| File   | Open Workspace…     | ⌘O       | Directory chooser; any directory inside a workspace opens it. |
| File   | Open Recent ▸       |          | Workspaces opened before, newest first.                       |
| File   | Open Recent ▸ Clear |          | Empties the list.                                             |
| File   | Close               | ⌘W       | Closes the window, or the frontmost tab.                      |
| Edit   | Copy Link           | ⇧⌘C      | Copies the selected conversation's `jp://` URI.               |
| View   | Hide/Show Sidebar   | ⌃⌘S      | Hides or shows the conversation list.                         |
| View   | Show All Tabs       | ⇧⌘\\     | Standard.                                                     |
| Window | Show Previous Tab   | ⌃⇧⇥      | Standard.                                                     |
| Window | Merge All Windows   |          | Standard.                                                     |

`File ▸ Open Workspace` and `Open Recent` act on the frontmost window rather
than opening a new one, so they are disabled when no window has focus.
⌘N first.

**New Window opens the last workspace rather than an empty window.** A window
with no workspace can do nothing but ask for one, and the overwhelmingly likely
answer is the workspace you were just reading.
This is also what makes ⌘W on the last tab acceptable: closing is cheap because
reopening is one keystroke and lands you back where you were.

## Conversation list

| Input                  | Does                                                  |
| ---------------------- | ----------------------------------------------------- |
| Click                  | Selects; the transcript follows.                      |
| Double-click           | Opens the conversation in its own window.             |
| ↑ / ↓                  | Moves the selection.                                  |
| Escape                 | Clears the selection; the transcript pane empties.    |
| Right-click            | Context menu: Open in New Window, Copy Link.          |
| Drag                   | Drags the conversation out as a `jp://` URI.          |
| Type in the filter     | Narrows the list to titles containing what was typed. |
| Click the clear button | Empties the filter box, restoring the whole list.     |
| Drag the divider       | Resizes the sidebar, between 220 and 480 points.      |

The whole row is a click target, including the padding around the text.

A row shows the conversation's title over its date and event count, and a pinned
conversation carries a pin glyph in the accent colour beside them.
Rows are a fixed height, sized for a title of two lines: a list has to know its
total content height to size a scroll bar, and variable-height rows mean
measuring every row rather than the visible ones.

A row draws its own background, its selection and the line under it.
The list contributes none of the three: its separators run edge to edge and its
selection is a full-width fill in the system accent colour.
The table view's own selection drawing is turned off outright — see
`Sources/ListSelectionHighlight.swift` — because nothing drawn above it hides
it reliably.

The selected row is a rounded fill with a thick accent bar down its leading
edge, both clipped to the same shape.
**No line is drawn against a selected row**, above or below it, so that fill is
not cut across at either end.

A title wraps to a second line before it truncates.
The row is a fixed height, so the space a one-line title leaves is simply empty
— which is where Bear puts a content preview, and where one would go.

**A line above the first row appears only once the list is scrolled away from
the top.** At rest there is nothing to separate it from; scrolled, it separates
the search field from the rows passing under it.

**Pinned conversations sort above the rest**, keeping the library's
most-recently-active order inside each group, so pinning lifts one conversation
and moves nothing else.

The filter's clear button is always there, whether or not there is anything to
clear.
A control that comes and goes with what has been typed moves the text's right
edge as it appears.

Selection, double-click and the context menu are all the list's own, through
`contextMenu(forSelectionType:primaryAction:)`, rather than gestures attached to
each row.
That is both why they behave like every other Mac list and why scrolling a large
sidebar stays cheap: a per-row context menu is rebuilt for every row the list
realizes.

The list holds one conversation at a time, so the context menu acts on that one:
Open in New Window opens a single window and Copy Link copies a single URI.
Shift-clicking does not extend the selection.

Edit ▸ Copy Link copies the same URI for the selected conversation, and is what
reaches it without a pointing device.
It takes ⇧⌘C rather than ⌘C so the transcript keeps the shortcut for copying
selected text.
It is greyed out while nothing is selected.

Escape clears the selection, which is the only way back to an empty transcript
pane once a conversation has been read.
It belongs to the list, so Escape while the filter box has focus still means
"clear what I typed".

**Copy and drag produce a `jp://<id>` URI**, which is the form JP itself uses to
reference a conversation, so pasting into a terminal or a query is useful.
Not a markdown file — that is [noted as future work](#not-implemented).

## Transcript

| Input                | Does                                                           |
| -------------------- | -------------------------------------------------------------- |
| Select text          | Selects across the whole transcript, not just one message.     |
| ⌘C                   | Copies the selected text.                                      |
| Scroll               | Scrolls; the scroll bar reflects the real height.              |
| Drag the window edge | Re-wraps the text as the window moves, at any scroll position. |

**The whole conversation is one text view.** Not a stack of one view per
message: a text view lays out what its viewport needs and re-wraps
incrementally, where a stack of views each measure themselves and a width change
costs the sum of them.
That is also why selection runs across messages rather than stopping at one, and
why the scroll bar can state a real height instead of an estimate.

**Only messages are shown.** A user message and an assistant message each render
under the name of whoever said it.
Tool calls, reasoning, inquiries, config changes and turn markers have no prose
to show and never cross the FFI boundary — the library leaves them out rather
than the app filtering them.
Tool calls, attachments and reasoning display are Non-Goals of RFD 099.

**Messages are grouped into turns.** A turn is one user request through the
assistant's final answer to it.
Where the boundaries fall is decided on the library side, because the rules are
not recoverable from the events alone: there is an implicit leading turn, and a
marker that opens a turn only sometimes.
The boundary is drawn as space — the gap above the first message of a turn is
wider than the gap between two messages inside one.

**Block markdown renders**: headings, ordered and unordered lists with nesting,
fenced code blocks, block quotes, thematic breaks, and the inline set (bold,
italic, `code`, links, strikethrough).
Soft line breaks reflow into the paragraph, per CommonMark, which is what the
terminal renderer does too.

**Tables lay out in columns**, one row per line, each cell carried to its column
by a tab stop.
A column takes the alignment the source declared — `---:` in the separator row
right-aligns it — and the header row is bold.
Column width is fixed rather than measured: measuring means laying every cell
out at a width the container has not settled on, and redoing it on every resize,
for a reader rather than an editor.

Deliberately not `NSTextTable`, which would give real cell boxes and is a
TextKit 1 feature — putting one in the string drags the text view off TextKit 2
silently, taking viewport layout with it.

There is no reading-width cap.
One text view re-wraps cheaply enough that capping the column bought nothing,
and the cap that used to be here never engaged on a wide display anyway.

The text view runs on **TextKit 1**, with contiguous layout, so the document
height is exact and the scroll bar states it rather than estimating.
That is the reason for the choice: an honest scroll bar was a goal, and TextKit
2's height is an estimate that refines as it scrolls, which moves the knob under
the pointer.

It costs real work.
The same ten programmatic resizes measured 438 samples here against 155 on
TextKit 2 for a 29-event conversation, and 412 against 355 for a 167-event one
— so TextKit 1 is flat with document size where TextKit 2 scales, and the gap
narrows as conversations grow.
Revisit if a long conversation starts feeling slow to resize;
`Sources/TranscriptTextView.swift` has it behind one named constant.

**The text container's width is set by hand on every frame of a drag.** A text
view normally hands its width to the container it is tracked by, and does not do
that while a live resize is in progress — the container keeps the width the
drag started from until the mouse comes up.
Nothing then invalidates layout, and the view faithfully redraws lines wrapped
to a width the window no longer has.
See `Sources/TranscriptTextView.swift`; `TranscriptReflowTests` holds it.

## Windows

- **No title bar.** A workspace window's content runs to the top of the window,
  with the close, minimize and zoom buttons over the sidebar's top-left corner
  and the search field beside them.
  The window still carries a title — it is what the Window menu lists and what
  an external driver addresses it by — but nothing displays it.
- **The window buttons are moved.** macOS puts them six points in and centres
  them fourteen points down, which is the middle of a title bar this window does
  not have.
  They are placed against the search field instead, and put back whenever AppKit
  lays the title bar out afresh.
  There is no supported way to ask for this: a title bar grows to fit a toolbar,
  and a toolbar would span the whole window.
  See `Sources/WindowButtons.swift`.
- **No sidebar toggle button**, because there is no title bar to put one in.
  View ▸ Hide Sidebar (⌃⌘S) is how the sidebar is hidden and brought back.
- **The window holds its two panes itself**, rather than in a
  `NavigationSplitView`.
  `NSSplitView` draws a translucent divider over whatever is behind it and
  offers no way to change its colour or width, which left the line between the
  panes two pixels of two different greys that shifted with the content
  underneath.
  The divider is now the app's own: two points, one colour, and draggable
  through a wider invisible strip around it.
- **Restored per window**: the workspace, the selected conversation, the
  sidebar's width, and whether the sidebar is showing.
- **Windows are not keyed by workspace.** ⌘N opens another window on the same
  workspace rather than bringing the existing one forward, and two windows may
  show one workspace.
  Each window holds its own choice in scene storage.
  Paths are still canonicalized, so the recents list and a window's stored path
  agree on spelling.
- **A conversation can be pulled into its own window** by double-clicking.
  That window carries the workspace path with it, so it can be restored at
  launch with no workspace window open.
- **Native tabbing**, through Window ▸ Merge All Windows and the tab bar.

## Accessibility

- The conversation list is labelled `Conversations`.
- Each row is one accessibility element combining the title and event count,
  rather than two unrelated fragments.
  A pinned row appends `, pinned`.
- A row's label leaves out the date it displays.
  The date is relative for anything active today, so a label carrying it would
  read differently one minute later and could not be pinned by a test.
- The transcript is selectable text, so VoiceOver reads messages as text.
- **There is no element per message.** The conversation is one text area, and
  its value is every message it is showing.
  A driver addresses `transcript.text` and reads that value; a test asserting on
  what is on screen compares it whole, which catches a missing speaker label or
  a duplicated message that a search for one phrase would not.

### Identifiers

Every element an external driver has to find carries an accessibility
identifier, so it can be reached without matching display text.
The names live in `Sources/AccessibilityID.swift` and are pinned by
`AccessibilityIDTests`.

| Identifier                     | Element                                                 |
| ------------------------------ | ------------------------------------------------------- |
| `sidebar.state.loading`        | Spinner while the workspace is read.                    |
| `sidebar.filter`               | The box that narrows the list.                          |
| `sidebar.filter.clear`         | The button that empties the filter box, always present. |
| `sidebar.list`                 | The conversation list.                                  |
| `sidebar.row.<conversationID>` | One row.                                                |
| `sidebar.state.nomatches`      | Message shown when a filter matches none.               |
| `sidebar.state.unavailable`    | Message shown instead of a list.                        |
| `transcript.state.loading`     | Spinner while a conversation is read.                   |
| `transcript.scroll`            | The scrolling transcript.                               |
| `transcript.text`              | The text the transcript is drawn as.                    |
| `transcript.state.unavailable` | Message shown instead of a transcript.                  |

A row is named by the conversation's ID, so retitling a conversation does not
move it.
There is no `sidebar.state.loaded` or `transcript.state.loaded`: a view carries
one identifier, and `sidebar.list` and `transcript.scroll` exist only in that
state, so they are the predicate.

## Not implemented

Named here so the gaps are visible rather than discovered.

- **Drag produces a URI, not a markdown file.** Dropping into Finder therefore
  does nothing useful.
  Filed as future work; it needs a presentation-neutral conversation-to-markdown
  projection, which RFD 099 lists under Non-Goals.
- **No drop targets.** Nothing accepts a dragged conversation, including other
  JP windows.
- **No live updates.** A window loads its workspace once.
  Turns written by a concurrent `jp query` are invisible until the workspace is
  reopened.
  This is RFD 099's stated v0.1 behavior.
- **The transcript's scroll position is not restored.** A relaunched window
  reopens at the top of the conversation it had selected.
  The workspace, the selection, the sidebar's width and its visibility are all
  restored; this one is not.
- **A table column is a fixed width**, so a cell longer than one wraps into the
  next column's space rather than widening it.
  Real cell boxes need `NSTextTable`, which is TextKit 1 only.
- **The pointer does not change over the pane divider.** Dragging it resizes the
  sidebar from either side, and `ResizeCursorAreaTests` shows the view asks for
  the horizontal-resize cursor — but the hosted `NSView` carrying that request
  sits inside the `accessibilityElement` that publishes `window.divider`, and
  the collapse appears to detach it.
  Moving the request outside that element restores the cursor and stops the drag
  reaching the strip, so the two want opposite orderings and the resize wins.
  Unresolved; the likely answer is hanging the cursor rect off a view that is
  not inside the accessibility element at all.
- **No find bar.** ⌘F does nothing.
  The text view is one document, so a find interaction would work across the
  whole conversation; it simply is not turned on.
- **⌘C does not copy from the conversation list.** It did, through a per-row
  `.copyable`, but that cost more in scrolling than the shortcut was worth.
  Edit ▸ Copy Link (⇧⌘C) and right click ▸ Copy Link both do the same thing.

[RFD 099]: ../../docs/rfd/099-native-macos-app-for-browsing-conversations.md
[`QA.md`]: QA.md
