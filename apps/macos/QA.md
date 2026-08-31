# QA checklist

The behavior in [`AFFORDANCES.md`] that has to be checked against a running app.

Each item says who checks it:

- **`JPUITests/<test>`** — a committed test in `apps/macos/UITests`.
  CI runs the whole bundle with `just test-app-ui`; while writing one, run it by
  name through the `swift_test_ui` tool.
  Each test launches the app and takes the screen, so `just test-app` and the
  `swift_test` tool leave them out.

  The names below are a convenience, not the index: `swift_test_ui` asks the
  built bundle what it holds, so that is the list to trust.

- **Eyes** — needs a person, permanently.
  Smoothness, rendering, and anything whose answer is "does this look right".

- **Not yet mechanized** — checked by hand today, and a candidate for a test.

- **`debug_app_profile`** — answered by driving the app and reading back what
  it timed, rather than by a committed test.
  These are the items about cost, and they are checked in counts (view-body
  evaluations, FFI calls) rather than in milliseconds: a count is the same for
  the same steps, so it can be compared against an earlier run, while a
  millisecond threshold would be met by a broken build on a quiet machine and
  missed by a good one on a busy machine.

For everything still checked by hand, run `just run-app` first.
It launches in the foreground with output attached to the terminal, so warnings
and crashes are visible while you work through this.

The hand-run items should be checked against a workspace with a few hundred
conversations, not an empty one: several of these only misbehave at size.
The UI tests build their own three-conversation workspace, which is why the ones
that only fail at size stay with a person.

A suite shares one launched app across its tests, because launching costs
seconds and the work under test costs milliseconds.
A test that needs an app nobody has touched launches its own and says why; none
currently does.

`swift_test_ui` stops a run at the first failure and closes the app it was
driving, because a broken app usually fails every test after the first one too
and each costs a second to find that out.
`just test-app-ui` sets `CI`, which turns that off: nobody is watching a CI run,
and one run reporting everything beats a first failure reported quickly.

## Launch and console

- [x] The window opens showing the workspace named by `JP_WORKSPACE` —
      `JPUITests` launches every test this way, so any test passing proves it.
- [ ] **No `reentrant operation in its NSTableView delegate` warning**, at
      launch, on selection, or on quit.
      **Eyes**, or `debug_app_snapshot`: a UI test cannot read the app's
      console, because `testmanagerd` launches the app and keeps its output.
      `selectingKeepsTheInjectedMenuItems` covers the damage that warning
      reports, but not the warning.
- [ ] No other warnings or exceptions in the terminal.
      **Eyes**, same reason.

## Conversation list

- [x] The window carries the workspace directory name as its title, and nothing
      counting conversations beside it —
      `ConversationListTests/namesTheWorkspace`.
- [ ] **Nothing displays that title, and there is no strip of chrome above the
      transcript**: the window buttons sit over the sidebar's top-left corner
      with the search field beside them, and there is no sidebar toggle button.
      **Eyes**: the test above proves the title is carried, not that it is
      hidden.
- [x] View ▸ Hide Sidebar hides the conversation list and Show Sidebar brings
      it back, which is the only way to now that the button is gone —
      `ConversationListTests/viewMenuTogglesTheSidebar`.
- [ ] **⌃⌘S does the same as the menu item.** **Eyes**: the test above chooses
      the item rather than pressing the key.
- [x] Conversations are ordered most recently active first —
      `ConversationListTests/ordersByActivity`.
- [x] A pinned conversation sits above every unpinned one, whatever their
      activity, and its row says it is pinned —
      `PinnedConversationTests/pinnedSortsFirst`.
- [x] A row reads as its title and event count together —
      `ConversationListTests/labelsRows`.
- [x] Typing in the filter box narrows the list, and the clear button beside it
      — there whether or not anything has been typed — restores the whole list
      — `ConversationListTests/filtersAndClears`.
- [ ] **The search field lines up with the window buttons**, its middle level
      with theirs, and is about as tall as Bear's.
      **Eyes**: the field's text is an accessibility element and is centred on
      the buttons, but the rounded box around it is drawn and cannot be
      measured.
- [ ] **A pinned row shows the pin glyph in the accent colour**, to the left of
      the date.
      **Eyes**: the test above proves the row is labelled pinned and sorted
      first, not that anything is drawn.
