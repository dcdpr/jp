// The anchor a heading's text gets on the site.
//
// A copy of VitePress's default, `slugify` from `@mdit-vue/shared`, which
// `withJpVite` hands to VitePress as `markdown.anchor.slugify`; VitePress passes
// it on to everything that names a heading, links included. Owning it keeps
// every anchor where it is when VitePress changes its default.
//
// `jp wiki check` computes anchors the same way in Rust (`doc::heading`), and
// both are tested against `crates/internal/doc/fixtures/heading-slugs.json`,
// so a `#fragment` the check accepts is one the site renders.
//
// No Node imports, so the browser can use it too.

// eslint-disable-next-line no-control-regex -- control characters are what it removes
const CONTROL = /[\u0000-\u001f]/g
const SPECIAL = /[\s~`!@#$%^&*()\-_+=[\]{}|\\;:"'“”‘’<>,.?/]+/g
const COMBINING = /[\u{300}-\u{36f}]/gu

// The slug for a heading whose text is `text`: decomposed and stripped of
// accents and control characters, each run of whitespace and punctuation made
// one `-`, a `-` at either end dropped, a leading digit given a `_`, and the
// whole lowercased.
//
// Two headings with one slug are told apart by markdown-it-anchor, which adds
// `-1`, `-2`, and so on to the later ones.
export function slugify(text) {
    return text
        .normalize('NFKD')
        .replace(COMBINING, '')
        .replace(CONTROL, '')
        .replace(SPECIAL, '-')
        .replace(/-{2,}/g, '-')
        .replace(/^-+|-+$/g, '')
        .replace(/^(\d)/, '_$1')
        .toLowerCase()
}
