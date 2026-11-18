# jp vite serve accepts writes sent by other sites

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-18
- **Label**: domain=plugins
- **Label**: relevant-rfd=116
- **Label**: type=bug

The dev server `jp vite serve` runs takes writes on `/__ticket-board`,
`/__ticket`, `/__wiki`, and the RFD priority board
(`crates/plugins/command/vite/site/jp-vite/middleware.mjs`).
`writeRefusal()` only checks that `jp vite serve` started the site and that its
checks passed.
Nothing checks where a request came from:

- every `POST` body is parsed as JSON whatever its `Content-Type`, so a
  `text/plain` post, which a browser sends cross-site without a preflight, gets
  through;
- the VitePress config sets `allowedHosts: true`, so Vite's own `Host` check is
  off.

A page open in the same browser can therefore move tickets or edit wiki pages
while the site runs: the same-origin policy stops it reading the answer, not
sending the request.
Not reproduced; this is from reading the middleware.

`jp serve web` and `jp review serve` refuse such writes with one rule,
`jp_plugin::origin::same_origin`: a write is refused when `Sec-Fetch-Site` says
another site sent it, or, without that header, when `Origin` doesn't match
`Host`.
A request carrying neither, from `curl` or a script, is allowed.

- [ ] Port that rule to `writeRefusal()` (JavaScript, same three headers, same
      answers), refusing with 403.
- [ ] Cover it in `js-tests/middleware.test.mjs`: same-origin, `none`,
      cross-site, mismatched `Origin`, no headers.
