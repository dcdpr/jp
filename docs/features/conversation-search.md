# Conversation Search

`jp conversation grep` searches the text of your conversations — what you
asked, what the assistant answered, what it reasoned about, and what its tools
returned.

Its job is to *locate*, not to display.
Every hit carries a coordinate that the rest of JP already understands, so a
search result is a starting point rather than a dead end.

```sh
jp c grep 'httpmock'
```

```
jp-c17727547754  Flaky openrouter multi-turn test        3 matches · 142 turns
  142:user:2026-03-06T09:27:11.044875Z  INFO httpmock::server::server: started
  --
  142:user:2026-03-06T09:29:39.907268Z  INFO httpmock::server::server: started

jp-c17727953962  Debugging llamacpp reasoning             1 match · 17 turns
   ..:title:Debugging llamacpp reasoning
```

## The coordinate

Each hit is located by three fields: the conversation, the turn, and the scope.

```
jp-c17727547754 : 142 : user : <the matching line>
       ↓           ↓      ↓
 conversation    turn   scope
```

The turn field is a value `jp conversation print --turn` accepts, so a hit feeds
straight back into the commands that show conversations:

```sh
jp c print jp-c17727547754 --turn 142     # read the turn the match came from
jp c use jp-c17727547754                  # switch to that conversation
jp query --attach 'jp-c17727547754?a:142' # attach that turn to a new question
```

A hit in the conversation *title* isn't turn-scoped, so its turn field is `..`
— "all turns", which `--turn` also accepts.
`jp c print <id> --turn ..` prints the whole conversation.

## Reading the output

The fourth field is `m` for a line that matched and `c` for a context line
pulled in by `--context`:

```sh
jp c grep --context 1 'MATCH'
```

```
jp-c17727547754:142:user:c:the line before
jp-c17727547754:142:user:m:the MATCH itself
jp-c17727547754:142:user:c:the line after
```

Unlike `grep`, the marker is a field rather than a change of separator.
A separator-based marker can't be parsed: you would have to know a line's kind
before you could find its field boundaries, and parse the fields to learn its
kind.
As a field, it costs one character and leaves every record splittable the same
way.

A `--` on its own line separates non-adjacent groups of hits — between context
blocks, between turns, and between conversations.
These separators belong to `--context` output; without it, every line is a
matching record.
They are the one thing in the stream that isn't a record, so a script that only
wants records can drop them by requiring five fields.

## Terminal versus pipe

In a terminal, hits are grouped under a per-conversation heading and lines are
fitted to the terminal width.
The heading shows how many matches the conversation gave you and how many turns
it has in total, so you can judge whether it's worth opening.

Only matching rows carry a coordinate.
Context rows leave that column blank, padded so the text still lines up — every
hit in a block comes from one event, so the turn and scope would be identical on
every row anyway, and printing them once per block leaves the coordinate as the
thing that marks a match:

```
jp-c17835864469  Store temporary files in workspace   1 match · 12 turns
           _think_ we should add it here, since it solves the durable
           storage problem:

  3:user:# Store temporary files in workspace
           #projects/jp #idea
```

When piped, each hit becomes one self-contained line, with no styling and no
truncation:

```
ID:TURN:SCOPE:KIND:TEXT
```

Four `:`-delimited fields, always in the same order and always present, then the
matched text verbatim.
None of the four can contain a `:`, and the text is last, so every line parses
the same way no matter which flags produced it:

```sh
jp c grep 'httpmock' | while IFS=: read -r id turn scope kind text; do
  echo "$kind $id turn $turn: $text"
done
```

The text may contain colons of its own — timestamps and Rust paths both do —
so split into exactly five fields and keep the last whole, as the `read` loop
above does.
`cut -d: -f5-` keeps fields five through the end; a plain fifth-field selection
like `awk -F: '{print $5}'` truncates at the next colon.

Force either shape with `--heading` or `--no-heading`.

Because the fields are fixed, `fzf` can browse the results with `jp c print` as
the preview:

```sh
jp c grep -F text-pretty --no-heading 'httpmock' \
  | fzf --ansi --delimiter : \
        --preview 'jp c print {1} --turn {2} -F text-pretty --width $FZF_PREVIEW_COLUMNS'
```

`-F text-pretty` keeps the styling that a pipe would otherwise turn off, and
`--ansi` tells `fzf` to render it rather than print the escapes.
Placeholders are unaffected: `fzf` strips styling before substituting, so `{1}`
and `{2}` are the plain ID and turn.

The preview needs `--width` because a pipe reports no terminal size, so nothing
is laid out to the pane.
`$FZF_PREVIEW_COLUMNS` is the size `fzf` exports for it.

`--context` records work here — a context line carries the same coordinate as
any other, so `{1}` and `{2}` resolve on them — but `--context` also emits `--`
separator rows, and selecting one gives `fzf` no ID or turn to substitute, so
the preview shows an error.
Filter them out (`| grep -v '^--$'`) if you want context here.

Simpler to skip `--context` altogether: the preview pane already shows the whole
turn each hit came from, which is more surrounding text than `--context` gives
you.

## Restricting the search

`--scope` limits which parts of a conversation are searched.
It accepts the concrete scopes `title`, `user`, `assistant`, `reasoning`,
`structured`, `tool-call`, `tool-result`, and `inquiry`, plus two shorthands:

| Scope  | Expands to                                     |
| ------ | ---------------------------------------------- |
| `chat` | `user`, `assistant`, `reasoning`, `structured` |
| `tool` | `tool-call`, `tool-result`                     |
| `all`  | every scope (the default)                      |

