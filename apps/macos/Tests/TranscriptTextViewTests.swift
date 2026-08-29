import AppKit
import Testing

@testable import JP

/// How the transcript's text view is set up.
///
/// Configuration rather than behaviour, and worth pinning because each of these
/// is a line that looks like tidying and is not: the transcript reads correctly
/// with any of them wrong, and then misbehaves in a way that looks like a layout
/// bug.
@Suite("TranscriptTextView")
@MainActor
struct TranscriptTextViewTests {
    private func configured() -> NSTextView {
        let textView = NSTextView(usingTextLayoutManager: false)
        TranscriptTextView.configure(textView)
        return textView
    }

    /// The pointing hand over a link is this dictionary and nothing else. The
    /// default carries a colour and an underline alongside it, which would draw
    /// over the ones the document already has — so the cursor is kept and the rest
    /// dropped, rather than the whole dictionary emptied.
    @Test("keeps the pointing hand over links without AppKit's link styling")
    func stylesLinkCursorOnly() {
        let attributes = configured().linkTextAttributes ?? [:]

        #expect(attributes[.cursor] as? NSCursor == NSCursor.pointingHand)
        #expect(attributes[.foregroundColor] == nil)
        #expect(attributes[.underlineStyle] == nil)
    }

    /// Readable and selectable, which is what makes ⌘C and VoiceOver work, and
    /// not editable, which is what makes it a transcript.
    @Test("reads as a selectable transcript rather than an editor")
    func isSelectableAndNotEditable() {
        let textView = configured()

        #expect(textView.isEditable == false)
        #expect(textView.isSelectable)
    }

    /// The container follows the view's width so the text re-wraps as the window
    /// is resized, and is unbounded in height so the document grows downwards
    /// instead of being clipped.
    @Test("tracks the view's width and grows without a height limit")
    func tracksWidthAndGrowsDown() throws {
        let container = try #require(configured().textContainer)

        #expect(container.widthTracksTextView)
        #expect(container.size.height == CGFloat.greatestFiniteMagnitude)
        // The document's margin is `textContainerInset`; this would add five more
        // points inside every line fragment.
        #expect(container.lineFragmentPadding == 0)
    }

    /// The SwiftUI background behind the pane is the one the design calls for, and
    /// AppKit's would paint over it.
    @Test("draws no background of its own")
    func drawsNoBackground() {
        #expect(configured().drawsBackground == false)
    }

    /// Contiguous layout is what gives an exact document height, and so a scroll
    /// bar that does not shift as it scrolls. Non-contiguous layout is faster to
    /// first paint and reports an estimate, which is the thing choosing this stack
    /// was meant to avoid.
    @Test("lays a TextKit 1 document out contiguously")
    func laysOutContiguously() throws {
        let textView = NSTextView(usingTextLayoutManager: false)
        TranscriptTextView.configure(textView)

        let layout = try #require(textView.layoutManager)
        #expect(layout.allowsNonContiguousLayout == false)
    }
}
