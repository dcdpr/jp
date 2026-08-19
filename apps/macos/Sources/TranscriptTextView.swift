import AppKit
import SwiftUI

/// A text view that reports what a window drag asked of it.
///
/// Whether the text re-wraps while the window is still moving is the difference
/// between the transcript feeling native and feeling like a screenshot that
/// catches up. It depends on the layout stack, and the two fail differently
/// enough that the count of frames the drag delivered is worth having either
/// way: a stale transcript with a high count is layout refusing to run, and a
/// stale transcript with a count of zero is AppKit serving cached pixels
/// instead of resizing the view at all.
private final class LiveWrappingTextView: NSTextView {
    /// How many frames of the current drag changed this view's size at all.
    private var frames = 0

    /// How many of those changed its *width*.
    ///
    /// The one that matters: a container only re-wraps when the width it tracks
    /// moves. A drag that delivers hundreds of frames of pure height change
    /// would leave the text correctly un-re-wrapped, and counting frames alone
    /// could not tell that apart from layout refusing to run.
    private var widthChanges = 0

    /// The width this view had when the drag began.
    private var widthAtStart: CGFloat = 0

    /// How many frames of the drag changed the text *container's* width.
    ///
    /// The link between a resized view and re-wrapped text. A container tracking
    /// the view is supposed to follow it, and a container whose geometry changes
    /// is what invalidates layout — so a view width that moves while this stays
    /// still is the whole bug, and one that moves in step with it puts the fault
    /// after this point.
    private var containerChanges = 0

    /// The container width seen at the previous frame.
    private var lastContainerWidth: CGFloat = 0

    /// The least of the document the layout manager had laid out at any frame
    /// of the drag, as a character index.
    ///
    /// Contiguous layout fills from the start, so this is how far down the
    /// document layout reached at its worst. The minimum rather than the last
    /// value, because the last frame of a drag is the one most likely to have
    /// caught up.
    private var laidOutTo = Int.max

    /// How many times AppKit asked this view to draw during the drag.
    private var draws = 0

    /// The tallest rectangle AppKit asked it to draw, in points.
    ///
    /// Compared against the height of what is on screen. A number far short of
    /// that is AppKit redrawing a strip and keeping the rest, which is what a
    /// view is told to expect when it says its content survives a resize.
    private var tallestDraw: CGFloat = 0

    /// Whether AppKit may keep what this view already drew when it resizes.
    ///
    /// Overridden to `false`. Left to itself the answer is yes, and then a
    /// narrowing drag exposes no new region, so nothing is marked dirty and the
    /// cached pixels are simply clipped — the text underneath has re-wrapped
    /// and nobody has been asked to draw it.
    ///
    /// The cost is redrawing the visible text on every frame of a drag, which
    /// is the work being watched anyway.
    override var preservesContentDuringLiveResize: Bool { false }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        guard inLiveResize else { return }

        draws += 1
        tallestDraw = max(tallestDraw, dirtyRect.height)
    }

    override func viewWillStartLiveResize() {
        super.viewWillStartLiveResize()

        frames = 0
        widthChanges = 0
        draws = 0
        tallestDraw = 0
        laidOutTo = Int.max
        containerChanges = 0
        widthAtStart = frame.width
        lastContainerWidth = textContainer?.size.width ?? 0
    }

    override func setFrameSize(_ newSize: NSSize) {
        let before = frame.width
        super.setFrameSize(newSize)
        guard inLiveResize else { return }

        frames += 1
        if newSize.width != before {
            widthChanges += 1
        }

        // The width a tracking container would take, handed to it directly.
        //
        // A text view passes its width to the container it is tracked by, and
        // does not do it while a resize is in progress: the container keeps the
        // width the drag started from until the mouse comes up. Nothing then
        // changes the container's geometry, nothing invalidates layout, and the
        // view faithfully redraws lines wrapped to a width the window no longer
        // has.
        //
        // Setting it here is what a tracking container would have done, one
        // frame earlier. The inset is counted twice because it applies to both
        // edges.
        if let container = textContainer {
            let wanted = newSize.width - textContainerInset.width * 2
            if container.size.width != wanted {
                container.size = NSSize(width: wanted, height: container.size.height)
            }
        }

        let containerWidth = textContainer?.size.width ?? 0
        if containerWidth != lastContainerWidth {
            containerChanges += 1
            lastContainerWidth = containerWidth
        }

        // Only on TextKit 2, which lays out around the viewport and leaves the
        // rest estimated. Contiguous layout has no viewport to nudge.
        //
        // Asked of `textLayoutManager` rather than of the stack constant,
        // because reading it is the one probe that answers which stack this
        // view is on without moving it to the other one.
        if let viewport = textLayoutManager?.textViewportLayoutController {
            viewport.layoutViewport()
        } else {
            laidOutTo = min(laidOutTo, layoutManager?.firstUnlaidCharacterIndex() ?? -1)
        }
    }

    override func viewDidEndLiveResize() {
        super.viewDidEndLiveResize()

        let visible = enclosingScrollView?.documentVisibleRect ?? .zero

        Trace.event(
            "transcript.liveresize",
            target: "JP.Transcript",
            fields: [
                ("frames", .int(frames)),
                ("width_changes", .int(widthChanges)),
                ("container_changes", .int(containerChanges)),
                ("container_width", .double(Double(textContainer?.size.width ?? 0))),
                ("tracks_width", .bool(textContainer?.widthTracksTextView ?? false)),
                ("draws", .int(draws)),
                ("tallest_draw", .double(Double(tallestDraw))),
                ("visible_height", .double(Double(visible.height))),
                ("width_from", .double(Double(widthAtStart))),
                ("width_to", .double(Double(frame.width))),
                ("laid_out_to", .int(laidOutTo == Int.max ? -1 : laidOutTo)),
                ("characters", .int(textStorage?.length ?? 0)),
                ("visible_from_y", .double(Double(visible.minY))),
                ("document_height", .double(Double(frame.height))),
            ]
        )
    }
}