- [ ] **A long title wraps to a second line and truncates there**, not after the
      first.
      **Eyes**, or `debug_app_pixels`: the accessibility label carries the whole
      title whatever is drawn, so the tree cannot see where it was cut, but a
      scan down a row shows one text band or two.
- [ ] **The selected row carries a thick accent bar down its leading edge**,
      inside the rounded fill rather than against the window's edge.
      **Eyes**, or a row scan across the selection.
- [ ] **A line appears above the first row when the list is scrolled**, and goes
      away at the top.
      **Eyes**.
- [ ] **The window buttons sit level with the search field**, about twenty
      points in from the window's left edge.
      Their frames are in the accessibility tree, so the centres can be compared
      against the field's without a screenshot; the visible circles inside them
      cannot, and want a scan.
- [ ] **Dragging the divider is smooth** on a workspace of a thousand
      conversations.
      **Eyes**: a synthesized drag needs the window frontmost, and the cost is
      in SwiftUI's own re-evaluation rather than in anything the app times.
- [ ] **A row's date reads the way the system locale writes one**: how long ago
      for anything active today, `31 Jul` inside this year, `13 May 2024` before
      that.
      **Eyes**: `ConversationDateTests` pins all three against a fixed locale,
      which is not the reader's.
- [ ] **The list is the palette's, not the system's**: white rows on a white
      sidebar in light appearance, `#1D1E20` in dark, separators inset to the
      text rather than running edge to edge, and a selected row filled `#F4F5F7`
      in an inset rounded rectangle.
      **Eyes**, in both appearances.
      `ThemeTests` proves each colour resolves per appearance; only a screenshot
      says the list is actually wearing them.
- [ ] **A selected row shows no trace of the system accent colour**, at the
      moment of the click or after it.
      **Eyes**: the table view's selection drawing is turned off through AppKit,
      and nothing outside the window can see what a row is filled with.
- [ ] **The line between the sidebar and the transcript is one point of one
      uniform colour**, the divider colour, top to bottom — two pixels on a
      retina display.
      **Eyes**: the app draws this line itself, so its geometry is checkable
      (the sidebar ends at 280 and the transcript starts at 281) but its colour
      is not.
- [ ] **The pointer becomes the horizontal-resize cursor over that line.**
      Currently it does not — see the Not implemented section of
      `AFFORDANCES.md`.
      `ResizeCursorAreaTests` shows the view *asks* for the right cursor over
      the right area, which is as far as a test reaches: a cursor is neither in
      the accessibility tree nor in a screenshot.
      The test passing while the pointer stays an arrow is the gap, and is why
      this line is unchecked.
- [x] **Dragging that line resizes the sidebar**, from either side of it —
      driven against `window.divider` with `debug_app_drive`'s `drag` step,
      starting on the right half, and the divider's frame moved by exactly the
      distance dragged.
      Approaching from the transcript side used to do nothing at all, because
      the pane is a later sibling and so in front of the grab strip.
- [ ] The sidebar stops at 220 and 480, and its width survives closing and
      reopening the window.
      **Not yet mechanized**; the drag above reaches it now.
- [ ] **The search field is a rounded box with a magnifier inside it** and no
      focus ring when it takes focus.
      **Eyes**.
- [ ] **The transcript sits on the editor background**, with prose in the body
      colour and speaker names in the secondary one.
      **Eyes**, in both appearances.
- [x] A single click selects that row, and the transcript follows —
      `ConversationListTests/clickSelects`.
- [x] A click in the empty space beside the title selects the row too —
      `ConversationListTests/clickBesideTitleSelects`.
- [x] ↑ and ↓ move the selection, and the transcript follows —
      `ConversationListTests/arrowKeysMoveSelection`.
- [x] A double-click opens the conversation in a new window, and that window
      shows the conversation rather than an empty pane —
      `ConversationListTests/doubleClickOpensAWindow`.
- [x] Right-click ▸ Open in New Window does the same —
      `ConversationListTests/contextMenuOpensAWindow`.
- [x] Right-click ▸ Copy Link puts `jp://<id>` on the pasteboard —
      `ConversationListTests/contextMenuCopiesTheURI`.
- [x] Escape clears the selection and empties the transcript pane —
      `ConversationListTests/editCopyLinkFollowsTheSelection`.
- [x] Edit ▸ Copy Link (⇧⌘C) puts the selected conversation's URI on the
      pasteboard, and is disabled while nothing is selected —
      `ConversationListTests/editCopyLinkFollowsTheSelection`.
