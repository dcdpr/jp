import ApplicationServices
import Foundation

/// One thing to do to one element.
///
/// Each case names its own mechanism. There is no step that picks a mechanism
/// based on what the element supports: a script that says `select` against
/// something unselectable fails and says so, which is how a change in the app's
/// accessibility becomes visible instead of being absorbed by a fallback.
///
/// Decoded from a single-key object, so a step reads as what it does:
///
/// ```json
/// {"select": {"identifier": "sidebar.row.17855681129"}}
/// ```
enum Step: Decodable {
    /// Write `AXSelected` on the nearest ancestor that accepts it.
    ///
    /// The mechanism for list and outline rows, where the identified element is
    /// below the one that owns selection. Independent of scroll position, so it
    /// reaches a row that is not on screen.
    case select(Target)

    /// Perform `AXPress` on the identified element itself.
    ///
    /// The mechanism for buttons and menu items, which advertise the action.
    case press(Target)

    /// Synthesize a mouse click at the element's activation point.
    ///
    /// The last resort, and the only step that depends on the world outside the
    /// accessibility tree: the window has to be frontmost and the element on
    /// screen, or the click lands somewhere else entirely.
    case click(Target)

    /// Perform a named accessibility action on the identified element.
    ///
    /// The long tail. `press` is this with `AXPress` and a better error message,
    /// and is worth keeping because it is the overwhelmingly common case; anything
    /// else an element advertises — `AXConfirm`, `AXShowMenu`, `AXScrollToVisible`,
    /// `AXCancel` — is reached through here rather than by growing a step per verb.
    case perform(ActionTarget)

    /// Put text into a text field.
    case type(TypeTarget)

    /// Set an element's size, which for a window resizes it.
    ///
    /// The one step that changes the shape of what is on screen rather than what
    /// is in it, and the only way to observe what a resize costs: a drag of a
    /// window's edge cannot be synthesized against a background application, and
    /// resizing is where a view that re-measures its contents shows up.
    case resize(SizeTarget)

    /// Drag the pointer across an element, with the button held.
    ///
    /// The gesture no other step can stand in for. `resize` sets a window's size
    /// in one write, which is not a drag: nothing enters live resize, and a view
    /// that behaves differently *during* a gesture than after it looks correct to
    /// every other step here.
    ///
    /// Not only for window edges. Any two points on any element — a split
    /// divider's handle, a stretch of text to select, a row to drag out — is the
    /// same gesture with different endpoints.
    ///
    /// Depends on the world outside the tree in the same way `click` does: the
    /// events go to whatever occupies those coordinates, so the window is raised
    /// first and has to be on screen.
    case drag(DragTarget)

    /// What to drag across, and along what path.
    struct DragTarget: Decodable {
        /// The element's `AXIdentifier`, matched exactly.
        ///
        /// Names the coordinate space, not necessarily the thing that reacts. A
        /// window's own frame is how its resize corner is addressed, and the
        /// window is what reacts.
        let identifier: String

        /// Where the button goes down, as a fraction of the element's frame.
        let from: Offset

        /// Where it comes up.
        let to: Offset

        /// How many moves to post between the two, not counting the press.
        ///
        /// Defaults to 24. The number is the point of the step: a drag posted as
        /// one jump exercises a single frame, and the behaviour usually under
        /// question is what happens across many.
        let steps: Int?

        /// How long to pause between moves, in milliseconds. Defaults to 8.
        let pauseMs: Int?

        private enum CodingKeys: String, CodingKey {
            case identifier
            case from
            case to
            case steps
            case pauseMs = "pause_ms"
        }
    }

    /// A point on an element, as a fraction of its frame.
    ///
    /// Fractions rather than points, so a script says "the right edge, halfway
    /// down" and keeps meaning it after the window is resized.
    ///
    /// `1.0` is the far edge exactly, and is what a window resize wants. The
    /// region that resizes a window is a few points wide and straddles the frame
    /// boundary, so aiming even five points inside lands in the content instead:
    /// the gesture runs, the pointer moves, and whatever is under it gets dragged
    /// rather than the window resized. Measured on a running window — `0.995` of
    /// a 1070-point window grabs text, `1.0` grabs the edge.
    struct Offset: Decodable {
        let dx: Double
        let dy: Double
    }

    /// What to resize, and to what.
    struct SizeTarget: Decodable {
        /// The element's `AXIdentifier`, matched exactly.
        ///
        /// A window carries one, so it is addressed the same way as anything else
        /// rather than through a step that means "the frontmost window".
        let identifier: String

        /// The width to ask for, in points.
        let width: Double

        /// The height to ask for, in points.
        let height: Double
    }

    /// What action to perform, and on what.
    struct ActionTarget: Decodable {
        /// The element's `AXIdentifier`, matched exactly.
        let identifier: String

        /// The action's own name, spelled as the accessibility API spells it.
        ///
        /// Not translated from a friendlier vocabulary: a step that says
        /// `AXConfirm` can be checked against what `jpdrive dump` reported for the
        /// element, and a friendlier name could not.
        let action: String
    }

