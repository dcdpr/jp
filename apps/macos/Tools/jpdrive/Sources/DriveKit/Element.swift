import ApplicationServices

extension Optional where Wrapped == String {
    /// The value read as a boolean.
    ///
    /// The accessibility API renders `AXEnabled`, `AXMain` and their like as `"0"`
    /// or `"1"`. Anything else, including an absent attribute, is neither true nor
    /// false.
    var axFlag: Bool? {
        switch self {
        case "0": return false
        case "1": return true
        default: return nil
        }
    }
}

/// Posts synthesized input to the window server.
///
/// Behind a protocol because posting is the one thing the driver does that is not
/// addressed to an element. A click goes to whatever occupies a screen
/// coordinate, which is global state and cannot be exercised against a fake tree
/// the way every other step can.
protocol EventPoster {
    /// Click once at `point`, in screen coordinates.
    ///
    /// Answers whether the events could be built and posted, which is not whether
    /// anything received them.
    func click(at point: CGPoint) -> Bool

    /// Press at the first point of `path`, move through the rest, release at the
    /// last.
    ///
    /// `pause` separates one move from the next. Without it the moves are posted
    /// faster than the target can consume them and the window server delivers a
    /// coalesced few, which is the opposite of what a drag is usually being
    /// synthesized to exercise: what a view does *during* the gesture, frame by
    /// frame.
    ///
    /// Answers whether every event could be built and posted.
    func drag(through path: [CGPoint], pausing pause: Duration) -> Bool
}

/// Posts through `CoreGraphics`.
struct SystemEventPoster: EventPoster {
    func click(at point: CGPoint) -> Bool {
        guard
            let down = event(.leftMouseDown, at: point),
            let up = event(.leftMouseUp, at: point)
        else {
            return false
        }

        down.post(tap: .cghidEventTap)
        up.post(tap: .cghidEventTap)
        return true
    }

    func drag(through path: [CGPoint], pausing pause: Duration) -> Bool {
        guard let first = path.first, let last = path.last else { return false }
        guard let down = event(.leftMouseDown, at: first) else { return false }

        down.post(tap: .cghidEventTap)

        for point in path.dropFirst() {
            guard let moved = event(.leftMouseDragged, at: point) else {
                // Released wherever it got to rather than returned from. A drag
                // abandoned with the button still down leaves the whole machine
                // holding a mouse button nobody is pressing, which outlives this
                // process and is not something a failed test should do to the
                // person running it.
                release(at: point)
                return false
            }

            moved.post(tap: .cghidEventTap)
            Thread.sleep(forTimeInterval: pause.seconds)
        }

        guard let up = event(.leftMouseUp, at: last) else {
            release(at: last)
            return false
        }

        up.post(tap: .cghidEventTap)
        return true
    }

    /// Let the button go, on a path that could not be finished.
    private func release(at point: CGPoint) {
        event(.leftMouseUp, at: point)?.post(tap: .cghidEventTap)
    }

    private func event(_ type: CGEventType, at point: CGPoint) -> CGEvent? {
        CGEvent(
            mouseEventSource: nil,
            mouseType: type,
            mouseCursorPosition: point,
            mouseButton: .left
        )
    }
}

/// One attribute, as reported name and rendered value.
///
/// A list of these rather than a dictionary, so attribute names reach the JSON
/// exactly as the accessibility API spells them. `JSONEncoder`'s snake-case key
/// strategy rewrites dictionary keys, which would turn `AXIdentifier` into
/// `ax_identifier` and make a dump a poor record of what the app reports.
struct Attribute: Encodable, Equatable {
    let name: String
    let value: String

    /// Whether the accessibility API reports this attribute as writable, when
    /// settability was asked for.
    ///
    /// This decides how the driver changes state. Writing `AXSelected` on a row is
    /// deterministic; synthesizing a click at a screen coordinate depends on the
    /// window being frontmost and unobscured.
    ///
    /// Absent unless requested: answering it costs one round-trip per attribute,
    /// which doubles the cost of a walk.
    let settable: Bool?
}

