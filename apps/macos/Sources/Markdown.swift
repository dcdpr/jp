import AppKit
import Foundation

/// Markdown, turned into text a TextKit view can draw.
///
/// Foundation's own parser does the reading: `AttributedString(markdown:)` with
/// full syntax records block structure in the `presentationIntent` attribute and
/// inline styling in `inlinePresentationIntent`. It records and does not render,
/// so the work here is the translation — intents into fonts, colours, paragraph
/// styles and list markers.
///
/// Foundation leaves out the separators between blocks: two paragraphs come back
/// as adjacent runs with nothing between them. The newlines are put back here,
/// which is also what makes block spacing this file's to decide.
enum Markdown {
    /// `source` as attributed text, with its block structure drawn.
    ///
    /// Text that cannot be parsed is returned as itself in the body style, so a
    /// malformed message still shows its content.
    static func attributed(_ source: String, style: MarkdownStyle) -> NSAttributedString {
        let parsed = parse(source)
        let output = NSMutableAttributedString()

        // Which list items have had their bullet drawn. A list item holding two
        // paragraphs is two blocks, and only the first of them is marked.
        var marked: Set<Int> = []

        // The table row being gathered. Every cell is a block of its own, and a
        // row is one line of tab-separated cells, so the cells are held until the
        // row they belong to ends.
        var row: TableRow?

        for block in blocks(of: parsed) {
            let components = block.intent?.components ?? []

            if let cell = tableCell(in: components) {
                if row?.identity != cell.row {
                    flush(&row, into: output, style: style)
                    row = TableRow(
                        identity: cell.row, columns: cell.columns, isHeader: cell.isHeader)
                }

                row?.cells.append(
                    content(
                        of: block, in: parsed,
                        font: cell.isHeader ? style.body.with(.bold) : style.body,
                        colour: style.text, style: style, inCodeBlock: false
                    )
                )
                continue
            }

            flush(&row, into: output, style: style)
            output.append(rendered(block, of: parsed, style: style, marked: &marked))
        }

        flush(&row, into: output, style: style)

        // Every block ends with the newline separating it from the next, so the
        // last one leaves a trailing empty line.
        if output.length > 0 {
            output.deleteCharacters(in: NSRange(location: output.length - 1, length: 1))
        }

        return output
    }

    /// One run of characters sharing a block intent.
    private struct Block {
        /// What the parser said this block is, absent for text it left unmarked.
        let intent: PresentationIntent?

        /// Where the block sits in the parsed string.
        let range: Range<AttributedString.Index>
    }

    /// One row of a table, gathered cell by cell.
    ///
    /// Foundation reports a table as one block per cell, each carrying the row and
    /// the table above it. A row is drawn as a single paragraph of tab-separated
    /// cells, so the cells are collected until the row changes.
    private struct TableRow {
        /// The row's own identity, which is what says a cell belongs to it.
        let identity: Int

        /// The table's columns, in order, carrying the alignment each was
        /// declared with.
        let columns: [PresentationIntent.TableColumn]

        /// Whether this is the header row, which is drawn in bold.
        let isHeader: Bool

        var cells: [NSAttributedString] = []
    }

    /// What a list item's marker is, and whether it has been drawn yet.
    private struct ListItem {
        let ordinal: Int
        let identity: Int
        let ordered: Bool
    }

    private static func parse(_ source: String) -> AttributedString {
        let parsed = try? AttributedString(
            markdown: source,
            options: .init(
                allowsExtendedAttributes: false,
                interpretedSyntax: .full,
                failurePolicy: .returnPartiallyParsedIfPossible
            )
        )

        return parsed ?? AttributedString(source)
    }

    /// The parsed string cut into blocks.
    ///
    /// Adjacent runs belong to the same block when they carry the same intent:
    /// every block the parser produces has an identity of its own, so two
    /// neighbouring list items compare unequal even though both are paragraphs
    /// in an unordered list.
    private static func blocks(of parsed: AttributedString) -> [Block] {
        var blocks: [Block] = []

        for run in parsed.runs {
            if let last = blocks.last, last.intent == run.presentationIntent {
                blocks[blocks.count - 1] = Block(
                    intent: last.intent,
                    range: last.range.lowerBound..<run.range.upperBound
                )
            } else {
                blocks.append(Block(intent: run.presentationIntent, range: run.range))
            }
        }

        return blocks
    }

