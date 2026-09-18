# Harness-design findings from arXiv 2609.20804, measured against JP

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-09-18
- **Label**: domain=conversation
- **Label**: domain=llm
- **Label**: domain=tooling
- **Label**: type=task

"An Empirical Study of Harness Design for Coding Agents"
(<https://arxiv.org/abs/2609.20804>, Fan et al., September 2026) ablates three
coding-harness components while holding the execution loop fixed: planning,
action space, and context management. 176 matched settings, four models
(Nemotron-3 30B/120B/550B, Mistral-Medium-3.5-128B), two benchmarks (SWE-Bench
Verified, Terminal-Bench 2.1), four context-window budgets from 32k to 128k.

This ticket records what the paper establishes, how JP's harness compares, and
where the comparison says JP should spend effort.
The proposals it produced are filed separately and listed at the bottom.

## What the paper establishes

1. **Context management's value scales inversely with the window budget, and
   almost all of it comes from preventing overflow termination.** Every managed
   tier overflowed on zero tasks at every budget.
   The unmanaged tier overflowed on 78.7% of SWE-Bench tasks at 32k and 8.7% at
   128k.
   Accuracy differences between managed tiers are small; the difference between
   managed and unmanaged is not.
2. **Staging cheap elision before expensive summarization wins on cost at equal
   accuracy.** Their T4 (elide at a soft threshold, summarize at a hard one) had
   the lowest mean cost in seven of eight model/benchmark panels and the lowest
   peak-context ratio at all four budgets.
3. **Model-facing recall is dead machinery.** 56% of the settings exposing a
   `recall_event` tool never called it; the mean falls to 0.007 calls per task
   at 128k; all 16 ablation settings recorded zero.
   No accuracy gain over elision alone.
4. **Planning and the action space are model-conditional.** Planning is an
   accuracy scaffold for weak models (+11.6 points for the 30B) and a cost saver
   for strong ones (roughly 30% cheaper, about 2 points less accurate).
   Predefined tools scaffold bash-weak models; bash-only is cheaper and more
   accurate for bash-capable ones.

### What it does not establish

- The action-space intervention is bundled: tool availability, interface
  prompts, file-state tracking, and post-edit diagnostics all vary together.
  The paper says so in its own limitations.
  It is not evidence that read-before-write or post-edit diagnostics help.
- One run per setting, and Terminal-Bench has 89 tasks, so most Terminal-Bench
  contrasts are not significant.
  Direction is usable; magnitude is not.
- Every crossover point sits at the weak end of its capability axis.
  JP's users run frontier models, where the paper's own data shows planning and
  predefined tools helping least.

## How JP compares

### Context management

| Paper                                                                      | JP                                                                             | State                                             |
| -------------------------------------------------------------------------- | ------------------------------------------------------------------------------ | ------------------------------------------------- |
| T0 (no management, overflow terminates)                                    | the `jp query` path                                                            | shipped behavior                                  |
| M1 elision (stub bulky stale tool observations)                            | `ToolCallPolicy::Strip { request, response }` with a `PolicySpec` `over` bound | exists, manual only                               |
| M2 recall (`recall_event` tool)                                            | absent; `--reset` plus always-preserved raw events                             | correct by design                                 |
| M3 summarization (running summary)                                         | `SummaryPolicy`                                                                | exists, manual only                               |
| Soft / hard thresholds (0.6 / 0.85), verbatim recent window (0.3, floor 2) | none                                                                           | RFD D50 proposes a single `trigger_ratio`         |
| Per-result cap (24k chars)                                                 | none                                                                           | RFD D24                                           |
| Truncation fallback                                                        | `window::truncate_to_fit`                                                      | exists; two call sites, neither on the query path |

JP has every mechanism the paper tested.
It has no trigger.

### Action space and safety

| Paper                                                      | JP                                                                                           | Verdict     |
| ---------------------------------------------------------- | -------------------------------------------------------------------------------------------- | ----------- |
| Compile-time choice: predefined set or bash-only           | every tool is a TOML declaration under `conversation.tools`; one built-in (`describe_tools`) | JP stronger |
| Full description in the tool schema, always                | `summary` in the schema, full description and examples via `describe_tools`                  | JP stronger |
| Permission: allow / ask / deny                             | `RunMode::{Ask, Unattended, Edit, Skip}` plus access grants (RFD 076)                        | JP stronger |
| Workspace guard, pre- and post-symlink                     | `jp_tool::AccessPolicy`, pre- and post-canonical                                             | comparable  |
| Read-before-write with a content hash                      | none                                                                                         | absent      |
| Post-edit diagnostics                                      | none                                                                                         | absent      |
| Stuck detection (5 identical warn, 8 identical fail abort) | none                                                                                         | absent      |
| Step budget (300 per task)                                 | none; `TurnState::request_count` is incremented and never read                               | absent      |
| Parallel read-only tools, capped at 8                      | all tools, in parallel, uncapped                                                             | different   |
| Cost and token accounting                                  | provider usage parsed into wire types and discarded                                          | absent      |
| Trajectories reconstructed from logs by an LLM judge       | durable event stream, stable event IDs (RFD 097), turn markers                               | JP stronger |

## SWOT

### Strengths

- **The action space is a config axis, not a code axis.** The paper's
  action-space finding is that the right tool set depends on the model, and
  their harness bakes the choice in at compile time.
  JP expresses either condition as config, per conversation and per persona.
  What the paper reports as a finding, JP already treats as a knob.
- **Recall is a human operation, not a tool.** RFD 064 made raw events
  recoverable by the user and never by the model.
  The paper's clearest negative result is that the model-facing version goes
  unused.
  JP put the boundary in the right place.
- **Truncation is prompt-cache-aware.** `truncate_to_fit` rounds its drop to 10%
  of target to keep the prefix stable across calls.
  The paper does not model caching at all, which makes its cost figures
  optimistic in shape for any harness that thrashes the cache.
- **The trajectory is a first-class durable artifact.** The paper spent an
  appendix and an LLM judge reconstructing what JP records natively.
- **Human-in-the-loop is a designed axis.** Inquiries (RFD 005, RFD 028), the
  interrupt ladder (RFD 045, RFD 092), the permission model.
  The paper's harness has one escape hatch: kill the run.

### Weaknesses

- **No context management on the query path.** The failure the paper measures
  most directly, and T-0de0hry is in-repo evidence it already bites.
- **Unbounded tool output.** RFD D24 records a 1,293,623-token request against a
  1,000,000 limit, durably persisted.
- **No loop bound and no repetition detection.** The turn loop cycles streaming
  to executing with no cap.
  The guards are a per-response byte ceiling, an idle timeout, and Ctrl-C.
- **No usage accounting.** Every proposal below is unfalsifiable without it.
  The paper's contribution is that it measured; JP cannot.
- **The harness knows nothing about what its tools do.** No concept of "a file
  was edited", so no read-before-write, no post-edit diagnostics, no per-turn
  touched-file set.
  This is the price of the declarative action space, and it is a real price.

### Opportunities

- Adopt the paper's tuned constants instead of deriving them: 0.6 and 0.85
  thresholds, a 0.3 recent-window budget floored at two turns, 5 and 8 streak
  thresholds.
- The recall result retires a question and saves the work.
- Per-model action-space presets are config in JP and impossible in the paper's
  harness.

### Threats

- **Model mismatch.** Every crossover point in the paper sits at the weak end of
  its capability axis.
  In its own strong-model cells, planning reduces accuracy slightly and
  predefined tools add cost.
- **Autonomy mismatch.** The paper optimizes for finishing without a human.
  Importing its shapes wholesale imports that assumption.
  Stuck detection that kills a run is wrong for JP; stuck detection that asks
  the user is right.

## What not to build

- **A model-facing recall tool.** The evidence against it is strong and JP's
  boundary is better.
  Worth checking RFD D39 is not recreating it under another name.
- **A benchmark suite.** RFD D65 is a test harness, which is a different thing
  and worth having.
  A SWE-Bench-style evaluation is not: effects are conditional on model
  capability, and JP's users change models faster than a suite stays calibrated.
  Instrument real sessions instead.
- **A hard-coded file tool set** to obtain read-before-write and post-edit
  diagnostics.
  The paper does not establish that those help, and hard-coding them collapses
  JP's best structural property.
  If the gap ever bites, the orthogonal move is a declarative effect annotation
  on tool configs, not a built-in tool set.

## Proposals

Ordered by leverage per unit of cost.
The first two attack the failure the paper measures most directly; the third is
what makes any of them checkable.

1. **T-0n0pcr0**: promote D24 and rank bounded tool output.
   The cheapest change, and it sits upstream of the rest: a single oversized
   response poisons a conversation permanently, and no compaction trigger
   rescues one that has already happened.
2. **T-0n0pjkk**: stage elision before summarization in D50's automatic
   compaction.
   JP has both policies already; this is about which fires when, and about
   borrowing the paper's thresholds rather than guessing new ones.
3. **T-0n0pgw6**: record per-response token usage as a conversation event.
   Build alongside 2, not after.
   Without it, tuning a threshold is guesswork and no claim about the harness
   can be checked.
4. **T-0n0pmyh**: bound a turn's tool-call cycles and notice repeated identical
   calls.
   Small, and the vestigial `TurnState::request_count` names the missing check.
   JP should route a streak to the user rather than nag the model.
5. **T-0n0ppbn**: decide whether a plan belongs in conversation history.
   Weakest-supported of the set, and explicitly gated on 3.
   Filed to keep the question findable.

One gap fell out of 2 and is filed on its own, because it holds independently of
whether automatic compaction ever lands:

- **T-0n0pex2**: the query path never fits a conversation to the model's context
  window.
  `truncate_to_fit` has two call sites and neither is `query`.
  D50 assumes this backstop exists.
