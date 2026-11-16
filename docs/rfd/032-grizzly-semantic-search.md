# RFD 032: Grizzly Semantic Search

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-07
- **Summary**: Adds FTS5 full-text search, typo tolerance, and local semantic
  vector search to `grizzly`'s `note_search` tool.

## Summary

This RFD adds FTS5 full-text search, typo tolerance, and local semantic vector
search to `grizzly`'s `note_search` tool, merging results with Reciprocal Rank
Fusion so queries match notes by meaning as well as literal text.

## Context

Grizzly is a Bear.app MCP server in `crates/contrib/grizzly/` that exposes
`note_get`, `note_search`, and `note_create` tools.
Its current search implementation uses SQL `LIKE '%query%'` with hand-rolled
relevance scoring, which has three weaknesses:

1. **Substring matching**: `"prod"` matches `"reproduce"` because matching
   ignores word boundaries.
2. **No typo tolerance**: `"productivty"` matches nothing.
3. **No semantic matching**: searching `"task management"` misses a note titled
   `"GTD methodology"` unless those exact words appear in the note.

This RFD addresses these weaknesses in three independent, additive layers: FTS5
full-text search, trigram typo tolerance, and local semantic search.

## Architecture Overview

```txt
                                  ┌─────────────────────┐
                                  │   note_search tool  │
                                  └──────────┬──────────┘
                                             │
                              ┌──────────────┼──────────────┐
                              ▼              ▼              ▼
                         ┌────────┐    ┌──────────┐   ┌──────────┐
                         │  LIKE  │    │   FTS5   │   │ Semantic │
                         │ search │    │  search  │   │  search  │
                         └────────┘    └──────────┘   └──────────┘
                              │              │              │
                              └──────────────┼──────────────┘
                                             ▼
                                    ┌────────────────┐
                                    │  Score Fusion  │
                                    │    (RRF)       │
                                    └────────────────┘
```

The search function dispatches to the available backends and merges their
results using Reciprocal Rank Fusion (RRF).

## Existing Code

| File            | Purpose                                  |
| --------------- | ---------------------------------------- |
| `src/db.rs`     | `BearDb` opens Bear's read-only SQLite   |
|                 | database, discovers the schema at init,  |
|                 | and hands out short-lived connections.   |
|                 | All queries run through                  |
|                 | `with_connection(fn(conn, cte))`.        |
| `src/schema.rs` | Discovers Bear's Core Data junction      |
|                 | table names (`Z_5TAGS`, etc.) at runtime |
|                 | and generates a normalizing CTE that     |
|                 | maps raw tables to `notes`, `tags`, and  |
|                 | `note_tags` views.                       |
| `src/search.rs` | `SearchParams` and `execute()`,          |
|                 | implementing the current `LIKE`-based    |
|                 | search with SQL-level scoring and        |
|                 | Rust-side line extraction. Most search   |
|                 | changes land here.                       |
| `src/server.rs` | MCP tool handlers. `note_search` calls   |
|                 | `db.search(&params)` and formats results |
|                 | as XML.                                  |
| `src/main.rs`   | CLI entry point (`--jp` for the JP tool  |
|                 | protocol, `--note-create` for write      |
|                 | access) where new flags and subcommands  |
|                 | are added.                               |
| `src/error.rs`  | Crate error enum.                        |
| `Cargo.toml`    | Crate manifest for workspace-gated       |
|                 | dependencies and feature flags.          |

Grizzly opens Bear's database in read-only mode and never writes to it.
Embeddings live in a separate read-write sidecar database owned by Grizzly.

## Layer 1: FTS5 Full-Text Search

Replacing `LIKE '%query%'` with SQLite FTS5 provides tokenized word matching
(eliminating false substring hits), BM25 relevance ranking, prefix queries
(`product*`), phrase queries (`"getting things done"`), and boolean operators
(`productivity OR gtd`).
The existing `rusqlite` `bundled` feature already compiles SQLite with FTS5
enabled, so no new dependencies are needed.

