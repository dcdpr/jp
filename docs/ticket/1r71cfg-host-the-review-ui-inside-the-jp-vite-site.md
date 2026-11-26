# Host the review UI inside the jp-vite site

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-24
- **Label**: domain=plugins
- **Label**: relevant-rfd=116
- **Label**: relevant-rfd=139
- **Label**: relevant-rfd=140
- **Label**: type=pre-rfd

The review UI ([RFD 140]) is a page of its own, which `jp review serve` serves.
The documentation site (`jp-vite`, [RFD 116]) shows the same RFDs, tickets, and
wiki pages with the project's theme, navigation, search, and outline.
This ticket is about making reviews a section of that site, beside the RFDs,
tickets, and wiki, shown only under `jp vite serve`, with the UI still written
in Rust ([RFD 139]).

It is not next.
The standalone UI is used and refined first, and this is taken up once its shape
has settled.
The plan is to build it and then bring RFD 140 in line, rather than write a new
RFD up front.

## Decided

- **The UI stays Rust.** `jp-vite` gains what hosts the UI and nothing of the UI
  itself: a page, a component that mounts the bundle, and a proxy to the review
  server.
- **The site becomes the only place the UI shows.** The page `jp review serve`
  serves today is removed once the site hosts the UI, for phones too.
  Where the site's layout gets in the way on a phone, the site's layout changes.
- **Development only.** A static `vitepress build` has no review page, and the
  navigation names the reviews only under `jp vite serve`.
- **Two processes at first.** `jp vite serve` and `jp review serve` run side by
  side.
  One process waits on T-1kyv4ey, which names this case.
- **A review is addressed by query**, `/review/?id=R-1h4k2m9`, as [RFD 128]
  addresses `/source/?path=...`.
- **The first version ports none of the site's rendering features.** They come
  with the answer to the first open question.

## What removing the standalone page costs

- `jp review serve` works in any workspace today, with nothing but the plugin's
  binary.
  Once the site alone hosts the UI, reviewing in a browser needs a `jp-vite`
  site in the workspace, and Node to run it.
- `jp review open`, `jp review serve --open`, and `jp review start --open` open
  the review server's address (`Store::server`), and have to open the site's.
- The web app manifest RFD 140 describes, for adding the UI to a phone's home
  screen, belongs to the page that goes.
- `just review-ui-watch` serves a development build through `jp review serve
  --ui-dir`, and reloads the page it serves.

## How the two stand today

- `jp-vite` knows the RFDs, tickets, and wiki by name, and nothing registers
  with it: the settings snapshot (`src/settings.rs`), the components
  (`site/jp-vite/theme/`), the dev server's endpoints
  (`site/jp-vite/middleware.mjs`), and the check that each plugin was built with
  the same site revision (`src/serve.rs`).
  A project writes its own navigation entries, in `.vitepress/config.mts`.
- The review UI mounts on `<body>` when its module loads (`start` in
  `ui/src/lib.rs`), reads the pages `/` and `/reviews/{id}` from the address
  (`ui/src/route.rs`), and calls `/api/reviews...` and `/ui/build` on its own
  origin (`ui/src/client.rs`).
- Its stylesheet imports Tailwind whole, the reset included, and copies the
  site's colors, fonts, and measures by value (`ui/style.css`).
- A file's text is `md-html`'s output.
  Every run of text carries the byte range of the source it came from (`data-s`,
  `data-e`), which selecting, highlighting, and placing a comment under its
  passage all read.
- `jp review serve` holds its store's server lock and writes the address it
  serves at into the store (`Store::serving_at`).
- Reviews are kept in user-local storage, outside the workspace, unless
  `plugins.command.review.options.dir` says otherwise.
  A review is no file in the site's source directory, which is what gives each
  ticket its page.

## Open questions

1. **One renderer.** A reviewed file should show what the site shows of it:
   highlighted code, alerts, the compact metadata header, linked mentions (`RFD
   139`, `T-...`, `W-...`), embeds, and task lists.
   The site renders those with markdown-it rules (`site/jp-vite/*.mjs`), and the
   review UI needs byte ranges, which markdown-it doesn't record: a token's
   source map is a pair of lines.
   Porting each rule to `md-html` gives two implementations to keep in step,
   which is what to avoid.
   Two directions, neither looked into:
   - `md-html` renders these pages for the site too, and the rules are written
     once, in Rust.
   - markdown-it is taught to record byte ranges, and the review UI reads the
     site's HTML.
2. **Writes through the site.** The review server refuses a write from another
   site, and on loopback a request for a name it doesn't answer to
   (`jp_plugin::origin`).
   The site's dev server accepts any `Host` (`allowedHosts: true`), and
   T-1n013mt says it accepts writes sent by other sites.
   A proxy from the site to the review server puts starting a pass, which edits
   workspace files, behind the site's checks.
   What the proxy forwards, and which side refuses what, has to be settled
   before the proxy exists.
3. **Finding the review server.** The address is in the review store, whose
   default place is user-local.
   The settings snapshot is a file in the repository, so it can't hold that
   place.
   What the page shows while no review server runs.
4. **Routing.** Whether the site's router leaves the UI's own history entries
   alone, and what back and forward do.
   Whether a review can have a path, `/review/R-1h4k2m9`: VitePress's dynamic
   routes take a paths loader with a `watch` option, which regenerates pages
   during development, but whether it follows a directory outside the site is
   untested, and so is the VitePress version the site pins.
5. **Styles.** A build of the stylesheet without the reset, which would restyle
   the whole site.
   Whether the utility classes sit beside the theme's, what the theme's document
   styles make of `md-html`'s HTML, and how the UI follows the site's appearance
   switch where it follows the system's scheme today.
6. **Leaving the page.** The site keeps one JavaScript context across pages.
   The UI polls, listens on the document and the window, and registers
   highlights in `CSS.highlights`, all of which have to stop when its page is
   left.
7. **Whose chrome.** The site has a top bar and an outline, and so has the
   review page.
   Which of the review's top bar survives, where its controls go, and how much
   height the two take on a phone, where the comments are a sheet and the
   keyboard takes half the screen.
8. **Version skew and sync.** `jp vite serve` checks that `jp rfd`, `jp ticket`,
   and `jp wiki` report its site revision.
   The component that mounts the bundle is synced into each workspace with the
   site's files, and the bundle comes from the `jp-review` installed, so the two
   can come from different releases: the coupling RFD 139 named when it turned
   down Vue components in `jp-vite`.
9. **What it reverses.** RFD 140 lists embedding the UI in the `jp-vite` site as
   a non-goal, and RFD 139 says `jp-vite` gets no more JavaScript for new UIs.
   Both are amended when this lands.

## Order

1. A throwaway spike that answers questions 2 to 6 against the real site.
2. Make the UI mountable: `mount` and `unmount` in place of mounting on load,
   the page's and the API's addresses passed in, and the stylesheet without the
   reset.
   Rust only, and mergeable while the standalone page still uses it.
3. `jp-vite` hosts it: the page, the mounting component, the proxy, and the
   navigation entry; the standalone page goes.
4. The renderer, as question 1 is answered.
5. One process, with T-1kyv4ey.

[RFD 116]: ../rfd/116-shareable-rfd-ticket-and-site-tooling.md
[RFD 128]: ../rfd/128-workspace-source-explorer-in-jp-vite.md
[RFD 139]: ../rfd/139-browser-uis-in-rust-with-leptos.md
[RFD 140]: ../rfd/140-browser-frontend-for-the-review-engine.md