    /// One block, drawn, with the newline that separates it from the next.
    private static func rendered(
        _ block: Block,
        of parsed: AttributedString,
        style: MarkdownStyle,
        marked: inout Set<Int>
    ) -> NSAttributedString {
        let components = block.intent?.components ?? []
        let leaf = components.first?.kind
        let item = listItem(in: components)
        let isCodeBlock = if case .codeBlock = leaf { true } else { false }

        let font = blockFont(leaf, style: style)
        let colour = blockColour(leaf, quoted: quoteDepth(in: components), style: style)
        let paragraph = paragraphStyle(
            leaf: leaf,
            indent: style.indent
                * CGFloat(listDepth(in: components) + quoteDepth(in: components)),
            marked: item != nil,
            style: style
        )

        let content = NSMutableAttributedString()

        if let item, marked.insert(item.identity).inserted {
            content.append(
                NSAttributedString(
                    string: "\(item.ordered ? "\(item.ordinal)." : "•")\t",
                    attributes: [.font: font, .foregroundColor: colour]
                )
            )
        }

        content.append(
            self.content(
                of: block, in: parsed, font: font, colour: colour, style: style,
                inCodeBlock: isCodeBlock)
        )

        // A fenced block's content keeps the newline before its closing fence,
        // which would draw an empty last line inside the block.
        if isCodeBlock {
            while content.string.hasSuffix("\n") {
                content.deleteCharacters(in: NSRange(location: content.length - 1, length: 1))
            }
        }

        content.append(NSAttributedString(string: "\n", attributes: [.font: font]))
        content.addAttribute(
            .paragraphStyle, value: paragraph,
            range: NSRange(location: 0, length: content.length))

        if isCodeBlock {
            content.addAttribute(
                .backgroundColor, value: style.codeBackground,
                range: NSRange(location: 0, length: content.length))
        }

        return content
    }

    /// Draw the gathered row, if there is one, and forget it.
    ///
    /// Cells are separated by tabs and the paragraph carries one stop per column
    /// boundary, so a cell begins where its column does. The stop takes the
    /// alignment the column was declared with, which is the one piece of table
    /// styling the source actually states — `---:` in the separator row right-
    /// aligns a column of numbers, and Foundation reports it.
    private static func flush(
        _ row: inout TableRow?, into output: NSMutableAttributedString, style: MarkdownStyle
    ) {
        guard let gathered = row, !gathered.cells.isEmpty else {
            row = nil
            return
        }

        let paragraph = NSMutableParagraphStyle()
        paragraph.lineSpacing = style.lineSpacing
        // A table reads as one block, so the space goes after the last row rather
        // than between every pair of them. The rows of one table are consecutive,
        // and whatever follows opens with its own spacing.
        paragraph.paragraphSpacing = 0
        paragraph.tabStops = gathered.columns.indices.dropFirst().map { column in
            NSTextTab(
                textAlignment: alignment(of: gathered.columns[column]),
                location: CGFloat(column) * style.tableColumnWidth
            )
        }

        let line = NSMutableAttributedString()
        for (column, cell) in gathered.cells.enumerated() {
            if column > 0 {
                line.append(NSAttributedString(string: "\t"))
            }
            line.append(cell)
        }

        line.append(NSAttributedString(string: "\n", attributes: [.font: style.body]))
        line.addAttribute(
            .paragraphStyle, value: paragraph, range: NSRange(location: 0, length: line.length))

        output.append(line)
        row = nil
    }

    /// How a column's cells sit against their tab stop.
    private static func alignment(
        of column: PresentationIntent.TableColumn
    )
        -> NSTextAlignment
    {
        switch column.alignment {
        case .left: .left
        case .center: .center
        case .right: .right
        @unknown default: .left
        }
    }

    /// The row and table a cell belongs to, or `nil` when the block is not a cell.
    ///
    /// Components run innermost first, so a cell's are the cell, then its row,
    /// then the table.
    private static func tableCell(
        in components: [PresentationIntent.IntentType]
    )
        -> (row: Int, columns: [PresentationIntent.TableColumn], isHeader: Bool)?
    {
        guard let leaf = components.first?.kind else { return nil }
        guard case .tableCell = leaf else { return nil }
        guard components.count >= 3, case .table(let columns) = components[2].kind else {
            return nil
        }

        let isHeader: Bool
        switch components[1].kind {
        case .tableHeaderRow: isHeader = true
        case .tableRow: isHeader = false
        default: return nil
        }

        return (components[1].identity, columns, isHeader)
    }

    /// A block's runs, styled inline over a base font and colour.
    private static func content(
        of block: Block,
        in parsed: AttributedString,
        font: NSFont,
        colour: NSColor,
        style: MarkdownStyle,
        inCodeBlock: Bool
    ) -> NSAttributedString {
        let content = NSMutableAttributedString()

        for run in parsed[block.range].runs {
            content.append(
                inline(
                    run, text: String(parsed[run.range].characters), font: font,
                    colour: colour, style: style, inCodeBlock: inCodeBlock)
            )
        }

        return content
    }

