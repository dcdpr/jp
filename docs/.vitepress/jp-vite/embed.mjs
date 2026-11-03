import { readFileSync, statSync } from 'node:fs'
import { posix, resolve } from 'node:path'

import { readSettings, SRC_DIR } from './settings.mjs'

// Embeds: a section of another document, or lines of a workspace file, shown
// where a page has an image alone in its paragraph.
//
//     ![Pinned Conversation](0a1b2c3-glossary.md#pinned-conversation)
//
//     ![Links::by_rfd](../../crates/plugins/command/rfd/src/links.rs#L88-L125)
//
// The markdown stays a plain image, which an editor follows and a forge shows
// as a link to the source. On the site, a section is read as part of its own
// document: its reference links resolve against that document's definitions,
// its relative links lead where they did there, and a link to one of its
// headings still leads to the source page. Its headings move to sit below the
// heading the embed is under, and keep no anchor, so the embedding page's own
// anchors are the ones `jp wiki check` computes. Lines become a code block in
// the file's language. Each embed sits in a box whose first line links to its
// source.
//
// `jp wiki show` expands embeds the same way, and `jp wiki check` holds them
// to the same rules. An image that points at a markdown document, or at a
// workspace file that isn't an image, has to be a standalone embed of a section
// or a line range, and embeds can't loop; here a broken one fails the build
// rather than rendering as a broken image.

// Files an image may show as itself.
const IMAGES = new Set([
    'apng', 'avif', 'bmp', 'gif', 'ico', 'jpeg', 'jpg', 'png', 'svg', 'tif', 'tiff', 'webp',
])

// Set on the environment of a document parsed for a section it holds: its
// embeds are expanded once the section is cut out of it, and only those.
const WHOLE_DOCUMENT = 'jpViteSectionSource'

// Add the rule that expands embeds to `md`.
//
// It runs after VitePress has given the page's headings their anchors and
// read its outline, so an embedded heading takes neither.
export function embedSections(md) {
    md.core.ruler.push('jp_vite_embeds', (state) => {
        const env = state.env ?? {}
        if (!env.relativePath || env[WHOLE_DOCUMENT]) return

        expand(state.tokens, {
            md: state.md,
            Token: state.Token,
            page: env.relativePath,
            cleanUrls: env.cleanUrls,
            // VitePress watches the files a page includes, and renders the
            // page again when one changes.
            includes: Array.isArray(env.includes) ? env.includes : null,
            stack: [],
        })
    })
}

// A line range a fragment names: `L12`, `L12-L24`, `L12-24`, or `12-24`, as
// `[start, end]` 1-based lines; `null` for any other fragment.
export function lineRange(fragment) {
    const number = text => (/^\d+$/.test(text) ? Number(text) : null)
    const pair = (start, end) => (start === null || end === null ? null : [start, end])

    if (fragment.startsWith('L')) {
        const rest = fragment.slice(1)
        const dash = rest.indexOf('-')
        if (dash < 0) {
            const line = number(rest)
            return line === null ? null : [line, line]
        }
        const end = rest.slice(dash + 1)
        return pair(number(rest.slice(0, dash)), number(end.startsWith('L') ? end.slice(1) : end))
    }

    const dash = fragment.indexOf('-')
    if (dash < 0) return null

    return pair(number(fragment.slice(0, dash)), number(fragment.slice(dash + 1)))
}

