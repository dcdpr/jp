// CLI companion to the web priority board (`/rfd/priority`).
//
// Both surfaces share `loaders/rfd-shared.mjs` so the board a human drags on
// the site and the list `just rfd-list` prints can't disagree.
//
// Usage:
//   just rfd-list                 # planned RFDs, in priority order (default)
//   just rfd-list --backlog       # the unranked backlog instead
//   just rfd-list --planned --backlog
//   just rfd-list --all           # planned + backlog + terminal/implemented
//   just rfd-list --full          # add summaries and dependencies
//   just rfd-list design          # filter to the "design" category
//   just rfd-list --json          # every entry, tagged, for `jq` etc.

import { assembleBoard } from './loaders/rfd-shared.mjs'

const args = process.argv.slice(2)
const flags = new Set(args.filter(a => a.startsWith('--')))
const asJson = flags.has('--json')
const full = flags.has('--full')
const category = args.find(a => !a.startsWith('-'))?.toLowerCase() ?? null

// Sections are opt-in. With no section flag we default to the planned list.
const showAll = flags.has('--all')
const anySection = flags.has('--planned') || flags.has('--backlog') || showAll
const wantPlanned = showAll || flags.has('--planned') || !anySection
const wantBacklog = showAll || flags.has('--backlog')
const wantOther = showAll

let board
try {
    board = assembleBoard()
} catch (err) {
    process.stderr.write(`rfd-list: ${err.message}\n`)
    process.exit(1)
}

const { entries, priority } = board
const cutoff = priority.order.length

const shown = category
    ? entries.filter(e => e.category?.toLowerCase() === category)
    : entries

// Planned (ranked `order`), backlog (on board, below the cutoff), and
// everything else (terminal or never placed).
const planned = shown
    .filter(e => e.priority !== null && e.priority < cutoff)
    .sort((a, b) => a.priority - b.priority)
const backlog = shown
    .filter(e => e.priority !== null && e.priority >= cutoff)
    .sort((a, b) => a.priority - b.priority)
const other = shown
    .filter(e => e.priority === null)
    .sort((a, b) => a.num.localeCompare(b.num))

if (asJson) {
    // Section flags are a text concern; JSON always emits every group so a
    // `jq` consumer can filter on the `group`/`rank` tags itself.
    const tagged = [
        ...planned.map((e, i) => ({ ...e, group: 'planned', rank: i + 1 })),
        ...backlog.map(e => ({ ...e, group: 'backlog', rank: null })),
        ...other.map(e => ({ ...e, group: 'other', rank: null })),
    ]
    process.stdout.write(JSON.stringify(tagged, null, 2) + '\n')
    process.exit(0)
}

const sections = [
    wantPlanned && { title: 'Planned (priority order, * in development)', list: planned, ranked: true },
    wantBacklog && { title: 'Backlog (not yet ranked)', list: backlog, ranked: false },
    wantOther && { title: 'Other (implemented, terminal, or unplaced)', list: other, ranked: false },
].filter(s => s && s.list.length > 0)

if (sections.length === 0) {
    process.stdout.write('No matching RFDs.\n')
    process.exit(0)
}

// `Superseded` rows carry the RFD that replaced them; everything else is the
// bare status. Width is measured across the rows actually shown so the common
// case stays tight and only `--all` widens for `Superseded (NNN)`.
function statusText(entry) {
    if (entry.status === 'Superseded' && entry.supersededBy) {
        return `Superseded (${entry.supersededBy})`
    }
    return entry.status ?? '?'
}

const statusWidth = Math.max(
    10,
    ...sections.flatMap(s => s.list).map(e => statusText(e).length),
)

function formatEntry(entry, rank) {
    const rankStr = rank ? String(rank).padStart(3) : '   '
    const dev = entry.inDevelopment ? '*' : ' '
    const num = entry.num.padEnd(3)
    const status = statusText(entry).padEnd(statusWidth)
    const head = `${rankStr}  ${dev}  ${num}  ${status}  ${entry.title}`
    if (!full) return head

    const lines = [head]
    if (entry.summary) lines.push(`         ${entry.summary}`)
    if (entry.dependsOn.length > 0) {
        lines.push(`         needs: ${entry.dependsOn.join(', ')}`)
    }
    return lines.join('\n')
}

function renderSection({ title, list, ranked }) {
    const body = list
        .map((e, i) => formatEntry(e, ranked ? i + 1 : null))
        .join('\n')
    return `${title}\n\n${body}`
}

// Discoverability: a static menu of the section flags with their counts, so
// it's obvious more is available. Counts respect the category filter.
const tips = [
    `--planned (${planned.length})`,
    `--backlog (${backlog.length})`,
    `--all (${planned.length + backlog.length + other.length})`,
    !full && '--full',
].filter(Boolean).join(', ')

const output = [tips, ...sections.map(renderSection)].join('\n\n')
process.stdout.write(output + '\n')
