import { readdirSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import { field, unescapeTitle } from './metadata.mjs'
import { referencedLabels } from './rfd-shared.mjs'

// Reading the tickets under `docs/ticket/` for the site.
//
// `tickets.data.js` composes these into the index and the board. The document
// format is specified in RFD 100 and implemented for the tooling side by the
// `ticket` crate; this is the site's reader, and it needs less: metadata, a
// comment count, and the board's column order. Ticket pages are rendered from
// the markdown file itself, so comment bodies never come through here.

// The board's three columns, in flow order. `key` is the field in
// `docs/ticket/.board.json`; `status` is what the ticket file carries.
export const COLUMNS = [
    { key: 'todo', status: 'Todo' },
    { key: 'in_progress', status: 'In Progress' },
    { key: 'done', status: 'Done' },
]

// How much of the Done column the board shows. The rest lives in the index,
// which lists every ticket regardless of status.
export const DONE_HEAD = 8

// Parse one ticket file.
export function parseTicket(content, filename) {
    const id = filename.match(/^([0-9a-z]{7})-/)?.[1] ?? '0000000'
    const rawTitle = content.match(/^# (.+)/m)?.[1]?.trim() ?? filename

    return {
        id: `T-${id}`,
        num: id,
        title: unescapeTitle(rawTitle),
        status: field(content, 'Status'),
        kind: field(content, 'Kind'),
        authors: field(content, 'Authors'),
        date: field(content, 'Date'),
        blockedBy: field(content, 'Blocked by'),
        labels: splitLabels(field(content, 'Labels')),
        implements: field(content, 'Implements'),
        promotedTo: field(content, 'Promoted to'),
        github: field(content, 'GitHub'),
        comments: countComments(content),
        // Cross-kind links, resolved in the browser against both sets.
        links: referencedLabels(content).filter(label => label !== `T${id}`),
        slug: filename.replace(/\.md$/, ''),
    }
}

// Split a `Labels` metadata value into its labels.
//
// Read as written rather than checked against the vocabulary: a listing that
// hid a label the file carries would disagree with the file. `findUnknownLabels`
// below is what catches one the board doesn't define.
export function splitLabels(value) {
    if (!value) return []

    return value.split(',').map(label => label.trim()).filter(Boolean)
}

// The labels the board defines: `{ active, retired }`, each a map of label to
// description.
//
// A board with no `.labels.json` defines none, which is a board that hasn't
// started using them.
export function loadVocabulary() {
    const path = resolve(import.meta.dirname, '../../ticket/.labels.json')

    let raw
    try {
        raw = readFileSync(path, 'utf-8')
    } catch {
        return { active: {}, retired: {} }
    }

    if (raw.trim() === '') return { active: {}, retired: {} }

    const parsed = JSON.parse(raw)
    const isMap = v => v !== undefined && v !== null
        && typeof v === 'object' && !Array.isArray(v)
    if (!isMap(parsed)) {
        throw new Error(`${path} is not a JSON object.`)
    }

    const active = parsed.active ?? {}
    const retired = parsed.retired ?? {}
    if (!isMap(active) || !isMap(retired)) {
        throw new Error(
            `${path} must have \`active\` and \`retired\` maps of label to description.`
        )
    }

    return { active, retired }
}

// Labels used on a ticket that the vocabulary doesn't define.
//
// Retired labels count as defined: retiring one keeps existing tickets valid,
// which is the whole difference between retiring and deleting. A typo in a
// hand-edited ticket would otherwise sit there silently, grouping with nothing
// and showing up as its own one-ticket category.
export function findUnknownLabels(tickets, vocabulary) {
    const known = new Set([
        ...Object.keys(vocabulary.active ?? {}),
        ...Object.keys(vocabulary.retired ?? {}),
    ])
    const offenders = tickets
        .map(ticket => [ticket.id, ticket.labels.filter(label => !known.has(label))])
        .filter(([, unknown]) => unknown.length > 0)

    if (offenders.length === 0) return null

    const report = offenders
        .map(([id, unknown]) => `  ${id}: ${unknown.join(', ')}`)
        .join('\n')
    return `Tickets carry labels the vocabulary doesn't define:\n${report}\n\n` +
        `Add them to the \`active\` or \`retired\` map in docs/ticket/.labels.json, ` +
        `or fix the tickets (\`jp ticket label <id> --label ...\`).`
}

// Count the comments in a ticket.
//
// A comment opens at a line of five or more dashes at column zero, followed by
// a blank line, followed by a metadata block carrying both `From` and `Date`.
// Lines inside fenced code blocks don't count, so a ticket quoting the format
// doesn't inflate its own total.
export function countComments(content) {
    const lines = content.split('\n')
    let fence = null
    let count = 0

    for (let i = 0; i < lines.length; i++) {
        const delimiter = fenceAt(lines[i])
        if (delimiter) {
            if (fence === null) fence = delimiter
            else if (delimiter.char === fence.char && delimiter.count >= fence.count
                && !delimiter.info) fence = null
            continue
        }
        if (fence !== null) continue

        if (!/^-{5,}\s*$/.test(lines[i])) continue
        if ((lines[i + 1] ?? 'x').trim() !== '') continue

        let from = false
        let date = false
        for (let j = i + 2; j < lines.length; j++) {
            const key = lines[j].match(/^- \*\*([^*]+)\*\*:/)?.[1]?.trim().toLowerCase()
            if (!key) break
            if (key === 'from') from = true
            if (key === 'date') date = true
        }
        if (from && date) count++
    }

    return count
}

// Read a line as a code fence delimiter.
function fenceAt(line) {
    const match = line.match(/^ {0,3}(`{3,}|~{3,})(.*)$/)
    if (!match) return null

    return { char: match[1][0], count: match[1].length, info: match[2].trim() !== '' }
}

// Read the board's column order. A missing file means no manual ranking, which
// is a valid board — every ticket falls back to its default position.
export function loadBoard(path) {
    let raw
    try {
        raw = JSON.parse(readFileSync(path, 'utf-8'))
    } catch {
        raw = {}
    }

    const ids = v => (Array.isArray(v) ? v.map(String) : [])
    const board = {}
    for (const column of COLUMNS) board[column.key] = ids(raw?.[column.key])

    return board
}

// Order the tickets of one column.
//
// Status decides which column a ticket is in; the board file decides the order
// within it. Tickets the board file doesn't mention fall to the bottom, newest
// first in Done and oldest first elsewhere — a new ticket enters Todo unranked,
// below whatever has already been prioritised.
export function orderColumn(tickets, column, order) {
    const rank = new Map(order.map((id, index) => [id, index]))
    const rows = tickets.filter(ticket => ticket.status === column.status)

    return rows.sort((a, b) => {
        const ar = rank.has(a.id) ? rank.get(a.id) : Infinity
        const br = rank.has(b.id) ? rank.get(b.id) : Infinity
        if (ar !== br) return ar - br
        return column.key === 'done' ? b.num.localeCompare(a.num) : a.num.localeCompare(b.num)
    })
}

// The RFDs currently being implemented, derived from ticket state.
//
// An RFD is in development when a ticket claiming to implement it sits in the In
// Progress column. Derived, not synced, so there is nothing to keep in step (see
// RFD 100).
export function inDevelopmentRfds(tickets) {
    const rfds = new Set()
    for (const ticket of tickets) {
        if (ticket.status !== 'In Progress' || !ticket.implements) continue
        const num = ticket.implements.match(/\d{1,3}/)?.[0]
        if (num) rfds.add(num.padStart(3, '0'))
    }

    return [...rfds].sort()
}

// Files left in the pre-RFD-102 `NNNN-slug.md` shape.
//
// A branch cut before the id change carries them, and the reader below would
// skip them silently — the ticket would vanish from the board with nothing
// said. Aborting the build is the only way that becomes visible.
export function findLegacyIds(files) {
    const legacy = files.filter(f => /^\d{4}-.+\.md$/.test(f))
    if (legacy.length === 0) return null

    return `Tickets still in the pre-RFD-102 id format:\n` +
        legacy.map(f => `  ${f}`).join('\n') + '\n\n' +
        `Run \`just ticket-migrate\` on this branch to convert them.`
}

// Ids claimed by more than one file.
//
// Ids are collision-resistant, not unique by construction, so two checkouts can
// draw the same one. Every reference to it is then ambiguous, which is why the
// build stops rather than rendering one of them.
export function findDuplicateIds(files) {
    const byId = new Map()
    for (const f of files) {
        const id = f.slice(0, 7)
        byId.set(id, [...(byId.get(id) ?? []), f])
    }

    const clashes = [...byId.entries()].filter(([, group]) => group.length > 1)
    if (clashes.length === 0) return null

    const report = clashes
        .map(([id, group]) => `  T-${id}: ${group.join(', ')}`)
        .join('\n')
    return `Duplicate ticket ids found:\n${report}\n\n` +
        `Each ticket id must map to exactly one file. ` +
        `Give the losing branch a fresh id before merging.`
}

// Read every ticket, ordered by id.
//
// Paths resolve from this file's location, so the result doesn't depend on the
// caller's working directory. A missing directory holds no tickets.
export function loadTickets() {
    const dir = resolve(import.meta.dirname, '../../ticket')

    let names
    try {
        names = readdirSync(dir)
    } catch {
        return []
    }

    const legacy = findLegacyIds(names)
    if (legacy) throw new Error(legacy)

    const files = names.filter(f => /^[0-9a-z]{7}-.+\.md$/.test(f)).sort()
    const duplicate = findDuplicateIds(files)
    if (duplicate) throw new Error(duplicate)

    const tickets = files.map(f => ({
        ...parseTicket(readFileSync(resolve(dir, f), 'utf-8'), f),
        path: `/ticket/${f.replace(/\.md$/, '')}`,
    }))

    const unknown = findUnknownLabels(tickets, loadVocabulary())
    if (unknown) throw new Error(unknown)

    return tickets
}

// Assemble the board: every ticket, plus the three columns in display order.
//
// `done` is capped at `DONE_HEAD`; `doneTotal` reports how many there are so the
// view can point at the index for the rest.
export function assembleBoard() {
    const tickets = loadTickets()
    const order = loadBoard(resolve(import.meta.dirname, '../../ticket/.board.json'))

    const columns = COLUMNS.map(column => {
        const rows = orderColumn(tickets, column, order[column.key])
        return {
            key: column.key,
            status: column.status,
            total: rows.length,
            tickets: column.key === 'done' ? rows.slice(0, DONE_HEAD) : rows,
        }
    })

    return { tickets, columns }
}
