# RFD 062: CLI Usage Tracking

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-21
- **Required by**: [RFD 063]
- **Summary**: Adds local-only CLI usage tracking to record command invocations
  and argument patterns per workspace for adaptive features.

## Summary

This RFD introduces a per-workspace `usage.json` file that atomically records
command invocations and flag usage on each CLI run.
The file stays on the local machine and is never shared with external services.
Adaptive CLI features read this data for frequency and recency signals.

## Motivation

JP has no memory of how users interact with the CLI across sessions.
Every invocation starts with the same config, field ordering, and defaults, even
when a user passes `--model=opus`, sets `--reasoning=auto`, or overrides the
same `--cfg` fields on nearly every run.

Adaptive features need a record of those habits.
Conversation state captures the event stream of an assistant interaction, but
not the CLI arguments that started it.

This RFD defines the storage format, access patterns, write semantics, and CLI
integration for usage tracking.
The initial implementation records argument usage per command, and the schema
can be extended to capture additional signals (such as query durations or token
counts) without changing the underlying storage mechanism.

## Design

### What to track

The usage file records only data that **cannot be derived from existing state**.
Anything reconstructible from conversation history or config files is omitted.

Initial tracked data:

| Data                       | Why                                          |
| -------------------------- | -------------------------------------------- |
| Command invocations        | Which commands are used, how often, when     |
| Argument usage per command | Which arguments are passed, with what values |

Excluded data (derivable from other sources):

| Data                       | Derivable from                     |
| -------------------------- | ---------------------------------- |
| Conversation count/history | Workspace conversation storage     |
| Model usage over time      | Conversation event streams         |
| Config field values        | Config files + conversation deltas |

Potential future extensions accommodated by the schema:

| Data               | Potential use                           |
| ------------------ | --------------------------------------- |
| Error frequency    | Surfacing reliability issues            |
| Interrupt patterns | UX decisions (streaming vs tool Ctrl+C) |

**Query durations** and **token counts** are not tracked here because they
belong on conversation events, where the event stream infrastructure already
exists.
The usage file tracks CLI invocation patterns rather than conversation-level
telemetry.

### Storage location

Usage data is stored **per-workspace** because flag patterns vary by project.
For example, a workspace that requires Google models sees different flag
patterns than one using Anthropic, so a workspace-local file provides the most
relevant signal.

- **With workspace**: `$XDG_DATA_HOME/jp/workspace/<id>/usage.json`
- **Without workspace** (fallback): `$XDG_DATA_HOME/jp/usage.json`

The workspace path uses `Workspace::user_storage_path()`, which already stores
per-user workspace data such as the active conversation ID.
The global fallback applies when a command runs outside a workspace context.

### Schema

The JSON schema mirrors the CLI subcommand tree:

```json
{
  "cli": {
    "last_used": "2026-07-21T10:30:00Z",
    "commands": {
      "query": {
        "last_used": "2026-07-21T10:30:00Z",
        "args": {
          "model": {
            "last_used": "2026-07-21T10:30:00Z",
            "count": 42,
            "values": {
              "opus": {
                "count": 30,
                "last_used": "2026-07-21T10:30:00Z"
              },
              "haiku": {
                "count": 12,
                "last_used": "2026-07-15T08:00:00Z"
              }
            }
          },
          "config": {
            "last_used": "2026-07-20T14:15:00Z",
            "count": 14,
            "values": {
              "assistant.tool_choice=auto": {
                "count": 5,
                "last_used": "2026-07-20T14:15:00Z"
              },
              "assistant.tool_choice=required": {
                "count": 2,
                "last_used": "2026-07-19T09:00:00Z"
              },
              "style.reasoning.display=full": {
                "count": 7,
                "last_used": "2026-07-18T09:00:00Z"
              }
            }
          },
          "reasoning": {
            "last_used": "2026-07-18T09:00:00Z",
            "count": 15,
            "values": {
              "auto": {
                "count": 10,
                "last_used": "2026-07-18T09:00:00Z"
              },
              "off": {
                "count": 5,
                "last_used": "2026-07-16T12:00:00Z"
              }
            }
          }
        }
      },
      "config": {
        "last_used": "2026-07-19T11:00:00Z",
        "args": {},
        "commands": {
          "show": {
            "last_used": "2026-07-19T11:00:00Z",
            "args": {
              "explain": {
                "last_used": "2026-07-19T11:00:00Z",
                "count": 3,
                "values": {
                  "assistant.model.id": {
                    "count": 2,
                    "last_used": "2026-07-19T11:00:00Z"
                  }
                }
              }
            }
          }
        }
      }
    }
  }
}
```

