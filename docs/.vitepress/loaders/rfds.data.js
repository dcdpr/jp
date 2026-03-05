import { readFileSync, readdirSync } from 'node:fs'
import { resolve } from 'node:path'

const rfdDir = resolve(import.meta.dirname, '../../rfd')
const cachePath = resolve(import.meta.dirname, '../cache/rfd-summaries.json')

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
    const num = filename.match(/^(\d+)/)?.[1] ?? '000'
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

export default {
    load() {
        const files = readdirSync(rfdDir)
            .filter(f => /^\d{3}-.+\.md$/.test(f) && !f.startsWith('000-'))
            .sort()

        const summaries = loadSummaries()

        const missing = files.filter(f => !summaries[f]?.summary)
        if (missing.length > 0) {
            const nums = missing.map(f => f.match(/^(\d+)/)?.[1]).join(', ')
            throw new Error(
                `Missing RFD summaries for: ${nums}. Run \`just rfd-summaries\` to generate them.`
            )
        }

        return files.map(f => ({
            ...parseMeta(readFileSync(resolve(rfdDir, f), 'utf-8'), f),
            summary: summaries[f].summary,
        }))
    },
}