/// The transcript, drawn by one text view.
///
/// The document is built here rather than handed in, so it is rebuilt only when
/// the conversation or the appearance changes — not on every layout pass, and
/// not on every frame of a window resize.
struct TranscriptTextView: NSViewRepresentable {
    /// Which conversation is on screen, and the cheap half of deciding whether
    /// the document has to be rebuilt.
    let conversationID: String?

    /// What to draw, oldest turn first.
    let turns: [ConversationTurn]

    func makeCoordinator() -> Coordinator {
        Coordinator()
    }

    func makeNSView(context: Context) -> NSScrollView {
        let textView = LiveWrappingTextView(usingTextLayoutManager: Self.usesTextKit2)
        Self.configure(textView)

        textView.setAccessibilityIdentifier(AccessibilityID.Transcript.text)

        let scroll = NSScrollView()
        scroll.documentView = textView
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.setAccessibilityIdentifier(AccessibilityID.Transcript.scroll)

        context.coordinator.watchForLayoutManagerDowngrade(of: textView)

        return scroll
    }

    /// Set a text view up to draw a transcript.
    ///
    /// Separate from ``makeNSView(context:)`` so it can be checked without a
    /// SwiftUI host: several of these settings are the difference between a
    /// transcript that behaves and one that looks right and does not.
    static func configure(_ textView: NSTextView) {
        textView.isEditable = false
        textView.isSelectable = true
        textView.isRichText = false
        // The SwiftUI background behind this view is the one the design calls
        // for; AppKit's would paint over it.
        textView.drawsBackground = false
        textView.textContainerInset = NSSize(width: Self.margin, height: Self.margin)
        textView.isVerticallyResizable = true
        textView.isHorizontallyResizable = false
        textView.autoresizingMask = [.width]
        textView.minSize = NSSize(width: 0, height: 0)
        textView.maxSize = NSSize(
            width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)

        // Width from the view, height unbounded: the container re-wraps as the
        // window is resized and grows downwards as far as the document needs.
        textView.textContainer?.widthTracksTextView = true
        textView.textContainer?.containerSize = NSSize(
            width: 0, height: CGFloat.greatestFiniteMagnitude)
        // The document's own margin is `textContainerInset`; this would add five
        // more points inside every line fragment.
        textView.textContainer?.lineFragmentPadding = 0

        // The cursor, and nothing else.
        //
        // This dictionary is what AppKit merges over a `.link` range as it draws,
        // and it is also the whole mechanism behind the pointing hand: the default
        // carries `.cursor` alongside a colour and an underline. Emptying it to
        // keep the document's own colour takes the cursor with it, and a link that
        // does not change the pointer does not read as a link.
        textView.linkTextAttributes = [.cursor: NSCursor.pointingHand]

        layOutContiguously(textView)
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        guard let textView = scroll.documentView as? NSTextView else { return }

        let appearance = textView.effectiveAppearance
        guard
            context.coordinator.needsDocument(
                for: conversationID, turnCount: turns.count, appearance: appearance)
        else { return }

        // A colour is resolved into the document as it is built rather than each
        // time it is drawn, so a window moved between appearances rebuilds.
        let style = MarkdownStyle.reading(in: appearance)
        let document = Trace.measuring(
            "transcript.render",
            target: Self.traceTarget,
            fields: [("turn_count", .int(turns.count))]
        ) {
            TranscriptDocument.attributed(turns, style: style)
        }

        textView.textStorage?.setAttributedString(document)
    }

