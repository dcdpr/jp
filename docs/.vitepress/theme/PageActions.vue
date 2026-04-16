<script setup>
import { useData } from 'vitepress'
import { computed } from 'vue'

const { page, frontmatter } = useData()

const mdPath = computed(() => {
    const rel = page.value.relativePath
    if (!rel) return null
    return '/' + rel
})

const visible = computed(() =>
    frontmatter.value.layout !== 'home' && !!mdPath.value
)
</script>

<template>
    <div v-if="visible" class="page-actions">
        <a :href="mdPath" class="page-actions-link" target="_blank" title="View raw markdown source">
            View Markdown
        </a>
    </div>
</template>
