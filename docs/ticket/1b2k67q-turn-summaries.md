# Turn Summaries

- **Status**: Done
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-10-30
- **Label**: tracking-rfd=126
- **Label**: type=tracking

Tracking ticket for [RFD 126].

## Implementation plan

- **Add TurnStart summary field and stream accessors**: Turns `TurnStart` into a
  struct with an optional `summary` that is omitted from JSON when `None`.
  Adds `Turn::id()`, `Turn::summary()` and
  `ConversationStream::set_turn_summary`, with tests for serde round-trips,
  legacy JSON and setter lookup by `EventId`.
  Independent, can merge on its own.
- **Add turn summary configuration**: Adds `TurnSummaryConfig` and
  `TurnSummaryGenerateConfig` (`auto`, `batch_size`, `model`) under
  `ConversationConfig` in `jp_config`, with the config traits implemented and
  tests for loading, `--cfg` assignment and deltas.
  Independent, can merge in parallel with Phase 1.
- **Build the turn summarizer in jp_llm**: Adds `jp_llm::turn_summary` with the
  model fallback chain, the stripped standalone transcript builder, turn
  fingerprints, turn-granular packing, the structured output schema and prompt
  with ID filtering, and `mechanical_turn_summary`.
  Covers these with unit tests, including compacted candidates, oversized turns
  and unrequested IDs.
  Depends on Phases 1 and 2.
- **Add TurnSummaryTask and wire into jp query**: Implements `TurnSummaryTask`:
  `run` generates summaries under a cancellation `select!`, and `sync` locks the
  conversation, drops results whose fingerprint changed, writes the rest with
  `set_turn_summary` and flushes.
  Spawns it from `jp query` when `auto && persist`, updates the `--events` long
  help, and adds integration tests for lagged summarization and for edits
  between spawn and sync.
  Depends on Phase 3.

[RFD 126]: ../rfd/126-turn-summaries.md