- [x] Selecting a conversation leaves the View and Window menus intact — Enter
      Full Screen, Merge All Windows and the rest are items AppKit injects, and
      a menu bar rebuilt at the wrong moment drops them —
      `ConversationListTests/selectingKeepsTheInjectedMenuItems`.
- [ ] Dragging a row into a text editor inserts `jp://<id>`.
      **Eyes**: the drop target is another application, which is outside what a
      UI test can drive.
- [ ] **Scrolling the sidebar of a large workspace is smooth**, with no stutter
      as rows come into view.
      **Eyes**.

### Copy Link is checked without a clipboard being lost

No test touches the *system* pasteboard.
There is one of those and it belongs to whoever is at the keyboard: a test that
copies into it destroys what they had, and saving and restoring around the test
is not a fix, because a pasteboard item can be a promise its owner fulfils
lazily.

So a debug build copies wherever `JP_DEBUG_PASTEBOARD` says, and each test
points the app at a private pasteboard of its own and reads that back.
The variable is compiled out of a release build, and an unset one means the
system pasteboard, so the shipped behaviour is the only behaviour a user can
get.

`ClipboardPolicyTests` scans `apps/macos/UITests` and fails on any spelling of
the system pasteboard, so this holds without anyone remembering it.

## Transcript

- [x] Each message is shown under the name of whoever said it, and the whole
      transcript is one text view rather than one view per message —
      `ConversationListTests/clickSelects` compares the text view's whole value
      against `Transcripts.configPipeline`, so a missing speaker label or a
      duplicated message fails it.
- [x] Nothing but messages is shown: no tool calls, no reasoning, no turn
      markers, no dimmed kind labels — the library never sends them.
      `jp_ffi`'s `drops_every_event_with_no_prose_to_show` holds the boundary
      and `ConversationTurnDecodingTests` holds the mirror.
- [ ] Block markdown looks right: heading sizes, list markers hanging outside
      their text, wrapped list lines aligning under the first rather than under
      the bullet, code blocks on their own background, quotes indented and
      dimmed.
      **Eyes**: `MarkdownTests` pins every one of these as attributes, and none
      of that says the result is legible.
- [ ] Paragraph spacing reads as paragraphs, and the gap between two turns is
      clearly wider than the gap between two messages inside one.
      **Eyes**: the numbers are pinned, the impression is not.
- [ ] A table reads as a table: columns line up down the rows, the header is
      bold, and a column declared right-aligned has its numbers ending together.
      **Eyes**: `MarkdownTests` pins the tab stops and their alignments, and
      none of that says the columns look aligned.
      A cell longer than the fixed column width will run into the next column —
      known, see `AFFORDANCES.md`.
- [ ] Text can be selected across message boundaries and copied with ⌘C.
      **Not yet mechanized**; selection across the whole document is the point
      of one text view, so a selection that stops at a message is a defect.
- [ ] **Scrolling a long conversation is smooth, and the scroll bar keeps a
      constant size** rather than resizing or jumping as you scroll.
      **Eyes**.
      Contiguous TextKit 1 layout is what makes the height exact, so a shifting
      knob here means something changed about that — see `AFFORDANCES.md`.
- [x] **Dragging a window edge re-wraps the text as it moves, at any scroll
      position** rather than waiting for the mouse to come up —
      `TranscriptReflowTests/reflowsWhileDragging`, which drags a real window
      edge and asserts the text container's width changed during the gesture.
      Verified red by disabling the container write.
- [ ] **Resizing stays smooth on a long conversation.** Measure with
      `debug_app_drive` using `reads: "none"` and a profile bracket, and compare
      counts against another recording; a run with tree reads on measures the
      reads instead.
      A resize evaluates no SwiftUI view bodies, so a climb here is layout or
      text measurement, not re-rendering.

## Performance

Most of this is **eyes**: how the app feels under a load the fixture workspace
does not have, and no threshold in milliseconds separates a good build from a
bad one across machines.

What is checkable is the work the app does rather than the time it takes.
`debug_app_drive` records when each step ran and `debug_app_profile` with `mode:
"report"` attributes the app's own intervals to those steps, so "does the third
selection cost more than the first" has an answer that does not depend on the
machine.

- [ ] Selecting a conversation renders it **without a visible spinner** for a
      conversation of a hundred events or so.
      **Eyes**.
