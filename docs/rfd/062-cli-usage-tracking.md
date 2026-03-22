# RFD 062: CLI Usage Tracking

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-21

## Summary

Introduce a `usage.json` file that records how the CLI is used — which commands
are invoked, which flags are passed, and how often. This data enables features
that adapt to a user's workflow. The file is stored per-workspace and updated
atomically on every command invocation.

This data is **NOT shared with any external service**, it is purely local-only,
used to enhance the local JP experience.

## Motivation

JP currently has no memory of how users interact with the CLI across sessions.
Every invocation starts from the same state: the same config, the same field
ordering, the same defaults. But users develop patterns — they frequently use
`--model=opus`, they always set `--reasoning=auto`, they reach for the same
`--cfg` fields repeatedly.

Without usage data, features that want to adapt to these patterns have no signal
to work with. Conversation state captures the stream of events during
interactions with the assistant, but not how the user invoked the CLI to get
there.

This RFD establishes the infrastructure for recording CLI usage. It defines the
storage format, access patterns, write semantics, and integration points. The
initial implementation records argument usage per command. Future RFDs can extend
the schema to capture additional signals (query durations, token counts, etc.)
without changing the infrastructure.

## Design

### What to track

The usage file records data that **cannot be derived from existing state**. The
guiding principle: if you can reconstruct it from conversation history, or
config files, don't duplicate it here.

Initial tracked data:

| Data                        | Why                                          |
|-----------------------------|----------------------------------------------|
| Command invocations         | Which commands are used, how often, when     |
| Argument usage per command  | Which arguments are passed, with what values |

Data explicitly **not** tracked (derivable from other sources):

| Data                       | Derivable from                     |
|----------------------------|------------------------------------|
| Conversation count/history | Workspace conversation storage     |
| Model usage over time      | Conversation event streams         |
| Config field values        | Config files + conversation deltas |

Future extensions (not implemented in this RFD, but the schema accommodates
them):

| Data               | Potential use                            |
|--------------------|------------------------------------------|
| Error frequency    | Surfacing reliability issues             |
| Interrupt patterns | UX decisions (streaming vs tool Ctrl+C)  |

Note that **query durations** and **token counts** are not tracked here. These
are metadata that belong on conversation events here the infrastructure already
exists. The usage file tracks CLI surface-level patterns, not conversation-level
telemetry.

### Storage location

Usage data is stored **per-workspace** because usage patterns vary across
workspaces. A user working on a project that requires Google models will have
different flag patterns than one using Anthropic. The workspace-local file
provides the most relevant signal.

- **With workspace**: `$XDG_DATA_HOME/jp/workspace/<id>/usage.json`
- **Without workspace** (fallback): `$XDG_DATA_HOME/jp/usage.json`

The fallback applies when no workspace is available (e.g., future commands that
operate outside a workspace context).

The workspace user storage path (`Workspace::user_storage_path()`) already
exists and is used for other per-user, per-workspace data (like the active
conversation ID). Usage tracking follows the same pattern.

### Schema

The file is JSON. The top-level structure mirrors the CLI's subcommand tree:

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

Every argument entry has the same structure — no argument gets special
treatment:

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

Arguments are keyed by their **clap argument ID** — the Rust field name in the
derive struct (e.g. `model`, `no_edit`, `reasoning`). This is the canonical
identifier that is stable regardless of whether the user typed `-m`, `--model`,
or `--model=opus`. Short flags, long flags, and positional arguments all share
the same ID space.

The `values` key is always the raw string the user typed as the argument's
value. For `model`, that's `opus`. For `reasoning`, that's `auto`. For `config`
(`--cfg`), that's `assistant.tool_choice=auto` (the full `KEY=VALUE` string).

This uniformity means consumers don't need to know which arguments are
"special." Any argument-specific interpretation (like splitting `--cfg` values
on `=` to group by field path) happens on the read side.

#### Recursive command structure

The `commands` object is recursive — subcommands nest naturally:

```rust
struct CommandUsage {
    last_used: DateTime<Utc>,
    args: HashMap<String, ArgUsage>,
    commands: HashMap<String, CommandUsage>,
}
```

`jp config show --explain` records under `cli.commands.config.commands.show`.
`jp query --model=opus` records under `cli.commands.query`.

### Integration: `CliUsage` type

A new `CliUsage` type in `jp_cli` manages loading, updating, and persisting
usage data. It is added to `Ctx` so any command handler can record usage:

