---
type: added
---
`jp vite` publishes a project's releases under `/releases/`: an index of every
release with its date, each release's notes and, when it has one, its upgrade
guide, and an Unreleased page for the changes no release lists yet.
A release in preparation is marked as such.
The pages are rendered from the fragments `jp release` keeps, by the `changelog`
binary, which has to be on `PATH` wherever the site is built or served.
JP's site lists its releases there in place of the change log, and `/change-log`
redirects to them.
