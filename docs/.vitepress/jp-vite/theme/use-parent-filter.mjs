import { onBeforeUnmount, onMounted, ref } from 'vue'

import { parseParentFilter, withParentFilter } from './ticket-tree.mjs'

// The parent filter in the page's address, for the ticket board, as
// `use-label-filter.mjs` keeps the label filter: read once the page has
// mounted and again on the back button, and pushed as a new address when it
// changes, keeping the other parameters.
export function useParentFilter() {
    const parent = ref(null)

    function read() {
        parent.value = parseParentFilter(window.location.search)
    }

    function set(next) {
        parent.value = next
        const { pathname, search, hash } = window.location
        window.history.pushState(window.history.state, '', pathname + withParentFilter(search, next) + hash)
    }

    onMounted(() => {
        read()
        window.addEventListener('popstate', read)
    })
    onBeforeUnmount(() => window.removeEventListener('popstate', read))

    return {
        parent,
        // Narrow to the group under `id`, or widen back when it is the one
        // shown.
        toggle: id => set(parent.value === id ? null : id),
        clear: () => set(null),
    }
}
