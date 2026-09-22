// Parsing for the `- **Key**: Value` metadata blocks that open RFD and ticket
// documents.
//
// Both kinds use the same idiom, for the same reason: it parses with a regex and
// renders as visible content (see RFD 001 and RFD 100). This module is what they
// share. No Node imports, so browser-side components can use it too.

// The metadata block: the run of `- **Key**: value` lines, and their wrapped
// continuations, that opens the document.
//
// The readers below are bounded to it. A whole-document scan would also find
// metadata lines quoted in a code block — RFD 001 documents the format that
// way — and a document whose header omits a field the example carries would
// read the example as its own.
function header(content) {
    const lines = content.split('\n')

    let start = -1
    for (const [i, line] of lines.entries()) {
        if (line.startsWith('## ')) break
        if (line.startsWith('- **')) {
            start = i
            break
        }
    }
    if (start === -1) return ''

    let end = start
    while (end < lines.length && lines[end].trim() !== '') end++
    return lines.slice(start, end).join('\n')
}

// Read one field from a document's metadata block. `null` when it has none.
export function field(content, key) {
    const pattern = `^- \\*\\*${key}\\*\\*:\\s*(.+)`
    return header(content).match(new RegExp(pattern, 'm'))?.[1].trim() ?? null
}

// Read one field whose value is long enough to wrap.
//
// `comfort` reflows a metadata value like any other paragraph, continuing it on
// lines indented past the list marker. Those lines are folded back into a
// single space-separated string, so a caller sees the value the author wrote
// rather than the shape the formatter left it in.
export function wrappedField(content, key) {
    const pattern = `^- \\*\\*${key}\\*\\*:\\s*(.+(?:\\n +\\S.*)*)`
    const value = header(content).match(new RegExp(pattern, 'm'))?.[1]
    return value?.replace(/\s+/g, ' ').trim() ?? null
}
