import DefaultTheme from 'vitepress/theme'
import RfdBreadcrumb from './RfdBreadcrumb.vue'
import RfdReferences from './RfdReferences.vue'
import RfdStatusBadge from './RfdStatusBadge.vue'
import { h } from 'vue'
import './custom.css'

export default {
    extends: DefaultTheme,
    Layout() {
        return h(DefaultTheme.Layout, null, {
            'doc-before': () => [h(RfdBreadcrumb), h(RfdReferences), h(RfdStatusBadge)],
        })
    },
}
