import { execFileSync } from 'node:child_process'
import { existsSync, readdirSync } from 'node:fs'
import { resolve } from 'node:path'

import { SETTINGS_FILE, SRC_DIR, readSettings } from './settings.mjs'

// The releases a project made with `jp release`, as the site shows them.
//
// The fragments directory holds a directory for each release, named by its
// version, and at its top level the fragments no release lists yet. The
// `changelog` binary renders both, reading git's history for the credits:
// `changelog render --format json` gives a release's version and date, which
// the index lists, and `--format markdown` its release notes and its upgrade
// guide, which this module places under the route the settings name, adding a
// title and the date.
//
// A release is released once it is tagged, which gives it its date. Until
// then it has no pages, and its notes are on the Unreleased page, which
// `changelog` renders without a version: what no tag ships yet, the fragments
// at the top level and those of every release not yet tagged. Under
// `jp vite serve`, a release in preparation also gets its pages, marked as a
// draft, so its notes can be read as they will appear; the built site never
// has them.
//
// The site therefore builds without `jp`, but `changelog` has to be on `PATH`
// wherever the site is built or served, in a clone that isn't shallow. When it
// can't render a release, the build fails rather than leave the release out.
//
// A tree without the fragments directory has no release pages: a release
// branch, or a release's tag, whose notes live on the main branch.

// The version of the release model this module reads, which `changelog` writes
// as `format`.
const FORMAT = 1

// The page of the changes no release ships yet, in place of a version.
export const UNRELEASED = 'unreleased'

// A release's directory: a semantic version, with no `v` in front.
const VERSION = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/

// The release directories among `names`, newest first.
export function releaseVersions(names) {
    return names
        .filter(name => VERSION.test(name))
        .sort((a, b) => compareVersions(b, a))
}

// `a` against `b`, both semantic versions: negative when `a` is the older.
//
// Pre-release identifiers are compared as text, which keeps `1.0.0-rc.1` below
// `1.0.0`; `jp release` makes no pre-releases.
export function compareVersions(a, b) {
    const [, ...left] = VERSION.exec(a)
    const [, ...right] = VERSION.exec(b)

    for (let index = 0; index < 3; index++) {
        const difference = Number(left[index]) - Number(right[index])
        if (difference !== 0) return difference
    }

    const [pre, otherPre] = [left[3], right[3]]
    if (pre === otherPre) return 0
    if (pre === undefined) return 1
    if (otherPre === undefined) return -1
    return pre < otherPre ? -1 : 1
}

// The workspace root, which `settings.site` names the source directory
// `srcDir` below.
export function workspaceRoot(settings, srcDir = SRC_DIR) {
    if (settings.site === '.') return srcDir

    return resolve(srcDir, ...settings.site.split('/').map(() => '..'))
}

// The arguments `changelog` takes to render the release `version`, or what no
// tag ships yet when it is `null`, from the fragments directory `dir`, with
// the forge, the web address credits link from, and the headings `release`,
// the settings' `release`, names, as `view`, such as `['--format', 'json']`.
export function renderArgs(release, dir, version, view) {
    const args = ['render']
    if (version) args.push(version)
    args.push('--dir', dir)
    if (release.forge) args.push('--forge', release.forge)
    if (release.repository) args.push('--repository', release.repository)
    for (const [type, heading] of Object.entries(release.headings ?? {})) {
        args.push('--heading', `${type}=${heading}`)
    }

    return [...args, ...view]
}

// Where the site serves the release notes of `name`, a version or
// `unreleased`, below the releases' `route`.
export function notesPath(route, name) {
    return `/${route}/${name}`
}

// Where the site serves the upgrade guide of `name`, a version or
// `unreleased`, below the releases' `route`.
export function upgradePath(route, name) {
    return `/${route}/${name}/upgrade`
}

// The releases among `releases`, each a version and the model `changelog`
// wrote of it, that a tag has made: the ones with a date.
export function released(releases) {
    return releases.filter(({ release }) => release.date)
}

// Whether releases in preparation get draft pages: under `jp vite serve`,
// which names the workspace root in `JP_VITE_SERVE` of the environment `env`.
export function drafting(env = process.env) {
    return Boolean(env.JP_VITE_SERVE)
}

// The releases among `releases` that get pages: each one a tag has made, and
// with `drafts`, each one in preparation too.
export function shown(releases, drafts) {
    return drafts ? releases : released(releases)
}

// What opens a draft page of the release `version`, below its title: a
// warning that it can still change, and that the built site doesn't show it.
function draftNotice(version) {
    return '> [!WARNING]\n'
        + `> Draft: ${version} isn't tagged yet, so this page can still change.\n`
        + '> Only `jp vite serve` shows it; until the tag, the built site lists these changes on '
        + 'the Unreleased page.\n'
}

// The release notes page of `release`, the model `changelog` wrote, around
// its `notes`: a title, and when it was released, or a draft's warning while
// it has no date.
export function notesContent(release, notes) {
    if (!release.version) {
        return `# Unreleased\n\nThe changes no release ships yet.\n\n${notes}`
    }
    if (!release.date) {
        return `# Release ${release.version}\n\n${draftNotice(release.version)}\n${notes}`
    }

    return `# Release ${release.version}\n\nReleased on ${release.date}.\n\n${notes}`
}

// The upgrade guide page of the release `version`, or of the changes no
// release ships yet when it is `null`, around its `guide`, linking back to its
// notes below the releases' `route`; a `draft` opens with a draft's warning.
export function upgradeContent(version, guide, route, draft = false) {
    if (!version) {
        return '# Upgrading to the unreleased changes\n\n'
            + `What the [unreleased changes](${notesPath(route, UNRELEASED)}) require you to do.\n\n`
            + guide
    }

    return `# Upgrading to ${version}\n\n`
        + (draft ? `${draftNotice(version)}\n` : '')
        + `What the changes in [release ${version}](${notesPath(route, version)}) require you `
        + `to do.\n\n${guide}`
}

