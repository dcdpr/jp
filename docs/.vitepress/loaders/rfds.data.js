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
