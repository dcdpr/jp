# Indexing and Counting Conventions

JP exposes turn positions and counts in two places: the CLI (flags, arguments)
and configuration (config files, `--cfg`, the inline compaction DSL).
Internally those same positions are stored as zero-based indices in the
conversation stream.
This document fixes the convention so the translation between the two sides is
consistent and happens in exactly one place per boundary.

## The rule

- **User-facing positions are 1-based.** The first turn is turn `1`.
  This holds for every CLI flag and configuration value that names a turn
  position.

- **Stored and internal positions are 0-based.** `Compaction.from_turn`,
  `Compaction.to_turn`, `Turn::index()`, and `RangeBound::Absolute` are all
  0-based and never change.
  The conversation stream is the source of truth and it counts from zero.

- **Translate at the boundary, once.** A 1-based user value becomes a 0-based
  index at the point where user input is resolved against the stream, and a
  0-based index becomes a 1-based display value at the point where it is
  rendered.
  Nothing in between carries an ambiguous "is this 0- or 1-based?" value.

`jp_conversation` carries core values (0-based) and never sees a user value.
Everything above it translates at the point where it parses or renders user
input, and each such point is listed under "Where the translation lives":
`jp_cli`'s flag parsers for `--turn`/`--from`/`--to`, and `RuleBound`'s
`FromStr`/`Display` for config values and the inline DSL.

One consequence worth knowing before touching `RuleBound`: because it normalizes
at parse time, its `FromEnd` payload is already a 0-based offset (`FromEnd(0)`
is the last turn, written `-1`), matching `jp_conversation::RangeBound::FromEnd`
so the two never need a shift between them.
`RuleBound::Absolute` is the exception — it holds the 1-based number as written
and is shifted in `keep_first_to_bound` / `keep_last_to_bound`.

## Positions vs. counts

Only *positions* (indices into the conversation) are subject to the 1-based
rule.
A *count* — "how many turns" — is base-independent and is never shifted.

| Kind                | Examples                                                                                   | Translated?                                          |
| ------------------- | ------------------------------------------------------------------------------------------ | ---------------------------------------------------- |
| Position (absolute) | `--turn N`, `--from N`, `--to N`, DSL `N..M`                                               | yes, `N` (1-based) → `N - 1` (0-based)               |
| Position (from end) | `--turn -N`, `--from -N`, `--to -N`, `--keep-last -N`, DSL `-N`, config `keep_last = "-N"` | yes, `-1` is the last turn (`-N` → `FromEnd(N - 1)`) |
| Count               | `--first N`, `--last N`, `--keep-first N`, `--keep-last N`, config `keep_first = N`        | no                                                   |
| Time                | `5h`, `2days`, `2026-01-01`, RFC 3339                                                      | resolved against timestamps, then snapped to a turn  |

Two consequences worth calling out:

- `--turn -1`, `--from -1`, and `--to -1` address the **last** turn, matching
  the 1-based reading where `1` is the first turn and `-1` is the last.
  As a result `--from -N` selects the same starting turn as `--last N`, and
  `--turn -2` is the second-to-last turn.
  Either end of a `--turn` range may count from the end, and the two forms mix
  freely: `--turn -3..-2` is the two turns before the last, `--turn 2..-1` is
  turn 2 through the end.

- `-N` is a position in every surface that accepts it, including the compaction
  DSL and the `keep_first`/`keep_last` config values.
  `..-3` compacts through the third turn from the end (leaving the final two),
  and `keep_last = 3` is the count that keeps the last three.

- The attachment selector (`jp query --attach 'ID?a:RANGE'`) uses the same
  rules: 1-based, `-1` is the last turn, `A..B` inclusive.
  `a:-3` is one turn; `a:-3..` is the last three.

- A bare integer in a position slot is always a turn number, never a year.
  The accepted date formats all require separators, so `--from 2026` is turn
  2026 and `--from 2026-01-01` is the date.

- Ranges are written `A..B` and are **inclusive on both ends** — `1..5` is
  turns 1 through 5 (five turns).
  This one format is shared by `--turn A..B`, the compaction DSL, and the
  timeline output.
  Either end may be omitted to mean the conversation start or end (`10..`,
  `..10`, `..`).
  Note this diverges from Rust's `..` (which is exclusive); there is no `..=`
  form.

## Which bounds go where

A bound is legal in a config rule only if it is **conversation-independent** —
a rule is written once and applied to every conversation, so its bounds have to
mean the same thing in a conversation of any length.
Counts, durations, from-end positions, and `last-compaction` all qualify.
An absolute turn number does not: "turn 47" is a fact about one conversation.