```rust
pub(crate) struct Ctx {
    pub(crate) workspace: Workspace,
    // ... existing fields ...
    pub(crate) usage: CliUsage,
}
```

#### Loading

`CliUsage::load` reads the file from the workspace's user storage path (or the
global fallback). If the file doesn't exist or fails to parse, it returns an
empty `CliUsage` — corrupted or missing usage data is never an error. A warning
is logged if the file exists but can't be parsed.

Note: the `Cli::parse()` split into `Cli::command().get_matches()` +
`Cli::from_arg_matches()` described below is shared infrastructure that [RFD
060][] also builds on. RFD 060 uses the retained `ArgMatches` for config explain
provenance; this RFD uses them for usage recording. Both operate on the same
single parse result.

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

The `command_path` is a slice like `&["query"]` or `&["config", "show"]`,
derived from the command's position in the clap hierarchy.

#### Recording pattern: automatic via `ArgMatches`

Rather than manually adding `record_flag` calls in each command's `run` method,
flag recording is automated using clap's `ArgMatches` API.

The entry point in `run_inner()` splits `Cli::parse()` into two steps to retain
access to the raw matches:

```rust
// Before (current):
let cli = Cli::parse();

// After:
let matches = Cli::command().get_matches();
let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
```

This is clap's canonical approach for mixing the derive and builder APIs. It
performs a single parse — no value parsers run twice, no file-reading side
effects from parsers like `string_or_path` are duplicated.

A generic `record_from_matches` function walks the `ArgMatches` tree and
records every argument whose `value_source()` is `ValueSource::CommandLine`:

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

This approach eliminates Phase 3 entirely — no per-command manual recording
needed. Every flag on every command is automatically captured. New flags added
to any command are tracked without any usage-tracking code changes.

Global arguments (like `--cfg`, `--verbose`, `--persist`) are handled correctly:
clap propagates them to subcommand matches, and `value_source()` accurately
reports `CommandLine` only when the user explicitly passed them.

#### Persisting

Usage is saved at the end of `run_inner()`, after the command completes but
before the process exits. This happens alongside the existing workspace
persistence:

```rust
// In run_inner(), after Commands::run() and task_handler.sync():
ctx.usage.save();
```

`save()` is fire-and-forget — it logs warnings on failure but never propagates
errors. Usage tracking must never cause a command to fail.

### Atomic writes

The current `write_json` helper in `jp_storage` writes directly to the target
path, which can corrupt the file if the process is interrupted mid-write. Usage
tracking requires atomic writes because every command invocation writes to the
same file.

`CliUsage::save` writes atomically using temp-file-then-rename:

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

`tempfile::NamedTempFile::persist` uses `rename(2)` on POSIX, which is atomic
within a filesystem. On Windows it uses `MoveFileEx` with
`MOVEFILE_REPLACE_EXISTING`.

### Concurrency

Multiple JP processes can run concurrently (e.g., detached queries, parallel
terminal sessions). Without locking, concurrent writes can race: both processes
read the file, both update their in-memory copy, and the last writer wins —
the first writer's updates are lost.

For the initial implementation, this is acceptable. The data is heuristic — a
lost increment of a flag counter doesn't affect correctness. The atomic write
ensures the file is never corrupted (partially written), only potentially
stale.

If this becomes a problem (e.g., detached queries become common and usage data
diverges noticeably), a future improvement could use advisory file locking
(`flock` on POSIX) with a read-modify-write cycle. This RFD does not implement
locking.

### Privacy

The usage file is **local-only**. It is never transmitted to any external
service. It exists solely to improve the local JP experience. This must be
documented clearly in user-facing docs and the `jp init` output.

The file contains no conversation content, prompts, or LLM responses — only
structural metadata about CLI invocations (command names, argument IDs,
argument values like model names or config field paths).

## Drawbacks

- **Write on every invocation**: Every command run writes to `usage.json`. This
  is a small I/O cost (~1-2ms for a JSON write + fsync + rename) but it's
  unavoidable overhead even when no feature consumes the data yet.

- **Stale data under concurrency**: Without locking, concurrent writes cause
  last-writer-wins races. Some usage increments will be silently lost. This is
  acceptable for heuristic data but means the counts are approximate, not
  exact.

- **Schema evolution**: The JSON schema will grow as more data is tracked. Old
  `usage.json` files need to be forward-compatible (ignore unknown fields) and
  the code must handle missing fields gracefully (default to zero/empty).

