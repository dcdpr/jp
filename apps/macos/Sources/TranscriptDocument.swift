import AppKit

/// A conversation's turns, as one piece of attributed text.
///
/// One string rather than one view per message, because a text view lays out
/// what its viewport needs and re-wraps incrementally, where a stack of views
/// each measure and wrap themselves and a width change costs the sum of them.
///
/// Turn boundaries are drawn as space rather than as a rule: the gap above the
/// first message of a turn is wider than the gap between two messages inside
/// one, which is what separates them.
enum TranscriptDocument {
    /// `turns` laid out for reading, oldest first.
    ///
    /// Empty when there is nothing to show, which a caller distinguishes from a
    /// conversation it could not read.
    static func attributed(
        _ turns: [ConversationTurn], style: MarkdownStyle
    ) -> NSAttributedString {
        let document = NSMutableAttributedString()

        for turn in turns {
            for (offset, event) in turn.events.enumerated() {
                // The gap belongs above the speaker's name rather than below the
                // message before it, so all of the spacing is decided in one
                // place and none of it has to reach back into text the markdown
                // renderer has already styled.
                let above: CGFloat =
                    if document.length == 0 { 0 } else if offset == 0 { style.turnSpacing } else
                    { style.eventSpacing }

                document.append(speaker(event.speaker, above: above, style: style))
                append(Markdown.attributed(event.text, style: style), to: document)
            }
        }

        // Every message ends with the newline separating it from the next, so
        // the last one leaves a trailing empty line.
        if document.length > 0 {
            document.deleteCharacters(in: NSRange(location: document.length - 1, length: 1))
        }

        return document
    }

    /// Who is speaking, as the line above what they said.
    private static func speaker(
        _ name: String, above: CGFloat, style: MarkdownStyle
    ) -> NSAttributedString {
        let paragraph = NSMutableParagraphStyle()
        paragraph.paragraphSpacingBefore = above
        paragraph.paragraphSpacing = 2

        return NSAttributedString(
            string: "\(name)\n",
            attributes: [
                .font: NSFont.systemFont(ofSize: style.body.pointSize - 2, weight: .semibold),
                .foregroundColor: style.secondary,
                .paragraphStyle: paragraph,
            ]
        )
    }

    /// Append `message` and the newline that ends it.
    ///
    /// The newline carries the message's own trailing attributes, so it sits on
    /// the same paragraph rather than opening an unstyled one of the default
    /// font's height.
    private static func append(
        _ message: NSAttributedString, to document: NSMutableAttributedString
    ) {
        document.append(message)

        let attributes =
            message.length > 0
            ? message.attributes(at: message.length - 1, effectiveRange: nil)
            : [:]

        document.append(NSAttributedString(string: "\n", attributes: attributes))
    }
}
