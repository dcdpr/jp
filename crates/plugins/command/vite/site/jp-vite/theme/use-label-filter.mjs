import { onBeforeUnmount, onMounted, ref } from 'vue'

import { parseLabelFilter, toggleLabelTerm, withLabelFilter } from './label-filter.mjs'

// The label filter in the page's address, for a listing that narrows to it:
// the ticket board, and the ticket and wiki tables.
//
// The filter is read once the page has mounted, since the server renders the
// whole listing, and again when the back button changes the address. Changing
// it pushes the new address, so a filtered view can be shared and the back
// button undoes a click. Other parameters in the address are kept.
export function useLabelFilter() {
    const filter = ref(null)

    function read() {
        filter.value = parseLabelFilter(window.location.search)
    }

    function set(next) {
        filter.value = next
        const { pathname, search, hash } = window.location
        window.history.pushState(window.history.state, '', pathname + withLabelFilter(search, next) + hash)
    }

    onMounted(() => {
        read()
        window.addEventListener('popstate', read)
    })
    onBeforeUnmount(() => window.removeEventListener('popstate', read))

    return {
        filter,
        // Narrow by `term`, or take it back out when it is there.
        toggle: term => set(toggleLabelTerm(filter.value, term)),
        clear: () => set(null),
    }
}
