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

| Kind                | Examples                                                                            | Translated?                                          |
| ------------------- | ----------------------------------------------------------------------------------- | ---------------------------------------------------- |
| Position (absolute) | `--turn N`, `--from N`, `--to N`, DSL `N..M`                                        | yes, `N` (1-based) → `N - 1` (0-based)               |
| Position (from end) | `--turn -N`, `--from -N`, `--to -N`, DSL `-N`, config `keep_last = "-N"`            | yes, `-1` is the last turn (`-N` → `FromEnd(N - 1)`) |
| Count               | `--first N`, `--last N`, `--keep-first N`, `--keep-last N`, config `keep_first = N` | no                                                   |
| Duration            | `5h`, `2days`                                                                       | n/a (resolved against timestamps)                    |

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

The CLI splits the same distinction across flags rather than values:

| Flag                           | Answers        | Accepts                                                          |
| ------------------------------ | -------------- | ---------------------------------------------------------------- |
| `--keep-first` / `--keep-last` | how many turns | a count (`3`), a duration (`2h`)                                 |
| `--from` / `--to`              | which turn     | a position (`5`, `-3`), a duration, `last-compaction` (`--from`) |

Each keep flag is mutually exclusive with the bound flag on its side, so the two
never compete.
Passing a position to a keep flag is an error pointing at `--from`/`--to`,
because `--keep-last 3` and `--keep-last -3` would otherwise look like the same
request while naming turns one apart.

## Where the translation lives

- **CLI `--from`/`--to`/`--first`/`--last`/`--turn`**
  (`jp_cli::cmd::turn_range`): `parse_bound` maps a 1-based absolute `N` to
  `RangeBound::Absolute(N - 1)` and a from-end `-N` to `RangeBound::FromEnd(N -
  1)`.
  `--from last-compaction` is the only non-numeric bound, and it is start-only.
  `--turn` endpoints go through `parse_turn_pos`, which produces the same two
  flavours (`TurnPos::Absolute` / `TurnPos::FromEnd`) holding the number as
  written; `TurnPos::to_range_bound` does the 1-based → 0-based shift.
  `--first`/`--last`/`--turn` are complete selectors that set both bounds:
  `--turn N` is `Absolute(N - 1)` on both ends (`--turn A..B` spans \`Absolute(A

  - 1\)`through`Absolute(B - 1)` ),  `--first
    N`is`Absolute(0)`through`Absolute(N - 1)` , and  `--last N`is`FromEnd(N -
    1)\` through the last turn.

- **Config `keep_first`/`keep_last` and the inline DSL**: `RuleBound`'s
  `FromStr` (`jp_config::conversation::compaction`) and `parse_dsl_bound`
  (`jp_cli::cmd::compact_flag`) both map `-N` to `RuleBound::FromEnd(N - 1)`.
  `keep_first_to_bound` / `keep_last_to_bound`
  (`jp_cli::cmd::conversation::compact`) then shift `Absolute(N)` to
  `RangeBound::Absolute(N - 1)` and pass `FromEnd` through unchanged.
  The `RuleBound::Turns(N)` (count) arm is untouched.

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
