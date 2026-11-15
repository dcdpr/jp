import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

// The settings `jp vite sync` writes for the site, read on the Node side.
//
// `settings.json` sits next to this file and holds what the loaders and the
// dev server need from the project's configuration: the RFD and ticket
// directories, the RFD categories, the ticket label vocabulary, how much of the
// Done column the board shows, and where links to repository files point. A
// workflow the project doesn't have is `null`.
//
// It is read on every call rather than imported, so a snapshot `jp vite serve`
// rewrites while the site runs is picked up without a restart.

// The VitePress source directory. This file lives in its `.vitepress/jp-vite/`.
export const SRC_DIR = resolve(import.meta.dirname, '../..')

// Where the snapshot is, for loaders to watch.
export const SETTINGS_FILE = resolve(import.meta.dirname, 'settings.json')

// The settings snapshot at `file`, `SETTINGS_FILE` by default.
//
// Throws, naming the file, when it can't be read or doesn't hold JSON.
export function readSettings(file = SETTINGS_FILE) {
    const source = readFileSync(file, 'utf-8')
    try {
        return JSON.parse(source)
    } catch (error) {
        throw new Error(`${file} is not valid JSON: ${error.message}.`)
    }
}

// The directory one workflow's documents live in, or `null` when the project
// doesn't have that workflow. `kind` is `rfd` or `ticket`.
export function workflowDir(settings, kind) {
    const workflow = settings[kind]
    return workflow ? resolve(SRC_DIR, workflow.route) : null
}

// The site path one workflow's pages are served under, such as `/rfd`, or
// `null` when the project doesn't have that workflow.
export function workflowPath(settings, kind) {
    const workflow = settings[kind]
    return workflow ? `/${workflow.route}` : null
}