Because `BearDb` uses short-lived connections, each search call creates a
temporary in-memory FTS5 table, populates it from the `notes` CTE, queries it,
and lets it drop when the connection closes:

```sql
CREATE VIRTUAL TABLE temp.fts_notes USING fts5(
    id UNINDEXED,
    title,
    content,
    tokenize='unicode61'
);

INSERT INTO temp.fts_notes(id, title, content)
SELECT id, title, content FROM notes WHERE is_trashed = 0;

-- Search with BM25 ranking
SELECT id, title, content, rank
FROM temp.fts_notes
WHERE fts_notes MATCH ?
ORDER BY rank
LIMIT ?;
```

`SearchParams` gains a `mode` field to control backend selection:

```rust
pub enum SearchMode {
    /// Try FTS5, fall back to LIKE on error.
    Auto,
    /// Force FTS5 (fail if unavailable).
    Fts,
    /// Force LIKE (current behavior).
    Like,
}
```

**File changes:**

- `src/fts.rs` (new): create the temporary FTS5 table, run FTS5 queries, and map
  results back to `ScoredNote`.
- `src/search.rs`: add `mode` to `SearchParams`; `execute()` tries FTS5 first
  and falls back to `LIKE` search if the FTS5 query fails (for example, on
  malformed syntax).
- `src/error.rs`: add the `FtsError` variant.

**Performance:** Building the FTS5 table per search takes under 50ms for a few
hundred notes, but may be slow for thousands of notes and should be benchmarked.
If per-search rebuilds are too slow, the FTS5 table can be cached in a
persistent temporary file that is rebuilt when Bear's database modification time
changes.

**Testing:** Add tests for FTS5 prefix, phrase, and boolean queries, and pass
malformed FTS5 queries to verify the `LIKE` fallback.

### Typo Tolerance via Trigram Tokenizer

FTS5's `trigram` tokenizer indexes character trigrams instead of words, which
tolerates typos because `"productivty"` and `"productivity"` share most of their
trigrams.
Adding a second temporary FTS5 table requires only a tokenizer change and no new
dependencies:

```sql
CREATE VIRTUAL TABLE temp.fts_trigram USING fts5(
    id UNINDEXED,
    title,
    content,
    tokenize='trigram'
);
```

When the `unicode61` FTS5 query returns few or no results, search retries
against `temp.fts_trigram`.
Because trigram matching produces more false positives, a larger in-memory
index, and weaker ranking quality, it serves only as a fallback.

## Layer 2: Semantic Vector Search

Layer 2 embeds notes and queries as vectors using a local ML model, then finds
semantically similar notes by cosine distance.

### Dependencies

Two new dependencies sit behind a `semantic` Cargo feature flag:

