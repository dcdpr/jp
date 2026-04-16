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
    const num = filename.match(/^(\d{3}|D\d{2})/)?.[1] ?? '000'
    const title = content.match(/^# RFD [\dA-Z]+:\s*(.+)/m)?.[1]?.trim() ?? filename

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
function parseReferences(content, ownNum) {
    const refs = new Set()
    const pattern = /\b(\d{3}|D\d{2})-[a-z0-9-]+(?:\.md)?/g
    let match
    while ((match = pattern.exec(content)) !== null) {
        const num = match[1]
        if (num !== '000' && num !== ownNum) refs.add(num)
    }
    return [...refs].sort()
}

export default {
    load() {
        const numbered = readdirSync(rfdDir)
            .filter(f => /^\d{3}-.+\.md$/.test(f) && !f.startsWith('000-'))
            .sort()

        const drafts = readdirSync(rfdDir)
            .filter(f => /^D\d{2}-.+\.md$/.test(f))
            .sort()

        const files = [...numbered, ...drafts]

        const summaries = loadSummaries()

        // Only numbered RFDs require summaries; drafts are excluded.
        const missing = []
        const stale = []
        for (const f of numbered) {
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
