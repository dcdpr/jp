<script setup>
import { withBase } from 'vitepress'

import { data } from '../loaders/releases.data.js'

// The releases as a table, newest first, which a page renders with
// `<ReleaseIndex />`: each one's date, or that it is in preparation, and its
// upgrade guide when it has one. Above the table, a link to the changes no
// release lists yet.

function changes(count) {
    if (count === 0) return 'no changes'
    return count === 1 ? '1 change' : `${count} changes`
}
</script>

<template>
    <p v-if="data.unreleased">
        <a :href="withBase(data.unreleased.path)">Unreleased</a>: {{ changes(data.unreleased.entries) }}
    </p>
    <table v-if="data.releases.length">
        <thead>
            <tr>
                <th>Release</th>
                <th>Date</th>
                <th>Upgrade guide</th>
            </tr>
        </thead>
        <tbody>
            <tr v-for="release in data.releases" :key="release.version">
                <td><a :href="withBase(release.path)">{{ release.version }}</a></td>
                <td>{{ release.date ?? 'In preparation' }}</td>
                <td><a v-if="release.upgrade" :href="withBase(release.upgrade)">Upgrade guide</a></td>
            </tr>
        </tbody>
    </table>
    <p v-else>No release has been made yet.</p>
</template>
