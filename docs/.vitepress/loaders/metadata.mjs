// Parsing for the `- **Key**: Value` metadata blocks that open RFD and ticket
// documents.
//
// Both kinds use the same idiom, for the same reason: it parses with a regex and
// renders as visible content (see RFD 001 and RFD 100). This module is what they
// share. No Node imports, so browser-side components can use it too.

// Read one field from a document's metadata block.
//
// Returns the first match anywhere in the document, so a document quoting a
// metadata line inside a code block can shadow its own header. Both document
// kinds keep their real block at the top, ahead of any such example.
export function field(content, key) {
    return content.match(new RegExp(`^- \\*\\*${key}\\*\\*:\\s*(.+)`, 'm'))?.[1]?.trim() ?? null
}

// Undo markdown escaping in a heading.
//
// Titles are consumed as plain text (CLI lists, boards, indexes, tooltips), but
// the markdown source escapes punctuation like `ask\_user` to avoid emphasis.
export function unescapeTitle(raw) {
    return raw.replace(/\\([^A-Za-z0-9])/g, '$1')
}