    /// What the transcript's events are attributed to.
    private static let traceTarget = "JP.Transcript"

    /// The space between the text and the edges of the pane.
    private static let margin: CGFloat = 24

    /// Which layout stack the text view runs on.
    ///
    /// TextKit 1, bought deliberately and not cheaply.
    ///
    /// TextKit 2 lays out around the viewport and estimates the rest, which is
    /// what a long document wants and is measurably faster here: the same ten
    /// programmatic resizes cost 155 samples against this stack's 438 on a
    /// 29-event conversation, and 355 against 412 on a 167-event one. TextKit 2
    /// scales with the document where this is flat, so the gap narrows as
    /// conversations grow, but at these sizes it is behind.
    ///
    /// What contiguous layout buys is an exact document height, and so a scroll
    /// bar that states the truth instead of an estimate that refines as it
    /// scrolls and shifts the knob under the pointer. That was a stated goal, and
    /// it is the reason for the trade.
    ///
    /// It is *not* what fixed re-wrapping during a window drag — that was the text
    /// container not being told its new width, and it needed fixing on both
    /// stacks. Switching here changes the scroll bar and the cost, nothing else.
    private static let usesTextKit2 = false

    /// Ask a TextKit 1 view for an exact document height.
    ///
    /// Non-contiguous layout skips the ranges nobody is looking at, which is
    /// faster to first paint and gives back an approximate total — the same
    /// estimate, and so the same shifting scroll bar, that choosing this stack
    /// was meant to avoid. Off, so the height is measured rather than guessed.
    ///
    /// Does nothing on TextKit 2, and asks in the order that keeps that true:
    /// `textLayoutManager` reports which stack the view is on without changing
    /// it, where reading `layoutManager` first would drag a TextKit 2 view down
    /// to TextKit 1 permanently and silently.
    private static func layOutContiguously(_ textView: NSTextView) {
        guard textView.textLayoutManager == nil else { return }

        textView.layoutManager?.allowsNonContiguousLayout = false
    }

    /// Per-view state that outlives a single layout pass.
    @MainActor
    final class Coordinator {
        /// What the document currently in the text view was built from.
        private var built:
            (conversationID: String?, turnCount: Int, appearance: NSAppearance.Name)?

        /// Whether the document has to be rebuilt for this conversation and
        /// appearance.
        ///
        /// The turn count stands in for the turns themselves, which would cost
        /// a comparison of every message's text on a pass that happens on every
        /// frame of a resize. It is enough because a window reads a conversation
        /// once — turns written by a concurrent `jp query` are invisible until
        /// the workspace is reopened — and because the view is rebuilt outright
        /// when the conversation changes.
        func needsDocument(
            for conversationID: String?, turnCount: Int, appearance: NSAppearance
        ) -> Bool {
            let wanted = (conversationID, turnCount, appearance.name)

            guard let built else {
                built = wanted
                return true
            }

            guard
                built.conversationID == wanted.0,
                built.turnCount == wanted.1,
                built.appearance == wanted.2
            else {
                self.built = wanted
                return true
            }

            return false
        }

        /// Report a text view falling back to TextKit 1.
        ///
        /// The downgrade is silent, permanent for that view, and takes
        /// viewport-driven layout with it — so a transcript that quietly became
        /// slow at size would look like the layout work never helped rather
        /// than like something switched it off.
        func watchForLayoutManagerDowngrade(of textView: NSTextView) {
            observer = NotificationCenter.default.addObserver(
                forName: NSTextView.willSwitchToNSLayoutManagerNotification,
                object: textView,
                queue: .main
            ) { _ in
                Trace.event(
                    "transcript.textkit.downgrade",
                    target: "JP.Transcript",
                    level: .warn
                )
            }
        }

        /// The registration to undo when this coordinator goes away.
        ///
        /// `nonisolated(unsafe)` because `deinit` is not isolated and this is
        /// not `Sendable`. Safe: it is written once while the view is being
        /// made, on the main actor, and read once in `deinit` — which runs only
        /// after the last reference to the coordinator is gone, so there is no
        /// second access to race with.
        private nonisolated(unsafe) var observer: (any NSObjectProtocol)?

        deinit {
            if let observer {
                NotificationCenter.default.removeObserver(observer)
            }
        }
    }
}
