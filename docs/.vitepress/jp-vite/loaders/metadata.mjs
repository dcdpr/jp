// Reading the `# Title` and `- **Key**: Value` metadata block that open RFD,
// ticket, and wiki documents.
//
// Every kind uses the same idiom, for the same reason: it parses with a regex
// and renders as visible content (see RFD 001 and RFD 100). This is the one
// reader the site uses for all three, and it reads the block as `jp` does
// (`doc::metadata` in Rust): keys without case, wrapped values whole, and
// what a formatter escaped unescaped. No Node imports, so browser-side
// components can use it too.

// A document's title and metadata, read once:
//
// - `title`: the text of the `# Title` line the document opens with, or
//   `null`.
// - `get(key)`: the field's value, or `null` when the block has none or it is
//   empty. A field named twice gives its first value.
// - `all(key)`: every value of a field the block repeats, such as `Label`, in
//   order.
// - `body`: what follows the title and the block, trimmed: the document's own
//   text.
export function readMetadata(content) {
    const lines = content.replace(/\r\n?/g, '\n').split('\n')
    const { fields, end } = block(lines)
    const heading = title(lines)

    const values = key => fields
        .filter(field => field.key.toLowerCase() === key.toLowerCase())
        .map(field => field.value)
        .filter(Boolean)

    // Without a block, the text starts after the title, when there is one.
    const first = lines.findIndex(line => line.trim() !== '')
    const from = end ?? (heading === null ? 0 : first + 1)

    return {
        title: heading,
        get: key => values(key)[0] ?? null,
        all: values,
        body: lines.slice(from).join('\n').trim(),
    }
}

// The text of the `# Title` heading, when the first line that isn't blank is
// one.
function title(lines) {
    const first = lines.find(line => line.trim() !== '')

    return first?.startsWith('# ') ? first.slice(2).trim() || null : null
}

const FIELD = /^- \*\*([^*]+)\*\*:[ \t]*(.*)$/

// The fields of the metadata block: the run of `- **Key**: value` lines, and
// their continuations, from the first such line before any section heading to
// the blank line that closes it.
//
// Bounded to the block: a whole-document scan would also find metadata lines
// quoted in a code block (RFD 001 documents the format that way), and a
// document whose header omits a field the example carries would read the
// example as its own.
//
// `comfort` reflows a long value like any other paragraph, continuing it on the
// next lines; a line at the margin continues it too, as markdown reads a list
// item. Each value is folded onto one line, so a caller sees what the author
// wrote rather than the shape the formatter left it in.
//
// Returns the fields, and `end`, the index of the line after the block; `null`
// when there is no block.
function block(lines) {
    let start = -1
    for (const [index, line] of lines.entries()) {
        if (line.startsWith('## ')) break
        if (FIELD.test(line)) {
            start = index
            break
        }
    }
    if (start === -1) return { fields: [], end: null }

    const fields = []
    let end = start
    for (; end < lines.length && lines[end].trim() !== ''; end++) {
        const opened = lines[end].match(FIELD)
        if (opened) {
            fields.push({ key: opened[1].trim(), parts: [opened[2]] })
        } else {
            fields.at(-1).parts.push(lines[end])
        }
    }

    return {
        fields: fields.map(({ key, parts }) => ({ key, value: unescape(parts.join(' ')) })),
        end,
    }
}

// A value as written, folded onto one line, with what a formatter escapes
// (`jp\_config` for `jp_config`) undone.
function unescape(text) {
    return text.replace(/\s+/g, ' ').trim().replace(/\\([!-/:-@[-`{-~])/g, '$1')
}
