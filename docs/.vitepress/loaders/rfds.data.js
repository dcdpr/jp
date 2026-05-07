import { createHash } from 'node:crypto'
import { readFileSync, readdirSync } from 'node:fs'
import { resolve } from 'node:path'

const rfdDir = resolve(import.meta.dirname, '../../rfd')
const cachePath = resolve(import.meta.dirname, '../rfd-summaries.json')

function loadSummaries() {
    try {
        return JSON.parse(readFileSync(cachePath, 'utf-8'))
    } catch {
        return {}
    }
}

// Parse the inline metadata from an RFD markdown file.
//
// RFDs use `- **Key**: Value` lines instead of YAML frontmatter, so we
// need a small custom parser.
function parseMeta(content, filename) {
    const num = filename.match(/^(\d{3})/)?.[1] ?? '000'
    const title = content.match(/^# RFD \d+:\s*(.+)/m)?.[1]?.trim() ?? filename

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
// Matches patterns like `NNN-slug.md`, `NNN-slug)`, `./NNN-slug`.
// Draft references (DNN-slug) are not tracked — drafts are not published
// and published RFDs are not allowed to link to them.
function parseReferences(content, ownNum) {
    const refs = new Set()
    const pattern = /\b\d{3}-[a-z0-9-]+(?:\.md)?/g
    let match
    while ((match = pattern.exec(content)) !== null) {
        const num = match[0].slice(0, 3)
        if (num !== '000' && num !== ownNum) refs.add(num)
    }
    return [...refs].sort()
}

export default {
    load() {
        const files = readdirSync(rfdDir)
            .filter(f => /^\d{3}-.+\.md$/.test(f) && !f.startsWith('000-'))
            .sort()

        const summaries = loadSummaries()

        const missing = []
        const stale = []
        for (const f of files) {
            const entry = summaries[f]
            if (!entry?.summary) {
                missing.push(f)
                continue
            }
            const content = readFileSync(resolve(rfdDir, f))
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
        if (problems.length > 0) {
            throw new Error(
                `${problems.join('. ')}. Run \`just rfd-summaries\` to update.`
            )
        }

        // Validate: no DNN-style references in published RFDs.
        //
        // Drafts (`DNN-slug.md`) live under `rfd/drafts/` and are not
        // published. A `D\d\d` token in a published RFD is either a stale
        // promotion artefact (an RFD that was promoted from DNN to NNN
        // without its internal references being rewritten) or an
        // accidental cross-link to a draft — in any context, including
        // code blocks. The only RFDs that may legitimately mention `D\d\d`
        // are those that describe the lifecycle or numbering convention
        // itself; those are listed here and skipped entirely.
        const dnnAllowlist = new Set([
            '001-jp-rfd-process.md',
        ])

        const strays = []
        for (const f of files) {
            if (dnnAllowlist.has(f)) continue
            const content = readFileSync(resolve(rfdDir, f), 'utf-8')
            const lines = content.split('\n')
            const hits = []
            for (let i = 0; i < lines.length; i++) {
                for (const m of lines[i].matchAll(/\bD\d\d\b/g)) {
                    hits.push({ line: i + 1, id: m[0] })
                }
            }
            if (hits.length > 0) strays.push({ file: f, hits })
        }

        if (strays.length > 0) {
            const report = strays
                .flatMap(({ file, hits }) =>
                    hits.map(({ line, id }) => `  ${file}:${line}: ${id}`)
                )
                .join('\n')
            throw new Error(
                `DNN-style references found in published RFDs:\n` +
                report + '\n\n' +
                `Drafts are not published; published RFDs must not reference ` +
                `them. If this RFD legitimately describes the DNN numbering ` +
                `convention, add it to \`dnnAllowlist\` in ` +
                `\`docs/.vitepress/loaders/rfds.data.js\`.`
            )
        }

        // Validate the dependency graph implied by `Requires` and `Extends`.
        //
        // The two relationships are unified for enforcement: an extension is
        // a kind of dependency (`Extends ⊆ Requires`). The gate, the
        // duplicate check, and the cycle detector all operate on the union.
        //
        // Three checks:
        //
        // 1. **No duplicates.** A target must not appear under both `Requires`
        //    and `Extends` on the same RFD. Same for the inverse pair
        //    (`Required by` / `Extended by`). Extension already implies the
        //    dependency — listing both is redundant and a future maintenance
        //    hazard.
        // 2. **Status gate.** An RFD with status `Accepted` must not depend on
        //    an RFD that is below `Accepted`. An RFD with status `Implemented`
        //    must not depend on an RFD that is below `Implemented`.
        //    `Superseded` counts as both (the dependency was satisfied at
        //    some point).
        // 3. **Cycle detection.** A → B → ... → A is rejected. The justfile
        //    `rfd-require` / `rfd-extend` recipes already refuse to *write*
        //    a cycle; the loader is the unconditional CI backstop for
        //    hand-edits.
        //
        // Drafts are not loaded here, so all checks apply only to the
        // published graph. The DNN check above already covers the orthogonal
        // case of a published RFD pointing at a draft.
        const ACCEPTED_PLUS = new Set(['Accepted', 'Implemented', 'Superseded'])
        const IMPLEMENTED_PLUS = new Set(['Implemented', 'Superseded'])

        // Extract a `- **Field**: ...` line and pull the 3-digit RFD numbers
        // out of it. Drafts can't appear here in published RFDs (DNN check
        // enforces), so we match 3-digit numbers only.
        const parseField = (content, field) => {
            const re = new RegExp(`^- \\*\\*${field}\\*\\*:\\s*(.+)$`, 'm')
            const line = content.match(re)?.[1] ?? ''
            return [...line.matchAll(/\bRFD\s+(\d{3})\b/g)].map(m => m[1])
        }

        // First parse pass: extract relationship metadata into a small graph.
        const graph = new Map()
        for (const f of files) {
            const content = readFileSync(resolve(rfdDir, f), 'utf-8')
            const num = f.match(/^(\d{3})/)?.[1]
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

        // 1. No-duplicate check across `Requires` / `Extends` (and the inverse
        //    pair `Required by` / `Extended by`).
        const duplicates = []
        for (const [num, entry] of graph) {
            const reqSet = new Set(entry.requires)
            for (const e of entry.extends_) {
                if (reqSet.has(e)) {
                    duplicates.push({
                        file: entry.file,
                        num,
                        dep: e,
                        fields: ['Requires', 'Extends'],
                    })
                }
            }
            const reqBySet = new Set(entry.requiredBy)
            for (const e of entry.extendedBy) {
                if (reqBySet.has(e)) {
                    duplicates.push({
                        file: entry.file,
                        num,
                        dep: e,
                        fields: ['Required by', 'Extended by'],
                    })
                }
            }
        }

        if (duplicates.length > 0) {
            const report = duplicates
                .map(({ file, num, dep, fields }) =>
                    `  ${file}: RFD ${num} lists RFD ${dep} under both '${fields[0]}' and '${fields[1]}'`
                )
                .join('\n')
            throw new Error(
                `Duplicate relationship metadata on published RFDs:\n` +
                report + '\n\n' +
                `Extension implies dependency — don't list the same target ` +
                `under both fields. Drop one entry; 'Extends' is the more ` +
                `specific of the pair.`
            )
        }

        // Build the unified deps map (Requires ∪ Extends) for the gate and
        // cycle detection. With the no-duplicate invariant just enforced, the
        // union has no double-counting.
        const deps = new Map()
        for (const [num, entry] of graph) {
            deps.set(num, [...new Set([...entry.requires, ...entry.extends_])])
        }

        // 2. Status gate.
        const gateViolations = []
        for (const [num, entry] of graph) {
            const allowed = entry.status === 'Accepted' ? ACCEPTED_PLUS
                          : entry.status === 'Implemented' ? IMPLEMENTED_PLUS
                          : null
            if (!allowed) continue
            for (const dep of deps.get(num) ?? []) {
                const depEntry = graph.get(dep)
                if (!depEntry) {
                    gateViolations.push({
                        file: entry.file, num, status: entry.status,
                        dep, depStatus: '(not found)',
                    })
                    continue
                }
                if (!allowed.has(depEntry.status)) {
                    gateViolations.push({
                        file: entry.file, num, status: entry.status,
                        dep, depStatus: depEntry.status,
                    })
                }
            }
        }

        if (gateViolations.length > 0) {
            const report = gateViolations
                .map(({ file, num, status, dep, depStatus }) =>
                    `  ${file}: RFD ${num} (${status}) depends on RFD ${dep} (${depStatus})`
                )
                .join('\n')
            throw new Error(
                `Promotion gate violations on published RFDs:\n` +
                report + '\n\n' +
                `Accepted RFDs require deps to be Accepted/Implemented/Superseded; ` +
                `Implemented RFDs require deps to be Implemented/Superseded. ` +
                `Both \`Requires\` and \`Extends\` participate.`
            )
        }

        // 3. Cycle detection on the unified graph (DFS with white/gray/black).
        const cycles = []
        {
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
        }

        if (cycles.length > 0) {
            const report = cycles
                .map(c => '  ' + c.map(n => `RFD ${n}`).join(' → '))
                .join('\n')
            throw new Error(
                `Dependency cycles detected (Requires ∪ Extends):\n` + report
            )
        }

        // First pass: parse metadata and references.
        const rfds = files.map(f => {
            const content = readFileSync(resolve(rfdDir, f), 'utf-8')
            const meta = parseMeta(content, f)
            return {
                ...meta,
                summary: summaries[f]?.summary ?? null,
                references: parseReferences(content, meta.num),
            }
        })

        // Second pass: compute inverse references ("referenced by").
        for (const rfd of rfds) {
            rfd.referencedBy = rfds
                .filter(other => other.references.includes(rfd.num))
                .map(other => other.num)
        }

        return rfds
    },
}
