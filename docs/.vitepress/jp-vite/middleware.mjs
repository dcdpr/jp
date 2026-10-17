import { spawn } from 'node:child_process'
import { readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'

import { assembleBoard, COLUMNS, inDevelopmentRfds, loadTickets } from './loaders/ticket-shared.mjs'
import { readSettings, SRC_DIR, workflowDir } from './settings.mjs'

// The dev server's endpoints for the boards and the ticket index.
//
// Reads are answered on any dev server. Writes are accepted only under
// `jp vite serve`, which names the workspace root in `JP_VITE_SERVE`, and only
// when the checks it ran before starting the site passed: a failure is in
// `JP_VITE_PROBLEM`, and a site that may misread its documents must not write
// them either.
//
// Every write but the priority board's goes through `jp`, the one
// implementation of the document formats, with the directories the site shows
// pinned by `--cfg`: a configuration changed since the settings were written
// can't move a write away from what the site displays. The priority board
// writes its file itself, because it posts the whole layout, milestones
// included, which no `jp rfd` command takes.

// Why this dev server refuses writes, or `null` when it accepts them.
export function writeRefusal() {
    if (process.env.JP_VITE_PROBLEM) return process.env.JP_VITE_PROBLEM
    if (!process.env.JP_VITE_SERVE) {
        return 'This dev server is read-only. Start the site with `jp vite serve` to edit it.'
    }
    return null
}

// How long a `jp` command a request waits on may run. None of them waits on a
// model, and each takes a second or two, so one still running after this is
// stuck.
const COMMAND_TIMEOUT = 15 * 1000

// How long reading an RFD's Implementation Plan may run. It waits on a model,
// and runs in the background, so nothing waits on it.
const PLAN_TIMEOUT = 5 * 60 * 1000

// Run `jp` with `args`, the site's directories pinned with `--cfg`.
//
// It runs in the workspace root `jp vite serve` named, or in the VitePress
// source directory on a dev server started another way, which only reads.
//
// It runs in a session of its own, with no terminal. `jp vite serve` runs in a
// background process group of the user's terminal, so a `jp` that kept that
// terminal would be stopped by the first read or mode change it made there, and
// nothing would resume it. There is nobody to answer a prompt either, hence
// `--no-interactive` and no stdin.
//
// A command still running after `timeout` is killed with its whole process
// group.
function jp(args, timeout = COMMAND_TIMEOUT) {
    const settings = readSettings()
    const pinned = ['--no-interactive']
    if (settings.rfd) pinned.push('--cfg', `plugins.command.rfd.options.dir=${settings.rfd.dir}`)
    if (settings.ticket) {
        pinned.push('--cfg', `plugins.command.ticket.options.dir=${settings.ticket.dir}`)
    }

    return new Promise((done) => {
        const command = `jp ${args.join(' ')}`
        let stdout = ''
        let stderr = ''
        let timedOut = false
        let settled = false
        let timer = null

        // A child that fails to start reports `error` and may report `close`
        // after it, so whichever comes first answers.
        const finish = (ok, output) => {
            if (settled) return
            settled = true
            clearTimeout(timer)
            if (!ok) console.warn(`[jp-vite] ${command} failed:\n${output}`)
            done({ ok, output: output.trim(), notes: stderr.trim() })
        }

        const child = spawn('jp', [...pinned, ...args], {
            cwd: process.env.JP_VITE_SERVE || SRC_DIR,
            detached: true,
            stdio: ['ignore', 'pipe', 'pipe'],
            windowsHide: true,
        })
        child.stdout.setEncoding('utf-8').on('data', (chunk) => { stdout += chunk })
        child.stderr.setEncoding('utf-8').on('data', (chunk) => { stderr += chunk })

        timer = setTimeout(() => {
            timedOut = true
            killGroup(child)
        }, timeout)

        child.on('error', (err) => finish(false, `${command}: ${err.message ?? err}`))
        child.on('close', (code) => {
            if (timedOut) {
                // Its stderr names the trace log, which says where it stopped.
                return finish(false, `${command} gave no answer within ` +
                    `${Math.round(timeout / 1000)} seconds and was stopped.\n${stderr}`)
            }
            finish(code === 0, stdout || stderr || `${command} exited with ${code}`)
        })
    })
}

// Kill a detached child and everything else in the process group it leads.
//
// `SIGKILL` rather than `SIGTERM`: `jp` takes `SIGTERM` as a request to shut
// down gracefully, which it acts on only where it checks for one, and a process
// its terminal has stopped doesn't act on `SIGTERM` at all.
function killGroup(child) {
    try {
        process.kill(process.platform === 'win32' ? child.pid : -child.pid, 'SIGKILL')
    } catch {
        child.kill('SIGKILL')
    }
}

// Read a JSON request body of at most `limit` bytes.
//
// Resolves to `undefined` once it has answered the request itself, because the
// body was too large or not JSON.
function readJson(req, res, limit) {
    return new Promise((done) => {
        let body = ''
        let tooBig = false
        req.on('data', (chunk) => {
            if (tooBig) return
            body += chunk
            if (body.length > limit) {
                tooBig = true
                res.statusCode = 413
                res.end('payload too large')
                req.destroy()
                done(undefined)
            }
        })
        req.on('end', () => {
            if (tooBig) return
            try {
                done(JSON.parse(body))
            } catch {
                res.statusCode = 400
                res.end('invalid JSON')
                done(undefined)
            }
        })
    })
}

// `handler` as connect middleware.
//
// The handlers are async, and connect only catches what a handler throws
// synchronously: a rejection would leave the request open. Passing it to `next`
// answers it with a 500 instead.
function endpoint(handler) {
    return (req, res, next) => {
        handler(req, res, next).catch(next)
    }
}

function reply(res, code, payload) {
    res.statusCode = code
    res.setHeader('Content-Type', 'application/json')
    res.end(JSON.stringify(payload))
}

// Answer in plain text, which is how the boards read a rejected request.
function refuse(res, code, message) {
    res.statusCode = code
    res.end(message)
}

// The RFD priority board (`/rfd/priority`).
//
// The board POSTs the reordered layout here, and this persists it to
// `.priority.json` in the RFD directory the settings name.
const rfdPriorityWriter = {
    name: 'rfd-priority-writer',
    configureServer(server) {
        server.middlewares.use('/__rfd-priority', endpoint(async (req, res, next) => {
            const dir = workflowDir(readSettings(), 'rfd')
            if (!dir) return refuse(res, 404, 'This project has no RFDs.')

            // Dotted, and deliberately not `priority.json`: a page lives at
            // `/rfd/priority`, and Vite's module resolver would answer a cold
            // document request for it with the JSON transformed into an ES
            // module.
            const file = resolve(dir, '.priority.json')

            // The board fetches this on mount to bypass the data loader's
            // dev-lifetime cache, so a refresh reflects the saved file.
            if (req.method === 'GET') {
                res.setHeader('Content-Type', 'application/json')
                try {
                    res.end(readFileSync(file))
                } catch {
                    res.end('{"planned":[],"backlog":[]}')
                }
                return
            }

            if (req.method !== 'POST') return next()

            const refusal = writeRefusal()
            if (refusal) return refuse(res, 403, refusal)

            const parsed = await readJson(req, res, 256 * 1024)
            if (parsed === undefined) return

            const isStrArray = (v) =>
                Array.isArray(v) && v.every((x) => typeof x === 'string')
            const isGroup = (g) =>
                g !== null &&
                typeof g === 'object' &&
                (g.milestone === null || typeof g.milestone === 'string') &&
                isStrArray(g.ids)
            if (
                !Array.isArray(parsed.planned) ||
                !parsed.planned.every(isGroup) ||
                !isStrArray(parsed.backlog)
            ) {
                return refuse(res, 400, 'expected { planned: [{ milestone, ids }], backlog }')
            }

            const out = {
                planned: parsed.planned.map((g) => ({
                    milestone: g.milestone,
                    ids: g.ids,
                })),
                backlog: parsed.backlog,
            }
            try {
                writeFileSync(file, JSON.stringify(out, null, 2) + '\n')
            } catch (err) {
                return refuse(res, 500, String(err))
            }

            reply(res, 200, { ok: true })
        }))
    },
}

// The ticket board (`/ticket/board`).
//
// Two pieces of state, kept apart (see RFD 100): the order within a column
// lives in the board file, `.board.json`, and a ticket's status lives in the
// ticket file. A drag posts one move — the ticket, the column it landed in, and
// its neighbours there — which becomes `jp ticket status` when it changed
// column and `jp ticket rank` against a neighbour.
const ticketBoardWriter = {
    name: 'ticket-board-writer',
    configureServer(server) {
        server.middlewares.use('/__ticket-board', endpoint(async (req, res, next) => {
            if (req.method === 'GET') {
                try {
                    const { columns } = assembleBoard()
                    return reply(res, 200, { columns })
                } catch (err) {
                    return refuse(res, 500, String(err?.message ?? err))
                }
            }

            if (req.method !== 'POST') return next()

            const refusal = writeRefusal()
            if (refusal) return refuse(res, 403, refusal)

            const parsed = await readJson(req, res, 4 * 1024)
            if (parsed === undefined) return

            // Arguments are built here rather than taken from the request, so
            // a request can only ask for what the CLI already offers.
            const ticketId = (v) => (typeof v === 'string' && /^T-[0-9a-z]{7}$/.test(v) ? v : null)
            const id = ticketId(parsed.id)
            const column = COLUMNS.find((c) => c.status === parsed.status)
            const after = ticketId(parsed.after)
            const before = ticketId(parsed.before)
            if (!id || !column) {
                return refuse(res, 400, 'expected { id, status, moved, after, before }')
            }

            const said = []
            if (parsed.moved === true) {
                const moved = await jp(['ticket', 'status', id, column.status])
                if (!moved.ok) return reply(res, 500, moved)
                said.push(moved.output)
            }

            // Ranked against the ticket above it, or the one below when it
            // landed at the top. Alone in its column, it needs no rank.
            const place = after ? ['--after', after] : before ? ['--before', before] : null
            if (place) {
                const ranked = await jp(['ticket', 'rank', id, ...place])
                if (!ranked.ok) return reply(res, 500, ranked)
                said.push(ranked.output)
            }

            reply(res, 200, { ok: true, output: said.join('\n') })
        }))
    },
}

// Ticket management on the ticket index: add, edit, delete.
//
// Every write shells out to `jp ticket`, which is the one implementation of the
// ticket format. Reimplementing id allocation, slugs, and the file template
// here would be a second writer to keep in step, and they would drift.
const ticketWriter = {
    name: 'ticket-writer',
    configureServer(server) {
        server.middlewares.use('/__ticket', endpoint(async (req, res, next) => {
            // A GET reads one ticket back, which is how the edit form gets the
            // current description: the index loader doesn't carry it.
            if (req.method === 'GET') {
                const id = new URL(req.url, 'http://localhost').searchParams.get('id')
                if (!id) return reply(res, 400, { ok: false, output: 'missing id' })

                const { ok, output } = await jp(['ticket', 'show', id, '--json'])
                if (!ok) return reply(res, 500, { ok, output })

                try {
                    return reply(res, 200, { ok: true, ...JSON.parse(output) })
                } catch {
                    return reply(res, 500, { ok: false, output: 'unreadable ticket JSON' })
                }
            }

            if (req.method !== 'POST') return next()

            const refusal = writeRefusal()
            if (refusal) return reply(res, 403, { ok: false, output: refusal })

            const parsed = await readJson(req, res, 256 * 1024)
            if (parsed === undefined) return

            // Flags are built here rather than taken from the client, so a
            // request can only ask for what the CLI already offers.
            const text = (value) => (typeof value === 'string' ? value : '')
            const flag = (name, value) => (text(value) ? [name, text(value)] : [])
            let args
            switch (parsed.action) {
                case 'add':
                    if (!text(parsed.kind) || !text(parsed.title)) {
                        return refuse(res, 400, 'add needs a kind and a title')
                    }
                    args = ['ticket', 'add', text(parsed.kind), text(parsed.title),
                        ...flag('--body', parsed.body)]
                    break
                case 'edit': {
                    // A title is set by `rename`, which also moves the file to
                    // the new slug; the rest is one `edit`.
                    if (text(parsed.title)) {
                        const renamed = await jp(['ticket', 'rename', text(parsed.id), text(parsed.title)])
                        if (!renamed.ok) return reply(res, 500, renamed)
                    }
                    args = ['ticket', 'edit', text(parsed.id),
                        ...flag('--body', parsed.body),
                        ...flag('--kind', parsed.kind),
                        ...flag('--status', parsed.status)]
                    break
                }
                case 'delete':
                    args = ['ticket', 'delete', text(parsed.id)]
                    break
                default:
                    return refuse(res, 400, 'expected action: add, edit, or delete')
            }

            const { ok, output } = await jp(args)
            reply(res, ok ? 200 : 500, { ok, output })
        }))
    },
}

// The RFDs whose Implementation Plan is being read in the background, so marking
// one again while its read runs doesn't start a second.
const planReads = new Set()

// Read RFD `num`'s Implementation Plan into its tracking ticket, in the
// background.
//
// `jp rfd track` asks a model for the plan, which takes a minute or more, and
// does nothing when the ticket holds one already. Nobody waits on it: the board
// has moved on, so what it did goes to the dev server's console.
function readPlanInBackground(num) {
    if (planReads.has(num)) return
    planReads.add(num)

    jp(['rfd', 'track', num], PLAN_TIMEOUT)
        .then(({ ok, notes }) => {
            if (ok && notes) console.info(`[jp-vite] RFD ${num}: ${notes}`)
        })
        .finally(() => planReads.delete(num))
}

// An RFD's in-development state (`/rfd/priority`).
//
// The state lives on the RFD's tracking ticket, so a write shells out to
// `jp rfd start` / `jp rfd stop` rather than touching a file. Marking an RFD
// files that ticket when it has none; the ticket is filed with a marker in place
// of the plan, and the plan is read in the background, so the request answers
// without waiting on a model.
const rfdDevelopmentWriter = {
    name: 'rfd-development-writer',
    configureServer(server) {
        server.middlewares.use('/__rfd-development', endpoint(async (req, res, next) => {
            // The board reads this on mount. Writing a ticket file reloads the
            // page, and the data loader caches for the server's lifetime, so
            // without this the board comes back showing the state it was built
            // with. Read through the site's own parser and rule.
            if (req.method === 'GET') {
                try {
                    return reply(res, 200, { inDevelopment: inDevelopmentRfds(loadTickets()) })
                } catch (err) {
                    return refuse(res, 500, String(err?.message ?? err))
                }
            }

            if (req.method !== 'POST') return next()

            const refusal = writeRefusal()
            if (refusal) return reply(res, 403, { ok: false, output: refusal })

            const parsed = await readJson(req, res, 4 * 1024)
            if (parsed === undefined) return

            // The command is chosen here rather than taken from the request, so
            // a request can only ask for one of the two operations, on a
            // published RFD. Whether that RFD is far enough along to carry a
            // tracking ticket is `jp rfd`'s call.
            if (
                typeof parsed.num !== 'string' ||
                !/^\d{3}$/.test(parsed.num) ||
                typeof parsed.active !== 'boolean'
            ) {
                return refuse(res, 400, 'expected { num: "NNN", active: boolean }')
            }

            if (!parsed.active) {
                const { ok, output } = await jp(['rfd', 'stop', parsed.num])
                return reply(res, ok ? 200 : 500, { ok, output })
            }

            const { ok, output } = await jp(['rfd', 'start', parsed.num, '--no-plan'])
            if (ok) readPlanInBackground(parsed.num)
            reply(res, ok ? 200 : 500, { ok, output })
        }))
    },
}

// Every endpoint, as Vite plugins.
export const devServer = [
    rfdPriorityWriter,
    rfdDevelopmentWriter,
    ticketBoardWriter,
    ticketWriter,
]
