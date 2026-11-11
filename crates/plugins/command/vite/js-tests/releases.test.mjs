import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
    drafting,
    missingRenderer,
    notesContent,
    notesPath,
    released,
    releaseVersions,
    renderArgs,
    renderer,
    shown,
    upgradeContent,
    upgradePath,
    workspaceRoot,
} from '../site/jp-vite/releases.mjs'

// The warning a draft page opens with, below its title, for release 0.4.0.
const DRAFT_NOTICE = '> [!WARNING]\n'
    + '> Draft: 0.4.0 isn\'t tagged yet, so this page can still change.\n'
    + '> Only `jp vite serve` shows it; until the tag, the built site lists these changes on the '
    + 'Unreleased page.\n'

// The settings' `release`, as `jp vite sync` writes it for JP.
const RELEASE = {
    dir: 'changelog.d',
    route: 'releases',
    forge: 'github',
    repository: 'https://github.com/dcdpr/jp',
    headings: { added: 'New features', fixed: 'Bug fixes' },
}

test('the release directories are listed newest first, and nothing else is', () => {
    const names = ['0.2.0', 'highlights', '0.10.0', '0.9.1', '1.0.0-rc.1', '1.0.0', 'v0.3.0', '01.0.0', 'README.md']

    assert.deepEqual(releaseVersions(names), ['1.0.0', '1.0.0-rc.1', '0.10.0', '0.9.1', '0.2.0'])
})

test('changelog renders with the forge, web address, and headings the settings name', () => {
    assert.deepEqual(renderArgs(RELEASE, '/work/changelog.d', '0.3.0', ['--format', 'json']), [
        'render',
        '0.3.0',
        '--dir',
        '/work/changelog.d',
        '--forge',
        'github',
        '--repository',
        'https://github.com/dcdpr/jp',
        '--heading',
        'added=New features',
        '--heading',
        'fixed=Bug fixes',
        '--format',
        'json',
    ])
})

// Without a version, `changelog` renders what no tag ships yet; without a
// forge or a web address, it reads them from git's `origin`.
test('the unreleased changes render without a version, and what is left unset is left out', () => {
    const release = { ...RELEASE, forge: null, repository: null, headings: {} }

    assert.deepEqual(renderArgs(release, '/work/changelog.d', null, ['--format', 'json']), [
        'render',
        '--dir',
        '/work/changelog.d',
        '--format',
        'json',
    ])
})

test('the workspace root is the source directory climbed back out of', () => {
    assert.equal(workspaceRoot({ site: 'docs' }, '/work/docs'), '/work')
    assert.equal(workspaceRoot({ site: 'site/docs' }, '/work/site/docs'), '/work')
    assert.equal(workspaceRoot({ site: '.' }, '/work'), '/work')
})

test('the notes and the upgrade guide of a release sit below the releases route', () => {
    assert.equal(notesPath('releases', '0.3.0'), '/releases/0.3.0')
    assert.equal(upgradePath('releases', '0.3.0'), '/releases/0.3.0/upgrade')
    assert.equal(notesPath('releases', 'unreleased'), '/releases/unreleased')
})

test('the notes of a release open with its title and the day it was released', () => {
    const release = { version: '0.3.0', date: '2026-10-28' }

    assert.equal(
        notesContent(release, 'JP 0.3.0 fans out.\n'),
        '# Release 0.3.0\n\nReleased on 2026-10-28.\n\nJP 0.3.0 fans out.\n',
    )
})

// A release without a date has no tag yet: its notes are on the Unreleased
// page until it is tagged.
test('only a release a tag has made gets pages of its own', () => {
    const releases = [
        { version: '0.4.0', release: { version: '0.4.0', date: null } },
        { version: '0.3.0', release: { version: '0.3.0', date: '2026-10-28' } },
    ]

    assert.deepEqual(released(releases).map(({ version }) => version), ['0.3.0'])
})

// `jp vite serve` names the workspace root in `JP_VITE_SERVE`; a build sets
// nothing.
test('only the dev server shows drafts', () => {
    assert.equal(drafting({ JP_VITE_SERVE: '/work' }), true)
    assert.equal(drafting({}), false)
})

// `jp vite serve` and `jp vite build` name their own binary in `JP_VITE_BIN`.
test('the binary jp vite names renders the release pages', () => {
    assert.deepEqual(renderer({ JP_VITE_BIN: '/opt/jp/jp-vite' }), {
        program: '/opt/jp/jp-vite',
        prefix: ['changelog'],
    })
})

test('a build vitepress runs by itself renders with changelog from PATH', () => {
    assert.deepEqual(renderer({}), { program: 'changelog', prefix: [] })
})

test('a missing changelog points at jp vite build', () => {
    assert.equal(
        missingRenderer('changelog'),
        'The release pages are rendered by the `changelog` binary, which isn\'t on `PATH`. '
            + 'Build the site with `jp vite build`, which renders them itself, or build '
            + '`changelog` from JP\'s repository, where it is `crates/internal/changelog`.',
    )
    assert.equal(
        missingRenderer('/opt/jp/jp-vite'),
        'The release pages are rendered by /opt/jp/jp-vite, which can\'t be run.',
    )
})

test('under the dev server, a release in preparation gets pages of its own too', () => {
    const releases = [
        { version: '0.4.0', release: { version: '0.4.0', date: null } },
        { version: '0.3.0', release: { version: '0.3.0', date: '2026-10-28' } },
    ]

    assert.deepEqual(shown(releases, false).map(({ version }) => version), ['0.3.0'])
    assert.deepEqual(shown(releases, true).map(({ version }) => version), ['0.4.0', '0.3.0'])
})

test('the notes of a release in preparation open with a draft warning', () => {
    const release = { version: '0.4.0', date: null }

    assert.equal(
        notesContent(release, 'JP 0.4.0 fans out.\n'),
        `# Release 0.4.0\n\n${DRAFT_NOTICE}\nJP 0.4.0 fans out.\n`,
    )
})

test('the upgrade guide of a release in preparation opens with a draft warning', () => {
    assert.equal(
        upgradeContent('0.4.0', '## Rebuild your plugins\n', 'releases', true),
        `# Upgrading to 0.4.0\n\n${DRAFT_NOTICE}\n`
            + 'What the changes in [release 0.4.0](/releases/0.4.0) require you to do.\n\n'
            + '## Rebuild your plugins\n',
    )
})

test('the unreleased changes have a page of their own', () => {
    const release = { version: null, date: null }

    assert.equal(
        notesContent(release, '1 entry is unreleased: Fixed (1).\n'),
        '# Unreleased\n\nThe changes no release ships yet.\n\n1 entry is unreleased: Fixed (1).\n',
    )
})

test('an upgrade guide links back to the notes of its release', () => {
    assert.equal(
        upgradeContent('0.3.0', '## Rebuild your plugins\n', 'releases'),
        '# Upgrading to 0.3.0\n\nWhat the changes in [release 0.3.0](/releases/0.3.0) require you to '
            + 'do.\n\n## Rebuild your plugins\n',
    )
    assert.match(upgradeContent(null, '## Rebuild\n', 'releases'), /\[unreleased changes\]\(\/releases\/unreleased\)/)
})