    /// Press a menu item, addressed by the titles leading to it.
    case menu(MenuTarget)

    /// What to type, and where.
    struct TypeTarget: Decodable {
        /// The field's `AXIdentifier`, matched exactly.
        let identifier: String

        /// The text to put in the field, replacing what is there.
        ///
        /// Written as a value and then confirmed, rather than typed a character at
        /// a time. Two calls, neither of which can be derailed by focus moving to
        /// another application halfway through, which a synthesized keystroke can.
        ///
        /// The confirm is not optional dressing. Writing `AXValue` on a `SwiftUI`
        /// text field changes the text the field displays without the binding
        /// behind it noticing, so the application carries on as though nothing was
        /// typed. Confirming commits the edit through the path the binding does
        /// observe.
        ///
        /// The cost is that per-character behaviour never runs. A field that
        /// validates each keystroke, or completes as you type, sees one change
        /// rather than a dozen. Assert the consequence — the list that narrowed,
        /// the button that enabled — rather than assuming the field's own handlers
        /// fired for every character.
        let text: String
    }

    /// Wait until an element with the given identifier exists.
    case waitFor(WaitTarget)

    /// A path through a menu.
    struct MenuTarget: Decodable {
        /// Titles from the top of the menu downwards, such as `["File", "Close"]`.
        ///
        /// Titles rather than identifiers, because the structure is the thing worth
        /// asserting. An identifier like `closeAll:` is an `AppKit` selector name:
        /// it survives the item moving to a different menu, so a script keyed on it
        /// cannot notice the menu bar being rearranged. A path cannot miss that.
        ///
        /// A path that does not resolve reports how far it got and what that level
        /// holds, which is the assertion failure a layout test wants to read.
        let path: [String]

        /// The element whose shown menu the path starts from.
        ///
        /// Absent, the path starts at the menu bar. Present, it starts at the menu
        /// that element is currently displaying, which `AXShowMenu` puts up.
        ///
        /// A title is the only way to name a context menu item: `SwiftUI` does not
        /// carry an accessibility identifier onto the `NSMenuItem` it bridges a
        /// menu button to, so every item in one reports the same selector name.
        let under: String?

        /// Spelled out so `under` can be left off, both here and on the wire.
        init(path: [String], under: String? = nil) {
            self.path = path
            self.under = under
        }
    }

    /// What a wait addresses, and for how long.
    struct WaitTarget: Decodable {
        /// The `AXIdentifier` to wait for, matched exactly.
        let identifier: String

        /// Identifier of a container to search inside, resolved once before
        /// polling begins.
        ///
        /// Strongly worth setting. A search for something absent has no early exit
        /// and reads every element in the application, which against a thousand-row
        /// sidebar takes longer than a typical timeout allows for a single attempt.
        /// Scoping to the container the element will appear in makes each poll
        /// cheap.
        ///
        /// A container that does not exist fails immediately, rather than being
        /// waited for.
        let under: String?

        /// How long to keep trying. Defaults to 5000.
        let timeoutMs: Int?

        /// How long to pause between attempts. Defaults to 100.
        ///
        /// Not the kind of sleep the driver avoids. Waiting a fixed duration and
        /// assuming the work finished is a guess; pausing between two observations
        /// of a condition is how polling stays off a busy loop that would flood the
        /// target with accessibility traffic.
        let intervalMs: Int?

        /// Spelled out, because the decoder converts no cases of its own: without
        /// these two, a step naming `timeout_ms` decodes as though it had named
        /// nothing and silently waits the default.
        private enum CodingKeys: String, CodingKey {
            case identifier
            case under
            case timeoutMs = "timeout_ms"
            case intervalMs = "interval_ms"
        }
    }

    /// What a step addresses.
    struct Target: Decodable {
        /// The element's `AXIdentifier`, matched exactly.
        ///
        /// Exact rather than by prefix: `sidebar.row.1785` is a prefix of many
        /// rows, and acting on whichever one happened to be found first is not a
        /// thing a script can mean.
        let identifier: String
    }

    private enum CodingKeys: String, CodingKey {
        case select
        case press
        case click
        case perform
        case type
        case menu
        case waitFor = "wait_for"
        case resize
        case drag
    }

    init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        if let target = try container.decodeIfPresent(Target.self, forKey: .select) {
            self = .select(target)
            return
        }
        if let target = try container.decodeIfPresent(Target.self, forKey: .press) {
            self = .press(target)
            return
        }
        if let target = try container.decodeIfPresent(Target.self, forKey: .click) {
            self = .click(target)
            return
        }
        if let target = try container.decodeIfPresent(ActionTarget.self, forKey: .perform) {
            self = .perform(target)
            return
        }
        if let target = try container.decodeIfPresent(TypeTarget.self, forKey: .type) {
            self = .type(target)
            return
        }
        if let target = try container.decodeIfPresent(MenuTarget.self, forKey: .menu) {
            self = .menu(target)
            return
        }
        if let target = try container.decodeIfPresent(WaitTarget.self, forKey: .waitFor) {
            self = .waitFor(target)
            return
        }
        if let target = try container.decodeIfPresent(SizeTarget.self, forKey: .resize) {
            self = .resize(target)
            return
        }
        if let target = try container.decodeIfPresent(DragTarget.self, forKey: .drag) {
            self = .drag(target)
            return
        }

