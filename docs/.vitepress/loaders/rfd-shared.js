import { createHash } from 'node:crypto'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

// Shared parsing and validation for the RFD data loaders.
//
// `rfds.data.js` (published) and `rfd-drafts.data.js` (drafts) both build on
// these helpers. The validation functions are pure: they read files and
// return a formatted error message, or `null` when the check passes. The
// caller decides severity — published RFDs throw, drafts warn (except
// duplicate ids, which abort either way).

// Parse the inline metadata from an RFD markdown file.
//
// RFDs use `- **Key**: Value` lines instead of YAML frontmatter, so we need a
// small custom parser. Handles both permanent (`NNN`) and draft (`DNN`) ids.
export function parseMeta(content, filename) {
    const num = filename.match(/^(\d{3}|D\d{2})/)?.[1] ?? '000'
    const title = content.match(/^# RFD (?:\d+|D\d+):\s*(.+)/m)?.[1]?.trim() ?? filename

    const field = (key) =>
        content.match(new RegExp(`^- \\*\\*${key}\\*\\*:\\s*(.+)`, 'm'))?.[1]?.trim() ?? null

    return {
        num,
        title,
        status: field('Status'),
        category: field('Category'),
        authors: field('Authors'),
        date: field('Date'),
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
    const pattern = /\b(\d{3}|D\d{2})-[a-z0-9-]+(?:\.md)?/g
    let match
    while ((match = pattern.exec(content)) !== null) {
        const num = match[1]
        if (num !== '000' && num !== ownNum) refs.add(num)
    }
    return [...refs].sort()
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
        }
    })

    for (const rfd of rfds) {
        rfd.referencedBy = rfds
            .filter(other => other.references.includes(rfd.num))
            .map(other => other.num)
    }

    return rfds
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
