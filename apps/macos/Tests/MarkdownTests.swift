import AppKit
import Testing

@testable import JP

/// What markdown turns into, as text a TextKit view draws.
///
/// The style is fixed rather than taken from ``Theme``, so every number and
/// colour asserted below is one this file states.
@Suite("Markdown")
struct MarkdownTests {
    /// Distinct, obviously-not-real colours, so an assertion says which one was
    /// applied rather than which appearance was resolved.
    var style: MarkdownStyle {
        MarkdownStyle(
            body: .systemFont(ofSize: 14),
            monospaced: .monospacedSystemFont(ofSize: 13, weight: .regular),
            text: ThemeColor.srgb(0x11_1111),
            secondary: ThemeColor.srgb(0x88_8888),
            codeBackground: ThemeColor.srgb(0xEE_EEEE),
            codeText: ThemeColor.srgb(0x22_2222),
            link: ThemeColor.srgb(0x00_00FF),
            indent: 20,
            tableColumnWidth: 100,
            blockSpacing: 10,
            lineSpacing: 3,
            eventSpacing: 18,
            turnSpacing: 40
        )
    }

    private func render(_ source: String) -> NSAttributedString {
        Markdown.attributed(source, style: style)
    }

    private func paragraph(of rendered: NSAttributedString, at index: Int) -> NSParagraphStyle?
    {
        rendered.attribute(.paragraphStyle, at: index, effectiveRange: nil) as? NSParagraphStyle
    }

    private func font(of rendered: NSAttributedString, at index: Int) -> NSFont? {
        rendered.attribute(.font, at: index, effectiveRange: nil) as? NSFont
    }

    /// Foundation's parser returns the blocks with nothing between them, so the
    /// separators are the renderer's to put back. Without this a heading runs
    /// into the paragraph under it.
    @Test("separates blocks with newlines and leaves none trailing")
    func separatesBlocks() {
        let rendered = render(
            """
            # Heading

            First paragraph.

            Second paragraph.
            """)

        #expect(rendered.string == "Heading\nFirst paragraph.\nSecond paragraph.")
    }

    @Test("draws a bullet before each item of an unordered list")
    func drawsBullets() {
        #expect(render("- first\n- second").string == "•\tfirst\n•\tsecond")
    }

    @Test("numbers the items of an ordered list")
    func numbersOrderedItems() {
        #expect(render("1. first\n2. second").string == "1.\tfirst\n2.\tsecond")
    }

    /// The number is the one written in the source, not the item's position:
    /// a list starting at 3 is displayed starting at 3.
    @Test("keeps the ordinal the source gave an item")
    func keepsSourceOrdinals() {
        #expect(render("3. third\n4. fourth").string == "3.\tthird\n4.\tfourth")
    }

    /// The marker hangs in the indent its own level added and the text sits at
    /// the indent, so a wrapped line lines up under the first rather than under
    /// the bullet.
    @Test("hangs a list marker outside the text it labels")
    func hangsTheMarker() {
        let rendered = render("- item")
        let paragraph = paragraph(of: rendered, at: 0)

        #expect(paragraph?.firstLineHeadIndent == 0)
        #expect(paragraph?.headIndent == 20)
        #expect(paragraph?.tabStops.first?.location == 20)
    }

    @Test("indents a nested list one level further")
    func indentsNestedLists() {
        let rendered = render("- outer\n  - inner")
        let inner = rendered.string.distance(
            from: rendered.string.startIndex,
            to: rendered.string.range(of: "inner")?.lowerBound ?? rendered.string.startIndex
        )

        #expect(paragraph(of: rendered, at: inner)?.headIndent == 40)
    }

    @Test("draws a heading larger than body text, and in bold")
    func drawsHeadings() {
        let first = font(of: render("# One"), at: 0)
        let third = font(of: render("### Three"), at: 0)

        #expect(first?.pointSize == 22)
        #expect(third?.pointSize == 16)
        #expect(first?.fontDescriptor.symbolicTraits.contains(.bold) == true)
    }

    /// A heading past the third is the body size in bold: another distinct size
    /// would be a difference nobody can see.
    @Test("draws a deep heading at body size")
    func drawsDeepHeadingsAtBodySize() {
        #expect(font(of: render("##### Five"), at: 0)?.pointSize == 14)
    }