        throw DecodingError.dataCorrupted(
            .init(
                codingPath: container.codingPath,
                debugDescription:
                    "expected one of select, press, click, perform, type, menu, wait_for, "
                    + "resize, drag"
            )
        )
    }
}

/// What a step did.
struct StepResult: Encodable, Equatable {
    /// The step that ran, named as it was written.
    let step: String

    let identifier: String

    /// The role of the element the step acted on.
    ///
    /// Not always the identified element: `select` climbs to the ancestor that
    /// owns selection, and reporting the role it reached is how a restructuring of
    /// the view surfaces as a changed role rather than as a puzzling failure.
    let role: String

    /// Whether the intended change was observed after the step ran.
    ///
    /// A write can succeed and change nothing, so this is read back from the
    /// element rather than inferred from the API's status.
    ///
    /// Absent for a step with nothing to read back. Pressing a button runs
    /// arbitrary code in the target and has no attribute that says it worked, so
    /// reporting `true` there would be claiming more than was checked.
    let confirmed: Bool?

    /// Where a click was aimed, in screen coordinates.
    ///
    /// Only `click` reports this. A click is the one step whose outcome depends on
    /// a number the caller cannot otherwise see, and "it clicked the wrong thing"
    /// is unanswerable without knowing where it clicked.
    let point: String?

    /// Whether an edit was committed through the element's confirm action.
    ///
    /// Only `type` reports this. `false` means the field took the text but
    /// advertises no `AXConfirm`, so whether the application noticed depends on it
    /// watching the value directly — worth knowing, because the text being in the
    /// field and the application having seen it are different facts.
    let committed: Bool?

    /// The size the element ended at, as `WIDTHxHEIGHT` in points.
    ///
    /// Only `resize` reports this. A window clamps a size to its own limits, so
    /// what was asked for and what happened are different facts and the second is
    /// the one worth reading.
    let size: String?

    /// How many moves a drag posted between pressing and releasing.
    ///
    /// Only `drag` reports this. It is what separates a gesture from a jump, and
    /// a caller asking why a view did not react during one wants to know how many
    /// chances it had.
    let moves: Int?

    init(
        step: String,
        identifier: String,
        role: String,
        confirmed: Bool? = nil,
        committed: Bool? = nil,
        point: String? = nil,
        size: String? = nil,
        moves: Int? = nil
    ) {
        self.step = step
        self.identifier = identifier
        self.role = role
        self.confirmed = confirmed
        self.committed = committed
        self.point = point
        self.size = size
        self.moves = moves
    }
}

/// Runs a single step against a running application.
enum Act {
    /// How far `select` looks above the identified element for one that accepts
    /// selection.
    ///
    /// The known chain is two levels, from the identified element through the cell
    /// to the row. The cap is above that so an extra wrapper does not break the
    /// step, and low enough that a miss fails rather than selecting the window.
    private static let maxAncestors = 4

    /// Resolve the step's target and act on it.
    static func run(_ step: Step, pid: pid_t) throws(DriveError) -> StepResult {
        guard ProcessTable.record(for: pid) != nil else {
            throw DriveError(
                kind: .appNotRunning,
                message: "no process is running under pid \(pid)",
                hint: "start the app, then pass its pid: --pid $(pgrep -f JP.app)"
            )
        }

        guard AXIsProcessTrusted() else {
            throw DriveError(
                kind: .notPermitted,
                message: "not trusted to read another application's accessibility tree",
                hint: DriveError.accessibilityHint
            )
        }

        return try run(step, in: AXElement.application(pid: pid), poster: SystemEventPoster())
    }

    /// Run a step against an already-resolved root.
    ///
    /// Split from ``run(_:pid:)`` so the part with the logic in it can be exercised
    /// against a tree that is not a running application. `poster` is separate for
    /// the same reason: a click is aimed using the tree but delivered outside it.
    /// `activation` is how long a menu step waits for the application to come
    /// forward and for the item to enable, and is a parameter so a test of either
    /// wait does not have to sit through the real one.
    static func run<E: Element>(
        _ step: Step,
        in root: E,
        poster: any EventPoster = SystemEventPoster(),
        activation: Duration = activationTimeout
    ) throws(DriveError) -> StepResult {
        switch step {
        case .select(let target):
            return try select(target, in: root)

        case .waitFor(let target):
            return try waitFor(target, in: root)

        case .press(let target):
            return try press(target, in: root)

        case .perform(let target):
            return try perform(target.action, on: target.identifier, in: root, step: "perform")

        case .type(let target):
            return try type(target, in: root)

        case .menu(let target):
            return try menu(target, in: root, within: activation)

        case .click(let target):
            return try click(target, in: root, poster: poster, activation: activation)

        case .resize(let target):
            return try resize(target, in: root)

        case .drag(let target):
            return try drag(target, in: root, poster: poster, activation: activation)
        }
    }

