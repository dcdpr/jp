import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
    notesContent,
    notesPath,
    releaseVersions,
    renderArgs,
    upgradeContent,
    upgradePath,
    workspaceRoot,
} from '../site/jp-vite/releases.mjs'

// The settings' `release`, as `jp vite sync` writes it for JP.
const RELEASE = {
    dir: 'changelog.d',
    route: 'releases',
    forge: 'github',
    headings: { added: 'New features', fixed: 'Bug fixes' },
}

test('the release directories are listed newest first, and nothing else is', () => {
    const names = ['0.2.0', 'highlights', '0.10.0', '0.9.1', '1.0.0-rc.1', '1.0.0', 'v0.3.0', '01.0.0', 'README.md']

    assert.deepEqual(releaseVersions(names), ['1.0.0', '1.0.0-rc.1', '0.10.0', '0.9.1', '0.2.0'])
})

test('changelog renders with the forge and headings the settings name', () => {
    assert.deepEqual(renderArgs(RELEASE, '/work/changelog.d', '0.3.0', ['--format', 'json']), [
        'render',
        '0.3.0',
        '--dir',
        '/work/changelog.d',
        '--forge',
        'github',
        '--heading',
        'added=New features',
        '--heading',
        'fixed=Bug fixes',
        '--format',
        'json',
    ])
})

// Without a version, `changelog` renders the fragments no release lists yet;
// without a forge, it reads the one git's `origin` is on.
test('the unreleased changes render without a version, and a forge left unset is left out', () => {
    const release = { ...RELEASE, forge: null, headings: {} }

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

test('the notes of a release in preparation say they can still change', () => {
    const release = { version: '0.3.0', date: null }

    assert.match(notesContent(release, 'Notes.\n'), /^# Release 0\.3\.0\n\nIn preparation: /)
})

test('the unreleased changes have a page of their own', () => {
    const release = { version: null, date: null }

    assert.equal(
        notesContent(release, '1 entry is unreleased: Fixed (1).\n'),
        '# Unreleased\n\nThe changes no release lists yet.\n\n1 entry is unreleased: Fixed (1).\n',
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
