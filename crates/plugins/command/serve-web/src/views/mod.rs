//! View modules for rendering HTML pages.
//!
//! Each page's scripts are served verbatim as classic `<script>` tags, so the
//! ones on a page share a single global lexical scope: a `const` in one
//! collides with the same name in the next, and neither file is wrong on its
//! own.
//! Which scripts land together is therefore worth knowing before naming
//! anything at a script's top level.
//!
//! | Page              | Scripts, in tag order     |
//! | ----------------- | ------------------------- |
//! | New conversation  | `configs.js`, `new.js`    |
//! | Conversation      | `configs.js`, `detail.js` |
//! | Conversation list | `list.js`, `filter.js`    |
//!
//! `just typecheck-js` reads all of them as one scope, which is stricter than
//! the pages are: it rejects a collision between two scripts that never load
//! together.
//! Keeping top-level names unique across every file satisfies both.

pub(crate) mod configs;
pub(crate) mod detail;
pub(crate) mod layout;
pub(crate) mod list;
pub(crate) mod new;