    /// How many moves a drag posts when it does not say.
    private static let defaultDragSteps = 24

    /// How long a drag pauses between moves when it does not say.
    private static let defaultDragPause = Duration.milliseconds(8)

    /// The most moves a drag will post.
    ///
    /// Clamped rather than rejected, for the same reason the count is clamped
    /// upwards from zero: a caller computing it from a distance should get a
    /// gesture, not an error. The cap is what keeps an implausible count from
    /// becoming a run that posts for minutes, or one whose route cannot be
    /// allocated at all — either of which ends the process without the JSON
    /// document every call promises.
    private static let maxDragSteps = 1000

    /// Drag the pointer from one point on an element to another.
    private static func drag<E: Element>(
        _ target: Step.DragTarget,
        in root: E,
        poster: any EventPoster,
        activation: Duration
    ) throws(DriveError) -> StepResult {
        let path = try find(target.identifier, from: root)
        guard let element = path.last else {
            throw DriveError(
                kind: .identifierNotFound,
                message: "no element has the identifier \(target.identifier)",
                hint: nil
            )
        }

        guard
            let origin = element.point(kAXPositionAttribute),
            let size = element.size(kAXSizeAttribute)
        else {
            throw DriveError(
                kind: .notClickable,
                message: "\(target.identifier) reports no frame to drag across",
                hint: "an element with no position or size cannot be aimed at"
            )
        }

        try check(target.from, named: "from")
        try check(target.to, named: "to")

        let steps = min(max(target.steps ?? defaultDragSteps, 1), maxDragSteps)
        let pause = target.pauseMs.map { Duration.milliseconds($0) } ?? defaultDragPause

        let start = point(target.from, in: origin, size)
        let end = point(target.to, in: origin, size)
        let route = (0...steps).map { step in
            let progress = Double(step) / Double(steps)
            return CGPoint(
                x: start.x + (end.x - start.x) * progress,
                y: start.y + (end.y - start.y) * progress
            )
        }

        // Activated, and then raised, and both are needed.
        //
        // `AXRaise` orders a window forward *within its own application*. Global
        // ordering between applications follows activation, so raising a
        // background app's window leaves it under the active app's windows: the
        // gesture lands on whatever is on top at those coordinates, which is
        // whatever the person at the keyboard is using. Measured, not assumed — a
        // drag posted without this was received by the frontmost terminal.
        //
        // The cost is that a gesture takes focus. Nothing here can give it back:
        // this process handles one step and exits, so the restore belongs to
        // whatever drives the whole list.
        try front(root, within: activation)
        raiseWindow(in: path)

        guard poster.drag(through: route, pausing: pause) else {
            throw DriveError(
                kind: .actionFailed,
                message:
                    "could not post a drag from \(start.x),\(start.y) to \(end.x),\(end.y)",
                hint: nil
            )
        }

        return StepResult(
            step: "drag",
            identifier: target.identifier,
            role: element.read([kAXRoleAttribute]).text[0] ?? "<none>",
            point: "\(start.x),\(start.y) -> \(end.x),\(end.y)",
            moves: route.count - 1
        )
    }

    /// Check that an offset names a point on the element.
    ///
    /// A value outside `0...1` resolves to a screen coordinate outside the
    /// element's frame, and the gesture is posted into global screen space, so
    /// the press or release lands on whatever occupies that point instead —
    /// another application's window, or the desktop. Reading `dx` as a
    /// percentage rather than a fraction is the mistake this catches, and it
    /// resolves a long way outside: `100` on an 800pt window aims 80,000 points
    /// to the right of it.
    private static func check(_ offset: Step.Offset, named name: String) throws(DriveError) {
        for (axis, value) in [("dx", offset.dx), ("dy", offset.dy)]
        where !value.isFinite || value < 0 || value > 1 {
            throw DriveError(
                kind: .badUsage,
                message: "\(name).\(axis) is \(value), which is not a fraction of the frame",
                hint: "0 is the near edge of the element and 1 the far one, so 0.5 is halfway"
            )
        }
    }

    /// One fractional offset as a screen coordinate inside a frame.
    private static func point(
        _ offset: Step.Offset, in origin: CGPoint, _ size: CGSize
    )
        -> CGPoint
    {
        CGPoint(
            x: origin.x + size.width * offset.dx,
            y: origin.y + size.height * offset.dy
        )
    }