## Alternatives

### SQLite instead of JSON

SQLite provides atomic writes, concurrent access, and efficient queries out of
the box. However, it adds a non-trivial dependency, is harder to inspect
manually, and is overkill for what is essentially a small counters file. JSON
is human-readable, debuggable, and sufficient for the expected data size.

### No persistent tracking — session-only

Track usage only within a single session (in-memory). This avoids all file I/O
concerns but provides no cross-session signal. Features like wizard field
ordering need historical data to be useful.

### Append-only log instead of mutable JSON

Write one line per invocation to a log file, aggregate on read. This avoids
concurrent-write races (appends are atomic on POSIX for small writes) but
makes reads expensive (must scan the full log) and requires periodic
compaction. Not worth the complexity for counter data.

## Non-Goals

- **Analytics or telemetry**: This is not a telemetry system. No data leaves
  the user's machine. There is no aggregation server, no opt-in/opt-out
  toggle, no anonymization pipeline.

- **Query performance tracking**: Tracking query durations, token counts, or
  error rates is a natural extension of this infrastructure but is out of
  scope for this RFD. The schema is designed to accommodate it (additional
  top-level keys alongside `cli`), but the recording logic is not
  implemented here.

- **Cross-workspace aggregation**: Each workspace has its own `usage.json`.
  There is no mechanism to merge or query across workspaces. If a user wants
  global stats, that's a future feature.

## Risks and Open Questions

- **Schema versioning**: The `usage.json` file has no version field in this
  initial design. If the schema changes in a breaking way, we'd need to
  detect the old format and migrate or discard. Adding a `"version": 1` field
  now is cheap insurance — worth considering before this RFD is accepted.

- **Large value cardinality**: Flags like `--cfg` can have unbounded value
  diversity (every unique `KEY=VALUE` string becomes an entry). Over months
  of use, the `values` map could grow large. A future RFD could add eviction
  (e.g., drop entries older than 90 days or with count < 3), but this isn't
  needed initially.

- **`--no-persist` interaction**: When the user passes `--no-persist` (or
  `-!`), workspace persistence is disabled. Should this also suppress usage
  tracking? The argument for: consistency with the "leave no trace" intent.
  The argument against: usage is user-local metadata, not workspace state.
  Recommend: suppress usage writes when `--no-persist` is active, for
  consistency.

- **Field renames orphan usage data**: Clap's derive API assigns argument IDs
  from the Rust field name, not from `long` or `short`. Renaming a field
  (e.g. `model` → `model_id`) changes the clap ID from `model` to `model-id`
  even if the CLI flags (`-m`, `--model`) are unchanged. This orphans the old
  ID's counters and starts fresh under the new ID. This is more likely than a
  CLI-breaking rename since the field name is an internal detail. Mitigations:
  orphaned entries age out via eviction (described above), and the data is
  heuristic — a reset counter is a temporary accuracy loss, not a failure. The
  alternative — manual `record_arg` calls with hardcoded strings — would
  survive a rename only if someone remembers to update the string, and silently
  records under the wrong name if they forget. If stability becomes important,
  explicit `#[arg(id = "model")]` attributes could decouple the ID from the
  field name, but that adds boilerplate to every arg definition and isn't worth
  it initially.

## Implementation Plan

### Phase 1: Core types and storage

1. Define `CliUsage`, `CommandUsage`, `ArgUsage`, and `ValueUsage` types with
   serde derives.
2. Implement `CliUsage::load()` with graceful fallback on missing/corrupt
   files.
3. Implement `CliUsage::save()` with atomic temp-file-then-rename writes.
4. Implement `CliUsage::storage_path()` with workspace-local and global
   fallback logic.
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
3. All commands and flags are tracked automatically — no per-command changes.

Phase 1 can be merged independently. Phase 2 depends on Phase 1. Phase 3
depends on Phase 2 and is a single PR touching only `run_inner()`.

## References

- [RFD D12]: Interactive config (first consumer of usage data)
- `Ctx` in `jp_cli/src/ctx.rs` — the CLI context struct
- `Workspace::user_storage_path()` — per-user, per-workspace storage
- `user_data_dir()` in `jp_workspace` — global user data directory
- `write_json` in `jp_storage/src/value.rs` — current (non-atomic) JSON writer
- `tempfile::NamedTempFile::persist` — atomic rename on POSIX/Windows

[RFD D12]: D12-interactive-config.md
[RFD 060]: 060-config-explain.md