```sh
jp c grep --scope chat 'retry'          # only what was said
jp c grep --scope tool-call 'fs_modify' # only tool invocations
jp c grep --scope title 'triage'        # only titles
```

For more than one scope, repeat the flag or comma-separate the values:

```sh
jp c grep --scope user --scope assistant 'retry'
jp c grep --scope user,assistant 'retry'
```

Searching `title` alone never reads the event streams, so it stays fast across a
large workspace.

Every conversation in the workspace is searched unless `--id` narrows it:

```sh
jp c grep -i. 'error'          # the conversation you're in
jp c grep --id recent 'error'  # the most recently activated one
jp c grep --id +pinned 'error' # every pinned conversation
```

`.` (long form `active`) is the session's active conversation — the one `jp
query` would continue and `jp c print` would show.
`+l` (long form `+live`) is every live conversation, which is what you get
without `--id`.

Run `jp c grep --help` for the full target grammar.

## Matching

Patterns are literal by default — `a.c` matches the three characters `a.c`, not
`abc`.
Pass `--regex` for a regular expression, with look-around and backreferences
available:

```sh
jp c grep --regex '\Atriage-\d{3}\z' --scope title
```

Case follows **smart-case**: an all-lowercase pattern matches
case-insensitively, and any uppercase character makes the whole pattern
case-sensitive.
Override with `--ignore-case` or `--case-sensitive`.

"Any uppercase character" counts characters anywhere in the pattern, including
inside regex syntax — `\W`, `\S`, `\D`, `\A`, `\z` all make a pattern
case-sensitive even when the text you're searching for is lowercase.
The example above is case-sensitive for that reason.
Pass `--ignore-case` explicitly when a regex needs those escapes and
case-insensitive matching.

## What to emit

`--output` picks *which records* you get.
It composes with the global `--format` flag, which picks *how they're encoded*.

| `--output`       | Emits                                                   |
| ---------------- | ------------------------------------------------------- |
| `hits` (default) | matching lines with their coordinates                   |
| `ids`            | the conversation ID only, one per line                  |
| `count`          | `ID:COUNT` — matching lines per conversation            |
| `text`           | matched and context lines, no coordinates or separators |

```sh
jp c grep --output ids 'error' | jp c archive -
jp c grep --output count 'retry'
jp c grep --output text 'TODO' > todos.txt
jp c grep --output count 'retry' --format json | jq '.[0].count'
```

`--output hits --format json` gives one object per hit, with `submatches`
carrying the byte offsets of each match within the line:

```json
[
  {
    "id": "jp-c17727547754",
    "turn": 142,
    "scope": "user",
    "timestamp": "2026-03-06T09:27:11.044875Z",
    "title": "Flaky openrouter multi-turn test",
    "text": "... INFO httpmock::server::server: started",
    "match": true,
    "submatches": [{ "match": "httpmock", "start": 10, "end": 18 }]
  }
]
```

A title hit reports `"turn": null`.

## Limits

| Flag              | Caps                               |
| ----------------- | ---------------------------------- |
| `--limit N`       | conversations shown, in sort order |
| `--max-matches N` | matching lines per conversation    |

Long lines are cut to the output width: the terminal's when stdout is a
terminal, unlimited when piped.
The global `--width` sets it explicitly.

## Long lines

A line wider than the output is shown through a window onto it, marked with `…`
at each end the window cut.
The window sits at the start of the line while the match fits there, and slides
right when it doesn't, so a match far down a long line is always visible:

```
jp-c17834351370  Model Aliases with Parameters   1 match · 12 turns

 10:assistant:…wrapper, not of the merge system. The envelope conve…
```

The window keeps a few columns of what follows the match in view, opens between
words rather than mid-word where a boundary is close enough, and shows the first
match when a line has several.
A match too wide for the window is shown from its start rather than its end.

A trailing `…` is highlighted along with the match when the match itself runs
past the window, and left plain when only the rest of the line does — so you
can tell how much of the hit is off-screen.

`--wrap` shows the whole line instead, broken across as many rows as it needs.
Continuation rows leave the coordinate blank and line up under the text above
them:

```
jp-c17834351370  Model Aliases with Parameters   1 match · 12 turns

 10:assistant:Nothing structural prevents it. The map-level
             limitation is a property of the *existing*
             `MergeableMap` wrapper, not of the merge system.
```

Wrapping happens at the output width, so pair it with `--width` to pick a
column:

```sh
jp c grep --width=72 --wrap 'MergeableMap'
```

A pipe reports no width, so `--wrap` needs `--width` there to have anything to
wrap at.
It also groups hits under headings, which a pipe would otherwise skip, and is
rejected alongside `--no-heading`: every line that mode emits is an
`ID:TURN:SCOPE:KIND:TEXT` record, and a continuation row has no coordinate to
carry.

## Scripting

Exit status follows `grep`:

| Status | Meaning                               |
| ------ | ------------------------------------- |
| `0`    | at least one match                    |
| `1`    | no matches                            |
| `2`    | the pattern was invalid, or a failure |

Splitting `1` from `2` lets a script tell "nothing matched" apart from "the
pattern was broken".
With no matches, nothing is written to stdout when piped, so `--output ids` is
safe to feed onward unconditionally.

Under the global `--quiet`, the exit status is the whole answer: no hits are
printed, and the search stops at the first match instead of reading every
conversation.

```sh
if jp c grep --quiet 'panic'; then
  echo "a conversation mentions a panic"
fi
```

## Sorting

`--sort` orders conversations by `created` (the default), `activated` (last
switched to), or `updated` (last event).
`--descending` reverses it.