    /// Ask the identified element to take a new size.
    ///
    /// The size it ends at is read back and reported rather than assumed: a window
    /// clamps to its own minimum and maximum, so asking for something outside those
    /// succeeds and lands somewhere else. `confirmed` says whether it landed on
    /// what was asked for.
    private static func resize<E: Element>(
        _ target: Step.SizeTarget, in root: E
    ) throws(DriveError) -> StepResult {
        let path = try find(target.identifier, from: root)
        guard let element = path.last else {
            throw DriveError(
                kind: .identifierNotFound,
                message: "no element has the identifier \(target.identifier)",
                hint: "list what is addressable with: jpdrive tree --identifier <prefix>"
            )
        }

        let role = element.read([kAXRoleAttribute]).text[0] ?? ""

        guard element.isSettable(kAXSizeAttribute) else {
            throw DriveError(
                kind: .notEditable,
                message: "\(target.identifier) does not accept a write to AXSize",
                hint: "a window does; most elements inside one do not"
            )
        }

        let wanted = CGSize(width: target.width, height: target.height)
        let status = element.setSize(kAXSizeAttribute, wanted)
        guard status == .success else {
            throw DriveError(
                kind: .writeFailed,
                message: "writing AXSize on \(target.identifier) failed: \(status.name)",
                hint: nil
            )
        }

        let reached = element.size(kAXSizeAttribute)

        return StepResult(
            step: "resize",
            identifier: target.identifier,
            role: role,
            confirmed: reached == wanted,
            size: reached.map { "\(Int($0.width))x\(Int($0.height))" }
        )
    }

    /// Click where the identified element says a click belongs.
    ///
    /// The last resort among the steps, and the only one whose effect is not
    /// addressed to the element: the event goes to whatever occupies that screen
    /// coordinate. Prefer `select` for rows and `press` for controls, both of which
    /// reach their target regardless of what is on top of it or whether it is
    /// scrolled into view.
    private static func click<E: Element>(
        _ target: Step.Target,
        in root: E,
        poster: any EventPoster,
        activation: Duration
    ) throws(DriveError) -> StepResult {
        let path = try find(target.identifier, from: root)
        guard let element = path.last else {
            throw DriveError(
                kind: .identifierNotFound,
                message: "no element has the identifier \(target.identifier)",
                hint: nil
            )
        }

        guard let point = element.point(AXElement.activationPoint) else {
            throw DriveError(
                kind: .notClickable,
                message: "\(target.identifier) reports no AXActivationPoint",
                hint:
                    "an element with no place to be clicked is usually one that wants `select` "
                    + "or `press` instead"
            )
        }

        // Brought forward, and then raised, and both are needed — the same pair
        // `drag` performs, for the same reason. `AXRaise` orders a window within
        // its own application; ordering *between* applications follows
        // activation. A driver invoked from a terminal leaves that terminal
        // frontmost, so raising alone posts the click into the terminal while
        // reporting the element it aimed at.
        try front(root, within: activation)
        raiseWindow(in: path)

        guard poster.click(at: point) else {
            throw DriveError(
                kind: .actionFailed,
                message: "could not post a click at \(point.x),\(point.y)",
                hint: nil
            )
        }

        return StepResult(
            step: "click",
            identifier: target.identifier,
            role: element.read([kAXRoleAttribute]).text[0] ?? "<none>",
            point: "\(point.x),\(point.y)"
        )
    }

    /// Bring the window holding the addressed element to the front, if it has one.
    ///
    /// Found along the path the search descended, for the same reason the selection
    /// owner is: the identified element does not report a parent to climb from.
    private static func raiseWindow<E: Element>(in path: [E]) {
        for element in path where element.read([kAXRoleAttribute]).text[0] == kAXWindowRole {
            _ = element.perform(kAXRaiseAction)
            return
        }
    }

    /// Press the identified element.
    private static func press<E: Element>(
        _ target: Step.Target, in root: E
    ) throws(DriveError)
        -> StepResult
    {
        return try perform(kAXPressAction, on: target.identifier, in: root, step: "press")
    }

    /// Perform `action` on the element with `identifier`.
    ///
    /// `step` names the result, so `press` reports itself rather than the general
    /// mechanism it is a shorthand for.
    private static func perform<E: Element>(
        _ action: String,
        on identifier: String,
        in root: E,
        step: String
    ) throws(DriveError) -> StepResult {
        let path = try find(identifier, from: root)
        guard let element = path.last else {
            throw DriveError(
                kind: .identifierNotFound,
                message: "no element has the identifier \(identifier)",
                hint: nil
            )
        }

        // Checked before performing, so the error can name what the element does
        // accept. Performing an unsupported action answers `action_unsupported`
        // with nothing to act on.
        let actions = element.actions
        guard actions.contains(action) else {
            throw DriveError(
                kind: .actionUnsupported,
                message: "\(identifier) does not accept \(action)",
                hint: actions.isEmpty
                    ? "it advertises no actions at all; a list row is activated with `select`"
                    : "it accepts: \(actions.joined(separator: ", "))"
            )
        }

        let status = element.perform(action)
        guard status == .success else {
            throw DriveError(
                kind: .actionFailed,
                message: "performing \(action) on \(identifier) answered \(status.name)",
                hint: nil
            )
        }

        return StepResult(
            step: step,
            identifier: identifier,
            role: element.read([kAXRoleAttribute]).text[0] ?? "<none>",
            confirmed: nil
        )
    }

