import { readdirSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import { knownDocuments, mentionedLabels } from '../references.mjs'
import { readSettings, workflowDir, workflowPath } from '../settings.mjs'
import { anyLabelQuery } from '../theme/label-filter.mjs'
import { field, wrappedField } from './metadata.mjs'
import {
    checkMilestones,
    normalizePriority,
    TERMINAL_STATUSES,
} from './rfd-priority.mjs'
import { loadTickets } from './ticket-shared.mjs'

// Shared parsing and validation for the RFD data loaders.
//
// `rfds.data.js` (published) and `rfd-drafts.data.js` (drafts) both build on
// these helpers. The validation functions are pure: they read files and
// return a formatted error message, or `null` when the check passes. The
// caller decides severity — published RFDs throw, drafts warn (except
// duplicate ids, which abort either way).
//
// `assembleBoard` (bottom of this file) composes the building blocks into the
// full priority board the web board (`rfd-board.data.js`) shows. `jp rfd list`
// reads the same files in Rust, so a change to how the board is read belongs in
// both.

// Parse the inline metadata from an RFD markdown file.
//
// RFDs use `- **Key**: Value` lines instead of YAML frontmatter, so we need a
// small custom parser. Handles both permanent (`NNN`) and draft (`DNN`) ids.
export function parseMeta(content, filename) {
    const num = filename.match(/^(\d{3}|D\d{2})/)?.[1] ?? '000'
    const title = content.match(/^# RFD (?:\d+|D\d+):\s*(.+)/m)?.[1]?.trim() ?? filename

    return {
        num,
        title,
        status: field(content, 'Status'),
        category: field(content, 'Category'),
        // The RFD number that superseded this one, if any (e.g. `033` -> `034`).
        supersededBy: parseField(content, 'Superseded by')[0] ?? null,
        authors: field(content, 'Authors'),
        date: field(content, 'Date'),
        // Long enough to wrap.
        summary: wrappedField(content, 'Summary'),
        slug: filename.replace(/\.md$/, ''),
    }
}

// Scan document content for links to other RFDs.
// Matches patterns like `NNN-slug.md`, `DNN-slug)`, `./NNN-slug`.
//
// Both permanent and draft ids are captured: published RFDs only ever contain
// `NNN` references (the stray-draft check rejects `DNN` tokens before we get
// here), while drafts legitimately link both.
export function parseReferences(content, ownNum) {
    const refs = new Set()
    for (const label of referencedLabels(content)) {
        // RFD ids only; ticket references belong to the cross-kind set the site
        // builds in `theme/documents.mjs`.
        if (label.startsWith('T')) continue
        if (label !== '000' && label !== ownNum) refs.add(label)
    }
    return [...refs].sort()
}

// Every document a body links to, as the labels `documents.mjs` keys on:
// `042` and `D12` for RFDs, `T-005zd00` for tickets.
//
// RFDs are matched through their linked filename, whose slug must carry a
// letter so a date like `2026-08-05` isn't read as a link, and by name in
// prose, as `RFD 042`. Tickets are matched as `T-005zd00`, or as `005zd00`
// when `known` holds that ticket. The names are the ones the page's text
// links; see `../references.mjs`.
export function referencedLabels(content, known = new Map()) {
    const labels = new Set()

    const rfds = /\b(\d{3}|D\d{2})-[a-z0-9-]*[a-z][a-z0-9-]*(?:\.md)?/g
    let match
    while ((match = rfds.exec(content)) !== null) {
        labels.add(match[1])
    }
    for (const label of mentionedLabels(content, known)) labels.add(label)

    return [...labels]
}

// Build the RFD entries consumed by the index pages and cross-reference
// widgets. `basePath` is the site path the files live under (`/rfd` or
// `/rfd/drafts`) and feeds each entry's absolute `path`.
//
// `referencedBy` is computed within the given file set only. Published RFDs
// can't reference drafts, so a draft's `referencedBy` lists drafts; a
// published RFD's stays published-only.
export function buildEntries(dir, files, basePath) {
    const known = knownDocuments(readSettings())
    const rfds = files.map(f => {
        const content = readFileSync(resolve(dir, f), 'utf-8')
        const meta = parseMeta(content, f)
        return {
            ...meta,
            path: `${basePath}/${meta.slug}`,
            references: parseReferences(content, meta.num),
            // Cross-kind links, resolved in the browser against both sets.
            links: referencedLabels(content, known).filter(label => label !== meta.num),
        }
    })

    for (const rfd of rfds) {
        rfd.referencedBy = rfds
            .filter(other => other.references.includes(rfd.num))
            .map(other => other.num)
    }

    return rfds
}

// The files of one RFD id space in `dir`, sorted: `pattern` picks them out. A
// directory that doesn't exist holds none.
export function listRfds(dir, pattern) {
    let names
    try {
        names = readdirSync(dir)
    } catch {
        return []
    }

    return names.filter(f => pattern.test(f)).sort()
}

// Published RFDs: `NNN-slug.md`, the templates' shared `000` excepted.
export const PUBLISHED = /^(?!000-)\d{3}-.+\.md$/

// Drafts: `DNN-slug.md`.
export const DRAFTS = /^D\d{2}-.+\.md$/

// Where one project keeps its RFDs: the published ones, the drafts, and the
// priority board, on disk and on the site. `null` when the project has no RFDs.
export function rfdLocations(settings = readSettings()) {
    const dir = workflowDir(settings, 'rfd')
    const path = workflowPath(settings, 'rfd')
    if (!dir) return null

    return {
        dir,
        path,
        drafts: resolve(dir, 'drafts'),
        draftsPath: `${path}/drafts`,
        priority: resolve(dir, '.priority.json'),
    }
}

// Read the priority board state. This is human-curated source of truth: the
// prioritised `planned` milestone groups (see `normalizePriority` for the
// exact shape) and the unsorted `backlog` below the cutoff. A missing file is
// an empty board.
export function loadPriority(path) {
    let raw
    try {
        raw = JSON.parse(readFileSync(path, 'utf-8'))
    } catch {
        raw = {}
    }
    return normalizePriority(raw)
}

// Annotate each entry with its board position and milestone. `priority` is the
// index in the combined `order` + `backlog` list (lower = higher priority) or
// `null` when the RFD holds no position. `milestone` is the name of the planned
// group the RFD sits in, or `null` (unassigned, backlogged, or unplaced).
//
// A terminal RFD holds no position whatever the board file says. The file goes
// stale as soon as a status changes, so membership is settled here, once, for
// every surface.
export function mergePriority(entries, priority) {
    const combined = [...priority.order, ...(priority.backlog ?? [])]
    const rank = new Map(combined.map((num, i) => [num, i]))
    const milestoneOf = new Map()
    for (const group of priority.planned) {
        for (const num of group.ids) milestoneOf.set(num, group.milestone)
    }
    for (const entry of entries) {
        const placed = !TERMINAL_STATUSES.has(entry.status) && rank.has(entry.num)
        entry.priority = placed ? rank.get(entry.num) : null
        entry.milestone = placed ? (milestoneOf.get(entry.num) ?? null) : null
    }
}

// The label keys tickets name RFDs under, as `jp rfd` is configured.
//
// A ticket labelled with the tracking key, `tracking-rfd=116` by default, is
// the RFD's tracking ticket; one labelled with the relevant key,
// `relevant-rfd=116`, is part of its work.
export function linkKeys(settings = readSettings()) {
    return {
        tracking: settings.rfd?.trackingKey ?? 'tracking-rfd',
        relevant: settings.rfd?.relevantKey ?? 'relevant-rfd',
    }
}

// An RFD id the way `jp rfd` reads one, `116`, `RFD 116`, or `d5`, in its
// canonical form, `116` or `D05`; `null` for text that names no RFD.
export function canonicalRfd(text) {
    const id = text.trim().replace(/^RFD\s+/i, '')
    if (/^D\d{1,2}$/i.test(id)) return `D${id.slice(1).padStart(2, '0')}`
    if (/^\d{1,3}$/.test(id)) return id.padStart(3, '0')

    return null
}

// The key and value of `label`, the key as written; the value is `null` for a
// bare label.
function splitLabel(label) {
    const split = label.indexOf('=')
    if (split < 0) return { key: label, value: null }

    return { key: label.slice(0, split), value: label.slice(split + 1) }
}

// Whether `key` is `wanted`, without case, the way `jp ticket` matches keys.
function sameKey(key, wanted) {
    return key.toLowerCase() === wanted.toLowerCase()
}

// What the tickets say about each RFD, by its canonical id: whether it is in
// development, and the ids of the tickets naming it either way.
//
// An RFD is in development while a ticket tracking it sits in the In Progress
// column. The board file records nothing about this, so moving that ticket is
// what changes it.
export function rfdTickets(tickets, settings = readSettings()) {
    const keys = linkKeys(settings)
    const byRfd = new Map()
    const at = num => {
        if (!byRfd.has(num)) byRfd.set(num, { inDevelopment: false, tickets: new Set() })
        return byRfd.get(num)
    }

    for (const ticket of tickets) {
        for (const label of ticket.labels) {
            const { key, value } = splitLabel(label)
            const num = value === null ? null : canonicalRfd(value)
            if (!num) continue

            if (sameKey(key, keys.tracking)) {
                const linked = at(num)
                linked.tickets.add(ticket.id)
                if (ticket.status === 'In Progress') linked.inDevelopment = true
            } else if (sameKey(key, keys.relevant)) {
                at(num).tickets.add(ticket.id)
            }
        }
    }

    return byRfd
}

// Mark each RFD with what its tickets say: `inDevelopment`, `ticketCount`, and
// `ticketsPath`, the ticket board showing them, or `null` in a project without
// tickets.
//
// The board finds them by their labels, so the link names both keys and the
// board knows nothing about RFDs.
export function mergeTickets(entries, tickets, settings = readSettings()) {
    const keys = linkKeys(settings)
    const board = workflowPath(settings, 'ticket')
    const byRfd = rfdTickets(tickets, settings)

    for (const entry of entries) {
        const linked = byRfd.get(entry.num)
        entry.inDevelopment = linked?.inDevelopment ?? false
        entry.ticketCount = linked?.tickets.size ?? 0
        entry.ticketsPath = board
            ? `${board}/board${anyLabelQuery([
                `${keys.tracking}=${entry.num}`,
                `${keys.relevant}=${entry.num}`,
            ])}`
            : null
    }
}

// Tickets whose link labels name no RFD, or name one in a spelling other than
// its id.
//
// `jp rfd` reads `relevant-rfd=91` as RFD 091, but the board's filter, `unique`,
// and every other reader compare a label as written, and miss it. A value
// naming no RFD, such as a typo, links nothing at all.
export function findBrokenRfdLinks(tickets, known, settings = readSettings()) {
    const keys = linkKeys(settings)
    const broken = []
    for (const ticket of tickets) {
        for (const label of ticket.labels) {
            const { key, value } = splitLabel(label)
            if (!sameKey(key, keys.tracking) && !sameKey(key, keys.relevant)) continue
            if (value === null || !known.has(value)) broken.push(`  ${ticket.id}: ${label}`)
        }
    }

    if (broken.length === 0) return null

    return `Tickets link to RFDs that don't exist, or by an id spelled another way:\n` +
        `${broken.join('\n')}\n\n` +
        `Write the RFD's id the way \`jp rfd list\` shows it, such as \`042\` or \`D05\`.`
}

// Attach each entry's hard ordering dependencies (Requires ∪ Extends) as an
// array of RFD ids, read from the relationship graph. The board forbids placing
// an RFD above one it depends on, mirroring the unified gate the build enforces.
export function mergeDependencies(entries, graph) {
    for (const entry of entries) {
        const node = graph.get(entry.num)
        entry.dependsOn = node
            ? [...new Set([...node.requires, ...node.extends_])]
            : []
    }
}

// Reject board entries that don't match a known RFD. Numbers are never reused,
// so an unknown id is real corruption: a hand-edit typo, or an id whose file
// went away. A terminal RFD still listed on the board is a separate, milder
// problem — see `checkTerminalOnBoard`.
export function checkPriority(entries, priority) {
    const known = new Set(entries.map(e => e.num))
    const unknown = [
        ...priority.order,
        ...(priority.backlog ?? []),
    ].filter(num => !known.has(num))

    if (unknown.length === 0) return null

    const ids = [...new Set(unknown)].sort().join(', ')
    return `Unknown RFD ids in priority board: ${ids}.\n\n` +
        `\`.priority.json\` references RFDs that don't exist. Fix the ` +
        `ids or remove them (the board UI rewrites this file on save).`
}

// Reject terminal RFDs listed on the priority board.
//
// The board ranks open work, so an Implemented, Superseded, or Abandoned RFD has
// no place on it. `graph` supplies the statuses and must span every id space the
// board can hold — drafts included, since a draft can be abandoned. Ids it
// doesn't know are left to `checkPriority`.
//
// This is an error rather than a warning because the board file is what a human
// reads and reorders: nothing renders a stale id, so nothing else would ever
// point it out.
export function checkTerminalOnBoard(graph, priority) {
    const placed = [...priority.order, ...(priority.backlog ?? [])]
    const terminal = [...new Set(placed)]
        .filter(num => TERMINAL_STATUSES.has(graph.get(num)?.status))
        .sort()

    if (terminal.length === 0) return null

    const report = terminal
        .map(num => `  ${num} (${graph.get(num).status})`)
        .join('\n')
    return `Terminal RFDs on the priority board:\n${report}\n\n` +
        `The board ranks work that is still open. Remove these from ` +
        `\`.priority.json\`.`
}

// Each id (`NNN` or `DNN`) must map to exactly one file. Once drafts left the
// website's validation pipeline it became possible to land two files sharing a
// draft id; this guards both id spaces.
export function findDuplicateIds(files) {
    const byId = new Map()
    for (const f of files) {
        const id = f.match(/^(\d{3}|D\d{2})/)?.[1]
        if (!id) continue
        if (!byId.has(id)) byId.set(id, [])
        byId.get(id).push(f)
    }

    const dups = [...byId.entries()].filter(([, group]) => group.length > 1)
    if (dups.length === 0) return null

    const report = dups
        .map(([id, group]) => `  ${id}: ${group.join(', ')}`)
        .join('\n')
    return `Duplicate RFD ids found:\n${report}\n\n` +
        `Each RFD id must map to exactly one file.`
}

// Every published RFD needs a one-line summary for the index pages and the
// priority board. Drafts are exempt: they are unpublished, and nothing indexes
// them by summary.
//
// Whether a summary still describes the document is left to whoever reviews
// the change. It sits in the same file as the prose it summarises, so a pull
// request that moves one and not the other says so in its diff.
export function checkSummaries(dir, files) {
    const missing = files.filter(f =>
        !wrappedField(readFileSync(resolve(dir, f), 'utf-8'), 'Summary'))

    if (missing.length === 0) return null

    const nums = missing.map(f => f.match(/^(\d+)/)?.[1]).join(', ')
    return `Missing summaries for: ${nums}.\n\n` +
        `Add a \`- **Summary**:\` field to the metadata header of each, one ` +
        `sentence saying what the RFD proposes. It is what the index pages ` +
        `and the priority board show.`
}

// Reject `DNN`-style references in published RFDs.
//
// Drafts (`DNN-slug.md`) live in the drafts directory and are not published. A
// `D\d\d` token in a published RFD is either a stale promotion artefact (an
// RFD that was promoted from DNN to NNN without its internal references being
// rewritten) or an accidental cross-link to a draft — in any context,
// including code blocks. The only RFD that may mention `D\d\d` is the one
// describing the lifecycle and its numbering, RFD 001: files in `allowlist` are
// skipped entirely.
export function findStrayDraftRefs(dir, files, allowlist) {
    const strays = []
    for (const f of files) {
        if (allowlist.has(f)) continue
        const content = readFileSync(resolve(dir, f), 'utf-8')
        const lines = content.split('\n')
        const hits = []
        for (let i = 0; i < lines.length; i++) {
            for (const m of lines[i].matchAll(/\bD\d\d\b/g)) {
                hits.push({ line: i + 1, id: m[0] })
            }
        }
        if (hits.length > 0) strays.push({ file: f, hits })
    }

    if (strays.length === 0) return null

    const report = strays
        .flatMap(({ file, hits }) =>
            hits.map(({ line, id }) => `  ${file}:${line}: ${id}`)
        )
        .join('\n')
    return `DNN-style references found in published RFDs:\n` +
        report + '\n\n' +
        `Drafts are not published; published RFDs must not reference ` +
        `them. Only RFD 001, the process document, may describe the DNN ` +
        `numbering convention.`
}

// CommonMark link labels are case-insensitive and collapse internal
// whitespace, so `[RFD 019]` and `[rfd  019]:` are the same label.
function normalizeLabel(label) {
    return label.trim().replace(/\s+/g, ' ').toLowerCase()
}

// Neutralize code, keeping line numbers intact. Bracket syntax inside code is
// never a link, and RFDs are full of sample JSON, shell, and Rust that would
// otherwise look like one.
//
// Fenced blocks are blanked outright. Inline spans are *unwrapped* instead,
// with only the brackets inside them blanked: a link label may contain a code
// span (``[`wit-bindgen`]: url``), so deleting the span erases the label from
// the usage and its definition alike and desynchronizes the two. Unwrapping
// keeps the label byte-identical on both sides, while `[RFD 038]` written
// inside backticks as literal syntax stops looking like a reference.
function stripCode(content) {
    let fence = null
    return content
        .split('\n')
        .map((line) => {
            const marker = line.match(/^ {0,3}(`{3,}|~{3,})/)?.[1]
            if (fence) {
                if (marker && marker[0] === fence[0] && marker.length >= fence.length) {
                    fence = null
                }
                return ''
            }
            if (marker) {
                fence = marker
                return ''
            }
            return line.replace(/(`+)([\s\S]*?)\1/g, (_, _ticks, body) =>
                body.replace(/[[\]]/g, ' ')
            )
        })
        .join('\n')
}

// Reference-link usages on one line, as `{ label, text }` pairs. `text` is the
// source form, so the error message shows what the author actually wrote.
function* referenceUsages(line) {
    // Full (`[text][label]`) and collapsed (`[label][]`) references. A leading
    // `!` makes it an image, which resolves its label the same way.
    for (const m of line.matchAll(/!?\[((?:[^[\]\\]|\\.)*)\]\[((?:[^[\]\\]|\\.)*)\]/g)) {
        yield { label: m[2].trim() === '' ? m[1] : m[2], text: m[0] }
    }

    // Shortcut references, restricted to the project's RFD citation form. A
    // following `[`, `(`, or `:` means it is a full reference, an inline link,
    // or the definition line itself.
    for (const m of line.matchAll(/\[(RFD\s+(?:\d{3}|D\d{2}))\](?![[(:])/g)) {
        yield { label: m[1], text: m[0] }
    }
}

// Reject reference-style links whose definition is missing.
//
// VitePress's dead-link check cannot see these. CommonMark resolves `[RFD 019]`
// against the document's link definitions, and when there is none the text is
// not a link at all — it renders as the literal string `[RFD 019]`. No href is
// emitted, so nothing downstream has anything to check. The failure is silent
// by construction, which is how `[RFD 038]` sat unlinked in RFD 050 for months.
//
// Two shapes are checked:
//
//   - Full and collapsed references (`[text][label]`, `[label][]`). These are
//     always reference links, so an undefined label is unambiguously a bug.
//   - Shortcut references of the form `[RFD NNN]` / `[RFD DNN]`. A bare
//     `[label]` is only a link when the label happens to be defined, so
//     checking every one would flag ordinary prose (`[!NOTE]`, `[y/n/...]`,
//     `[c] Continue`). Restricting to the RFD citation form catches the case
//     that actually recurs, without the false positives.
export function findUndefinedRefLinks(dir, files) {
    const broken = []
    for (const f of files) {
        const lines = stripCode(readFileSync(resolve(dir, f), 'utf-8')).split('\n')

        const defined = new Set()
        for (const line of lines) {
            const label = line.match(/^ {0,3}\[([^\]]+)\]:\s*\S/)?.[1]
            if (label) defined.add(normalizeLabel(label))
        }

        const hits = []
        for (let i = 0; i < lines.length; i++) {
            for (const { label, text } of referenceUsages(lines[i])) {
                if (defined.has(normalizeLabel(label))) continue
                hits.push({ line: i + 1, text })
            }
        }
        if (hits.length > 0) broken.push({ file: f, hits })
    }

    if (broken.length === 0) return null

    const report = broken
        .flatMap(({ file, hits }) =>
            hits.map(({ line, text }) => `  ${file}:${line}: ${text}`)
        )
        .join('\n')
    return `Reference links with no matching definition:\n` +
        report + '\n\n' +
        `A reference link whose label is undefined renders as literal text ` +
        `instead of a link, so the dead-link check never sees it. Add the ` +
        `matching \`[label]: target\` line at the bottom of the document, or ` +
        `rewrite the text so it is not a reference.`
}

// The RFD ids a relationship field in the metadata block cites, wrapped lines
// included.
function parseField(content, field) {
    const value = wrappedField(content, field) ?? ''
    return [...value.matchAll(/\bRFD\s+(\d{3}|D\d{2})\b/g)].map(m => m[1])
}

// Parse the `Requires` / `Extends` relationship metadata into a small graph
// keyed by RFD id. The relationship checks below all operate on this graph.
export function buildGraph(dir, files) {
    const graph = new Map()
    for (const f of files) {
        const content = readFileSync(resolve(dir, f), 'utf-8')
        const num = f.match(/^(\d{3}|D\d{2})/)?.[1]
        if (!num) continue
        const status = content
            .match(/^- \*\*Status\*\*:\s*(\w+)/m)?.[1] ?? null
        graph.set(num, {
            file: f,
            status,
            requires: parseField(content, 'Requires'),
            extends_: parseField(content, 'Extends'),
            requiredBy: parseField(content, 'Required by'),
            extendedBy: parseField(content, 'Extended by'),
        })
    }
    return graph
}

// A target must not appear under both `Requires` and `Extends` on the same
// RFD (same for the inverse pair `Required by` / `Extended by`). Extension
// already implies the dependency, so listing both is redundant.
export function checkRelationshipDuplicates(graph) {
    const duplicates = []
    for (const [num, entry] of graph) {
        const reqSet = new Set(entry.requires)
        for (const e of entry.extends_) {
            if (reqSet.has(e)) {
                duplicates.push({
                    file: entry.file, num, dep: e,
                    fields: ['Requires', 'Extends'],
                })
            }
        }
        const reqBySet = new Set(entry.requiredBy)
        for (const e of entry.extendedBy) {
            if (reqBySet.has(e)) {
                duplicates.push({
                    file: entry.file, num, dep: e,
                    fields: ['Required by', 'Extended by'],
                })
            }
        }
    }

    if (duplicates.length === 0) return null

    const report = duplicates
        .map(({ file, num, dep, fields }) =>
            `  ${file}: RFD ${num} lists RFD ${dep} under both '${fields[0]}' and '${fields[1]}'`
        )
        .join('\n')
    return `Duplicate relationship metadata:\n` +
        report + '\n\n' +
        `Extension implies dependency — don't list the same target ` +
        `under both fields. Drop one entry; 'Extends' is the more ` +
        `specific of the pair.`
}

// Build the unified dependency map (Requires ∪ Extends) used by the gate and
// cycle detection.
function unifiedDeps(graph) {
    const deps = new Map()
    for (const [num, entry] of graph) {
        deps.set(num, [...new Set([...entry.requires, ...entry.extends_])])
    }
    return deps
}

// An RFD with status `Accepted` must not depend on an RFD below `Accepted`; an
// `Implemented` RFD must not depend on one below `Implemented`. `Superseded`
// counts as both (the dependency was satisfied at some point). Drafts have no
// such status, so the gate is inert for them.
export function checkStatusGate(graph) {
    const ACCEPTED_PLUS = new Set(['Accepted', 'Implemented', 'Superseded'])
    const IMPLEMENTED_PLUS = new Set(['Implemented', 'Superseded'])
    const deps = unifiedDeps(graph)

    const violations = []
    for (const [num, entry] of graph) {
        const allowed = entry.status === 'Accepted' ? ACCEPTED_PLUS
                      : entry.status === 'Implemented' ? IMPLEMENTED_PLUS
                      : null
        if (!allowed) continue
        for (const dep of deps.get(num) ?? []) {
            const depEntry = graph.get(dep)
            if (!depEntry) {
                violations.push({
                    file: entry.file, num, status: entry.status,
                    dep, depStatus: '(not found)',
                })
                continue
            }
            if (!allowed.has(depEntry.status)) {
                violations.push({
                    file: entry.file, num, status: entry.status,
                    dep, depStatus: depEntry.status,
                })
            }
        }
    }

    if (violations.length === 0) return null

    const report = violations
        .map(({ file, num, status, dep, depStatus }) =>
            `  ${file}: RFD ${num} (${status}) depends on RFD ${dep} (${depStatus})`
        )
        .join('\n')
    return `Promotion gate violations:\n` +
        report + '\n\n' +
        `Accepted RFDs require deps to be Accepted/Implemented/Superseded; ` +
        `Implemented RFDs require deps to be Implemented/Superseded. ` +
        `Both \`Requires\` and \`Extends\` participate.`
}

// Reject dependency cycles (A → B → ... → A) over the unified graph.
export function findCycles(graph) {
    const deps = unifiedDeps(graph)
    const cycles = []
    const WHITE = 0, GRAY = 1, BLACK = 2
    const color = new Map()
    for (const num of graph.keys()) color.set(num, WHITE)

    const visit = (num, path) => {
        color.set(num, GRAY)
        for (const next of deps.get(num) ?? []) {
            if (!graph.has(next)) continue
            if (color.get(next) === GRAY) {
                const start = path.indexOf(next)
                cycles.push([...path.slice(start), next])
                continue
            }
            if (color.get(next) === BLACK) continue
            visit(next, [...path, next])
        }
        color.set(num, BLACK)
    }

    for (const num of graph.keys()) {
        if (color.get(num) === WHITE) visit(num, [num])
    }

    if (cycles.length === 0) return null

    const report = cycles
        .map(c => '  ' + c.map(n => `RFD ${n}`).join(' → '))
        .join('\n')
    return `Dependency cycles detected (Requires ∪ Extends):\n${report}`
}

// Reject a `Requires` entry that targets an already-`Implemented` RFD.
//
// `Requires` exists to gate promotion on an unbuilt dependency: it tells the
// gate (and readers) that the target must reach a sufficient status first.
// Once the target is `Implemented`, the dependency is satisfied for good and the
// link is redundant. `jp rfd promote` strips `Requires` when an RFD itself
// reaches `Implemented`; this check catches the other direction, a dependency
// that became `Implemented` while the dependent RFD is still in flight. Use
// `Extends` instead when the relationship is design lineage worth keeping past
// implementation.
export function checkRequiresOnImplemented(graph) {
    const violations = []
    for (const [num, entry] of graph) {
        for (const dep of entry.requires) {
            if (graph.get(dep)?.status === 'Implemented') {
                violations.push({ file: entry.file, num, dep })
            }
        }
    }

    if (violations.length === 0) return null

    const report = violations
        .map(({ file, num, dep }) =>
            `  ${file}: RFD ${num} requires RFD ${dep}, which is Implemented`)
        .join('\n')
    return `\`Requires\` on an Implemented RFD:\n${report}\n\n` +
        `\`Requires\` gates promotion on an unbuilt dependency; once the target ` +
        `is Implemented the link is redundant. Remove the \`Requires\` entry ` +
        `(and the matching \`Required by\` back-link), or use \`Extends\` if the ` +
        `relationship is design lineage worth keeping.`
}

// Assemble the full priority board: every published RFD and prioritisable
// draft, annotated with board position (`priority`), what the tickets naming
// it say (see `mergeTickets`), and hard dependencies (`dependsOn`). Entries not
// placed on the board — including terminal RFDs — carry `priority: null`.
//
// Returns the entries alongside the normalized `priority` record so callers
// can tell the prioritised `order` — and its milestone groups, `planned` —
// from the unsorted `backlog` (the cutoff sits at `priority.order.length`).
// Throws when the board references an unknown id (see `checkPriority`), when
// the milestone groups are malformed (see `checkMilestones`), or when a ticket
// links to an RFD that isn't there (see `findBrokenRfdLinks`).
//
// A project without RFDs has an empty board.
export function assembleBoard(settings = readSettings()) {
    const rfds = rfdLocations(settings)
    if (!rfds) return { entries: [], priority: normalizePriority({}) }

    const publishedFiles = listRfds(rfds.dir, PUBLISHED)
    const draftFiles = listRfds(rfds.drafts, DRAFTS)

    const entries = [
        ...buildEntries(rfds.dir, publishedFiles, rfds.path),
        ...buildEntries(rfds.drafts, draftFiles, rfds.draftsPath),
    ]

    // Combined graph so the ordering constraint spans both id spaces (a draft
    // may require a published RFD).
    const graph = new Map([
        ...buildGraph(rfds.dir, publishedFiles),
        ...buildGraph(rfds.drafts, draftFiles),
    ])

    const priority = loadPriority(rfds.priority)
    const tickets = loadTickets(settings)
    mergePriority(entries, priority)
    mergeTickets(entries, tickets, settings)
    mergeDependencies(entries, graph)

    const known = new Set(entries.map(entry => entry.num))
    const error = checkPriority(entries, priority) ??
        checkMilestones(priority.planned) ??
        findBrokenRfdLinks(tickets, known, settings)
    if (error) throw new Error(error)

    return { entries, priority }
}
