// Deterministic linter for RFD documents.
//
// Runs the checks that don't need a model: prose budget, sentence length,
// hedging, AI-slop vocabulary, link hygiene, metadata, leftover template text.
// `just rfd-lint` wraps it; `just rfd-promote` gates on its errors; the RFD
// review cycle runs it after every applied triage round and feeds failures
// back to the agent.
//
// Two severities, and the split is deliberate. Errors gate `just rfd-promote`
// and CI: broken metadata, broken links, leftover template text, and the
// prose budget, which is the one measure that tracks the bloat the pipeline
// exists to prevent. Everything line-level is a warning, because a 31-word
// sentence written last March must not block an RFD from being accepted
// today. Warnings are for the author and the review cycle to act on.
//
// The budget and structure gates only bite while a document is in flight
// (Draft, Discussion). Accepted and later are a permanent record that RFD 001
// says not to churn.
//
// Usage:
//   node docs/.vitepress/rfd-lint.mjs                # every RFD and draft
//   node docs/.vitepress/rfd-lint.mjs 070 D33        # named ids only
//   node docs/.vitepress/rfd-lint.mjs 070 --since HEAD~1
//   node docs/.vitepress/rfd-lint.mjs --summary      # word-count table only
//   node docs/.vitepress/rfd-lint.mjs --errors-only
//   node docs/.vitepress/rfd-lint.mjs --json

import { execFileSync } from 'node:child_process'
import { existsSync, readdirSync, readFileSync } from 'node:fs'
import { dirname, relative, resolve } from 'node:path'

const RFD_DIR = resolve(import.meta.dirname, '../rfd')
const DRAFT_DIR = resolve(import.meta.dirname, '../rfd/drafts')
const REPO_ROOT = resolve(import.meta.dirname, '../..')

// Prose budgets in words, code blocks and the metadata header excluded.
// Mirrored in `.jp/config/knowledge/rfd-writing.toml`; this file is
// authoritative.
const BUDGETS = {
    design: { target: 1200, hard: 2000 },
    decision: { target: 500, hard: 800 },
    guide: { target: 2000, hard: 3500 },
    process: { target: 2000, hard: 3500 },
}

const VALID_STATUS = new Set([
    'Draft', 'Discussion', 'Accepted', 'Implemented', 'Superseded', 'Abandoned',
])
const VALID_CATEGORY = new Set(['Design', 'Decision', 'Guide', 'Process'])

// Statuses where the document is still being shaped. Style findings gate here
// and are advisory everywhere else.
const IN_FLIGHT = new Set(['Draft', 'Discussion'])

const MAX_SENTENCE_WORDS = 30
const MAX_PARAGRAPH_SENTENCES = 6
const MAX_SUMMARY_SENTENCES = 3

// Below this, a repeated sentence is more likely a legitimate short phrase
// ("This is a lint error.") than duplicated content.
const MIN_DUPLICATE_WORDS = 8

// Hedges RFD 001 bans outright. Kept narrow on purpose: every entry here is a
// phrase with no defensible use in a design document.
const HEDGES = [
    /\bit seems\b/i,
    /\bseems (?:like|to be)\b/i,
    /\bprobably\b/i,
    /\barguably\b/i,
    /\bpresumably\b/i,
    /\bperhaps\b/i,
    /\bit m(?:ight|ay) be worth\b/i,
    /\bit could be argued\b/i,
    /\bwe believe\b/i,
    /\bin our opinion\b/i,
    /\bmore or less\b/i,
]

// STE bans these modals because a reader cannot tell a requirement from a
// suggestion. Process and Guide RFDs use "should" normatively, so the rule
// only applies to Design and Decision.
const MODALS = /\b(should|would|may|might)\b/gi

// The never-list from `.jp/config/knowledge/voice.toml`.
const SLOP = [
    /\bcomprehensive\b/i,
    /\bkey insight\b/i,
    /\bload[- ]bearing\b/i,
    /\bhonest (?:take|with you)\b/i,
    /\bbelt and suspenders\b/i,
    /\bsmoking gun\b/i,
    /\b(?:a )?testament to\b/i,
    /\bit'?s important to note\b/i,
    /\bin today'?s .{0,20}landscape\b/i,
    /\bseamless(?:ly)?\b/i,
    /\bleverage[sd]?\b/i,
    /\butilize[sd]?\b/i,
    /\bdelve\b/i,
    /\bshowcase[sd]?\b/i,
]

