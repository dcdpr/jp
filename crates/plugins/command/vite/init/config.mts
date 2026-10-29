import { defineConfig } from 'vitepress'

import { withJpVite } from './jp-vite/config.mjs'

// https://vitepress.dev/reference/site-config
//
// `withJpVite` adds what the RFD and ticket pages need, from the files
// `jp vite sync` keeps in `./jp-vite/`. Everything else here is yours.
export default defineConfig(withJpVite({
    title: @@title@@,
    description: @@description@@,
    cleanUrls: true,
    themeConfig: {
        nav: [
@@nav@@        ],
    },
}))