    /// Put text into the identified field.
    private static func type<E: Element>(
        _ target: Step.TypeTarget, in root: E
    ) throws(DriveError)
        -> StepResult
    {
        let path = try find(target.identifier, from: root)
        guard let element = path.last else {
            throw DriveError(
                kind: .identifierNotFound,
                message: "no element has the identifier \(target.identifier)",
                hint: nil
            )
        }

        guard element.isSettable(kAXValueAttribute) else {
            throw DriveError(
                kind: .notEditable,
                message: "\(target.identifier) does not accept a write to AXValue",
                hint: "a static label and a disabled field both look like this; check with "
                    + "`jpdrive dump --settable`"
            )
        }

        let status = element.setText(kAXValueAttribute, target.text)
        guard status == .success else {
            throw DriveError(
                kind: .writeFailed,
                message: "writing AXValue to \(target.identifier) answered \(status.name)",
                hint: nil
            )
        }

        let committed = try confirm(element, identifier: target.identifier)

        // `confirmed` says the field holds the text, and nothing more. Whether the
        // application reacted is the caller's assertion to make, against whatever
        // the typing was supposed to change.
        let after = element.read([kAXValueAttribute, kAXRoleAttribute])

        return StepResult(
            step: "type",
            identifier: target.identifier,
            role: after.text[1] ?? "<none>",
            confirmed: after.text[0] == target.text,
            committed: committed
        )
    }

    /// Commit an edit, if the element offers a way to.
    ///
    /// Answers whether it did. An element with no confirm action is not a failure:
    /// some fields publish every change as it happens and need nothing further.
    private static func confirm<E: Element>(
        _ element: E, identifier: String
    ) throws(DriveError) -> Bool {
        guard element.actions.contains(kAXConfirmAction) else { return false }

        let status = element.perform(kAXConfirmAction)
        guard status == .success else {
            throw DriveError(
                kind: .actionFailed,
                message: "confirming \(identifier) answered \(status.name)",
                hint:
                    "the text was written but not committed, so the application has not seen it"
            )
        }

        return true
    }

    /// Attributes read while walking a menu path.
    private static let menuBatch = [
        kAXRoleAttribute,
        AXElement.attributedDescription,
        kAXDescriptionAttribute,
        kAXTitleAttribute,
    ]

    /// How long to wait for the application to come forward, and for the item
    /// addressed through it to be enabled.
    static let activationTimeout = Duration.milliseconds(2000)

    /// Press the menu item at the end of a titled path.
    private static func menu<E: Element>(
        _ target: Step.MenuTarget, in root: E, within timeout: Duration
    ) throws(DriveError)
        -> StepResult
    {
        guard !target.path.isEmpty else {
            throw DriveError(
                kind: .badUsage,
                message: "a menu step needs a path, such as [\"File\", \"Close\"]",
                hint: nil
            )
        }

        let start: E
        let origin: String

        if let owner = target.under {
            // A menu already on screen. No activation: showing it required the
            // application to be active, and asking again would be a no-op at best.
            start = try shownMenu(of: owner, in: root)
            origin = "'\(owner)' is showing a menu that"
        } else {
            // The one step that takes focus from whatever had it. AppKit disables
            // every menu item that acts on the front window or on the responder
            // chain while the application is in the background, which is most of
            // the menu bar: without this, a path resolves to an item that cannot
            // be pressed.
            try front(root, within: timeout)

            guard let bar = root.elements(kAXMenuBarAttribute).first else {
                throw DriveError(
                    kind: .notFound,
                    message: "the application reports no menu bar",
                    hint: "an agent or accessory application has none"
                )
            }
            start = bar
            origin = "the menu bar"
        }

        var current = start
        var reached: [String] = []

        for title in target.path {
            guard let next = child(titled: title, of: current) else {
                throw DriveError(
                    kind: .notFound,
                    message: reached.isEmpty
                        ? "\(origin) holds no item titled '\(title)'"
                        : "'\(reached.joined(separator: " > "))' holds no item titled '\(title)'",
                    hint: "it holds: \(titles(of: current).joined(separator: ", "))"
                )
            }
            current = next
            reached.append(title)
        }

        let path = target.path.joined(separator: " > ")
        try waitUntilEnabled(current, named: path, within: timeout)

        let actions = current.actions
        guard actions.contains(kAXPressAction) else {
            throw DriveError(
                kind: .actionUnsupported,
                message: "'\(path)' does not accept AXPress",
                hint: actions.isEmpty
                    ? "the path names a submenu rather than an item; name the item inside it"
                    : "it accepts: \(actions.joined(separator: ", "))"
            )
        }

        let status = current.perform(kAXPressAction)
        guard status == .success else {
            throw DriveError(
                kind: .actionFailed,
                message: "pressing '\(path)' answered \(status.name)",
                hint: nil
            )
        }

        return StepResult(
            step: "menu",
            identifier: path,
            role: current.read([kAXRoleAttribute]).text[0] ?? "<none>",
            confirmed: nil
        )
    }