// Qualifiers that survive deletion without changing the claim.
const FILLER = /\b(significantly|quite|fairly|very|somewhat|essentially|actually|simply|really|basically)\b/gi

const DASH = /[\u2014\u2013]/

// Sentence-ending tokens that are not sentence ends.
const ABBREVIATIONS = /\b(e\.g|i\.e|etc|vs|cf|approx|Dr|Mr|Ms|No|Fig|Sec|Ch)\.$/i

// -- parsing ---------------------------------------------------------------

// Classify every line so the rules can address prose without tripping over
// code, tables, or metadata. Returns lines paired with a kind and, for prose,
// a copy with inline code spans masked out.
function classify(content) {
    const raw = content.split('\n')
    const out = []
    let inCode = false
    let inComment = false
    let inMeta = false
    let seenHeading = false
    let section = null

    for (let i = 0; i < raw.length; i++) {
        const line = raw[i]
        const trimmed = line.trim()
        let kind

        if (/^(?:```|~~~)/.test(trimmed)) {
            out.push({ line: i + 1, text: line, kind: 'fence' })
            inCode = !inCode
            continue
        }
        if (inCode) {
            out.push({ line: i + 1, text: line, kind: 'code' })
            continue
        }

        if (!inComment && trimmed.startsWith('<!--')) {
            inComment = !trimmed.includes('-->')
            out.push({ line: i + 1, text: line, kind: 'comment' })
            continue
        }
        if (inComment) {
            if (trimmed.includes('-->')) inComment = false
            out.push({ line: i + 1, text: line, kind: 'comment' })
            continue
        }

        if (/^# RFD /.test(trimmed)) {
            seenHeading = true
            inMeta = true
            out.push({ line: i + 1, text: line, kind: 'title' })
            continue
        }
        if (inMeta) {
            if (/^- \*\*[A-Za-z ]+\*\*:/.test(trimmed)) {
                out.push({ line: i + 1, text: line, kind: 'meta' })
                continue
            }
            if (trimmed === '') {
                out.push({ line: i + 1, text: line, kind: 'blank' })
                continue
            }
            inMeta = false
        }

        if (/^#{1,6} /.test(trimmed)) section = trimmed.replace(/^#+\s*/, '')

        if (trimmed === '') kind = 'blank'
        else if (/^#{1,6} /.test(trimmed)) kind = 'heading'
        else if (/^\[[^\]]+\]:\s*\S/.test(trimmed)) kind = 'refdef'
        else if (/^\|/.test(trimmed)) kind = 'table'
        else if (/^>/.test(trimmed)) kind = 'quote'
        else if (/^(?:[-*+]|\d+\.)\s/.test(trimmed)) kind = 'list'
        else kind = seenHeading ? 'prose' : 'preamble'

        out.push({ line: i + 1, text: line, kind, section, masked: mask(line) })
    }

    return out
}

// Group consecutive prose lines into blocks, so a sentence that `comfort`
// wrapped across three lines counts as one sentence rather than three.
//
// A blank line, heading, fence, or table row closes the current block. Each
// list item opens its own, because bullet lists carry no blank line between
// items and a bullet is a unit by itself.
function blocks(lines) {
    const out = []
    let current = null
    const close = () => {
        if (current) out.push(current)
        current = null
    }

    for (const l of lines) {
        if (!SENTENCE_KINDS.has(l.kind)) {
            close()
            continue
        }
        const body = l.masked
            .replace(/^[#>\s|*+-]+/, '')
            .replace(/^\d+\.\s*/, '')
        if (l.kind === 'list' || current === null) {
            close()
            current = { line: l.line, kind: l.kind, section: l.section, text: body }
        } else {
            current.text += ` ${body}`
        }
    }
    close()

    return out
}

// Replace inline code spans and link targets with placeholders of the same
// shape, so word counts and sentence splits don't choke on `foo.rs` or a URL.
function mask(text) {
    return text
        .replace(/`[^`]*`/g, 'CODE')
        .replace(/\]\([^)]*\)/g, '](LINK)')
        .replace(/<[^>\s]+>/g, 'TAG')
}

// Everything a reader reads: counted toward the budget and scanned for
// vocabulary findings.
const PROSE_KINDS = new Set(['prose', 'list', 'heading', 'table', 'quote'])

// Where sentence-length and paragraph-length rules apply. Table rows and
// headings are neither sentences nor paragraphs, so they are left out.
const SENTENCE_KINDS = new Set(['prose', 'list', 'quote'])

function countWords(text) {
    return text.split(/\s+/).filter(w => /[A-Za-z0-9]/.test(w)).length
}

// Prose word count: everything a reader reads, minus code blocks, the
// metadata header, reference definitions, and HTML comments.
function proseWords(lines) {
    let total = 0
    for (const l of lines) {
        if (!PROSE_KINDS.has(l.kind)) continue
        total += countWords(l.masked.replace(/^[#>\s|*+-]+/, ''))
    }
    return total
}

function codeLines(lines) {
    return lines.filter(l => l.kind === 'code').length
}

// Split a masked line into sentences. The repository writes one sentence per
// line (semantic linefeeds), so this mostly returns a single element; the
// split exists to catch lines that drifted from the convention.
function sentences(text) {
    const parts = text.split(/(?<=[.!?])\s+(?=["'(\[`A-Z])/)
    const merged = []
    for (const part of parts) {
        const prev = merged[merged.length - 1]
        if (prev !== undefined && ABBREVIATIONS.test(prev)) {
            merged[merged.length - 1] = `${prev} ${part}`
            continue
        }
        merged.push(part)
    }
    return merged.filter(s => s.trim() !== '')
}

function parseMetaFields(content) {
    const field = key =>
        content.match(new RegExp(`^- \\*\\*${key}\\*\\*:\\s*(.+)`, 'm'))?.[1]?.trim() ?? null
    return {
        status: field('Status'),
        category: field('Category'),
        authors: field('Authors'),
        date: field('Date'),
        overBudget: field('Over budget'),
    }
}

// -- rules -----------------------------------------------------------------

function lintFile(path, content) {
    const rel = relative(REPO_ROOT, path)
    const basename = path.split('/').pop()
    const findings = []
    const meta = parseMetaFields(content)
    const lines = classify(content)
    const words = proseWords(lines)
    const code = codeLines(lines)

    const status = meta.status ?? 'Draft'
    const category = (meta.category ?? 'design').toLowerCase()
    const inFlight = IN_FLIGHT.has(status)

    // Budget and structure gate a document that is still cheap to change.
    const gate = inFlight ? 'error' : 'warn'

    const add = (line, rule, severity, message) =>
        findings.push({ file: rel, line, rule, severity, message })

    // --- metadata
    if (!meta.status) add(1, 'meta', 'error', 'missing `- **Status**:` field')
    else if (!VALID_STATUS.has(meta.status)) {
        add(1, 'meta', 'error', `unknown status '${meta.status}'`)
    }
    if (!meta.category) add(1, 'meta', 'error', 'missing `- **Category**:` field')
    else if (!VALID_CATEGORY.has(meta.category)) {
        add(1, 'meta', 'error', `unknown category '${meta.category}'`)
    }
    if (!meta.authors) add(1, 'meta', 'error', 'missing `- **Authors**:` field')
    if (!/^\d{4}-\d{2}-\d{2}$/.test(meta.date ?? '')) {
        add(1, 'meta', 'error', `\`Date\` must be YYYY-MM-DD, found '${meta.date ?? ''}'`)
    }

    // --- identity: filename, heading, and id must agree. These drift when a
    // renumber or promotion rewrites one of the three and not the others.
    const fileId = basename.match(/^(\d{3}|D\d{2})-/)?.[1] ?? null
    const headingLine = lines.find(l => l.kind === 'title')
    const headingId = headingLine?.text.match(/^# RFD (\d+|D\d+):/)?.[1] ?? null
    if (!headingLine) {
        add(1, 'identity', 'error', 'no `# RFD <id>: <title>` heading')
    } else if (fileId && headingId !== fileId) {
        add(headingLine.line, 'identity', 'error',
            `heading says RFD ${headingId}, filename says ${fileId}.`)
    }

    // --- budget
    const budget = BUDGETS[category] ?? BUDGETS.design
    if (words > budget.hard && !meta.overBudget) {
        add(1, 'budget', gate,
            `${words} prose words, hard limit ${budget.hard} for ${category}. ` +
            `Cut ${words - budget.hard}, split the RFD, run ` +
            `\`just rfd-prose\`, or promote with a recorded reason: ` +
            `\`just rfd-promote <id> "why"\`.`)
    } else if (words > budget.target) {
        add(1, 'budget', 'warn',
            `${words} prose words, target ${budget.target} for ${category} ` +
            `(hard limit ${budget.hard}).`)
    }

    // --- sentence-shaped rules, over joined blocks rather than raw lines.
    //
    // `seen` collects normalised sentences so duplicated content can be
    // reported afterwards. Repetition is the signature of a bloated RFD:
    // Summary, Motivation, and Design all reach for the same explanation.
    const seen = new Map()
    let summarySentences = 0

    for (const b of blocks(lines)) {
        const sents = sentences(b.text)

        for (const s of sents) {
            const n = countWords(s)
            if (n > MAX_SENTENCE_WORDS) {
                add(b.line, 'sentence', 'warn',
                    `sentence runs ${n} words, limit ${MAX_SENTENCE_WORDS}.`)
            }
            if (n >= MIN_DUPLICATE_WORDS) {
                const key = s.toLowerCase().replace(/\s+/g, ' ')
                    .replace(/[.,;:!?]+$/, '').trim()
                if (!seen.has(key)) seen.set(key, [])
                seen.get(key).push(b.line)
            }
        }

        if (b.kind === 'prose' && sents.length > MAX_PARAGRAPH_SENTENCES) {
            add(b.line, 'paragraph', 'warn',
                `paragraph runs ${sents.length} sentences, limit ` +
                `${MAX_PARAGRAPH_SENTENCES}. Split it or cut it.`)
        }

        if (b.section === 'Summary') summarySentences += sents.length
    }

    // --- vocabulary rules, line by line, so the line numbers are exact.
    const dashes = []

    for (const l of lines) {
        if (!PROSE_KINDS.has(l.kind)) continue

        const body = l.masked.replace(/^[#>\s|*+-]+/, '').replace(/^\d+\.\s*/, '')

        for (const re of HEDGES) {
            const m = body.match(re)
            if (m) add(l.line, 'hedge', 'warn', `hedging: '${m[0]}'`)
        }

        for (const re of SLOP) {
            const m = body.match(re)
            if (m) add(l.line, 'slop', 'warn', `banned vocabulary: '${m[0]}'`)
        }

        if (category === 'design' || category === 'decision') {
            for (const m of body.matchAll(MODALS)) {
                add(l.line, 'modal', 'warn',
                    `'${m[0]}' hides whether this is a requirement. Use ` +
                    `'can', 'will', or 'must'.`)
            }
        }

        for (const m of body.matchAll(FILLER)) {
            add(l.line, 'filler', 'warn', `'${m[0]}' can be deleted.`)
        }

        if (DASH.test(l.text)) dashes.push(l.line)

        if (/^#{4,} /.test(l.text.trim())) {
            add(l.line, 'depth', 'warn',
                'heading deeper than `###`. Flatten the section or split the RFD.')
        }
    }

    // One finding, not one per occurrence. A document that predates the rule
    // has dozens, and forty-five identical warnings bury everything else in
    // the report.
    if (dashes.length > 0) {
        const shown = dashes.slice(0, 12).join(', ')
        const rest = dashes.length > 12 ? `, +${dashes.length - 12} more` : ''
        add(dashes[0], 'dash', 'warn',
            `${dashes.length} em/en dash(es). Use a comma, period, colon, or ` +
            `parentheses. Lines: ${shown}${rest}`)
    }

    if (summarySentences > MAX_SUMMARY_SENTENCES) {
        add(1, 'summary', 'warn',
            `Summary runs ${summarySentences} sentences, limit ` +
            `${MAX_SUMMARY_SENTENCES}. A reader who stops after it has to ` +
            `understand the proposal.`)
    }

    for (const [, at] of seen) {
        if (at.length < 2) continue
        add(at[0], 'duplicate', 'warn',
            `sentence repeated at line(s) ${at.slice(1).join(', ')}. ` +
            `Say it once, where a reader will look for it.`)
    }

    // --- leftover draft scaffolding
    if (status !== 'Draft') {
        for (const l of lines) {
            if (l.kind === 'comment') {
                add(l.line, 'template', 'error',
                    'HTML comment left in a promoted RFD.')
                break
            }
        }
        for (const l of lines) {
            if (PROSE_KINDS.has(l.kind) && /\b(TODO|FIXME|TBD|XXX)\b/.test(l.text)) {
                add(l.line, 'template', 'error', 'leftover marker in a promoted RFD.')
            }
        }
    }

    // --- structure
    if (category === 'design' && !/^#{2,3} Non-Goals/m.test(content)) {
        add(1, 'nongoals', gate,
            'Design RFDs need a `## Non-Goals` section. The review protocol ' +
            'treats it as the binding scope contract.')
    }

    findings.push(...lintLinks(rel, path, lines))

    return { file: rel, meta, status, category, words, code, budget, findings }
}

// Link hygiene: RFD cross-references use reference style, every label used has
// a definition, and every relative target exists on disk.
function lintLinks(rel, path, lines) {
    const findings = []
    const add = (line, rule, severity, message) =>
        findings.push({ file: rel, line, rule, severity, message })

    const defined = new Set()
    for (const l of lines) {
        const m = l.kind === 'refdef' ? l.text.trim().match(/^\[([^\]]+)\]:\s*(\S+)/) : null
        if (!m) continue
        defined.add(m[1].toLowerCase())

        const target = m[2]
        if (/^[a-z]+:/i.test(target) || target.startsWith('#')) continue
        const [file] = target.split('#')
        if (file === '') continue
        if (!existsSync(resolve(dirname(path), file))) {
            add(l.line, 'link-dead', 'error', `reference target not found: ${target}`)
        }
    }

    for (const l of lines) {
        if (!PROSE_KINDS.has(l.kind)) continue

        // Inline links to other RFDs bypass the reference-link convention and
        // break silently when a file is renumbered.
        for (const m of l.masked.matchAll(/\[([^\]]+)\]\(LINK\)/g)) {
            if (/^RFD\s+\d/i.test(m[1])) {
                add(l.line, 'link-inline', 'warn',
                    `inline link for '${m[1]}'. Use reference style and define ` +
                    'the target at the bottom.')
            }
        }

        // `[RFD NNN]` and `[label]` shortcut references must resolve.
        for (const m of l.masked.matchAll(/\[([^\]\n]+)\](?![([:])/g)) {
            const label = m[1]
            if (/^[!x ]$/.test(label) || label.startsWith('!')) continue
            if (!/^RFD\s+(?:\d{3}|D\d{2})$/i.test(label)) continue
            if (!defined.has(label.toLowerCase())) {
                add(l.line, 'link-undefined', 'error',
                    `'[${label}]' has no reference definition.`)
            }
        }
    }

    return findings
}

// -- word-count delta ------------------------------------------------------

function wordsAtRef(ref, relPath) {
    try {
        const prev = execFileSync('git', ['show', `${ref}:${relPath}`], {
            cwd: REPO_ROOT,
            encoding: 'utf-8',
            stdio: ['ignore', 'pipe', 'ignore'],
        })
        return proseWords(classify(prev))
    } catch {
        return null
    }
}

// -- cli -------------------------------------------------------------------

const argv = process.argv.slice(2)
const flags = new Set(argv.filter(a => a.startsWith('--')))
const sinceIdx = argv.indexOf('--since')
const since = sinceIdx === -1 ? null : argv[sinceIdx + 1]
const ids = argv.filter((a, i) =>
    !a.startsWith('--') && (sinceIdx === -1 || i !== sinceIdx + 1))

if (sinceIdx !== -1 && !since) {
    process.stderr.write('rfd-lint: --since needs a git revision\n')
    process.exit(1)
}

function collect() {
    const files = []
    for (const [dir, re] of [[RFD_DIR, /^\d{3}-.+\.md$/], [DRAFT_DIR, /^D\d{2}-.+\.md$/]]) {
        for (const f of readdirSync(dir).sort()) {
            if (!re.test(f) || f.startsWith('000-')) continue
            files.push(resolve(dir, f))
        }
    }
    if (ids.length === 0) return files

    const wanted = ids.map(id =>
        /^d\d+$/i.test(id) ? id.toUpperCase() : String(id).padStart(3, '0'))
    const picked = []
    for (const id of wanted) {
        const hit = files.find(f => f.split('/').pop().startsWith(`${id}-`))
        if (!hit) {
            process.stderr.write(`rfd-lint: no RFD found for '${id}'\n`)
            process.exit(1)
        }
        picked.push(hit)
    }
    return picked
}

const reports = collect().map(p => {
    const report = lintFile(p, readFileSync(p, 'utf-8'))
    if (since) {
        const before = wordsAtRef(since, report.file)
        report.delta = before === null ? null : report.words - before
    }
    return report
})

const errorsOnly = flags.has('--errors-only')
const shown = reports.map(r => ({
    ...r,
    findings: errorsOnly ? r.findings.filter(f => f.severity === 'error') : r.findings,
}))

const errorCount = reports.reduce(
    (n, r) => n + r.findings.filter(f => f.severity === 'error').length, 0)
const warnCount = reports.reduce(
    (n, r) => n + r.findings.filter(f => f.severity === 'warn').length, 0)

if (flags.has('--json')) {
    process.stdout.write(JSON.stringify({
        errors: errorCount,
        warnings: warnCount,
        files: shown.map(r => ({
            file: r.file,
            status: r.status,
            category: r.category,
            words: r.words,
            codeLines: r.code,
            target: r.budget.target,
            hard: r.budget.hard,
            delta: r.delta ?? null,
            findings: r.findings,
        })),
    }, null, 2) + '\n')
    process.exit(errorCount > 0 ? 1 : 0)
}

if (flags.has('--summary')) {
    const rows = reports
        .slice()
        .sort((a, b) => b.words - a.words)
        .map(r => {
            const over = r.words > r.budget.hard ? '!!'
                : r.words > r.budget.target ? ' !' : '  '
            const delta = r.delta === null || r.delta === undefined ? ''
                : `  ${r.delta >= 0 ? '+' : ''}${r.delta}`
            const id = r.file.split('/').pop().slice(0, 4).replace(/-$/, '')
            return `${over} ${id}  ${String(r.words).padStart(5)}` +
                `/${String(r.budget.target).padEnd(4)}  ${r.category.padEnd(8)}` +
                `  ${r.status.padEnd(11)}${delta}`
        })
    process.stdout.write(
        `   id    words/target  category  status\n${rows.join('\n')}\n`)
    process.exit(0)
}

for (const r of shown) {
    if (r.findings.length === 0 && r.delta === undefined) continue
    const head = r.delta === undefined || r.delta === null
        ? `${r.file}  (${r.words} words, target ${r.budget.target})`
        : `${r.file}  (${r.words} words, target ${r.budget.target}, ` +
          `${r.delta >= 0 ? '+' : ''}${r.delta} since ${since})`
    process.stdout.write(`\n${head}\n`)
    for (const f of r.findings) {
        const tag = f.severity === 'error' ? 'error' : 'warn '
        process.stdout.write(`  ${tag} ${f.file}:${f.line}: [${f.rule}] ${f.message}\n`)
    }
}

process.stdout.write(
    `\n${errorCount} error(s), ${warnCount} warning(s) across ` +
    `${reports.length} RFD(s).\n`)

process.exit(errorCount > 0 ? 1 : 0)
