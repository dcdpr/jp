// Collapse newlines inside inline backtick code spans to a single space.
//
// VitePress pulls in `@mdit-vue/plugin-component`, which forks markdown-it's
// `html_block` rule so that any unknown tag at the start of a line is treated
// as a component tag and terminates the current paragraph. That breaks a
// perfectly valid inline code span when it happens to wrap mid-line:
//
//     ... for `--cfg
//     <name>` baz
//
// Line 2 starts with `<name>`, the forked block rule fires, the paragraph
// ends after line 1, and `<name>` leaks through as raw HTML. Vue's template
// compiler then blows up with "Element is missing end tag."
//
// CommonMark already says line endings inside a code span are equivalent to
// spaces, so collapsing them in the source preserves semantics and sidesteps
// the block-rule misfire. We operate on the raw source before block parsing,
// via `md.core.ruler.after('normalize', ...)`.
//
// Fenced code blocks (``` or ~~~) are passed through untouched. Spans that
// cross block-syntax boundaries (blockquote markers, list markers, headings)
// are left alone, since collapsing them would change interpretation.

export function joinMultilineInlineCode(src: string): string {
    const fences = findFenceRanges(src)
    const out: string[] = []
    let pos = 0
    for (const [start, end] of fences) {
        if (pos < start) out.push(collapseInlineBackticks(src.slice(pos, start)))
        out.push(src.slice(start, end))
        pos = end
    }
    if (pos < src.length) out.push(collapseInlineBackticks(src.slice(pos)))
    return out.join('')
}

function collapseInlineBackticks(src: string): string {
    let out = ''
    let pos = 0
    while (pos < src.length) {
        if (src[pos] !== '`') {
            out += src[pos]
            pos++
            continue
        }
        const openerStart = pos
        while (pos < src.length && src[pos] === '`') pos++
        const openerLen = pos - openerStart

        const closer = findCloser(src, pos, openerLen)
        if (closer === -1) {
            // Unmatched opener — emit literally.
            out += src.slice(openerStart, pos)
            continue
        }

        const content = src.slice(pos, closer)
        if (spansBlockSyntax(content)) {
            // Collapsing would change or leak block syntax (e.g. a `>` on a
            // continuation line would become part of the code content).
            out += src.slice(openerStart, closer + openerLen)
        } else {
            out += src.slice(openerStart, pos)
            out += content.replace(/\n/g, ' ')
            out += src.slice(closer, closer + openerLen)
        }
        pos = closer + openerLen
    }
    return out
}

// Find the start of a closing backtick run of the given length, starting from
// `from`. Returns -1 if not found before a paragraph break (blank line) or
// end of input.
function findCloser(src: string, from: number, length: number): number {
    let i = from
    while (i < src.length) {
        if (src[i] === '\n') {
            // Paragraph break ends the search — inline code never crosses one.
            let j = i + 1
            while (j < src.length && (src[j] === ' ' || src[j] === '\t')) j++
            if (j >= src.length || src[j] === '\n') return -1
        }
        if (src[i] !== '`') {
            i++
            continue
        }
        const runStart = i
        while (i < src.length && src[i] === '`') i++
        if (i - runStart === length) return runStart
    }
    return -1
}

function spansBlockSyntax(content: string): boolean {
    return (
        /\n[ \t]{0,3}[>#]/.test(content) ||
        /\n[ \t]{0,3}[-*+] /.test(content) ||
        /\n[ \t]{0,3}\d+[.)] /.test(content)
    )
}

// Fenced code block ranges as [start, endExclusive] byte offsets. Unclosed
// fences extend to end of input, mirroring markdown-it's behavior.
function findFenceRanges(src: string): Array<[number, number]> {
    const ranges: Array<[number, number]> = []
    const lines = src.split('\n')
    let offset = 0
    let open: { char: string; len: number; start: number } | null = null
    for (const line of lines) {
        const lineEnd = offset + line.length
        const m = line.match(/^ {0,3}(`{3,}|~{3,})/)
        if (m) {
            const marker = m[1]
            if (open === null) {
                open = { char: marker[0], len: marker.length, start: offset }
            } else if (
                marker[0] === open.char &&
                marker.length >= open.len &&
                /^\s*$/.test(line.slice(m[0].length))
            ) {
                ranges.push([open.start, Math.min(lineEnd + 1, src.length)])
                open = null
            }
        }
        offset = lineEnd + 1
    }
    if (open !== null) ranges.push([open.start, src.length])
    return ranges
}
