import { releaseIndex, releaseWatch } from '../releases.mjs'

// The releases, for the release index: each one's version, date, and upgrade
// guide, and how many entries no release ships yet. See `../releases.mjs`.

export default {
    watch: releaseWatch(),
    load() {
        return releaseIndex()
    },
}