    /// The menu an element is currently displaying.
    ///
    /// A shown menu hangs off the element that opened it, after that element's
    /// own children, which is why a capped or filtered read passes straight over
    /// it.
    private static func shownMenu<E: Element>(
        of identifier: String, in root: E
    ) throws(DriveError) -> E {
        let path = try find(identifier, from: root)
        guard let owner = path.last else {
            throw DriveError(
                kind: .identifierNotFound,
                message: "no element has the identifier \(identifier)",
                hint: nil
            )
        }

        let children = owner.read([kAXRoleAttribute]).children
        for child in children where child.read([kAXRoleAttribute]).text[0] == kAXMenuRole {
            return child
        }

        throw DriveError(
            kind: .notFound,
            message: "\(identifier) is not showing a menu",
            hint: """
                open one first, in an earlier step: \
                {"perform": {"identifier": "\(identifier)", "action": "AXShowMenu"}}
                """
        )
    }

    /// Bring the application forward, and wait until it reports that it is.
    ///
    /// Writing `AXFrontmost` is a request. The window server grants it a moment
    /// later, and whatever depends on it later still.
    ///
    /// Failing rather than carrying on is the point. Every caller does something
    /// that only means what it says once the application is in front: a menu item
    /// acting on the front window is disabled until then, and a synthesized
    /// pointer event goes to whoever owns the screen coordinate, which is the
    /// application the person at the keyboard is using.
    private static func front<E: Element>(
        _ root: E, within timeout: Duration
    ) throws(DriveError) {
        guard root.flag(kAXFrontmostAttribute) != true else { return }

        let status = root.setFlag(kAXFrontmostAttribute, true)
        guard status == .success else {
            throw DriveError(
                kind: .writeFailed,
                message: "bringing the application forward answered \(status.name)",
                hint: "the step cannot be trusted to reach the app while it is behind another"
            )
        }

        guard poll(untilTrue: { root.flag(kAXFrontmostAttribute) == true }, within: timeout)
        else {
            throw DriveError(
                kind: .timeout,
                message:
                    "the application did not come forward within \(timeout.milliseconds)ms",
                hint: "another application may be holding focus with a modal panel"
            )
        }
    }

    /// Wait for an item to stop reporting itself disabled.
    ///
    /// An element that reports no `AXEnabled` at all is not disabled: plenty
    /// carry no such attribute, and treating its absence as a refusal would
    /// reject every one of them.
    private static func waitUntilEnabled<E: Element>(
        _ item: E, named path: String, within timeout: Duration
    ) throws(DriveError) {
        if poll(untilTrue: { item.flag(kAXEnabledAttribute) != false }, within: timeout) {
            return
        }

        throw DriveError(
            kind: .disabled,
            message: "'\(path)' is disabled",
            hint:
                "an item acting on a selection is disabled while nothing is selected, and one "
                + "acting on the front window while no window has focus"
        )
    }

    /// Poll `condition` until it holds, or `timeout` elapses.
    private static func poll(untilTrue condition: () -> Bool, within timeout: Duration) -> Bool
    {
        let clock = ContinuousClock()
        let started = clock.now

        while true {
            if condition() { return true }
            guard clock.now - started < timeout else { return false }
            Thread.sleep(forTimeInterval: defaultInterval.seconds)
        }
    }

    /// The child of `parent` whose title is `title`.
    ///
    /// Descends through `AXMenu`, which carries no title of its own: a bar item's
    /// items live inside one, so a path names `["File", "Close"]` rather than
    /// spelling out the container between them.
    private static func child<E: Element>(titled title: String, of parent: E) -> E? {
        for child in parent.read([]).children {
            let text = child.read(menuBatch).text

            if text[1] ?? text[2] ?? text[3] == title {
                return child
            }

            guard text[0] == "AXMenu", let found = self.child(titled: title, of: child) else {
                continue
            }
            return found
        }

        return nil
    }

    /// The titles a level offers, for saying what a path could have named instead.
    private static func titles<E: Element>(of parent: E) -> [String] {
        var found: [String] = []

        for child in parent.read([]).children {
            let text = child.read(menuBatch).text

            if let title = text[1] ?? text[2] ?? text[3], !title.isEmpty {
                found.append(title)
                continue
            }

            // An untitled `AXMenu` is the container a path skips, so what it holds
            // is what this level effectively offers.
            guard text[0] == "AXMenu" else { continue }
            found.append(contentsOf: titles(of: child))
        }

        return found
    }