- [ ] A conversation of a couple of thousand events opens without a stall.
      **Eyes**.
- [ ] Selecting several conversations in a row stays responsive; the second and
      third selections are not slower than the first.
      **`debug_app_profile`**: drive five selections, then `mode: "report"`.
      The `View bodies` and `FFI calls` columns should stay flat down the table.
      A column that climbs is re-evaluation rather than loading, and the report
      says so.
- [ ] Revisiting conversations does not cost memory a second time.
      **`debug_app_profile`**: drive the same two conversations alternately six
      times and read the `Footprint` column.
      It should plateau, because the climb on first visit is the allocator's
      high-water mark rather than retention.
      A column that keeps climbing while the same two conversations are
      re-selected is a leak.
- [ ] **Switching conversations does not flash an empty pane**: the previous
      transcript stays until the next one replaces it.
      **Eyes**.

## Windows and tabs

All **not yet mechanized**.
`XCUIApplication.windows` counts and titles reach most of this.

- [ ] ⌘N opens a window on the workspace you were last reading.
- [ ] ⌘W closes the frontmost window or tab.
- [ ] Closing the last window and pressing ⌘N puts you back in the same
      workspace.
- [ ] ⌘T opens a new tab.
- [ ] Window ▸ Merge All Windows collects windows into tabs.
- [ ] A tab can be dragged out into its own window.
      **Eyes**: a tear-off drag has no accessibility action behind it.
- [ ] Opening the same workspace twice — once via ⌘O, once via Open Recent —
      brings the existing window forward rather than opening a second one.

## Open and Open Recent

All **not yet mechanized**.
The recents list is already isolated per test by `JP_DEBUG_STATE_DIR`, so even
Clear Menu is safe to drive.

- [ ] ⌘O offers a directory chooser that only allows directories.
- [ ] Choosing a directory *inside* a workspace opens that workspace.
- [ ] Choosing a directory that is not in any workspace shows a readable message
      rather than an empty list.
- [ ] The chosen workspace appears at the top of File ▸ Open Recent.
- [ ] A workspace whose directory has been deleted disappears from the menu on
      the next launch.
- [ ] Open Recent ▸ Clear empties the menu and disables it.

## State restoration

Quit with ⌘Q and relaunch for each of these.

All **not yet mechanized**.
`XCUIApplication.terminate()` and `.launch()` are the natural fit, but every UI
test today launches with `-ApplePersistenceIgnoreState` so it neither restores
nor saves window state; these need that turned off, and with it a way to keep a
run out of the developer's own saved state — the same problem the pasteboard
has, and it may well have the same answer.

- [ ] The window reopens on the same workspace.
- [ ] The conversation that was selected is selected again.
- [ ] The transcript is scrolled roughly where it was left.
- [ ] The sidebar keeps the width it was dragged to.
      **Eyes**: see the Not implemented section of `AFFORDANCES.md`.
- [ ] A conversation window opened on its own reopens showing its conversation.

## Accessibility

With VoiceOver on (⌘F5).
All **eyes**: what VoiceOver announces is not what the accessibility tree holds,
and only a person hears the difference.

- [ ] The sidebar announces itself as `Conversations`.
- [ ] Each row is read as one item, with its title and event count together.
- [ ] Messages in the transcript are read as text.

Against the identifier table in `AFFORDANCES.md`:

- [x] A row's identifier is the conversation ID, and does not change when the
      conversation is retitled — every `ConversationListTests` case addresses
      rows by ID, and `AccessibilityIDTests` pins the shape.
- [x] The transcript publishes one text area named `transcript.text`, whose
      value is every message it is showing —
      `ConversationListTests/clickSelects` reads that value and compares it
      whole.
      There is deliberately no element per message; see the Accessibility
      section of `AFFORDANCES.md`.
- [x] `sidebar.filter` and `sidebar.filter.clear` are both reachable while the
      filter is narrowing the list — `ConversationListTests/filtersAndClears`.
- [ ] Every other identifier in the table is reachable while its state is on
      screen.
      **Not yet mechanized**: the three empty states have no test driving them
      into view.

## Known gaps

Not defects; see the Not implemented section of `AFFORDANCES.md`.

- Dragging into Finder produces nothing useful.
- Nothing accepts a dropped conversation.
- New turns written by a concurrent `jp query` do not appear until the workspace
  is reopened.

[`AFFORDANCES.md`]: AFFORDANCES.md
