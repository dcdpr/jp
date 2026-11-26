// Show raw HTML in an RFD, ticket, or wiki page as text.
//
// These pages are written by `jp`'s tools and by assistants, which name
// placeholders the way help text does: `--id <id>`, `Vec<String>`, `Name
// <email>`. Outside a code span, markdown-it reads each of those as an HTML tag.
// VitePress hands the rendered page to Vue's template compiler, and one element
// that is never closed fails the whole site build with "Element is missing end
// tag".
//
// So every HTML token parsed from the page's source becomes text, except:
//
// - a comment, `<!-- ... -->`, which renders as nothing either way;
// - a self-closing component tag, such as `<TicketBoard />`, which is how the
//   index pages mount their views.
//
// A tag at the start of a line is a block to markdown-it, because VitePress's
// component plugin ends the paragraph there. That block is joined back onto the
// paragraph it interrupted, so the sentence reads as it was written.
//
// The rule has to run before the rules that add HTML of their own (embeds, task
// lists, the metadata header), so their markup is left alone.

// Turn the HTML tokens in `tokens` into text.
//
// `parseInline(content)` returns the inline tokens markdown-it parses from
// `content`, which a paragraph rebuilt from an HTML block needs.
export function escapeHtml(tokens, Token, parseInline) {
    for (let at = 0; at < tokens.length; at++) {
        const token = tokens[at]
        if (token.type !== 'html_block' || isKept(token.content)) continue

        const content = token.content.replace(/\n+$/, '')
        const before = tokens[at - 1]
        const paragraph = tokens[at - 2]

        // The block interrupted the paragraph right above it: put it back.
        if (
            before?.type === 'paragraph_close'
            && paragraph?.type === 'inline'
            && adjacent(paragraph, token)
        ) {
            paragraph.content = `${paragraph.content}\n${content}`
            paragraph.children = parseInline(paragraph.content)
            for (const opened of [paragraph, tokens[at - 3]]) {
                if (opened?.map && token.map) opened.map = [opened.map[0], token.map[1]]
            }
            tokens.splice(at, 1)
            at--
            continue
        }

        tokens.splice(at, 1, ...paragraphOf(Token, token, content, parseInline(content)))
        at += 2
    }

    for (const token of tokens) {
        if (token.type !== 'inline' || !token.children) continue

        for (const child of token.children) {
            if (child.type === 'html_inline' && !isKept(child.content)) child.type = 'text'
        }
    }
}

// Whether `html` is markup the page means: a comment, or a self-closing
// component tag.
function isKept(html) {
    const trimmed = html.trim()
    return trimmed.startsWith('<!--') || /^<[A-Z][A-Za-z0-9]*(\s[^<>]*)?\/>$/.test(trimmed)
}

// Whether `block` starts on the line after the paragraph that `inline` holds
// ends. Without line maps, a block right after a paragraph is taken to have
// interrupted it.
function adjacent(inline, block) {
    if (!inline.map || !block.map) return true
    return inline.map[1] === block.map[0]
}

// A paragraph holding `children`, at the place of `block`.
function paragraphOf(Token, block, content, children) {
    const open = new Token('paragraph_open', 'p', 1)
    const inline = new Token('inline', '', 0)
    const close = new Token('paragraph_close', 'p', -1)

    open.map = block.map
    open.level = block.level
    inline.map = block.map
    inline.level = block.level + 1
    inline.content = content
    inline.children = children
    close.level = block.level

    return [open, inline, close]
}