`RuleBound::Absolute` is therefore reachable only from the inline DSL, which
runs once against a conversation the user is looking at.
`RuleBound`'s `Serialize` refuses it, so it cannot reach a config file even
indirectly.

On the CLI the distinction is between *selecting* and *protecting* rather than
between value forms:

| Flag                           | Answers         | Accepts                                                                      |
| ------------------------------ | --------------- | ---------------------------------------------------------------------------- |
| `--from` / `--to`              | which turns     | a position (`5`, `-3`), a time, `last-compaction` (`--from`)                 |
| `--keep-first` / `--keep-last` | what to protect | a count (`3`), a position (`-3`), a time, `last-compaction` (`--keep-first`) |

The two compose: the range flags name the selection, and the keep flags then
protect turns at either end of it.
A bare `N` on a keep flag is a count, so `--keep-last 3` and `--keep-last -4`
protect the same three turns.

## A bound never splits a turn

Every position form resolves to a whole turn, including the time-based ones.
A time value is an *addressing mode for a turn*, not a cut point inside one:
`--from <time>` starts at the first turn to begin after the cutoff, and `--to
<time>` ends at (and includes) the turn that was running at the cutoff.

This is why the range ends are inclusive rather than half-open.
Half-open reads well for a continuous coordinate, but once a bound names a turn,
"which turn" is inherently inclusive.
The conversation-creation filter on `jp c rm` / `jp c archive` / `jp c use` is
the opposite case — it compares raw timestamps and never snaps to anything —
so it is half-open and uses distinct flag names (`--created-since` /
`--created-before`).

## Where the translation lives

- **CLI turn selection** (`jp_cli::cmd::turn_selection`): `parse_bound` maps a
  1-based absolute `N` to `RangeBound::Absolute(N - 1)` and a from-end `-N` to
  `RangeBound::FromEnd(N - 1)`.
  `--turn` endpoints go through `parse_turn_pos`, which produces the same two
  flavours (`TurnPos::Absolute` / `TurnPos::FromEnd`) holding the number as
  written; `TurnPos::to_range_bound` does the 1-based → 0-based shift.
  `TurnSelection::resolve` then resolves every bound against the stream and
  produces a `TurnSet` of inclusive 0-based windows.

  `--turn`, `--from`/`--to`, and `--first`/`--last` are three ways of naming the
  base selection and are mutually exclusive:

  | Selector      | Start bound       | End bound         |
  | ------------- | ----------------- | ----------------- |
  | `--turn N`    | `Absolute(N - 1)` | `Absolute(N - 1)` |
  | `--turn -N`   | `FromEnd(N - 1)`  | `FromEnd(N - 1)`  |
  | `--turn A..B` | `Absolute(A - 1)` | `Absolute(B - 1)` |
  | `--first N`   | `Absolute(0)`     | `Absolute(N - 1)` |
  | `--last N`    | `FromEnd(N - 1)`  | `FromEnd(0)`      |

  Either end of `--turn A..B` may be a from-end position, and the two forms mix
  freely (`--turn 2..-1`).
  `--first` and `--last` given together produce two windows and skip the turns
  between them.

- **Keep flags** (`jp_cli::cmd::turn_selection`): `keep_first_bound` /
  `keep_last_bound` map `RuleBound::Absolute(N)` (the 1-based value parsed from
  `@N`) to `RangeBound::Absolute(N - 1)`; the `RuleBound::Turns(N)` (count) arm
  is untouched.
  `TurnSelection::trim` then clamps each window to the turns those bounds leave
  unprotected — it clamps rather than shifts, so a turn already outside the
  window needs no protecting.

- **Config `keep_first`/`keep_last` and the inline DSL**: `RuleBound`'s
  `FromStr` (`jp_config::conversation::compaction`) and `parse_dsl_bound`
  (`jp_cli::cmd::compact_flag`) both map `-N` to `RuleBound::FromEnd(N - 1)`.

  The two parsers differ on a bare `N`: in config it is a count (`keep_first =
  5` keeps five turns), in the DSL it is a position (`5..` starts at turn 5).
  Only the DSL produces `RuleBound::Absolute`; see "Which bounds go where".

- **Timeline output** (`jp_cli::cmd::conversation::compact::timeline_lines`):
  the stored 0-based `from_turn`/`to_turn` are rendered as 1-based, e.g.
  `Compacted turns 2..8`.

When adding a new flag, config key, or output that names a turn, decide first
whether it is a position or a count.
If it is a position, it is 1-based on the user side and translated to 0-based
exactly where it meets the stream.
