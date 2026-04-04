# RFD 063: Usage-Based Wizard Field Ordering

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-21

## Summary

Extend the interactive config wizard ([RFD 061]) with a frecency-based tier in
the field selector ordering, powered by the CLI usage tracking infrastructure
from [RFD 062]. Fields the user frequently sets via `--cfg` or dedicated CLI
flags are promoted in the list, making the wizard increasingly personalized over
time.

## Motivation

[RFD 061] ships the interactive config wizard with a two-tier field ordering:
configured (non-default) fields first, then all remaining fields in natural
order. This is functional but static — a user who sets `assistant.tool_choice`
on every other invocation sees it in the same position as a field they've never
touched.

With [RFD 062]'s usage tracking in place, the wizard has access to per-flag
usage data: which `--cfg` fields and dedicated flags are used, how often, and
when. This RFD adds a middle tier — "frecent" (frequently + recently used) —
that promotes fields based on this data.

## Design

### Field ordering (updated)

The wizard's field selector orders fields in three tiers:

1. **Already configured** (non-default) fields — marked with a visual
   indicator (e.g., `●` prefix).
2. **Frecent** fields — fields the user has set in previous
   invocations but that are currently at their default value. Marked with a
   distinct indicator (e.g., `◦` prefix or dimmed text).
3. **All remaining** fields in their natural order (as returned by
   `AppConfig::fields()`).

Fields configured during the current wizard session are also marked, distinct
from fields configured by other layers.

### Data sources

The ranking signal comes from two places in the `CliUsage` data ([RFD 062]):

1. **`--cfg` argument values**: The `values` map under
   `cli.commands.query.args.config` contains entries like
   `assistant.tool_choice=auto` (raw `KEY=VALUE` strings). The clap argument ID
   is `config` (the Rust field name in `Globals`), though users know it as
   `--cfg` / `-c`. The wizard groups these by field path on the read side —
   parsing each raw value through `KvAssignment::from_str` to extract the key
   — and aggregates their counts.

   For example, if `assistant.tool_choice=auto` has count 5 and
   `assistant.tool_choice=required` has count 2, the field
   `assistant.tool_choice` has an aggregate count of 7.

2. **Dedicated CLI arguments**: Arguments like `model` and `reasoning` map to
   known config field paths. The reverse mapping comes from the `CliRecord`
   infrastructure in [RFD 060] (each `CliRecord` has a `field` and `arg_id`
   pair). The wizard uses this to translate `model` usage into
   `assistant.model.id` ranking signal, and `reasoning` usage into
   `assistant.model.parameters.reasoning`.

Both sources are merged into a single ranking score per field path.

### Ranking heuristic

Fields are ranked by a combined score of frequency and recency:

```rust
fn usage_score(count: u64, last_used: DateTime<Utc>, now: DateTime<Utc>) -> f64 {
    let days_ago = (now - last_used).num_days().max(0) as f64;
    let recency = 1.0 / (1.0 + days_ago / 7.0);
    let frequency = (count as f64).ln_1p();
    frequency * recency
}
```

This gives:

- A field used 30 times yesterday a higher score than one used 30 times a month
  ago.
- A field used 5 times yesterday a higher score than one used once yesterday.
- Logarithmic frequency scaling so that a field used 100 times isn't
  dramatically more prominent than one used 20 times.

The exact formula can be tuned based on real-world feedback. The important
property is that both frequency and recency contribute, and neither dominates
completely.

Fields with a score of zero (never used) are not included in the frecent tier —
they remain in the natural-order tier.

### Signature change

The `interactive_config_browser` function gains a `CliUsage` parameter:

```rust
fn interactive_config_browser(
    current: &PartialAppConfig,
    schema: &Schema,
    usage: &CliUsage,
) -> Result<Vec<KvAssignment>>;
```

The caller (in `run_inner()`) passes the `CliUsage` loaded from `Ctx`:

```rust
if has_interactive_cfg(&cli.globals.config) {
    let schema = SchemaBuilder::build_root::<AppConfig>();
    let assignments = interactive_config_browser(&partial, &schema, &ctx.usage)?;
    // ...
}
```

### Field-path extraction

A helper function extracts per-field-path usage stats from `CliUsage`:

```rust
fn field_usage_scores(
    usage: &CliUsage,
    command_path: &[&str],
    now: DateTime<Utc>,
) -> HashMap<String, f64> {
    let Some(cmd) = usage.get_command(command_path) else {
        return HashMap::new();
    };

    let mut scores: HashMap<String, f64> = HashMap::new();

    // `--cfg` values: parse with KvAssignment to extract field paths.
    // The clap arg ID for `--cfg` is `config` (the Rust field name).
    // We use KvAssignment::from_str rather than splitting on '='
    // because the assignment syntax supports =, :=, +=, :+= etc.
    if let Some(cfg_arg) = cmd.args.get("config") {
        for (raw_value, stats) in &cfg_arg.values {
            if let Ok(kv) = KvAssignment::from_str(raw_value) {
                let score = usage_score(stats.count, stats.last_used, now);
                *scores.entry(kv.key_string()).or_default() += score;
            }
        }
    }

    // Dedicated args: reverse-map arg ID to config field path.
    for (arg_id, arg_stats) in &cmd.args {
        if arg_id == "config" {
            continue; // already handled
        }

        if let Some(field_path) = reverse_map_arg(command_path, arg_id) {
            let score = usage_score(arg_stats.count, arg_stats.last_used, now);
            *scores.entry(field_path.to_owned()).or_default() += score;
        }
    }

    scores
}
```

