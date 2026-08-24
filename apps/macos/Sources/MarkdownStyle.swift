import AppKit

/// The fonts, colours and metrics block markdown is drawn with.
///
/// Passed in rather than read from ``Theme`` inside the renderer, so the
/// translation from markdown to attributes can be checked against fixed numbers
/// without a running app deciding what "body text" resolves to.
struct MarkdownStyle {
    /// Prose, and the size every other size is derived from.
    var body: NSFont

    /// Code spans and code blocks.
    var monospaced: NSFont

    /// What prose is drawn in.
    var text: NSColor

    /// What a block quote and a thematic break are drawn in.
    var secondary: NSColor

    /// Behind a code span or a code block.
    var codeBackground: NSColor

    /// A code span's or code block's text.
    var codeText: NSColor

    /// A link's text, which is also what underlines it.
    var link: NSColor

    /// How far one level of list or quote nesting indents.
    var indent: CGFloat

    /// How wide one column of a table is.
    ///
    /// Fixed rather than measured. Measuring would mean laying every cell out to
    /// find the widest, at a width the container has not settled on yet, and
    /// re-doing it on every resize — for a reader, not an editor. A column wide
    /// enough for a short phrase is what a plain-text table gives and is legible
    /// at the sizes JP transcripts use.
    var tableColumnWidth: CGFloat

    /// The gap left below a block, before the next one.
    var blockSpacing: CGFloat

    /// How much taller than its font a line of prose is drawn.
    var lineSpacing: CGFloat

    /// The gap above a message that follows another in the same turn.
    var eventSpacing: CGFloat

    /// The gap above the first message of a turn.
    ///
    /// Wider than ``eventSpacing``, because it is the only thing separating one
    /// turn from the last.
    var turnSpacing: CGFloat

    /// The app's palette, at the reading size.
    ///
    /// `appearance` decides which half of each ``ThemeColor`` is taken, because a
    /// colour baked into an attributed string is resolved once when the string is
    /// built rather than each time it is drawn.
    @MainActor
    static func reading(in appearance: NSAppearance) -> MarkdownStyle {
        let size = NSFont.systemFontSize + 1

        return MarkdownStyle(
            body: .systemFont(ofSize: size),
            monospaced: .monospacedSystemFont(ofSize: size - 1, weight: .regular),
            text: resolved(Theme.bodyText, in: appearance),
            secondary: resolved(Theme.secondaryText, in: appearance),
            codeBackground: resolved(Theme.inlineCodeBackground, in: appearance),
            codeText: resolved(Theme.inlineCodeText, in: appearance),
            link: resolved(Theme.accent, in: appearance),
            indent: 22,
            tableColumnWidth: 150,
            blockSpacing: 10,
            lineSpacing: 3,
            eventSpacing: 18,
            turnSpacing: 40
        )
    }

    /// How large a heading of `level` is drawn, relative to ``body``.
    ///
    /// Levels past the third are the body size in bold, which is what a document
    /// nested that deep wants: another distinct size would be a difference nobody
    /// can see.
    func headingSize(_ level: Int) -> CGFloat {
        let scale: CGFloat =
            switch level {
            case 1: 1.6
            case 2: 1.35
            case 3: 1.15
            default: 1
            }

        return (body.pointSize * scale).rounded()
    }

    /// One palette colour, fixed to the half `appearance` shows.
    private static func resolved(_ colour: ThemeColor, in appearance: NSAppearance) -> NSColor {
        ThemeColor.srgb(colour.value(under: appearance))
    }
}