// What the release pages need from the settings: the workspace root, the
// fragments directory, and the settings' `release`; `null` when the project
// keeps no release notes, or this tree has no fragments directory.
function context() {
    const settings = readSettings()
    const release = settings.release
    if (!release) return null

    const root = workspaceRoot(settings)
    const dir = resolve(root, release.dir)
    if (!existsSync(dir)) return null

    return { release, root, dir }
}

// The versions the fragments directory holds a release of, newest first.
function versions(ctx) {
    const names = readdirSync(ctx.dir, { withFileTypes: true })
        .filter(entry => entry.isDirectory())
        .map(entry => entry.name)

    return releaseVersions(names)
}

// Each release a tag has made, and under `jp vite serve` each one in
// preparation too, newest first, with the model `changelog` wrote of it.
function releases(ctx) {
    const all = versions(ctx).map(version => ({ version, release: model(ctx, version) }))

    return shown(all, drafting())
}

// The pages to build: the changes no release ships yet, as `version` `null`,
// then each release with pages of its own, each with the model `changelog`
// wrote of it.
function pages(ctx) {
    return [{ version: null, release: model(ctx, null) }, ...releases(ctx)]
}

// What `changelog` printed, run with `args` in the workspace root.
//
// Throws when it can't be run or fails, saying why.
function changelog(ctx, args) {
    try {
        return execFileSync('changelog', args, {
            cwd: ctx.root,
            encoding: 'utf-8',
            stdio: ['ignore', 'pipe', 'pipe'],
            maxBuffer: 64 * 1024 * 1024,
        })
    } catch (error) {
        if (error.code === 'ENOENT') {
            throw new Error(
                'The release pages are rendered by the `changelog` binary, which isn\'t on `PATH`. '
                + 'Build it from JP\'s repository, where it is `crates/internal/changelog`.',
            )
        }

        const said = String(error.stderr ?? '').trim() || error.message
        throw new Error(`\`changelog ${args.join(' ')}\` failed: ${said}`)
    }
}

// The release `version`, or what no tag ships yet when it is `null`, as the
// model `changelog` writes.
function model(ctx, version) {
    const json = changelog(ctx, renderArgs(ctx.release, ctx.dir, version, ['--format', 'json']))
    const release = JSON.parse(json)
    if (release.format !== FORMAT) {
        throw new Error(
            `\`changelog\` wrote release format ${release.format}, and this site reads format `
            + `${FORMAT}: build \`changelog\` from the revision of JP this \`jp vite\` came from.`,
        )
    }

    return release
}

// What the release index lists: each release with pages of its own, newest
// first, with its date, `null` for a draft, and its upgrade guide's path when
// it has one; and how many entries no release ships yet.
export function releaseIndex() {
    const ctx = context()
    if (!ctx) return { unreleased: null, releases: [] }

    const route = ctx.release.route
    const unreleased = model(ctx, null)

    return {
        unreleased: { entries: unreleased.entries.length, path: notesPath(route, UNRELEASED) },
        releases: releases(ctx).map(({ version, release }) => ({
            version,
            date: release.date,
            path: notesPath(route, version),
            upgrade: release.entries.some(entry => entry.upgrade)
                ? upgradePath(route, version)
                : null,
        })),
    }
}

// The release notes pages: one for each release with pages of its own, and one
// for the changes no release ships yet.
export function notesPages() {
    const ctx = context()
    if (!ctx) return []

    const route = ctx.release.route
    return pages(ctx).map(({ version, release }) => {
        const name = version ?? UNRELEASED
        const view = ['--format', 'markdown', '--page', 'notes', '--upgrade-url', upgradePath(route, name)]
        const notes = changelog(ctx, renderArgs(ctx.release, ctx.dir, version, view))

        return { params: { version: name }, content: notesContent(release, notes) }
    })
}

// The upgrade guide pages: one for each release with pages of its own and
// upgrade sections, and one for the changes no release ships yet when they have
// any.
export function upgradePages() {
    const ctx = context()
    if (!ctx) return []

    const route = ctx.release.route
    return pages(ctx).flatMap(({ version, release }) => {
        if (!release.entries.some(entry => entry.upgrade)) return []

        const view = ['--format', 'markdown', '--page', 'upgrade']
        const guide = changelog(ctx, renderArgs(ctx.release, ctx.dir, version, view))
        const draft = Boolean(version) && !release.date

        return [{
            params: { version: version ?? UNRELEASED },
            content: upgradeContent(version, guide, route, draft),
        }]
    })
}

// What a dev server renders the release pages again on: the settings, and the
// fragments and overviews. Absolute, so a loader that defers to this module can
// sit anywhere.
export function releaseWatch() {
    const ctx = context()
    if (!ctx) return [SETTINGS_FILE]

    return [SETTINGS_FILE, `${ctx.dir}/**/*.md`]
}

// The paths loader of the release notes, `<route>/[version].md`, which a
// project's `[version].paths.mjs` exports.
//
// `watch` is read when VitePress loads the loader, not when this module is
// imported.
export const notesRoutes = {
    get watch() {
        return releaseWatch()
    },
    paths: () => notesPages(),
}

// The paths loader of the upgrade guides, `<route>/[version]/upgrade.md`, which
// a project's `upgrade.paths.mjs` exports.
export const upgradeRoutes = {
    get watch() {
        return releaseWatch()
    },
    paths: () => upgradePages(),
}
