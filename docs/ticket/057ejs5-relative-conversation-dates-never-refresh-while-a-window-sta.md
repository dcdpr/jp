# Relative conversation dates never refresh while a window stays open

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-08-20

A row in the macOS app's conversation list dates itself against
`WorkspaceWindow.listingReadAt`, which is set once when the workspace is read
and never again.
A row that said `21 minutes ago` when the window opened still says it hours
later, and a conversation from yesterday keeps a minutes-and-hours label after
midnight instead of switching to `2 Aug`, because `ConversationDate.label` picks
its branch from a `now` that stopped moving.

The current behaviour is deliberate and the code says so:

```swift
/// The cost is that "21 minutes ago" is 21 minutes after the workspace was
/// opened, not after now.
@State private var listingReadAt = Date()
```

Taking a fresh `Date()` per render is not the fix.
`ConversationList` is `Equatable` and compares `now`, and a render happens on
every frame of a divider drag — a clock that changes each time makes the list
unequal to itself and undoes the skipping that keeps the drag smooth.

A timer ticking once a minute and writing `listingReadAt` keeps that trade
intact: the value changes once a minute rather than once a frame, so the
equality comparison still skips the list on every drag frame.
The row labels are only ever accurate to the minute anyway, so nothing finer is
needed.

Found while triaging review feedback on the macOS app PR (dcdpr/jp\#1008,
comment 3815396254).