#### Uniform argument shape

Every argument entry uses the same structure:

```rust
struct ArgUsage {
    last_used: DateTime<Utc>,
    count: u64,
    values: HashMap<String, ValueUsage>,
}

struct ValueUsage {
    count: u64,
    last_used: DateTime<Utc>,
}
```

Arguments are keyed by their **clap argument ID**, which is the Rust field name
on the derive struct (e.g. `model`, `no_edit`, `reasoning`).
Short flags, long flags, and positional arguments share this ID space, so `-m`,
`--model`, and `--model=opus` all record under `model`.

The `values` map is keyed by the raw argument string from the command line:
`opus` for `model`, `auto` for `reasoning`, and the full `KEY=VALUE` string
(such as `assistant.tool_choice=auto`) for `config` (`--cfg`).
Because every argument uses the same representation, argument-specific parsing
(such as splitting `--cfg` values on `=` to group by field path) happens on the
read side.

#### Recursive command structure

The `commands` map is recursive so subcommands nest under their parent command:

```rust
struct CommandUsage {
    last_used: DateTime<Utc>,
    args: HashMap<String, ArgUsage>,
    commands: HashMap<String, CommandUsage>,
}
```

`jp config show --explain` records under `cli.commands.config.commands.show`,
and `jp query --model=opus` records under `cli.commands.query`.

### Integration: `CliUsage` type

A `CliUsage` type in `jp_cli` loads, updates, and persists usage data.
It lives on `Ctx` so any command handler can access it:

```rust
pub(crate) struct Ctx {
    pub(crate) workspace: Workspace,
    // ... existing fields ...
    pub(crate) usage: CliUsage,
}
```

#### Loading

`CliUsage::load` reads `usage.json` from the workspace user storage path or the
global fallback.
If the file is missing or fails to parse, it returns an empty `CliUsage` so
corrupted or missing usage data never aborts a command.
Parse failures on an existing file log a warning.

```rust
impl CliUsage {
    pub fn load(workspace: Option<&Workspace>) -> Self {
        let path = Self::storage_path(workspace);
        match path.and_then(|p| Self::read_from(&p)) {
            Some(usage) => usage,
            None => Self::default(),
        }
    }

    fn storage_path(workspace: Option<&Workspace>) -> Option<Utf8PathBuf> {
        workspace
            .and_then(Workspace::user_storage_path)
            .map(|p| p.join("usage.json"))
            .or_else(|| {
                user_data_dir().ok().map(|p| p.join("usage.json"))
            })
    }
}
```

#### Recording

Commands record their argument usage via `CliUsage::record`:

```rust
impl CliUsage {
    /// Record that an argument was used with the given value.
    pub fn record_arg(
        &mut self,
        command_path: &[&str],
        arg_id: &str,
        value: Option<&str>,
        now: DateTime<Utc>,
    ) {
        let cmd = self.get_or_create_command(command_path);
        cmd.last_used = now;

        let entry = cmd.args.entry(arg_id.to_owned()).or_default();
        entry.count += 1;
        entry.last_used = now;

        if let Some(val) = value {
            let val_entry = entry.values.entry(val.to_owned()).or_default();
            val_entry.count += 1;
            val_entry.last_used = now;
        }
    }
}
```

`command_path` is a slice such as `&["query"]` or `&["config", "show"]`,
corresponding to the command's path in the clap hierarchy.

#### Automatic recording via `ArgMatches`

Instead of adding manual `record_flag` calls in each command's `run` method,
`run_inner()` splits `Cli::parse()` into two steps to retain the raw
`ArgMatches`:

```rust
// Before (current):
let cli = Cli::parse();

// After:
let matches = Cli::command().get_matches();
let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
```

This is clap's standard pattern for combining the derive and builder APIs.
It parses the command line once, so value parsers (including file-reading
parsers such as `string_or_path`) do not run twice.
[RFD 060] shares this parse split, using the retained `ArgMatches` for config
explain provenance while this RFD uses them for usage recording.

A generic `record_from_matches` function walks the `ArgMatches` tree and records
every argument whose `value_source()` is `ValueSource::CommandLine`:

