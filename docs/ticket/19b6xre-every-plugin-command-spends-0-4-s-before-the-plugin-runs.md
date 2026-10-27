# Every plugin command spends 0.4 s before the plugin runs

- **Status**: Todo
- **Kind**: Feature
- **Authors**: jp
- **Date**: 2026-10-27
- **Label**: domain=plugins
- **Label**: package=jp_cli
- **Label**: type=enhancement

`jp ticket --version` takes about 410 ms, where `jp --version` takes 4 to 7 ms.
Every plugin command pays that before the plugin does anything: `jp ticket
list`, `jp ticket board`, `jp -F json ticket show`, and `jp rfd show` all take
380 to 425 ms, for work that takes milliseconds.
The site's dev server starts one `jp` per write, so every drag on the ticket
board costs the same.

Measured on 2026-10-21 in JP's own workspace, median of five runs:

| Command               | Median |
| --------------------- | ------ |
| `jp --version`        | 4 ms   |
| `jp workspace show`   | 108 ms |
| `jp config show`      | 189 ms |
| `jp ticket --version` | 414 ms |

## Where the time goes

Not profiled.
From the code, before it starts the plugin, `jp`:

- loads the workspace and its configuration, which the two commands above put at
  100 to 190 ms;
- reads every `jp-*` binary in its plugin directory and on `$PATH` in full, to
  find each one's manifest (`discovery::discover_in` calls `read_manifest`,
  which reads the whole file, for every executable it finds);
- computes the SHA-256 of the plugin it is about to run, even when the plugin's
  configuration says `run = "allow"` (`admission::admit` hashes the binary
  before `decide` looks at the policy).

## Suggested change

Profile first: the configuration load may be the larger share, and it is a
separate fix.

For the binaries, cache each one's manifest and SHA-256 in the user-local plugin
directory, keyed by canonical path, size, and modification time.
A hit skips reading the binary; a miss reads it once and records the result.
A rebuilt binary changes its modification time, misses the cache, and is checked
against its approval again.

The cache trusts the file's size and modification time.
Someone who can rewrite an approved binary and restore both would go unnoticed,
where the hash today catches them; weigh that against the trust model in RFD 077
before choosing the key.