// `url`, a destination in the document at `from`, as it reads from the
// document at `to`; both paths are below the source directory, written with
// `/`.
//
// A destination naming a scheme, or a path from the site's root, reads the same
// anywhere; one naming only a fragment names the document it is in.
export function rebase(url, from, to) {
    if (/^(?:[a-z][a-z0-9+.-]*:|\/)/i.test(url)) return url

    const cut = url.search(/[?#]/)
    const path = cut < 0 ? url : url.slice(0, cut)
    const suffix = cut < 0 ? '' : url.slice(cut)
    const target = path === '' ? from : posix.normalize(posix.join(posix.dirname(from), path))

    return relativeLink(to, target) + suffix
}

// The link from the document at `page` to the file at `target`.
function relativeLink(page, target) {
    const href = posix.relative(posix.dirname(page), target)

    return href.startsWith('.') ? href : `./${href}`
}

// What the image `src` in the document at `page` embeds:
//
// - `{ kind: 'section', target, slug }` for a section of a markdown document,
// - `{ kind: 'lines', target, start, end }` for lines of any other file,
// - `{ problem }` for a destination that can be neither, or
// - `null` for an image the page shows as one.
//
// `target` is the file below the source directory, written with `/`.
export function classify(src, page, settings) {
    if (!src || /^(?:[a-z][a-z0-9+.-]*:|[/#])/i.test(src)) return null

    const cut = src.indexOf('#')
    const encoded = cut < 0 ? src : src.slice(0, cut)
    const fragment = cut < 0 ? '' : decode(src.slice(cut + 1))
    if (encoded === '') return null

    const target = posix.normalize(posix.join(posix.dirname(page), decode(encoded)))
    const extension = posix.extname(target).slice(1).toLowerCase()
    const range = extension === 'md' || !fragment ? null : lineRange(fragment)
    // An image without a line range shows as itself, wherever it is.
    if (!range && IMAGES.has(extension)) return null

    const inWorkspace = settings.site === '.' ? target : posix.join(settings.site, target)
    if (inWorkspace.startsWith('../') || inWorkspace === '..') {
        return { problem: 'points outside the workspace.' }
    }

    if (extension === 'md') {
        if (!fragment) {
            return { problem: 'names a document but no section of it: add one, as `#turn`.' }
        }
        return { kind: 'section', target, slug: fragment }
    }
    if (range) return { kind: 'lines', target, start: range[0], end: range[1] }
    // A file that isn't there is left to VitePress, which says so.
    if (!isFile(resolve(SRC_DIR, target))) return null

    return { problem: 'names a file that is no image: embed a line range of it, as `#L12-L24`.' }
}

function decode(text) {
    try {
        return decodeURIComponent(text)
    } catch {
        return text
    }
}

function isFile(path) {
    try {
        return statSync(path).isFile()
    } catch {
        return false
    }
}

// The image a paragraph holds and nothing else, or `null`.
function standaloneImage(inline) {
    const meaningful = (inline.children ?? []).filter(child =>
        !(child.type === 'text' && child.content.trim() === '') &&
        child.type !== 'softbreak' && child.type !== 'hardbreak')

    return meaningful.length === 1 && meaningful[0].type === 'image' ? meaningful[0] : null
}

// The heading level of a `heading_open` or `heading_close` token.
function levelOf(token) {
    return Number(token.tag.slice(1))
}

// Expand the embeds in `tokens`, block tokens of the document at
// `context.page`, in place.
//
// `context` holds markdown-it's instance and token constructor, the page, the
// files VitePress watches for it, and the sections being expanded around these
// tokens, as `target#slug`.
function expand(tokens, context) {
    let settings = null
    const settingsNow = () => (settings ??= readSettings())
    // What an embed put in, whose headings are no heading of this document.
    const inserted = new WeakSet()

    for (let index = 0; index < tokens.length; index++) {
        const token = tokens[index]
        if (token.type !== 'inline') continue

        const paragraph = tokens[index - 1]?.type === 'paragraph_open' &&
            tokens[index + 1]?.type === 'paragraph_close'
        const image = paragraph ? standaloneImage(token) : null
        if (image) {
            const src = image.attrGet('src')
            const embed = classify(src, context.page, settingsNow())
            if (embed?.problem) fail(context.page, src, embed.problem)
            if (embed) {
                context.includes?.push(resolve(SRC_DIR, embed.target).replaceAll('\\', '/'))
                const enclosing = enclosingLevel(tokens, index, inserted)
                const replacement = embed.kind === 'section'
                    ? embedSection(embed, image, enclosing, context)
                    : embedLines(embed, image, context)
                for (const added of replacement) inserted.add(added)
                tokens.splice(index - 1, 3, ...replacement)
                index += replacement.length - 2
                continue
            }
        }

        for (const child of token.children ?? []) {
            if (child.type !== 'image') continue
            const src = child.attrGet('src')
            if (classify(src, context.page, settingsNow()) !== null) {
                fail(
                    context.page,
                    src,
                    'isn\'t alone in its paragraph: an embed stands on its own, with blank lines ' +
                        'around it.',
                )
            }
        }
    }
}

// The level of the last heading of the document before the block at `index`,
// passing over those in what `inserted` holds; 0 above the first.
function enclosingLevel(tokens, index, inserted) {
    for (let at = index; at >= 0; at--) {
        if (tokens[at].type === 'heading_open' && !inserted.has(tokens[at])) {
            return levelOf(tokens[at])
        }
    }

    return 0
}

function fail(page, src, problem) {
    throw new Error(`${page}: the embed \`${src}\` ${problem}`)
}

// The tokens that show section `embed.slug` of `embed.target` where `image`
// stands, under a heading of level `enclosing`.
function embedSection(embed, image, enclosing, context) {
    const { md, Token, page } = context
    const src = image.attrGet('src')
    const key = `${embed.target}#${embed.slug}`
    if (context.stack.includes(key)) {
        const chain = [...context.stack.slice(context.stack.indexOf(key)), key]
        fail(page, src, `loops: ${chain.join(' embeds ')}.`)
    }

    const path = resolve(SRC_DIR, embed.target)
    let source
    try {
        source = readFileSync(path, 'utf-8')
    } catch (error) {
        fail(page, src, `can't be read: ${error.message}.`)
    }

    // The whole document, so its definitions and its anchors are the ones it
    // has on its own page.
    const document = md.parse(source, {
        path,
        relativePath: embed.target,
        cleanUrls: context.cleanUrls,
        [WHOLE_DOCUMENT]: true,
    })
    const section = cutSection(document, embed.slug, message => fail(page, src, message))
    // Read before `adapt` moves the section's headings, the title among them
    // when the section is the document's first.
    const title = documentTitle(document) ?? posix.basename(embed.target, '.md')
    const label = textToken(Token, `${title} › ${section.title}`)

    expand(section.tokens, {
        ...context,
        page: embed.target,
        stack: [...context.stack, key],
    })
    adapt(section.tokens, embed.target, page, enclosing + 1 - section.level)

    return box(Token, 'section', relativeLink(page, embed.target) + `#${embed.slug}`, [label],
        section.tokens)
}

// The tokens that show lines `embed.start` to `embed.end` of `embed.target`
// where `image` stands: a code block in the file's language, under a link to
// the lines.
function embedLines(embed, image, context) {
    const { Token, page } = context
    const src = image.attrGet('src')

    let lines
    try {
        lines = readFileSync(resolve(SRC_DIR, embed.target), 'utf-8').split(/\r?\n/)
    } catch (error) {
        fail(page, src, `can't be read: ${error.message}.`)
    }
    // A file ending in a newline has no line after it.
    if (lines.at(-1) === '') lines.pop()

    const { start, end } = embed
    if (start < 1 || end < start || end > lines.length) {
        fail(page, src, `names lines ${start} to ${end}, and the file has ${lines.length}.`)
    }

    const fence = new Token('fence', 'code', 0)
    fence.info = posix.extname(embed.target).slice(1)
    fence.content = `${lines.slice(start - 1, end).join('\n')}\n`
    fence.markup = '```'
    fence.block = true

    const label = image.children?.length
        ? image.children
        : [textToken(Token, posix.basename(embed.target))]
    const href = relativeLink(page, embed.target) + `#L${start}-L${end}`

    return box(Token, 'lines', href, label, [fence])
}

// Section `slug` of the parsed `document`: its heading's level and text, and
// its tokens, from its heading to the first top-level block that is, or holds,
// a heading of its level or higher.
//
// The anchors are the ones the document's own page gives its headings, so
// `slug` is matched against them. `fail` is called with what went wrong.
function cutSection(document, slug, fail) {
    const start = document.findIndex(token =>
        token.type === 'heading_open' && token.attrGet('id') === slug)
    if (start < 0) {
        const anchors = document
            .filter(token => token.type === 'heading_open' && token.attrGet('id'))
            .map(token => `\`#${token.attrGet('id')}\``)
        fail(`names no section \`#${slug}\`; the sections are ${anchors.join(', ') || 'none'}.`)
    }
    if (document[start].level !== 0) {
        fail(`names a section whose heading sits in a list or a quote, which can't be cut out of it.`)
    }

    const level = levelOf(document[start])
    let end = blockEnd(document, start) + 1
    while (end < document.length) {
        const last = blockEnd(document, end)
        const stops = document.slice(end, last + 1)
            .some(token => token.type === 'heading_open' && levelOf(token) <= level)
        if (stops) break
        end = last + 1
    }

    return {
        level,
        title: inlineText(document[start + 1]),
        tokens: document.slice(start, end),
    }
}

// The index of the last token of the top-level block opening at `index`.
function blockEnd(tokens, index) {
    const token = tokens[index]
    if (token.nesting !== 1) return index

    let end = index + 1
    while (end < tokens.length && !(tokens[end].level === token.level && tokens[end].nesting === -1)) {
        end++
    }

    return end
}

// The text of an inline token, as its heading reads.
function inlineText(inline) {
    return (inline?.children ?? [])
        .filter(child => child.type === 'text' || child.type === 'code_inline')
        .map(child => child.content)
        .join('')
        .trim()
}

// The text of the `#` heading a parsed document opens with, if it has one.
function documentTitle(document) {
    const heading = document.findIndex(token => token.type === 'heading_open' && token.tag === 'h1')

    return heading < 0 ? null : inlineText(document[heading + 1]) || null
}

// Make the tokens of a section of the document at `from` read the same from
// the page at `to`: links rebased, and headings moved by `shift` levels and
// stripped of their anchors.
function adapt(tokens, from, to, shift) {
    tokens.forEach((token, index) => {
        if (token.type === 'heading_open' || token.type === 'heading_close') {
            token.tag = `h${Math.min(6, Math.max(1, levelOf(token) + shift))}`
        }
        if (token.type === 'heading_open') {
            removeAttr(token, 'id')
            removeAttr(token, 'tabindex')
            stripPermalink(tokens[index + 1])
        }
        if (token.type === 'inline') rebaseChildren(token.children, from, to)
    })
}

function rebaseChildren(children, from, to) {
    for (const child of children ?? []) {
        const attr = child.type === 'link_open' ? 'href' : child.type === 'image' ? 'src' : null
        const value = attr ? child.attrGet(attr) : null
        if (value !== null && value !== undefined) child.attrSet(attr, rebase(value, from, to))
    }
}

function removeAttr(token, name) {
    if (token.attrs) token.attrs = token.attrs.filter(([key]) => key !== name)
}

// Take the link to its own anchor out of a heading's text, with the space
// before it.
function stripPermalink(inline) {
    const children = inline?.children
    if (!children) return

    const open = children.findIndex(child => child.type === 'link_open' &&
        (child.attrGet('class') ?? '').split(/\s+/).includes('header-anchor'))
    if (open < 0) return

    let close = open
    while (close < children.length && children[close].type !== 'link_close') close++
    const from = open > 0 && children[open - 1].type === 'text' && children[open - 1].content === ' '
        ? open - 1
        : open
    children.splice(from, close - from + 1)
}

function textToken(Token, content) {
    const token = new Token('text', '', 0)
    token.content = content
    return token
}

// `content`, block tokens, in a box whose first line is a link to `href`
// showing `label`, inline tokens.
function box(Token, kind, href, label, content) {
    const open = new Token('html_block', '', 0)
    open.content = `<div class="jp-embed jp-embed--${kind}">\n`
    open.block = true

    const source = new Token('paragraph_open', 'p', 1)
    source.attrSet('class', 'jp-embed-source')
    source.block = true

    const link = new Token('link_open', 'a', 1)
    link.attrSet('href', href)
    const line = new Token('inline', '', 0)
    line.children = [link, ...label, new Token('link_close', 'a', -1)]

    const sourceClose = new Token('paragraph_close', 'p', -1)
    sourceClose.block = true

    const close = new Token('html_block', '', 0)
    close.content = '</div>\n'
    close.block = true

    return [open, source, line, sourceClose, ...content, close]
}
