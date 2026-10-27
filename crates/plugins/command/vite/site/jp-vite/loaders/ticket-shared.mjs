import { readdirSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import { readSettings, workflowDir, workflowPath } from '../settings.mjs'
import { field } from './metadata.mjs'
import { referencedLabels } from './rfd-shared.mjs'

// Reading the tickets in the ticket directory for the site.
//
// `tickets.data.js` composes these into the index and the board. The document
// format is specified in RFD 100 and implemented for the tooling side by the
// `ticket` crate; this is the site's reader, and it needs less: metadata, a
// comment count, and the board's column order. Ticket pages are rendered from
// the markdown file itself, so comment bodies never come through here.
//
// Where the tickets live, the label vocabulary, and how much of the Done column
// the board shows come from the settings `jp vite sync` writes.

// The board's three columns, in flow order. `key` is the field in the board's
// order file, `.board.json`; `status` is what the ticket file carries.
export const COLUMNS = [
    { key: 'todo', status: 'Todo' },
    { key: 'in_progress', status: 'In Progress' },
    { key: 'done', status: 'Done' },
]

// How much of the Done column the board shows when the settings don't say. The
// rest lives in the index, which lists every ticket regardless of status.
const DEFAULT_DONE_LIMIT = 8

// Parse one ticket file.
export function parseTicket(content, filename) {
    const id = filename.match(/^([0-9a-z]{7})-/)?.[1] ?? '0000000'
    const title = content.match(/^# (.+)/m)?.[1]?.trim() ?? filename

    return {
        id: `T-${id}`,
        num: id,
        title,
        status: field(content, 'Status'),
        authors: field(content, 'Authors'),
        date: field(content, 'Date'),
        labels: readLabels(content),
        implements: field(content, 'Implements'),
        promotedTo: field(content, 'Promoted to'),
        github: field(content, 'GitHub'),
        comments: countComments(content),
        // Cross-kind links, resolved in the browser against both sets.
        links: referencedLabels(content).filter(label => label !== `T${id}`),
        slug: filename.replace(/\.md$/, ''),
    }
}

// Every `- **Label**: key=value` line in a ticket's metadata block, in order.
//
// The field repeats once per pair rather than joining them onto one line, so no
// line grows with the number of labels and the block survives `comfort`'s width
// wrapping. A formatter escapes what would otherwise be emphasis (`jp_config`
// becomes `jp\_config`), so reading undoes that.
//
// Read as written rather than checked against the vocabulary: a listing that
// hid a label the file carries would disagree with the file.
// `findUnknownLabels` below is what catches one the board doesn't define.
//
// Line endings are normalized first: the header match needs a literal blank
// line, and `.` does not consume the `\r` of a CRLF file. Rust's `str::lines()`
// reads such a file fine, so without this the CLI and the board disagree.
export function readLabels(content) {
    const block = content.replace(/\r\n/g, '\n')
        .match(/^# .+\n\n((?:- \*\*[^*]+\*\*:.*\n)+)/m)?.[1]
    if (!block) return []

    return block
        .split('\n')
        .map(line => line.match(/^- \*\*Label\*\*:\s*(.*)$/i)?.[1])
        .filter(value => value !== undefined)
        .map(value => value.replace(/\\([!-/:-@[-`{-~])/g, '$1').trim())
        .filter(Boolean)
        .sort()
}

// The labels the board defines, as a map of key to `{ values, open, retired,
// min, max, unique }`.
//
// A board that configures none defines none, and takes any label.
export function loadVocabulary(settings = readSettings()) {
    const labels = settings.ticket?.labels ?? {}
    const isMap = v => v !== undefined && v !== null
        && typeof v === 'object' && !Array.isArray(v)
    if (!isMap(labels)) {
        throw new Error('The label vocabulary in the site settings is not a map of label key to entry.')
    }

    return labels
}

// Whether the vocabulary accepts `label`, retired values included.
//
// Retired values count as defined: retiring one keeps existing tickets valid,
// which is the whole difference between retiring and deleting. A key that
// takes any value accepts every `key=value` under it, but not the key alone.
function knownLabel(vocabulary) {
    const known = new Set()
    const open = new Set()
    for (const [key, entry] of Object.entries(vocabulary)) {
        if (entry?.open) {
            open.add(key)
            continue
        }
        const values = [...(entry?.values ?? []), ...(entry?.retired ?? [])]
        if (values.length === 0) {
            known.add(key)
            continue
        }
        for (const value of values) known.add(`${key}=${value}`)
    }

    return label => {
        if (known.has(label)) return true
        const split = label.indexOf('=')
        return split > 0 && split < label.length - 1 && open.has(label.slice(0, split))
    }
}

// Labels used on a ticket that the vocabulary doesn't define.
//
// A typo in a hand-edited ticket would otherwise sit there silently, grouping
// with nothing and showing up as its own one-ticket category. A board that
// defines no labels takes any, so it has none to report.
export function findUnknownLabels(tickets, vocabulary) {
    if (Object.keys(vocabulary).length === 0) return null

    const known = knownLabel(vocabulary)
    const offenders = tickets
        .map(ticket => [ticket.id, ticket.labels.filter(label => !known(label))])
        .filter(([, unknown]) => unknown.length > 0)

    if (offenders.length === 0) return null

    const report = offenders
        .map(([id, unknown]) => `  ${id}: ${unknown.join(', ')}`)
        .join('\n')
    return `Tickets carry labels the vocabulary doesn't define:\n${report}\n\n` +
        `Add them to \`plugins.command.ticket.options.labels\`, or fix the tickets ` +
        `(\`jp ticket label <id> --label key=value\`).`
}

// Tickets carrying too few or too many values of a key the vocabulary bounds
// with `min` or `max`.
//
// Every label write `jp ticket` makes is held to the bounds; this catches a
// ticket edited by hand, or one filed before a bound was added.
export function findMiscountedLabels(tickets, vocabulary) {
    const bounded = Object.entries(vocabulary)
        .map(([key, entry]) => [key, entry?.min ?? 0, entry?.max ?? Infinity])
        .filter(([, min, max]) => min > 0 || max !== Infinity)
    if (bounded.length === 0) return null

    const offenders = []
    for (const ticket of tickets) {
        const problems = []
        for (const [key, min, max] of bounded) {
            const count = ticket.labels
                .filter(label => label === key || label.startsWith(`${key}=`))
                .length
            if (count < min || count > max) {
                problems.push(`${count} \`${key}\` (needs ${describeBound(min, max)})`)
            }
        }
        if (problems.length > 0) offenders.push(`  ${ticket.id}: ${problems.join(', ')}`)
    }

    if (offenders.length === 0) return null

    return `Tickets carry too few or too many labels of one key:\n${offenders.join('\n')}\n\n` +
        `Fix the tickets (\`jp ticket label <id> --label key=value\`), or change the key's ` +
        `\`min\` and \`max\` in \`plugins.command.ticket.options.labels\`.`
}

// Labels the vocabulary lets one ticket carry, found on several.
//
// `unique: "key"` lets one ticket carry the key at all, and `"key-value"` lets
// one carry each of its values; a key without values is held as a whole either
// way. Values match without case, as `jp ticket` matches them. `jp ticket`
// refuses such a write; this catches a ticket edited by hand, or one filed
// before the key was made unique.
export function findSharedLabels(tickets, vocabulary) {
    const unique = Object.entries(vocabulary).filter(([, entry]) => entry?.unique)
    if (unique.length === 0) return null

    // Keyed without case; each holder keeps the first spelling it was seen in.
    const holders = new Map()
    for (const ticket of tickets) {
        for (const [key, entry] of unique) {
            const carried = ticket.labels.filter(label => label === key || label.startsWith(`${key}=`))
            if (carried.length === 0) continue

            const whole = entry.unique === 'key' || carried.every(label => label === key)
            const names = new Map((whole ? [key] : carried).map(name => [name.toLowerCase(), name]))
            for (const [folded, name] of names) {
                const holder = holders.get(folded) ?? { name, ids: [] }
                holder.ids.push(ticket.id)
                holders.set(folded, holder)
            }
        }
    }

    const shared = [...holders.values()].filter(({ ids }) => ids.length > 1)
    if (shared.length === 0) return null

    const report = shared.map(({ name, ids }) => `  ${name}: ${ids.join(', ')}`).join('\n')
    return `Labels the board lets one ticket carry are on several:\n${report}\n\n` +
        `Keep each on one ticket (\`jp ticket label <id> --label key=value\`), or drop ` +
        `\`unique\` from the key in \`plugins.command.ticket.options.labels\`.`
}

// A key's bounds in words, as the count they ask for.
function describeBound(min, max) {
    if (min === max) return `exactly ${min}`
    if (max === Infinity) return `at least ${min}`
    if (min === 0) return `at most ${max}`
    return `${min} to ${max}`
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

// Read the board's column order from the ticket directory `dir`. A missing file,
// or no directory, means no manual ranking, which is a valid board — every
// ticket falls back to its default position.
export function loadBoard(dir) {
    let raw = {}
    if (dir) {
        try {
            raw = JSON.parse(readFileSync(resolve(dir, '.board.json'), 'utf-8'))
        } catch { /* no ranking yet */ }
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
        `Rename them to the \`<id>-slug.md\` form RFD 102 defines.`
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
// A project without a ticket directory, or with one that doesn't exist yet,
// holds no tickets.
export function loadTickets(settings = readSettings()) {
    const dir = workflowDir(settings, 'ticket')
    const route = workflowPath(settings, 'ticket')
    if (!dir) return []

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
        path: `${route}/${f.replace(/\.md$/, '')}`,
    }))

    const vocabulary = loadVocabulary(settings)
    const unknown = findUnknownLabels(tickets, vocabulary)
    if (unknown) throw new Error(unknown)
    const miscounted = findMiscountedLabels(tickets, vocabulary)
    if (miscounted) throw new Error(miscounted)
    const shared = findSharedLabels(tickets, vocabulary)
    if (shared) throw new Error(shared)

    return tickets
}

// Assemble the board: every ticket, plus the three columns in display order.
//
// Every column holds all its tickets. The board shows the head of Done, up to
// `doneLimit`, and the whole column only when a filter narrows it: a filtered
// board has to find a matching ticket past the cap.
export function assembleBoard(settings = readSettings()) {
    const tickets = loadTickets(settings)
    const order = loadBoard(workflowDir(settings, 'ticket'))
    const doneLimit = settings.ticket?.doneLimit ?? DEFAULT_DONE_LIMIT

    const columns = COLUMNS.map(column => ({
        key: column.key,
        status: column.status,
        tickets: orderColumn(tickets, column, order[column.key]),
    }))

    return { tickets, columns, doneLimit }
}
