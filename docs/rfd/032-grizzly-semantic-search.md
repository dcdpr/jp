# RFD 032: Grizzly Semantic Search

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-07

## Summary

Add semantic vector search, FTS5 full-text search, and typo tolerance to
grizzly's `note_search` tool. The goal is to find notes by *meaning*, not just
literal text. Searching "productivity systems" should surface notes about GTD
and Pomodoro even when those exact words don't appear.

## Context

Grizzly is a Bear.app MCP server that lives in `crates/contrib/grizzly/`. It
exposes `note_get`, `note_search`, and `note_create` tools over the MCP
protocol. The current search implementation uses SQL `LIKE '%query%'` with
hand-rolled relevance scoring. It works but has three weaknesses:

1. **Substring matching** — "prod" matches "reproduce", no word boundary
   awareness.
2. **No typo handling** — "productivty" finds nothing.
3. **No semantic understanding** — searching "task management" won't find a note
   titled "GTD methodology" unless those words appear literally.

This RFD covers three layers of improvement, each independent and additive.

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

The search function dispatches to one or more backends depending on what's
available, then merges results using Reciprocal Rank Fusion.

## Existing Code You Need to Know

| File            | What it does                             |
|-----------------|------------------------------------------|
| `src/db.rs`     | `BearDb` — opens Bear's read-only SQLite |
|                 | database, discovers schema at init,      |
|                 | hands out short-lived connections. All   |
|                 | queries go through                       |
|                 | `with_connection(fn(conn, cte))`.        |
| `src/schema.rs` | Discovers Bear's Core Data junction      |
|                 | table names (`Z_5TAGS`, etc.) at         |
|                 | runtime, generates a normalizing CTE     |
|                 | that maps raw tables to clean `notes`,   |
|                 | `tags`, `note_tags` views.               |
| `src/search.rs` | `SearchParams` and `execute()` — the     |
|                 | current LIKE-based search with SQL-level |
|                 | scoring and Rust-side line extraction.   |
|                 | This is the file you'll modify most.     |
| `src/server.rs` | MCP tool handlers. `note_search` calls   |
|                 | `db.search(&params)` and formats results |
|                 | as XML.                                  |
| `src/main.rs`   | CLI entry point. `--jp` flag for JP tool |
|                 | protocol, `--note-create` for write      |
|                 | access. You'll add new flags here.       |
| `src/error.rs`  | Error enum. You'll add new variants.     |
| `Cargo.toml`    | Dependencies are gated behind the        |
|                 | workspace. New deps go here with feature |
|                 | flags.                                   |

Bear's database is read-only to us (we never write to it). The sidecar database
for embeddings is ours and we can write to it freely.

## Layer 1: FTS5 Full-Text Search

### What

Replace `LIKE '%query%'` with SQLite FTS5 for word-aware search with BM25
ranking.

### Why

FTS5 gives us tokenized word matching (no false substring hits), BM25 relevance
ranking, prefix queries (`product*`), phrase queries (`"getting things done"`),
and boolean operators (`productivity OR gtd`).

### How

Our `rusqlite` dependency already compiles SQLite with FTS5 enabled (the
`bundled` feature includes it). No new dependencies needed.

**On each search call**, create a temporary in-memory FTS5 table, populate it
from the notes CTE, query it, then let it drop when the connection closes. Since
we use short-lived connections, this happens every search.

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

**File changes:**

- New file `src/fts.rs` — functions to create the temp FTS table, run FTS5
  queries, and map results back to `ScoredNote`.
- Modify `src/search.rs` — `execute()` tries FTS5 first. If the FTS5 query fails
  (malformed syntax, etc.), fall back to LIKE search. Add a `mode` field to
  `SearchParams`:

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

- Modify `src/error.rs` — add `FtsError` variant.

**Performance concern:** For a few hundred notes, building the FTS table
per-search is <50ms. For thousands, it could be slow. Measure this. If it's a
problem, consider caching the FTS table in a persistent temp file that gets
rebuilt when Bear's database modification time changes.

**Testing:** Add tests that exercise FTS5 query syntax (prefix, phrase,
boolean). Test the LIKE fallback by passing malformed FTS5 queries.

### Typo Tolerance via Trigram Tokenizer

FTS5 supports a `trigram` tokenizer that indexes character trigrams instead of
words. This naturally handles typos because "productivty" and "productivity"
share most trigrams.

Add a second FTS5 table using the trigram tokenizer:

```sql
CREATE VIRTUAL TABLE temp.fts_trigram USING fts5(
    id UNINDEXED,
    title,
    content,
    tokenize='trigram'
);
```

When the `unicode61` FTS5 search returns few or no results, retry against the
trigram table. The trigram table has worse ranking quality (many false
positives) so it's a fallback, not the primary.

This is a one-line tokenizer change with no new dependencies. The tradeoff is a
larger in-memory index and noisier results.

## Layer 2: Semantic Vector Search

### What

Embed notes and queries as vectors using a local ML model, then find
semantically similar notes by cosine distance.

### Dependencies

Two new dependencies, both behind a `semantic` cargo feature flag:

1. **`fastembed`** (Apache 2.0) — Rust library for generating text embeddings
   locally using ONNX Runtime. Handles model download, tokenization, and
   inference. We use it to convert text into dense float vectors. Crate:
   https://crates.io/crates/fastembed

2. **`sqlite-vector`** (Elastic License 2.0 with OSI open-source exception) —
   C-based SQLite extension for vector similarity search. SIMD-accelerated
   distance functions, quantization for memory efficiency, no preindexing
   required. We compile it from source via `build.rs`. Source:
   https://github.com/sqliteai/sqlite-vector

The license situation: `sqlite-vector` is free for OSI-licensed open-source
projects (grizzly is MIT). If grizzly is ever used in a closed-source product, a
commercial license would be needed. `fastembed` is Apache 2.0, no restrictions.

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

For `sqlite-vector`, vendor the C source files into
`crates/contrib/grizzly/vendor/sqlite-vector/` and compile them in `build.rs`
using the `cc` crate. Then register the extension at runtime via `rusqlite`'s
`load_extension_enable()` + `load_extension()`, or (better) use
`Connection::load_extension()` with the compiled static library. The exact
integration path needs prototyping — rusqlite's `load_extension` wants a
`.dylib`/`.so` path, so the static compilation approach may require using
rusqlite's `create_module()` API or calling `sqlite3_vector_init` directly via
FFI.

### Sidecar Database

Embeddings are stored in a separate SQLite `embeddings.db` database. This
database is owned by grizzly and is read-write. The path is set using a CLI flag
or environment variable. Ideally the caller uses something like
`directories::ProjectDirs::cache_dir()` to get the cache directory.

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