```rust
fn record_from_matches(
    usage: &mut CliUsage,
    command_path: &[&str],
    matches: &ArgMatches,
    now: DateTime<Utc>,
) {
    // Record each explicitly-provided argument.
    for id in matches.ids() {
        let arg_id = id.as_str();
        if matches.value_source(arg_id) != Some(ValueSource::CommandLine) {
            continue;
        }

        // get_raw returns the pre-value-parser OsStr values.
        match matches.get_raw(arg_id) {
            Some(raw_values) => {
                for val in raw_values.filter_map(|v| v.to_str()) {
                    usage.record_arg(command_path, arg_id, Some(val), now);
                }
            }
            None => {
                // Boolean flag with no value (e.g. no_edit).
                usage.record_arg(command_path, arg_id, None, now);
            }
        }
    }

    // Recurse into subcommands.
    if let Some((name, sub_matches)) = matches.subcommand() {
        let mut path = command_path.to_vec();
        path.push(name);
        record_from_matches(usage, &path, sub_matches, now);
    }
}
```

Walking `ArgMatches` captures every flag on every command automatically, so new
flags require no usage-tracking changes.
Global arguments (such as `--cfg`, `--verbose`, and `--persist`) also need no
special handling: clap propagates them to subcommand matches, and
`value_source()` reports `CommandLine` only when the user passed them on the
command line.

#### Persisting

Usage is saved at the end of `run_inner()`, after the command completes and
alongside workspace persistence:

```rust
// In run_inner(), after Commands::run() and task_handler.sync():
ctx.usage.save();
```

`save()` is fire-and-forget: it logs a warning on failure and never propagates
errors, so usage tracking cannot cause a command to fail.

### Atomic writes

The `write_json` helper in `jp_storage` writes directly to the target path,
which can corrupt the file if the process is interrupted mid-write.
Because every command invocation updates `usage.json`, `CliUsage::save` writes
to a temporary file in the target directory and renames it into place:

```rust
impl CliUsage {
    pub fn save(&self) {
        let Some(path) = Self::storage_path(self.workspace.as_ref()) else {
            return;
        };

        if let Err(error) = self.write_atomic(&path) {
            tracing::warn!(%error, path = %path, "Failed to save usage data.");
        }
    }

    fn write_atomic(&self, path: &Utf8Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Write to a temp file in the same directory (same filesystem
        // guarantees atomic rename on POSIX).
        let dir = path.parent().unwrap_or(path);
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer_pretty(&mut tmp, &self.data)?;
        tmp.as_file_mut().flush()?;
        tmp.persist(path)?;

        Ok(())
    }
}
```

`tempfile::NamedTempFile::persist` uses `rename(2)` on POSIX (atomic within a
single filesystem) and `MoveFileEx` with `MOVEFILE_REPLACE_EXISTING` on Windows.

### Concurrency

Multiple JP processes can run concurrently, such as detached queries or parallel
terminal sessions.
Without locking, concurrent writes can race: both processes read `usage.json`,
each updates its in-memory copy, and the last writer overwrites the first
writer's updates.

This race is acceptable for the initial implementation.
Usage counts are heuristics, so a lost counter increment does not affect
correctness, and atomic writes prevent partial writes from corrupting the file.
If concurrent runs cause noticeable drift, a future change can guard the
read-modify-write cycle with advisory file locking (`flock` on POSIX).

### Privacy

The usage file is **local-only** and is never transmitted to external services.
It contains only structural metadata about CLI invocations (command names,
argument IDs, and argument values such as model names or config field paths),
never conversation content, prompts, or LLM responses.
User-facing documentation and `jp init` output must state this local-only
guarantee clearly.

## Drawbacks

- **Write on every invocation**: Every command run writes to `usage.json`,
  adding roughly 1-2ms of I/O (JSON serialization, flush, and rename) even when
  no feature consumes the data.

- **Stale data under concurrency**: Without locking, concurrent writes have
  last-writer-wins races that silently drop increments, so usage counts are
  approximate rather than exact.

- **Schema evolution**: As the JSON schema grows, deserialization must remain
  forward-compatible by ignoring unknown fields and defaulting missing fields to
  zero or empty.

## Alternatives

### SQLite instead of JSON

SQLite provides atomic writes, concurrent access, and indexed queries out of the
box, but adds a heavy dependency and stores data in a binary format that is
harder to inspect by hand.
For a small counters file, human-readable JSON is simpler to debug and fast
enough.