1. **`fastembed`** (Apache 2.0, https://crates.io/crates/fastembed): Rust
   library that generates dense float embeddings locally with ONNX Runtime,
   handling model download, tokenization, and inference.

2. **`sqlite-vector`** (Elastic License 2.0 with OSI open-source exception,
   https://github.com/sqliteai/sqlite-vector): C SQLite extension for vector
   similarity search with SIMD-accelerated distance functions and quantization
   for memory efficiency, requiring no preindexing.
   Compiled from source via `build.rs`.

`fastembed` is Apache 2.0 with no restrictions.
`sqlite-vector` is free for OSI-licensed open-source projects such as
MIT-licensed `grizzly`, though using `grizzly` in a closed-source product would
require a commercial license.

```toml
# In Cargo.toml
[features]
default = []
semantic = ["dep:fastembed"]

[dependencies]
fastembed = { version = "5", optional = true, default-features = false, features = [
    "ort-download-binaries-native-tls",
    "hf-hub-native-tls",
] }

[build-dependencies]
cc = "1" # for compiling sqlite-vector C source
```

The `sqlite-vector` C source files are vendored into
`crates/contrib/grizzly/vendor/sqlite-vector/` and compiled in `build.rs` using
the `cc` crate.
Runtime registration requires prototyping: `rusqlite`'s
`load_extension_enable()` and `load_extension()` (or
`Connection::load_extension()`) expect a `.dylib` or `.so` path, so static
compilation may require `rusqlite`'s `create_module()` API or calling
`sqlite3_vector_init` directly via FFI.

### Sidecar Database

Embeddings are stored in a separate read-write SQLite database (`embeddings.db`)
owned by `grizzly`.
Its path is configured via a CLI flag or environment variable, typically inside
the cache directory returned by `directories::ProjectDirs::cache_dir()`.

Schema:

```sql
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT
);
-- Stores: model_name, dimension, last_full_index_at

CREATE TABLE IF NOT EXISTS embeddings (
    note_id TEXT PRIMARY KEY,
    embedding BLOB NOT NULL,      -- float32 vector, dimension from meta
    updated_at TEXT NOT NULL      -- ISO 8601, from Bear's ZMODIFICATIONDATE
);
```

The `embedding` column stores raw `float32` bytes produced by `fastembed`, and
`sqlite-vector` operates directly on these `BLOB`s.

### Indexing

Embeddings are built via a CLI subcommand:

```
grizzly index [--model MODEL_NAME]
```

This command:

1. Opens Bear's database (read-only) and the sidecar (read-write).
2. Loads the embedding model (`BAAI/bge-small-en-v1.5` by default, 512
   dimensions, ~127MB download on first run).
3. Queries all non-trashed notes from Bear.
4. Generates an embedding from `"{title}\n{content}"` for each note missing from
   the sidecar or whose `updated_at` timestamp has changed.
5. Writes embeddings to the sidecar in batches.
6. Calls `vector_init` and `vector_quantize` on the sidecar for fast search.

Indexing is incremental by default, re-embedding only new or modified notes.
Passing `--rebuild` (`grizzly index --rebuild`) forces a full rebuild.

**File changes:**

- `src/embedding.rs` (new): sidecar DB management (open/create, schema
  migration), embedding generation via `fastembed`, and incremental update
  logic.
- `src/semantic.rs` (new): vector search against the sidecar, returning
  `Vec<(note_id, distance)>`.
- `src/main.rs`: add the `index` subcommand with `--model` and `--rebuild`
  flags.
- `src/error.rs`: add `EmbeddingError` and `SidecarError` variants.

### Search Flow

When semantic search is enabled (embeddings exist in the sidecar):

1. Embed the query string using the same model.
2. Query the sidecar: `SELECT note_id, distance FROM
   vector_quantize_scan('embeddings', 'embedding', ?, 50)`.
3. Run FTS5/LIKE search in parallel for the same query string.
4. Merge results using Reciprocal Rank Fusion (RRF).

For each note appearing in any result list, RRF computes `score = sum(1 / (k +
rank_i))`, where `k = 60` (the standard constant) and `rank_i` is the note's
position in result list `i`, then sorts notes by fused score descending:

```rust
fn reciprocal_rank_fusion(
    results: &[Vec<String>],  // each inner vec is note_ids in ranked order
    k: f64,                   // typically 60.0
) -> Vec<(String, f64)> {
    let mut scores: HashMap<String, f64> = HashMap::new();
    for list in results {
        for (rank, note_id) in list.iter().enumerate() {
            *scores.entry(note_id.clone()).or_default() += 1.0 / (k + rank as f64 + 1.0);
        }
    }
    let mut ranked: Vec<_> = scores.into_iter().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked
}
```

**File changes:**

- `src/fusion.rs` (new): RRF implementation.
- `src/search.rs`: `execute()` checks whether the sidecar exists and contains
  embeddings, dispatches to semantic and text search, and merges results via
  RRF.

### Graceful Degradation

The search pipeline degrades gracefully:

| Condition                  | Behavior                         |
| -------------------------- | -------------------------------- |
| Sidecar exists, FTS5 works | Semantic + FTS5, merged with RRF |
| Sidecar exists, FTS5 fails | Semantic + LIKE, merged with RRF |
| No sidecar, FTS5 works     | FTS5 only                        |
| No sidecar, FTS5 fails     | LIKE only (current behavior)     |

Missing embeddings never surface as an error to the user; search falls back to
text matching with lower relevance.

### CLI Flags

```
grizzly [--jp] [--note-create] [--no-semantic]
grizzly index [--model MODEL] [--rebuild]
```

- `--no-semantic`: disable semantic search even if embeddings exist (useful for
  debugging).
- `index` subcommand: build or update the embedding index.
- `--model`: override the embedding model (default: `BAAI/bge-small-en-v1.5`).
- `--rebuild`: force full re-embedding of all notes.

### Model Choice

Default model: `BAAI/bge-small-en-v1.5` (`fastembed`'s default):

- 512 dimensions, 33M parameters, ~127MB download
- Cached in `~/.cache/huggingface/` after first download
- Fast inference (~1ms per short text on M-series Mac)
- Ranks significantly higher than `all-MiniLM-L6-v2` on MTEB benchmarks (rank 94
  vs 123, mean score 43.76 vs 41.39) at the same memory tier

Alternative models trade larger downloads and memory usage for higher quality:

| Model                                | Memory | Dims | MTEB Rank |
| ------------------------------------ | ------ | ---- | --------- |
| `BAAI/bge-base-en-v1.5`              | 390MB  | 768  | 82        |
| `nomic-ai/nomic-embed-text-v1.5`     | 522MB  | 768  | 87        |
| `mixedbread-ai/mxbai-embed-large-v1` | 639MB  | 1024 | 61        |

Users can select a different model with `--model`, which requires a full rebuild
because embeddings from different models are incompatible.

The sidecar stores the active model name in its `meta` table.
On search, `grizzly` verifies that the configured model matches `meta`; on
mismatch, it logs a warning and skips semantic search rather than failing.

## Implementation Order

1. **FTS5**: no new dependencies, moderate complexity.
   Validate that building the temporary table per search is fast enough.
2. **Trigram typo tolerance**: add the fallback trigram table on top of FTS5.
3. **`sqlite-vector` integration**: prototype the `build.rs` compilation and FFI
   registration in isolation before integrating, as this is the highest-risk
   step.
4. **`fastembed` integration**: integrate behind the `semantic` feature flag and
   verify that the ONNX Runtime binary does not bloat the default build.
5. **Sidecar database**: implement schema creation, migration, and incremental
   indexing.
6. **Search fusion**: combine semantic and text search results with RRF.

Steps 1-2 can ship independently.
Steps 3-6 ship together as the `semantic` feature.

## Risks

- **`sqlite-vector` build integration**: `sqlite-vector` expects
  `load_extension()` to call `sqlite3_vector_init`.
  With `rusqlite`'s bundled SQLite, static compilation via `build.rs` may
  require obtaining the `sqlite3*` handle from `rusqlite` and calling
  `sqlite3_vector_init` directly via FFI.
  This requires a prototype.

- **ONNX Runtime binary size**: The `ort` crate (used by `fastembed`) downloads
  prebuilt ONNX Runtime binaries (~50MB) at build time.
  Gating `fastembed` behind the `semantic` feature keeps default builds
  unaffected.

- **First-run model download latency**: The first `grizzly index` run downloads
  the embedding model from Hugging Face (~127MB for the default
  `bge-small-en-v1.5`), while subsequent runs use the local cache.
  `grizzly` should report download progress to the user via `fastembed`'s
  download progress callback.

- **FTS5 temp table rebuild cost**: If Bear has 5,000+ notes, rebuilding the
  FTS5 index on every search may be slow and should be benchmarked early.
  If needed, cache the FTS5 table in a persistent temporary file keyed by Bear's
  database modification time.

- **License**: `sqlite-vector`'s Elastic License 2.0 with the open-source
  exception permits use in MIT-licensed `grizzly`, but requires tracking if the
  project's licensing changes.

## Non-Goals

- Multi-language default embedding models (English-only by default; users can
  override with `--model` if needed).
- Streaming index updates or watch mode (batch rebuilds suffice for personal
  note collections).
- Remote embedding APIs (all inference stays local).
- Image embeddings (Bear notes are text).
- Graph-based note relationships (`sqlitegraph` was evaluated and rejected due
  to its GPL license and unnecessary complexity).