The `reverse_map_arg` function uses the `CliRecord` registry from [RFD 060] to
look up the config field path for a given clap argument ID and command.

### Visual indicators

The field selector uses distinct markers for each tier:

| Tier                       | Indicator | Example                     |
|----------------------------|-----------|-----------------------------|
| Configured (non-default)   | `●`       | `● assistant.model.id`      |
| Wizard-edited this session | `◆`       | `◆ style.reasoning.display` |
| Frecent                    | `◦`       | `◦ assistant.tool_choice`   |
| Remaining                  | (none)    | `  assistant.name`          |

## Drawbacks

- **Cold start**: A new workspace has no usage data. The wizard falls back to
  the two-tier ordering from [RFD 061] until enough invocations accumulate. This
  is by design — the wizard is useful without usage data, just not personalized.

- **Stale ranking**: If the user's workflow changes (e.g., switches models), the
  old model's flag still has high counts. The recency decay in the scoring
  formula mitigates this — unused fields fade over ~2-4 weeks — but don't
  disappear entirely. A future "reset usage" command could help.

## Alternatives

### Manual pinning instead of automatic ranking

Let users explicitly pin fields to the top of the wizard list. This gives full
control but requires upfront configuration. Automatic ranking adapts without
user effort.

These approaches aren't mutually exclusive — pinning could be layered on top of
automatic ranking in a future RFD.

### Workspace-global ranking (not per-command)

Aggregate usage across all commands instead of per-command. Simpler, but less
accurate: `--model` usage on `query` doesn't mean `assistant.model.id` should be
prominent when configuring `conversation ls`.

## Non-Goals

- **Usage data recording**: This RFD consumes usage data; [RFD 062] handles
  recording it. The boundary is: RFD 062 writes, this RFD reads.

- **Usage data UI**: Displaying usage stats to the user (e.g., `jp usage show`)
  is out of scope. This RFD only uses usage data to improve wizard field
  ordering.

## Risks and Open Questions

- **Ranking tuning**: The scoring formula (logarithmic frequency × inverse
  recency) is a reasonable starting point but hasn't been validated with real
  usage data. The formula is easy to adjust without changing the architecture.

- **Reverse mapping completeness**: The `CliRecord` registry from [RFD 060] may
  not cover all arguments initially. Missing mappings mean those arguments don't
  contribute to field ranking — the wizard still works, just with less signal.
  This degrades gracefully.

- **Field renames**: Since [RFD 062] keys usage data by clap argument ID
  (derived from the Rust field name), renaming a field orphans its usage
  counters (see RFD 062's risk discussion). For this RFD, the consequence is
  that a renamed argument temporarily loses its ranking boost until enough new
  usage accumulates. This is a minor UX regression, not a failure.

## Implementation Plan

### Phase 1: Field-path extraction and scoring

1. Implement `field_usage_scores()` that reads `CliUsage` and produces a
   `HashMap<String, f64>` of field path → score.
2. Implement `--cfg` value grouping by field path via `KvAssignment` parsing.
3. Implement `reverse_map_arg()` using `CliRecord` from [RFD 060].
4. Implement `usage_score()` with the frequency × recency formula.
5. Unit tests with synthetic usage data.

Depends on: [RFD 062] Phase 1 (core types).

### Phase 2: Integration into wizard

1. Add `usage: &CliUsage` parameter to `interactive_config_browser()`.
2. Insert frecent tier into field ordering logic.
3. Add `◦` visual marker for the new tier.
4. Pass `ctx.usage` from `run_inner()` into the wizard.

Depends on: Phase 1, [RFD 061] Phase 1 (core wizard loop).

Both phases can be merged as a single PR since the feature is small and
self-contained.

## References

- [RFD 061]: Interactive config (the wizard this extends)
- [RFD 060]: Config explain (`CliRecord` reverse mapping)
- [RFD 062]: CLI usage tracking (provides the data)
- `KvAssignment::from_str` in `jp_config/src/assignment.rs` — `KEY=VALUE`
  parsing used to extract field paths from `--cfg` values
- `Globals.config` in `jp_cli/src/lib.rs` — the `--cfg` flag (clap arg ID:
  `config`)

[RFD 060]: 060-config-explain.md
[RFD 062]: 062-cli-usage-tracking.md
[RFD 061]: 061-interactive-config.md