### Session-only tracking (in-memory)

Tracking usage only in memory within a single session avoids file I/O, but
provides no cross-session signal for features like wizard field ordering.

### Append-only log instead of mutable JSON

Appending one line per invocation to a log file and aggregating on read avoids
concurrent-write races (small appends are atomic on POSIX), but makes reads scan
the full log and requires periodic compaction.
Counter data does not justify that complexity.

## Non-Goals

- **Analytics or telemetry**: No data leaves the user's machine.
  There is no aggregation server, opt-in/opt-out toggle, or anonymization
  pipeline.

- **Query performance tracking**: Tracking query durations, token counts, or
  error rates is out of scope for this RFD.
  The schema leaves room for additional top-level keys alongside `cli`, but this
  RFD only implements CLI argument recording.

- **Cross-workspace aggregation**: Each workspace has its own `usage.json`, with
  no mechanism to merge or query usage across workspaces.

## Risks and Open Questions

- **Schema versioning**: The initial `usage.json` schema has no version field,
  so a breaking schema change would require detecting the old format to migrate
  or discard it.
  Adding `"version": 1` before accepting this RFD is a low-cost safeguard.

- **Large value cardinality**: Flags like `--cfg` accept arbitrary `KEY=VALUE`
  strings, so the `values` map can grow large over months of use.
  A future RFD can add eviction (for example, dropping entries older than 90
  days or with count < 3).

- **`--no-persist` interaction**: Passing `--no-persist` (or `-!`) disables
  workspace persistence.
  Although usage is user-local metadata rather than workspace state,
  `--no-persist` signals a "leave no trace" invocation, so `run_inner()` should
  also suppress `usage.json` writes when `--no-persist` is active.

- **Field renames orphan usage data**: Clap's derive API derives each argument
  ID from the Rust struct field name rather than `long` or `short`.
  Renaming a field (e.g. `model` → `model_id`) changes the clap ID from `model`
  to `model-id` even if `-m` and `--model` are unchanged, orphaning the old ID's
  counters.
  Because usage data is heuristic, a reset counter is only a temporary loss of
  accuracy, and orphaned entries would eventually age out under eviction.
  Manual `record_arg` calls with hardcoded strings would survive a rename only
  if someone remembers to update the string, and explicit `#[arg(id = "model")]`
  attributes on every argument add boilerplate that is not needed initially.

## Implementation Plan

### Phase 1: Core types and storage

1. Define `CliUsage`, `CommandUsage`, `ArgUsage`, and `ValueUsage` types with
   serde derives.
2. Implement `CliUsage::load()` with graceful fallback on missing/corrupt files.
3. Implement `CliUsage::save()` with atomic temp-file-then-rename writes.
4. Implement `CliUsage::storage_path()` with workspace-local and global fallback
   logic.
5. Unit tests for load/save roundtrip, corrupt file handling, missing file
   handling.

### Phase 2: Integration into Ctx and run_inner

1. Add `usage: CliUsage` field to `Ctx`.
2. Load usage in `run_inner()` after workspace initialization.
3. Save usage at the end of `run_inner()`, gated on `persist` flag.
4. Implement `CliUsage::record_arg()` and
   `CliUsage::record_command_invocation()`.

### Phase 3: Wire up automatic recording in `run_inner`

1. Split `Cli::parse()` into `Cli::command().get_matches()` +
   `Cli::from_arg_matches()` in `run_inner()`.
2. Call `record_from_matches` with the root `ArgMatches` after workspace
   initialization.
3. Track all commands and flags automatically without per-command changes.

Phase 1 can be merged independently.
Phase 2 depends on Phase 1, and Phase 3 is a single PR touching only
`run_inner()` on top of Phase 2.

## References

- [RFD 061]: Interactive config (first consumer of usage data)
- `Ctx` in `jp_cli/src/ctx.rs`: CLI context struct
- `Workspace::user_storage_path()`: per-user, per-workspace storage
- `user_data_dir()` in `jp_workspace`: global user data directory
- `write_json` in `jp_storage/src/value.rs`: current (non-atomic) JSON writer
- `tempfile::NamedTempFile::persist`: atomic rename on POSIX/Windows

[RFD 060]: 060-config-explain.md
[RFD 061]: 061-interactive-config.md
[RFD 063]: 063-usage-based-wizard-field-ordering.md
