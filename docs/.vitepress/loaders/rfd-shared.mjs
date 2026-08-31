import { createHash } from 'node:crypto'
import { readdirSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import { field, unescapeTitle } from './metadata.mjs'
import {
    checkMilestones,
    normalizePriority,
    TERMINAL_STATUSES,
} from './rfd-priority.mjs'
import { inDevelopmentRfds, loadTickets } from './ticket-shared.mjs'

// Shared parsing and validation for the RFD data loaders.
//
// `rfds.data.js` (published) and `rfd-drafts.data.js` (drafts) both build on
// these helpers. The validation functions are pure: they read files and
// return a formatted error message, or `null` when the check passes. The
// caller decides severity — published RFDs throw, drafts warn (except
// duplicate ids, which abort either way).
//
// `assembleBoard` (bottom of this file) composes the building blocks into the
// full priority board. Both the web board (`rfd-board.data.js`) and the
// `just rfd-list` CLI (`../rfd-list.mjs`) go through it, so the two never
// drift.

// Parse the inline metadata from an RFD markdown file.
//
// RFDs use `- **Key**: Value` lines instead of YAML frontmatter, so we need a
// small custom parser. Handles both permanent (`NNN`) and draft (`DNN`) ids.
export function parseMeta(content, filename) {
    const num = filename.match(/^(\d{3}|D\d{2})/)?.[1] ?? '000'
    const rawTitle = content.match(/^# RFD (?:\d+|D\d+):\s*(.+)/m)?.[1]?.trim() ?? filename
    const title = unescapeTitle(rawTitle)

    return {
        num,
        title,
        status: field(content, 'Status'),
        category: field(content, 'Category'),
        // The RFD number that superseded this one, if any (e.g. `033` -> `034`).
        supersededBy:
            content.match(/^- \*\*Superseded by\*\*:.*?\bRFD\s+(\d{3}|D\d{2})\b/m)?.[1]
            ?? null,
        authors: field(content, 'Authors'),
        date: field(content, 'Date'),
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
// RFDs are matched through their linked filename; the slug must carry a letter,
// so a date like `2026-08-05` isn't read as a link. Tickets are matched through
// the canonical `T-` token, which is distinctive enough to appear in prose.
export function referencedLabels(content) {
    const labels = new Set()

    const rfds = /\b(\d{3}|D\d{2})-[a-z0-9-]*[a-z][a-z0-9-]*(?:\.md)?/g
    let match
    while ((match = rfds.exec(content)) !== null) {
        labels.add(match[1])
    }

    const tickets = /\bT-([0-9a-z]{7})\b/g
    while ((match = tickets.exec(content)) !== null) {
        labels.add(`T-${match[1]}`)
    }

    return [...labels]
}

// Build the RFD entries consumed by the index pages and cross-reference
// widgets. `basePath` is the site path the files live under (`/rfd` or
// `/rfd/drafts`) and feeds each entry's absolute `path`.
//
// `referencedBy` is computed within the given file set only. Published RFDs
// can't reference drafts, so a draft's `referencedBy` lists drafts; a
// published RFD's stays published-only.
export function buildEntries(dir, files, summaries, basePath) {
    const rfds = files.map(f => {
        const content = readFileSync(resolve(dir, f), 'utf-8')
        const meta = parseMeta(content, f)
        return {
            ...meta,
            path: `${basePath}/${meta.slug}`,
            summary: summaries[f]?.summary ?? null,
            references: parseReferences(content, meta.num),
            // Cross-kind links, resolved in the browser against both sets.
            links: referencedLabels(content).filter(label => label !== meta.num),
        }
    })

    for (const rfd of rfds) {
        rfd.referencedBy = rfds
            .filter(other => other.references.includes(rfd.num))
            .map(other => other.num)
    }

    return rfds
}

// Read the priority board state. This is human-curated source of truth: the
// prioritised `planned` milestone groups (see `normalizePriority` for the
// exact shape) and the unsorted `backlog` below the cutoff. Kept deliberately
// separate from the regenerable
// `rfd-summaries.json` cache, so clearing the cache never loses the board. A
// missing file is an empty board.
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

// Mark the RFDs someone is currently implementing.
//
// Derived from the tickets: an RFD is in development when a ticket claiming to
// implement it sits in the In Progress column. The board doesn't record this and
// can't set it — closing the ticket clears the flag on its own.
export function mergeInDevelopment(entries, tickets) {
    const inDev = new Set(inDevelopmentRfds(tickets))
    for (const entry of entries) {
        entry.inDevelopment = inDev.has(entry.num)
    }
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
        `\`docs/rfd/.priority.json\` references RFDs that don't exist. Fix the ` +
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
        `The board ranks work that is still open. Run ` +
        `\`just rfd-board-prune\` to drop these from ` +
        `\`docs/rfd/.priority.json\`.`
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

// Every published RFD needs a current one-line summary in the cache. Drafts
// are exempt (they carry no cached summaries).
export function checkSummaries(dir, files, summaries) {
    const missing = []
    const stale = []
    for (const f of files) {
        const entry = summaries[f]
        if (!entry?.summary) {
            missing.push(f)
            continue
        }
        const content = readFileSync(resolve(dir, f))
        const hash = createHash('sha256').update(content).digest('hex')
        if (hash !== entry.hash) {
            stale.push(f)
        }
    }

    const problems = []
    if (missing.length > 0) {
        const nums = missing.map(f => f.match(/^(\d+)/)?.[1]).join(', ')
        problems.push(`Missing summaries for: ${nums}`)
    }
    if (stale.length > 0) {
        const nums = stale.map(f => f.match(/^(\d+)/)?.[1]).join(', ')
        problems.push(`Stale summaries for: ${nums}`)
    }
    if (problems.length === 0) return null

    return `${problems.join('. ')}. Run \`just rfd-summaries\` to update.`
}

// Reject `DNN`-style references in published RFDs.
//
// Drafts (`DNN-slug.md`) live under `rfd/drafts/` and are not published. A
// `D\d\d` token in a published RFD is either a stale promotion artefact (an
// RFD that was promoted from DNN to NNN without its internal references being
// rewritten) or an accidental cross-link to a draft — in any context,
// including code blocks. The only RFDs that may legitimately mention `D\d\d`
// are those that describe the lifecycle or numbering convention itself; those
// are passed in `allowlist` and skipped entirely.
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
        `them. If this RFD legitimately describes the DNN numbering ` +
        `convention, add it to \`dnnAllowlist\` in ` +
        `\`docs/.vitepress/loaders/rfds.data.js\`.`
}

// Extract a `- **Field**: ...` line and pull the RFD ids out of it.
function parseField(content, field) {
    const re = new RegExp(`^- \\*\\*${field}\\*\\*:\\s*(.+)$`, 'm')
    const line = content.match(re)?.[1] ?? ''
    return [...line.matchAll(/\bRFD\s+(\d{3}|D\d{2})\b/g)].map(m => m[1])
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
// link is redundant. `rfd-promote` strips `Requires` when an RFD itself reaches
// `Implemented`; this check catches the other direction, a dependency that
// became `Implemented` while the dependent RFD is still in flight. Use `Extends`
// instead when the relationship is design lineage worth keeping past
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
// draft, annotated with board position (`priority`), in-development flag (from
// ticket state), and hard dependencies (`dependsOn`). Entries not placed on the
// board — including terminal RFDs — carry `priority: null`.
//
// Returns the entries alongside the normalized `priority` record so callers
// can tell the prioritised `order` — and its milestone groups, `planned` —
// from the unsorted `backlog` (the cutoff sits at `priority.order.length`).
// Throws when the board references an unknown id (see `checkPriority`) or
// when the milestone groups are malformed (see `checkMilestones`).
//
// Paths are resolved from this file's location, so the result is independent
// of the caller's working directory.
export function assembleBoard() {
    const rfdDir = resolve(import.meta.dirname, '../../rfd')
    const draftsDir = resolve(import.meta.dirname, '../../rfd/drafts')
    const cachePath = resolve(import.meta.dirname, '../rfd-summaries.json')
    const priorityPath = resolve(import.meta.dirname, '../../rfd/.priority.json')

    const publishedFiles = readdirSync(rfdDir)
        .filter(f => /^\d{3}-.+\.md$/.test(f) && !f.startsWith('000-'))
        .sort()
    const draftFiles = readdirSync(draftsDir)
        .filter(f => /^D\d{2}-.+\.md$/.test(f))
        .sort()

    let summaries
    try {
        summaries = JSON.parse(readFileSync(cachePath, 'utf-8'))
    } catch {
        summaries = {}
    }

    const entries = [
        ...buildEntries(rfdDir, publishedFiles, summaries, '/rfd'),
        ...buildEntries(draftsDir, draftFiles, {}, '/rfd/drafts'),
    ]

    // Combined graph so the ordering constraint spans both id spaces (a draft
    // may require a published RFD).
    const graph = new Map([
        ...buildGraph(rfdDir, publishedFiles),
        ...buildGraph(draftsDir, draftFiles),
    ])

    const priority = loadPriority(priorityPath)
    mergePriority(entries, priority)
    mergeInDevelopment(entries, loadTickets())
    mergeDependencies(entries, graph)

    const error = checkPriority(entries, priority) ?? checkMilestones(priority.planned)
    if (error) throw new Error(error)

    return { entries, priority }
}