/// Attribute text and children, read together.
///
/// The pair exists because reading them separately costs an extra round-trip per
/// element, and walking to a child is the most common read the driver makes.
struct Reading<E> {
    /// One entry per requested name, positionally, `nil` where the element has no
    /// value for that attribute.
    let text: [String?]

    let children: [E]

    /// Whether the call that produced this failed as a whole.
    ///
    /// Distinct from a `nil` in ``text``, which says the element reports no value
    /// for that one attribute. This says the accessibility API refused the read,
    /// so every entry is `nil` and ``children`` is empty because nothing could be
    /// asked — not because the element has none. A target that is busy or exiting
    /// answers that way, and the two are indistinguishable without this.
    let failed: Bool

    init(text: [String?], children: [E], failed: Bool = false) {
        self.text = text
        self.children = children
        self.failed = failed
    }
}

/// One element of an accessibility tree, as the driver's traversal needs it.
///
/// The traversal is where the driver's logic lives: pruning a filtered walk,
/// spending a match budget, finding which ancestor of an identified element owns
/// selection. None of that is about the accessibility API, and all of it has been
/// wrong at least once. Behind this protocol it can be tested against a fake tree
/// instead of against a running application.
///
/// Deliberately narrow. Everything here is something a walk actually does, so a
/// fake stays small enough to read at a glance and cannot drift far from the real
/// implementation.
protocol Element {
    /// Read the named attributes and the element's children.
    ///
    /// Implementations batch: this is one round-trip in the real one.
    func read(_ names: [String]) -> Reading<Self>

    /// Every attribute the element reports, rendered and sorted by name.
    ///
    /// Separate from ``read(_:)``, which asks for a list of names known in
    /// advance. A dump asks for whatever the element happens to have, which costs
    /// an extra round-trip to enumerate and is why only a dump does it.
    ///
    /// `nil` when the read failed as a whole, which is not the same as an element
    /// that reports no attributes.
    func reportedAttributes(settable: Bool) -> [Attribute]?

    /// Actions the element accepts, such as `AXPress`.
    ///
    /// Separate from ``read(_:)`` because it costs its own round-trip and most
    /// elements a filtered walk passes through are discarded unread.
    var actions: [String] { get }

    /// Whether `name` can be written on this element.
    func isSettable(_ name: String) -> Bool

    /// Read a boolean attribute, `nil` when it is absent or not a boolean.
    func flag(_ name: String) -> Bool?

    /// Write a boolean attribute, answering the accessibility API's own status.
    ///
    /// A successful write is not a successful change: the target can accept the
    /// value and do nothing with it. Read it back to find out.
    func setFlag(_ name: String, _ value: Bool) -> AXError

    /// Write a string attribute, answering the accessibility API's own status.
    func setText(_ name: String, _ value: String) -> AXError

    /// Perform an action, answering the accessibility API's own status.
    ///
    /// What the action did is not observable from here. Pressing a button runs
    /// arbitrary code in the target, and success means the press was delivered,
    /// not that anything came of it.
    func perform(_ action: String) -> AXError

    /// The point held in an attribute, in screen coordinates.
    ///
    /// `nil` when the attribute is absent or holds something else. Separate from
    /// ``read(_:)`` because a caller aiming a click needs the numbers, not the
    /// text they render as.
    func point(_ name: String) -> CGPoint?

    /// The size held in an attribute, in points.
    ///
    /// `nil` when the attribute is absent or holds something else.
    func size(_ name: String) -> CGSize?

    /// Write a size attribute, answering the accessibility API's own status.
    ///
    /// As with every other write here, success is not change: a window clamps a
    /// size to its own minimum and maximum, so what it ends up at has to be read
    /// back.
    func setSize(_ name: String, _ value: CGSize) -> AXError

    /// The elements held in an attribute, such as `AXWindows` or `AXMenuBar`.
    ///
    /// Answers a single element as a one-element array, since the accessibility
    /// API spells "the menu bar" and "the windows" the same way apart from the
    /// plural.
    func elements(_ name: String) -> [Self]
}