    /// Select the row that owns the identified element.
    private static func select<E: Element>(
        _ target: Step.Target, in root: E
    ) throws(DriveError)
        -> StepResult
    {
        let path = try find(target.identifier, from: root)

        guard let owner = selectionOwner(in: path) else {
            throw DriveError(
                kind: .notSelectable,
                message:
                    "neither \(target.identifier) nor its \(maxAncestors) nearest ancestors accept "
                    + "a write to AXSelected",
                hint: "check what the element reports with: jpdrive dump --settable"
            )
        }

        let status = owner.setFlag(kAXSelectedAttribute, true)
        guard status == .success else {
            throw DriveError(
                kind: .writeFailed,
                message: "writing AXSelected to \(target.identifier) answered \(status.name)",
                hint: nil
            )
        }

        return StepResult(
            step: "select",
            identifier: target.identifier,
            role: owner.read([kAXRoleAttribute]).text[0] ?? "<none>",
            confirmed: owner.flag(kAXSelectedAttribute) ?? false
        )
    }

    /// Default time to keep polling for an element to appear.
    private static let defaultTimeout = Duration.milliseconds(5000)

    /// Default pause between polling attempts.
    private static let defaultInterval = Duration.milliseconds(100)

    /// Wait until an element with the target's identifier exists.
    ///
    /// Returns as soon as it is found, including on the first attempt when it was
    /// already there.
    private static func waitFor<E: Element>(
        _ target: Step.WaitTarget, in root: E
    ) throws(DriveError)
        -> StepResult
    {
        // Resolved once, before the loop. This is the expensive search, and paying
        // it on every attempt is what makes an unscoped wait useless.
        let scope: E
        if let under = target.under {
            guard let container = try find(under, from: root).last else {
                throw DriveError(
                    kind: .identifierNotFound,
                    message: "no element has the identifier \(under) to wait inside",
                    hint: "`under` names a container that must already exist"
                )
            }
            scope = container
        } else {
            scope = root
        }

        let timeout = target.timeoutMs.map { Duration.milliseconds($0) } ?? defaultTimeout
        let interval = target.intervalMs.map { Duration.milliseconds($0) } ?? defaultInterval

        let clock = ContinuousClock()
        let started = clock.now
        var attempts = 0

        while true {
            attempts += 1

            if let path = try? find(target.identifier, from: scope), let found = path.last {
                return StepResult(
                    step: "wait_for",
                    identifier: target.identifier,
                    role: found.read([kAXRoleAttribute]).text[0] ?? "<none>",
                    confirmed: true
                )
            }

            guard clock.now - started < timeout else { break }
            Thread.sleep(forTimeInterval: interval.seconds)
        }

        let elapsed = clock.now - started
        throw DriveError(
            kind: .timeout,
            message:
                "\(target.identifier) did not appear within \(timeout.milliseconds)ms "
                + "(\(attempts) attempts over \(elapsed.milliseconds)ms)",
            hint: attempts == 1
                ? "one attempt exhausted the timeout; scope the search with `under`"
                : nil
        )
    }

    /// The nearest element at or above the end of `path` that accepts a write to
    /// `AXSelected`.
    ///
    /// Walks the chain the search descended rather than reading `AXParent`. The
    /// identified element is a SwiftUI leaf that does not report a parent, so
    /// climbing from it arrives nowhere, while the chain that reached it is known
    /// for free and is not subject to that.
    private static func selectionOwner<E: Element>(in path: [E]) -> E? {
        for element in path.suffix(maxAncestors + 1).reversed()
        where element.isSettable(kAXSelectedAttribute) {
            return element
        }
        return nil
    }

    /// What a search reads at each element.
    ///
    /// Children arrive alongside, so the identifier is all the search asks for.
    private static let searchBatch = [kAXIdentifierAttribute]

    /// Find the element whose identifier is exactly `identifier`, and the chain of
    /// elements that reached it.
    ///
    /// The path comes back rather than the element alone because acting on an
    /// element often means acting on one of its ancestors, and this tree cannot
    /// reliably be walked upwards.
    ///
    /// Depth-first with an early exit, reading only what the search needs. Reading
    /// every attribute of each element on the way past would make a step against
    /// this app's few thousand elements cost seconds.
    private static func find<E: Element>(
        _ identifier: String, from root: E
    ) throws(DriveError)
        -> [E]
    {
        var stack = [[root]]
        var unreadable = 0

        while let path = stack.popLast() {
            guard let element = path.last else { continue }
            let reading = element.read(searchBatch)

            if reading.failed {
                unreadable += 1
            }

            if reading.text[0] == identifier {
                return path
            }

            // Reversed, so a depth-first walk visits siblings in the order the
            // application reports them.
            for child in reading.children.reversed() {
                stack.append(path + [child])
            }
        }

        // A miss with a gap in it is not a miss. The element may sit under one of
        // the branches that could not be read, and reporting a clean
        // `identifier_not_found` invites the caller to change an identifier that
        // was right all along.
        guard unreadable == 0 else {
            throw DriveError(
                kind: .readFailed,
                message:
                    "\(identifier) was not found, and \(unreadable) element(s) could not be "
                    + "read",
                hint: "the application may be busy or exiting; the identifier may exist"
            )
        }

        throw DriveError(
            kind: .identifierNotFound,
            message: "no element has the identifier \(identifier)",
            hint: "list what is addressable with: jpdrive tree --identifier <prefix>"
        )
    }
}