    @Test("applies emphasis to the emphasized run alone")
    func appliesEmphasis() {
        let rendered = render("plain **bold** plain")
        let bold = 6

        #expect(rendered.string == "plain bold plain")
        #expect(
            font(of: rendered, at: bold)?.fontDescriptor.symbolicTraits.contains(.bold) == true)
        #expect(
            font(of: rendered, at: 0)?.fontDescriptor.symbolicTraits.contains(.bold) == false)
    }

    @Test("sets an inline code span in the monospaced font, on the code background")
    func stylesInlineCode() {
        let rendered = render("run `jp query` now")
        let code = 4

        #expect(rendered.string == "run jp query now")
        #expect(font(of: rendered, at: code) == style.monospaced)
        #expect(
            rendered.attribute(.backgroundColor, at: code, effectiveRange: nil) as? NSColor
                == style.codeBackground
        )
        // The prose either side keeps the body font and no background.
        #expect(font(of: rendered, at: 0) == style.body)
        #expect(rendered.attribute(.backgroundColor, at: 0, effectiveRange: nil) == nil)
    }

    /// A fenced block keeps its own newlines, and loses the one before the
    /// closing fence — which would otherwise draw an empty last line inside the
    /// block's background.
    @Test("keeps a code block's lines and drops the fence's trailing newline")
    func stylesCodeBlocks() {
        let rendered = render("```swift\nlet x = 1\nprint(x)\n```")

        #expect(rendered.string == "let x = 1\nprint(x)")
        #expect(font(of: rendered, at: 0) == style.monospaced)
        #expect(
            rendered.attribute(.backgroundColor, at: rendered.length - 1, effectiveRange: nil)
                as? NSColor == style.codeBackground
        )
    }

    @Test("carries a link's destination, colour and underline")
    func stylesLinks() {
        let rendered = render("see [the docs](https://example.com/x) for more")
        let link = 4

        #expect(rendered.string == "see the docs for more")
        #expect(
            rendered.attribute(.link, at: link, effectiveRange: nil) as? URL
                == URL(string: "https://example.com/x")
        )
        #expect(
            rendered.attribute(.foregroundColor, at: link, effectiveRange: nil) as? NSColor
                == style.link
        )
    }

    @Test("indents a block quote and dims it")
    func stylesBlockQuotes() {
        let rendered = render("> quoted")

        #expect(rendered.string == "quoted")
        #expect(paragraph(of: rendered, at: 0)?.headIndent == 20)
        #expect(
            rendered.attribute(.foregroundColor, at: 0, effectiveRange: nil) as? NSColor
                == style.secondary
        )
    }

    /// A soft break inside a paragraph is a space, per CommonMark, and the
    /// terminal renderer reflows the same way. A hard break is a new line.
    @Test("reflows a soft break and honours a hard one")
    func handlesLineBreaks() {
        #expect(render("one\ntwo").string == "one two")
        #expect(render("one  \ntwo").string == "one\ntwo")
    }

    /// A row is one line of tab-separated cells, not one line per cell. The tab is
    /// what carries a cell to its column, so its presence is the assertion.
    @Test("lays a table row out as one line of tab-separated cells")
    func laysOutTableRows() {
        let rendered = render(
            """
            | container | samples |
            | --- | --- |
            | VStack | 2442 |
            """)

        #expect(rendered.string == "container\tsamples\nVStack\t2442")
    }

    /// One stop per column boundary, so the second cell starts where the second
    /// column does. The first needs none: it starts at the paragraph's own edge.
    @Test("puts a tab stop at each column boundary")
    func stopsAtColumnBoundaries() {
        let rendered = render("| a | b | c |\n| --- | --- | --- |\n| 1 | 2 | 3 |")

        #expect(paragraph(of: rendered, at: 0)?.tabStops.map(\.location) == [100, 200])
    }

    /// The one piece of table styling the source states outright. `---:` in the
    /// separator row right-aligns a column, and Foundation reports it, so a column
    /// of numbers lines up on its digits.
    @Test("takes each column's alignment from the source")
    func alignsColumnsAsWritten() {
        let rendered = render("| a | b | c |\n| :-- | :-: | --: |\n| 1 | 2 | 3 |")
        let stops = paragraph(of: rendered, at: 0)?.tabStops

        // The first column has no stop, so these are columns two and three.
        #expect(stops?.map(\.alignment) == [.center, .right])
    }

    @Test("draws a table's header row in bold and its body rows plain")
    func boldsTheHeaderRow() {
        let rendered = render("| head |\n| --- |\n| body |")
        let body = rendered.string.distance(
            from: rendered.string.startIndex,
            to: rendered.string.range(of: "body")?.lowerBound ?? rendered.string.startIndex
        )

        #expect(
            font(of: rendered, at: 0)?.fontDescriptor.symbolicTraits.contains(.bold) == true)
        #expect(
            font(of: rendered, at: body)?.fontDescriptor.symbolicTraits.contains(.bold) == false
        )
    }

    /// Inline styling inside a cell survives, which is what says the cells go
    /// through the same run walk as prose rather than being flattened to plain
    /// text on the way into a row.
    @Test("keeps inline styling inside a cell")
    func stylesInsideCells() {
        let rendered = render("| a |\n| --- |\n| `code` |")
        let cell = rendered.string.distance(
            from: rendered.string.startIndex,
            to: rendered.string.range(of: "code")?.lowerBound ?? rendered.string.startIndex
        )

        #expect(font(of: rendered, at: cell) == style.monospaced)
    }

    /// A table is one block, so the rows sit against each other and the spacing
    /// belongs to whatever follows.
    @Test("separates a table from the prose around it")
    func separatesTablesFromProse() {
        let rendered = render("before\n\n| a |\n| --- |\n| 1 |\n\nafter")

        #expect(rendered.string == "before\na\n1\nafter")
    }

    @Test("renders text that is not markdown as itself")
    func rendersPlainText() {
        #expect(render("just a sentence.").string == "just a sentence.")
        #expect(render("").string == "")
    }
}