    /// One run of a block, with its inline styling applied over the block's.
    private static func inline(
        _ run: AttributedString.Runs.Run,
        text: String,
        font: NSFont,
        colour: NSColor,
        style: MarkdownStyle,
        inCodeBlock: Bool
    ) -> NSAttributedString {
        let intent = run.inlinePresentationIntent ?? []
        var font = font

        if intent.contains(.stronglyEmphasized) {
            font = font.with(.bold)
        }
        if intent.contains(.emphasized) {
            font = font.with(.italic)
        }

        var attributes: [NSAttributedString.Key: Any] = [
            .font: font,
            .foregroundColor: colour,
        ]

        // Only a span inside prose: the whole of a fenced block is already
        // monospaced and already sitting on the code background.
        if intent.contains(.code), !inCodeBlock {
            attributes[.font] = style.monospaced
            attributes[.foregroundColor] = style.codeText
            attributes[.backgroundColor] = style.codeBackground
        }

        if intent.contains(.strikethrough) {
            attributes[.strikethroughStyle] = NSUnderlineStyle.single.rawValue
        }

        if let url = run.link {
            attributes[.link] = url
            attributes[.foregroundColor] = style.link
            attributes[.underlineStyle] = NSUnderlineStyle.single.rawValue
        }

        // A hard break carries the text it was written with — two spaces, or a
        // backslash — and means a new line inside the same paragraph.
        let text = intent.contains(.lineBreak) ? "\n" : text

        return NSAttributedString(string: text, attributes: attributes)
    }

    /// The innermost list item enclosing a block, if it is in a list at all.
    ///
    /// Components run innermost first, so the list an item belongs to is the
    /// component after it, and that is what says whether the marker is a bullet
    /// or a number.
    private static func listItem(in components: [PresentationIntent.IntentType]) -> ListItem? {
        guard
            let index = components.firstIndex(where: {
                if case .listItem = $0.kind { true } else { false }
            }),
            case .listItem(let ordinal) = components[index].kind
        else {
            return nil
        }

        let enclosing = components.dropFirst(index + 1).first?.kind
        let ordered = if case .orderedList = enclosing { true } else { false }

        return ListItem(
            ordinal: ordinal, identity: components[index].identity, ordered: ordered)
    }

    private static func listDepth(in components: [PresentationIntent.IntentType]) -> Int {
        components.count {
            switch $0.kind {
            case .orderedList, .unorderedList: true
            default: false
            }
        }
    }

    private static func quoteDepth(in components: [PresentationIntent.IntentType]) -> Int {
        components.count {
            if case .blockQuote = $0.kind { true } else { false }
        }
    }

    private static func blockFont(
        _ leaf: PresentationIntent.Kind?, style: MarkdownStyle
    ) -> NSFont {
        switch leaf {
        case .header(let level):
            NSFont.systemFont(ofSize: style.headingSize(level), weight: .semibold)
        case .codeBlock:
            style.monospaced
        default:
            style.body
        }
    }

    private static func blockColour(
        _ leaf: PresentationIntent.Kind?, quoted: Int, style: MarkdownStyle
    ) -> NSColor {
        switch leaf {
        case .codeBlock: style.codeText
        case .thematicBreak: style.secondary
        default: quoted > 0 ? style.secondary : style.text
        }
    }

    private static func paragraphStyle(
        leaf: PresentationIntent.Kind?,
        indent: CGFloat,
        marked: Bool,
        style: MarkdownStyle
    ) -> NSParagraphStyle {
        let paragraph = NSMutableParagraphStyle()
        paragraph.lineSpacing = style.lineSpacing
        paragraph.paragraphSpacing = style.blockSpacing
        paragraph.firstLineHeadIndent = indent
        paragraph.headIndent = indent

        // A heading opens a section, so it wants air above it as well as below.
        if case .header = leaf {
            paragraph.paragraphSpacingBefore = style.blockSpacing
        }

        if case .thematicBreak = leaf {
            paragraph.alignment = .center
        }

        // The marker hangs in the indent its own level added, and a tab puts the
        // text back at the indent — so a wrapped line lines up under the first
        // rather than under the bullet.
        if marked {
            paragraph.firstLineHeadIndent = max(indent - style.indent, 0)
            paragraph.tabStops = [NSTextTab(textAlignment: .left, location: max(indent, 1))]
            paragraph.defaultTabInterval = style.indent
        }

        return paragraph
    }
}

extension NSFont {
    /// This font with `traits` added to whatever it already has.
    ///
    /// Through the descriptor rather than `NSFontManager`, which is main-actor
    /// bound and would isolate the whole renderer to the main actor for the sake
    /// of making one word bold.
    func with(_ traits: NSFontDescriptor.SymbolicTraits) -> NSFont {
        let descriptor = fontDescriptor.withSymbolicTraits(
            fontDescriptor.symbolicTraits.union(traits))

        return NSFont(descriptor: descriptor, size: pointSize) ?? self
    }
}
