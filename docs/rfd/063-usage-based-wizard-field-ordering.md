# RFD 063: Usage-Based Wizard Field Ordering

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-21
- **Requires**: [RFD 061], [RFD 062], [RFD 060]
- **Summary**: Orders fields in the interactive `--cfg` wizard by frecency using
  per-workspace CLI usage data.

## Summary

This RFD adds a frecency-ranked tier to the interactive `--cfg` wizard ([RFD
061]) using the local CLI usage data from [RFD 062].
Fields that the user frequently or recently sets via `--cfg` or dedicated CLI
flags appear directly below already-configured fields.

## Motivation

[RFD 061] orders fields in the interactive config wizard in two tiers:
configured (non-default) fields first, followed by all remaining fields in
natural order.
That ordering is static: a user who sets `assistant.tool_choice` on every other
invocation sees it in the same position as a field they have never touched.

[RFD 062] tracks which `--cfg` fields and dedicated flags the user passes, how
often, and when.
This RFD uses that data to add a middle "frecent" (frequently and recently used)
tier that promotes commonly used fields above the remaining defaults.

## Design

### Field ordering

The wizard's field selector orders fields in three tiers:

1. **Already configured** (non-default) fields, marked with a visual indicator
   (e.g., `●` prefix).
2. **Frecent** fields: fields set in previous invocations that are currently at
   their default value, marked with a distinct indicator (e.g., `◦` prefix or
   dimmed text).
3. **All remaining** fields in the natural order returned by
   `AppConfig::fields()`.

Fields configured during the current wizard session carry their own marker,
distinct from fields configured by other layers.

### Data sources

The ranking signal comes from two sources in `CliUsage` ([RFD 062]):

1. **`--cfg` argument values**: The `values` map under
   `cli.commands.query.args.config` stores raw `KEY=VALUE` strings such as
   `assistant.tool_choice=auto` (keyed under the clap argument ID `config`, the
   Rust field name in `Globals` for `--cfg` / `-c`).
   When reading usage data, the wizard parses each entry with
   `KvAssignment::from_str` to extract its field path and aggregates the counts
   by field.
   For example, if `assistant.tool_choice=auto` has count 5 and
   `assistant.tool_choice=required` has count 2, `assistant.tool_choice` has an
   aggregate count of 7.

2. **Dedicated CLI arguments**: Each `CliRecord` ([RFD 060]) pairs a config
   `field` path with a clap `arg_id`.
   The wizard uses these records to map dedicated arguments to config fields,
   attributing `model` usage to `assistant.model.id` and `reasoning` usage to
   `assistant.model.parameters.reasoning`.

Both sources are combined into a single ranking score per field path.

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

Under this formula:

- A field used 30 times yesterday scores higher than one used 30 times a month
  ago.
- A field used 5 times yesterday scores higher than one used once yesterday.
- Logarithmic frequency scaling prevents a field used 100 times from dominating
  one used 20 times.

The exact weights can be tuned with usage feedback, as long as both frequency
and recency contribute without either dominating.
Unused fields (score of zero) remain in the natural-order tier.

### Browser signature

`interactive_config_browser` takes a `CliUsage` reference:

```rust
fn interactive_config_browser(
    current: &PartialAppConfig,
    schema: &Schema,
    usage: &CliUsage,
) -> Result<Vec<KvAssignment>>;
```

`run_inner()` passes the `CliUsage` loaded on `Ctx`:

```rust
if has_interactive_cfg(&cli.globals.config) {
    let schema = SchemaBuilder::build_root::<AppConfig>();
    let assignments = interactive_config_browser(&partial, &schema, &ctx.usage)?;
    // ...
}
```

### Field-path extraction

`field_usage_scores` computes the score for each config field path from
`CliUsage`:

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

`reverse_map_arg` looks up the config field path for a command and clap argument
ID in the `CliRecord` registry ([RFD 060]).

### Visual indicators

The field selector uses a distinct prefix for each tier:

| Tier                       | Indicator | Example                     |
| -------------------------- | --------- | --------------------------- |
| Configured (non-default)   | `●`       | `● assistant.model.id`      |
| Wizard-edited this session | `◆`       | `◆ style.reasoning.display` |
| Frecent                    | `◦`       | `◦ assistant.tool_choice`   |
| Remaining                  | (none)    | ` assistant.name`           |

## Drawbacks

- **Cold start**: A new workspace has no usage data, so the wizard falls back to
  the two-tier ordering from [RFD 061] until invocations accumulate.

- **Stale ranking**: When a user's workflow changes (for example, after
  switching models), the old model's flag retains high counts.
  Recency decay fades unused fields over two to four weeks, though their scores
  never reach zero.
  A future "reset usage" command could clear stale history.

## Alternatives

### Manual field pinning

Users could explicitly pin fields to the top of the wizard list.
Manual pinning gives full control, but requires upfront configuration, whereas
automatic ranking adapts without user effort.
The two approaches are compatible, and manual pinning could be layered on top of
automatic ranking in a future RFD.

### Workspace-global ranking

Usage could be aggregated across all commands rather than tracked per command.
Global aggregation is simpler, but less accurate: `--model` usage on `query`
does not mean `assistant.model.id` belongs near the top when configuring
`conversation ls`.

## Non-Goals

- **Usage data recording**: [RFD 062] records CLI usage data; this RFD only
  reads it.

- **Usage data UI**: Displaying usage statistics to the user (e.g., `jp usage
  show`) is out of scope.

## Risks and Open Questions

- **Ranking tuning**: The scoring formula (logarithmic frequency × inverse
  recency) has not yet been validated against real usage data, though its
  weights can be adjusted without architectural changes.

- **Reverse mapping completeness**: The `CliRecord` registry from [RFD 060] may
  not cover every argument initially.
  Unmapped arguments simply do not contribute to field ranking, so the wizard
  degrades gracefully with less signal.

- **Field renames**: [RFD 062] keys usage data by clap argument ID, which is
  derived from the Rust field name.
  Renaming a field orphans its usage counters, so the argument loses its ranking
  boost until new usage accumulates.

## Implementation Plan

### Phase 1: Field-path extraction and scoring

1. Implement `field_usage_scores()` to read `CliUsage` and return a
   `HashMap<String, f64>` mapping each field path to its score.
2. Group `--cfg` values by field path via `KvAssignment` parsing.
3. Implement `reverse_map_arg()` using `CliRecord` from [RFD 060].
4. Implement `usage_score()` with the frequency × recency formula.
5. Add unit tests with synthetic usage data.

Depends on: [RFD 062] Phase 1 (core types).

### Phase 2: Wizard integration

1. Add the `usage: &CliUsage` parameter to `interactive_config_browser()`.
2. Insert the frecent tier into the field ordering logic.
3. Add the `◦` visual marker for the frecent tier.
4. Pass `ctx.usage` from `run_inner()` into the wizard.

Depends on: Phase 1, [RFD 061] Phase 1 (core wizard loop).

Both phases can merge in a single pull request.

## References

- [RFD 061]: Interactive config (the wizard this extends)
- [RFD 060]: Config explain (`CliRecord` reverse mapping)
- [RFD 062]: CLI usage tracking (provides the data)
- `KvAssignment::from_str` in `jp_config/src/assignment.rs`: `KEY=VALUE` parsing
  used to extract field paths from `--cfg` values
- `Globals.config` in `jp_cli/src/lib.rs`: the `--cfg` flag (clap arg ID:
  `config`)

[RFD 060]: 060-config-explain.md
[RFD 061]: 061-interactive-config.md
[RFD 062]: 062-cli-usage-tracking.md
