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

The boundary is the `jp_cli` resolution layer.
`jp_config` carries user values (1-based), `jp_conversation` carries core values
(0-based), and `jp_cli` translates between them.

## Positions vs. counts

Only *positions* (indices into the conversation) are subject to the 1-based
rule.
A *count* — "how many turns" — is base-independent and is never shifted.

| Kind                | Examples                                                                                                | Translated?                                          |
| ------------------- | ------------------------------------------------------------------------------------------------------- | ---------------------------------------------------- |
| Position (absolute) | `--turn N`, `--from N`, `--to N`, DSL `N..M`, config `keep_first = "@N"`                                | yes, `N` (1-based) → `N - 1` (0-based)               |
| Position (from end) | `--from -N`, `--to -N`                                                                                  | yes, `-1` is the last turn (`-N` → `FromEnd(N - 1)`) |
| Count               | `--first N`, `--last N`, `--keep-first N`, `--keep-last N`, config `keep_first = N`, DSL `-N` shorthand | no                                                   |
| Duration            | `5h`, `2days`                                                                                           | n/a (resolved against timestamps)                    |

Two consequences worth calling out:

- `--from -1` and `--to -1` address the **last** turn, matching the 1-based
  reading where `1` is the first turn and `-1` is the last.
  As a result `--from -N` selects the same starting turn as `--last N`.

- The compaction DSL's `-N` (e.g. `..-3`, "keep the last 3 turns") is a *count*,
  not a position.
  It is unaffected by the 1-based rule and keeps its Python-slice "keep last N"
  meaning.
  Only the DSL's absolute bounds (`5..`) are positions and are 1-based.

- Ranges are written `A..B` and are **inclusive on both ends** — `1..5` is
  turns 1 through 5 (five turns).
  This one format is shared by `--turn A..B`, the compaction DSL, and the
  timeline output.
  Either end may be omitted to mean the conversation start or end (`10..`,
  `..10`, `..`).
  Note this diverges from Rust's `..` (which is exclusive); there is no `..=`
  form.

## Where the translation lives

- **CLI `--from`/`--to`/`--first`/`--last`/`--turn`**
  (`jp_cli::cmd::turn_range`): `parse_bound` maps a 1-based absolute `N` to
  `RangeBound::Absolute(N - 1)` and a from-end `-N` to `RangeBound::FromEnd(N -
  1)`.
  `--first`/`--last`/`--turn` are complete selectors that set both bounds:
  `--turn N` is `Absolute(N - 1)` on both ends (`--turn A..B` spans \`Absolute(A

  - 1\)`through`Absolute(B - 1)` ),  `--first
    N`is`Absolute(0)`through`Absolute(N - 1)` , and  `--last N`is`FromEnd(N -
    1)\` through the last turn.

- **Config `keep_first`/`keep_last` and the inline DSL**
  (`jp_cli::cmd::conversation::compact`): `keep_first_to_bound` /
  `keep_last_to_bound` map `RuleBound::Absolute(N)` (the 1-based value parsed
  from `@N`) to `RangeBound::Absolute(N - 1)`.
  The `RuleBound::Turns(N)` (count) arm is untouched.

- **Timeline output** (`jp_cli::cmd::conversation::compact::timeline_lines`):
  the stored 0-based `from_turn`/`to_turn` are rendered as 1-based, e.g.
  `Compacted turns 2..8`.

When adding a new flag, config key, or output that names a turn, decide first
whether it is a position or a count.
If it is a position, it is 1-based on the user side and translated to 0-based
exactly where it meets the stream.
