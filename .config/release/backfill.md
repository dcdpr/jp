Draft the changelog fragments of JP {version} from the changes listed below,
which reached JP's `main` between {from} and {until}.

JP is a command-line tool for working with LLMs in a terminal: `jp query` asks a
model, conversations keep the history, and tools, attachments, configuration,
and plugins extend it.
Each fragment becomes one entry of the release notes, read by someone using JP.

Write each entry the way JP's `changelog.d/README.md` asks:

- One or two short sentences saying what users can do, what behaves differently,
  or what was fixed.
  Omit implementation details.
- Code formatting only for what users type or read literally: commands, flags,
  config keys, and file names.
- Plain words: no "now", no "previously", and no marketing.

Choose what to write about:

- Omit changes users don't notice: refactoring, tests, CI, builds, dependency
  updates with no visible effect, internal documentation, RFDs, tickets, and
  JP's own development tooling, such as personas, `.config/jp/tools`, and the
  justfile.
- Write one fragment per user-visible change, crediting every listed change that
  makes it, and a fragment for each unrelated user-visible change a single pull
  request makes.
- Describe only what a change shows.
  When its effect on users is unclear, state only what is clear, or omit it.

Fill in each fragment:

- `type`: `added` for a new capability, `changed` for different behavior,
  `deprecated`, `removed`, `fixed` for a bug users would notice, `security`, or
  `performance`.
- `slug`: two to five lowercase words joined by hyphens that name the change,
  unique among the fragments.
- `credits`: the credit of each change the fragment describes, copied from its
  heading below: `#123` for a pull request, or a 12-character commit hash for a
  commit made without one.
- `upgrade_title` and `upgrade_body`: only for a change that requires users to
  act, such as a renamed config key or a removed flag.
  The title says what broke, and the body says what to do instead.
  Leave both empty otherwise.

Return no fragments when none of the changes is user-visible.