The `embedding` column stores raw float32 bytes (via `fastembed`'s output).
`sqlite-vector` operates directly on these BLOBs.

### Indexing

Embeddings are built via a CLI subcommand:

```
grizzly index [--model MODEL_NAME]
```

This:
1. Opens Bear's database (read-only) and the sidecar (read-write).
2. Loads the embedding model (`BAAI/bge-small-en-v1.5` by default, 512
   dimensions, ~127MB download on first run).
3. Queries all non-trashed notes from Bear.
4. For each note whose `updated_at` has changed (or is missing from the
   sidecar), generates an embedding from `"{title}\n{content}"`.
5. Writes embeddings to the sidecar in batches.
6. Calls `vector_init` and `vector_quantize` on the sidecar for fast search.

Incremental: only re-embeds notes whose modification timestamp changed. Full
rebuild via `grizzly index --rebuild`.

**File changes:**

- New file `src/embedding.rs` — sidecar DB management (open/create, schema
  migration), embedding generation via `fastembed`, incremental update logic.
- New file `src/semantic.rs` — vector search against the sidecar, returns
  `Vec<(note_id, distance)>`.
- Modify `src/main.rs` — add `index` subcommand with `--model` and `--rebuild`
  flags.
- Modify `src/error.rs` — add `EmbeddingError`, `SidecarError` variants.

### Search Flow

When semantic search is enabled (embeddings exist in the sidecar):

1. Embed the query string using the same model.
2. Query the sidecar: `SELECT note_id, distance FROM
   vector_quantize_scan('embeddings', 'embedding', ?, 50)`.
3. Run FTS5/LIKE search in parallel (same query string).
4. Merge results using Reciprocal Rank Fusion (RRF).

RRF formula: for each note appearing in any result list, `score = sum(1 / (k +
rank_i))` where `k = 60` (standard constant) and `rank_i` is the note's position
in result list `i`. Notes are then sorted by fused score descending.

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

- New file `src/fusion.rs` — the RRF implementation.
- Modify `src/search.rs` — `execute()` checks if sidecar exists and has
  embeddings, dispatches to semantic + text search, merges via RRF.

### Graceful Degradation

The search pipeline degrades gracefully:

| Condition                  | Behavior                         |
|----------------------------|----------------------------------|
| Sidecar exists, FTS5 works | Semantic + FTS5, merged with RRF |
| Sidecar exists, FTS5 fails | Semantic + LIKE, merged with RRF |
| No sidecar, FTS5 works     | FTS5 only                        |
| No sidecar, FTS5 fails     | LIKE only (current behavior)     |

The user never sees an error from missing embeddings. They just get less
relevant results.

### CLI Flags

```
grizzly [--jp] [--note-create] [--no-semantic]
grizzly index [--model MODEL] [--rebuild]
```

- `--no-semantic` — disable semantic search even if embeddings exist (useful for
  debugging).
- `index` subcommand — build or update the embedding index.
- `--model` — override the embedding model (default: `BAAI/bge-small-en-v1.5`).
- `--rebuild` — force full re-embedding of all notes.

### Model Choice

Default: `BAAI/bge-small-en-v1.5` (fastembed's own default)

- 512 dimensions, 33M parameters, ~127MB download
- Cached in `~/.cache/huggingface/` after first download
- Fast inference (~1ms per short text on M-series Mac)
- Ranks significantly higher than `all-MiniLM-L6-v2` on MTEB benchmarks
  (rank 94 vs 123, mean score 43.76 vs 41.39) at the same memory tier

Other reasonable choices if users want to trade speed/size for quality:

| Model                                | Memory | Dims | MTEB Rank |
|--------------------------------------|--------|------|-----------|
| `BAAI/bge-base-en-v1.5`              | 390MB  | 768  | 82        |
| `nomic-ai/nomic-embed-text-v1.5`     | 522MB  | 768  | 87        |
| `mixedbread-ai/mxbai-embed-large-v1` | 639MB  | 1024 | 61        |

Users can switch via `--model`, but changing models requires a full rebuild
since embeddings from different models aren't compatible.

Store the model name in the sidecar's `meta` table. On search, verify the model
matches. If it doesn't, log a warning and skip semantic search (don't crash).

## Implementation Order

1. **FTS5** — no new dependencies, moderate complexity. Validate that the
   temp-table-per-search approach is fast enough.
2. **Trigram typo tolerance** — trivial addition on top of FTS5.
3. **sqlite-vector integration** — figure out the build.rs / FFI story. This is
   the riskiest part. Prototype it in isolation before integrating.
4. **fastembed integration** — straightforward API, but the ONNX Runtime binary
   is large. Verify it works behind a feature flag without bloating the default
   build.
5. **Sidecar database** — schema, migration, incremental indexing.
6. **Search fusion** — wire it all together with RRF.

Steps 1-2 can ship independently. Steps 3-6 ship together as the `semantic`
feature.

## Risks

- **sqlite-vector build integration**: Compiling C source via `build.rs` and
  loading it into rusqlite's bundled SQLite may be tricky. The extension expects
  to be loaded via `sqlite3_vector_init`, which is normally called by
  `load_extension()`. With bundled SQLite, we may need to call the init function
  directly via FFI after getting the `sqlite3*` handle from rusqlite. This needs
  a prototype.

- **ONNX Runtime binary size**: The `ort` crate (used by `fastembed`) downloads
  prebuilt ONNX Runtime binaries (~50MB). This only affects builds with the
  `semantic` feature, but it's a large addition. The download happens at build
  time, not runtime.

- **First-run model download latency**: The first `grizzly index` call downloads
  the embedding model from Hugging Face (~127MB for the default
  `bge-small-en-v1.5`). Subsequent runs use the cache. This should be clearly
  communicated to the user (fastembed supports a download progress callback).

- **FTS5 temp table rebuild cost**: If Bear has 5,000+ notes, rebuilding the
  FTS5 index on every search could be slow. Measure this early. Mitigation:
  cache the FTS table in a persistent temp file, keyed by the Bear database's
  file modification time.

- **License**: `sqlite-vector`'s Elastic License 2.0 with open-source exception
  is fine for grizzly (MIT), but worth tracking if the project's license
  situation changes.

## Not In Scope

- Multi-language embedding models (English-only for now; users can override with
  `--model` if needed)
- Streaming index updates / watch mode (batch rebuild is fine for personal note
  collections)
- Remote embedding APIs (everything stays local)
- Image embeddings (Bear notes are text)
- Graph-based note relationships (explored `sqlitegraph`, rejected due to GPL
  license and overkill)
