# jp vite serve accepts writes sent by other sites

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-18
- **Label**: domain=plugins
- **Label**: relevant-rfd=116
- **Label**: type=bug

The dev server started by `jp vite serve` accepts writes on `/__ticket-board`,
`/__ticket`, `/__wiki`, and the RFD priority board
(`crates/plugins/command/vite/site/jp-vite/middleware.mjs`).
`writeRefusal()` only checks that `jp vite serve` started the site and that its
startup checks passed; it does not validate the request's origin:

- every `POST` body is parsed as JSON regardless of `Content-Type`, so a
  `text/plain` form submission (which browsers send cross-origin without a CORS
  preflight) is accepted;
- the VitePress config sets `allowedHosts: true`, disabling Vite's `Host` header
  check.

An attacker-controlled page open in the same browser can therefore move tickets
or edit wiki pages while the dev server runs: the same-origin policy blocks
reading the response, not sending the request.
(Found by code inspection, not reproduced.)

`jp serve web` and `jp review serve` reject cross-origin writes using
`jp_plugin::origin::same_origin`: a write is rejected when `Sec-Fetch-Site` is
present and not `"same-origin"` or `"none"`, or when `Sec-Fetch-Site` is absent
and the `Origin` authority does not match `Host`.
Requests with neither header (such as `curl` or scripts) are allowed.

- [ ] Port that check to `writeRefusal()` in JavaScript and return HTTP 403 on
      failure.
- [ ] Add test cases in `js-tests/middleware.test.mjs`: `same-origin`, `none`,
      `cross-site`, mismatched `Origin`, and no origin headers.
