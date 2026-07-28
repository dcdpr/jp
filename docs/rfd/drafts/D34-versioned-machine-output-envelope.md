# RFD D34: Versioned Machine Output Envelope

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-28
- **Extends**: [RFD 048]

## Summary

Every JSON machine record JP emits is wrapped in a three-key envelope —
`{"format", "version", "data"}` — that names the record and its version.
The line-oriented ID formats have no in-band place for a version and are the
stated exception: they keep their current bytes and are versioned out-of-band.
All existing shapes start at version `0`, which explicitly promises nothing and
exists so the envelope can ship before any shape is stabilized.
A `--format-version` flag lets consumers pin what they parse, validated before
the command runs, with reserved exit codes `20` and `21` for "too old" and "too
new."

## Motivation

`jp -F json` today is not one format.
It is seven unrelated shapes, three of which are byproducts of human rendering:

| Producer                      | Shape                         | Where the shape comes from                                                                     |
| ----------------------------- | ----------------------------- | ---------------------------------------------------------------------------------------------- |
| `jp c ls`                     | array of objects              | rendered table headers — keys are `ID`, `#`, `Activity ▼`; columns appear and vanish with data |
| `jp c show`, `attachment ls`  | object                        | rendered detail labels, ANSI-stripped                                                          |
| `jp c grep`                   | array of objects              | hand-built `json!` in `render_json`                                                            |
| `jp c grep -l`, `new`, `fork` | array of ID strings           | deliberate, per [RFD 050]                                                                  |
| `jp query`                    | NDJSON of `ConversationEvent` | direct serialization of the persisted event type                                               |
| errors                        | `{message, metadata, code}`   | `parse_error`, on stderr                                                                       |
| `Printer::print` / `println`  | `{"message":"…"}` per call    | `Printer::wrap_json`, on stdout                                                                |

`list_json` keys each object by `strip_ansi_escapes::strip_str(header_cell)`.
Renaming a column or adding a sort arrow silently changes the JSON, and columns
appear and vanish with the data (`expires_at` and `local` are emitted only when
some row has a value).
The `(3 hidden)` row becomes an object.
Hyrum's Law already applies: anyone parsing `jp c ls -F json` depends on all of
it.

The last row is the widest hole.
`Printer::print` and `Printer::println` wrap their content as `{"message":"…"}`
and write it to **stdout**, so any command that prints through either emits
unannounced JSON on the data channel.
That includes real data: `conversation path` prints filesystem paths this way,
`config show` prints TOML this way, and `attachment ls` prints a message on the
empty branch but a details object on the non-empty branch.

[RFD 086] builds `jp c grep -l … | jp c archive -` pipelines on this output,
and those pipelines are the intended workflow, not an accident.
Doing nothing means every future change to a table column is an unannounced
break in a documented pipeline, detectable only by the consumer's parser
producing wrong answers.

The insurance has two halves, and shipping one without the other is close to
worthless:

- **Announcement** — the output says what it is.
  A consumer can detect that it received something it does not understand.
- **Negotiation** — the consumer says what it wants.
  JP honors it or refuses loudly, before doing any work.

## Design

### What is and isn't a machine record

Everything in this RFD hangs off one boundary, so it is drawn once, here.

A **machine record** is a structured command result on the stdout data channel,
plus the structured post-parse `error` record on stderr — the data a command
was asked to produce, or the failure explaining why it could not.
Within that scope, records are classified by what the producer intends, not by
whether the bytes happen to parse as JSON and not by which `OutputFormat` is
active — `print_json` already emits JSON under `Text` and `TextPretty` to serve
the no-flag pipelines in [RFD 029], so "JSON output" cannot mean
`OutputFormat::Json`.

JP emits other machine-readable output that is deliberately outside this scope.
That is an ownership boundary, not a claim that the output is unparseable.

Every machine record has a registry address.
Every **JSON** machine record also carries that address in band, inside an
envelope:

- every JSON machine record on **stdout**, in every output format;
- the `error` record on **stderr**, under JSON output modes.

The line-oriented encodings are the exception: they have a registry address but
no in-band announcement, and are covered out-of-band by `--format-version`
alone.

These are **not machine-readable output at all**, and carry no envelope:

- **Chrome.** Progress, tool headers, status, and countdowns are human-facing
  output on stderr.
  How they are serialized per output mode is [RFD 048]'s concern and this RFD
  does not change it.
- **Human renderings.** The pretty and plain tables `-F text-pretty` and `-F
  text` produce are presentation, not data.
  They cannot be pinned.
- **Argument-parsing diagnostics.** `Cli::try_parse` failures exit through
  `e.exit()` before the output format is resolved, so clap's usage errors,
  `--help`, and `--version` never reach the envelope.

These **are** machine-readable, but belong to another contract with its own
owner and version namespace:

- **Observability output.** Tracing events — written to the log file, or to
  stderr via `--log-file=-` — and the announcement of where the log was
  written.
  JP does not today commit to the trace event shape; if [RFD D15] and [RFD D32]
  give it typed events, whether those join this registry is their call, not this
  RFD's.
  The log-path announcement belongs with them too: `run()` currently writes
  `{"trace_log": "<path>"}` to stderr and in-repo tooling parses it — genuinely
  a record for a parser — but [RFD D15] already folds that path into a richer
  post-run log report.
  Registering it here would freeze one shape while another RFD replaces it.
- **Plugin output.** Text a plugin sends via `PrintMessage` is relayed to stdout
  by JP verbatim, and may itself be JSON.
  JP owns the channel but not the meaning; giving it a format address requires a
  protocol extension (see [Plugins](#plugins)).

Errors therefore fall into three tiers, and only the third is a machine record:

| Failure                                        | Surface                       |
| ---------------------------------------------- | ----------------------------- |
| Argument-parsing errors, `--help`, `--version` | clap diagnostics, unversioned |
| Post-parse failure under a text output mode    | human error rendering         |
| Post-parse failure under a JSON output mode    | `error` record, enveloped     |

Preflight version rejections bypass clap and use the normal post-parse error
renderer for the resolved output mode: an enveloped `error` record under `json`
and `json-pretty`, human error text under the text modes.
Either way the run exits `20` or `21` before the workspace is loaded and writes
nothing to stdout, which is what the line-oriented consumers' negotiation
contract actually rests on.

No JSON record from a built-in command reaches stdout unannounced.
Output that is a status line rather than data gets the `message` address at
version `0` — which is exactly what `Printer::wrap_json` already produces, now
labelled.
`message` is a migration artifact, not a design: it exists because
`Printer::print` and `println` will wrap *any* string as JSON, which is how
`conversation path` and `config show` currently emit real data.
It is deleted, not stabilized, once every command declares a real address — see
[Negotiation happens before the command
runs](#negotiation-happens-before-the-command-runs), which is the mechanism that
makes escaping it impossible.

### The envelope

```json
{
  "format": "conversation.summary_list",
  "version": 0,
  "data": [ … ]
}
```

Three required keys.
The envelope is repeated on every line of an NDJSON stream, so its size is a
recurring cost:

```jsonl
{"format":"conversation.event","version":0,"data":{"type":"chat_request","content":"…"}}
{"format":"conversation.event","version":0,"data":{"type":"chat_response","message":"…"}}
```

Payloads that are arrays or bare values — `jp c grep`, `jp c grep -l`, `jp
query --schema` — gain a place to carry their version for the first time.

The line-oriented ID format that [RFD 086] consumes has no in-band place for a
version and keeps its current bytes; it is versioned out-of-band by
`--format-version` alone.
This is a second reason the flag is not optional: it is the only versioning that
reaches the text machine formats.

### Format addresses

`format` is a dot-delimited name identifying a **semantic record contract**,
independent of the channel that carries it.
It is the address a version number attaches to.

A contract may have more than one encoding.
`conversation.id_list` is a JSON array under the JSON formats and one ID per
line under the text formats; both are the same contract at the same version.

The version governs two things: the record's semantic contract — its meaning,
field set, and documented guarantees — and, for each encoding, that encoding's
**parse-relevant grammar**: delimiters, quoting, ordering, and how many records
a line carries.
Presentation-only differences are not versioned, which is why compact and pretty
JSON share a number: every conforming JSON parser accepts both.
A line-oriented encoding has no such parser floor, so its grammar *is* contract
— quoting each ID or switching from newlines to commas breaks consumers without
changing a single semantic field, and is a version bump.

If two encodings drift far enough apart to stop representing the same semantic
record, the answer is a new address, not a version bump.

The name is `format`, not `type`, deliberately.
`EventKind` already serializes with `#[serde(tag = "type")]`, so an envelope
keyed `type` would produce `{"type": …, "data": {"type": "chat_request"}}` —
two different `type`s one level apart is a reader trap.

Addresses are owned by the shape, not by the command that emits it.
Several commands may emit the same address, and one command may emit several.
Both already happen: the `error` format comes out of every command in the CLI,
and `jp query -F json` emits an event stream (`JsonEmitter`) alongside the
structured schema result (`print_json`) — one access point, two unrelated
shapes.

Addresses name *semantic* records, not structural coincidences.
`details_json` produces the same outer `{title, details}` structure for
conversation details and attachment listings; those are two addresses, not one.

#### Naming convention

Addresses are `<subject>.<shape>`, lowercase and snake\_case:

- **Subject** is the domain entity the record describes, using the [ubiquitous
  language] term where one exists.
  When the record is not about an entity, the domain area is acceptable —
  `query.result` describes the outcome of a request, not a thing.
- **Shape** names what the record *is*.
  Never the command that produced it (`list`, not `ls`) and never the encoding
  it arrived in (no `.json`, no `.ndjson`).
- **Collections end in `_list`,** and the segment before it names the element.
  Where the element has its own address the two names match —
  `conversation.summary_list` is a list of `conversation.summary` — which is
  how the scheme makes shared payload types visible instead of inviting a second
  summary type for the list.
- **Single-segment addresses** are reserved for cross-cutting records with no
  subject.
  There are two, `error` and `message`, and a third needs justification.
- **JP owns unprefixed addresses.** `plugin.*` is reserved so a future
  plugin-format scheme cannot collide with a JP address.

`_list` is used in preference to an English plural because
`conversation.summary` and `conversation.summaries` differ by two characters in
a string literal that consumers match exactly.

Two segments cover every address in the registry.
Deeper nesting is not forbidden, but the first case that wants it should justify
it at stabilization rather than being anticipated now.

Version `0` addresses may be renamed; version `1` addresses may not.
That makes the convention cheap to apply now and expensive to retrofit later,
which is why the registry below already follows it.

### The version 0 registry

| Address                     | Producers                                    | Encoding                          | Primary            | Negotiable |
| --------------------------- | -------------------------------------------- | --------------------------------- | ------------------ | ---------- |
| `conversation.summary_list` | `jp c ls`                                    | JSON document                     | yes                | yes        |
| `conversation.summary`      | `jp c show`                                  | JSON document                     | yes                | yes        |
| `conversation.match_list`   | `jp c grep`                                  | JSON document                     | yes                | yes        |
| `conversation.id_list`      | `jp c grep -l` (planned: `c new`, `c fork`)  | JSON array or lines               | yes                | yes        |
| `conversation.event`        | `jp query`                                   | JSON record (NDJSON under `json`) | without `--schema` | yes        |
| `query.result`              | `jp query --schema`                          | JSON document                     | with `--schema`    | yes        |
| `attachment.summary_list`   | `jp attachment ls`                           | JSON document                     | yes                | yes        |
| `error`                     | every command, post-parse                    | JSON document (JSON modes)        | n/a                | no         |
| `message`                   | any command via `Printer::print` / `println` | JSON record (NDJSON under `json`) | no                 | no         |

`c new` and `c fork` are listed as *planned* producers.
There is no `conversation new` command today and `fork` prints a status message
rather than an ID; both adopt this address when [RFD 050] lands.

Two ownership notes:

- `query.result` versions JP's **result container**, never the caller-supplied
  JSON Schema inside it.
  JP does not own that payload and makes no promise about it.
- `conversation.event` names the record JP **emits**, which is not the persisted
  stream entry.
  The emitter serializes `ConversationEvent` directly; `InternalEvent` base64-
  encodes content fields at rest and carries `config_delta` and `compaction`
  entries the emitter never produces.

`message` and `error` are not negotiable and have no primary status.
A command whose only output is `message` has no machine format, so
`--format-version` is rejected for it — which is the intended pressure to give
it a real address.
`error` is scoped to post-parse failures under JSON output modes; the other two
error tiers are outside the contract (see [What is and isn't a machine
record](#what-is-and-isnt-a-machine-record)).

The two non-negotiable addresses are non-negotiable for different reasons, and
neither is a category:

- `error` cannot be pinned because pinning the record that reports a failed
  negotiation is circular.
- `message` cannot be pinned because there is no contract to pin — it is a
  migration artifact scheduled for deletion, not a shape.

Both are announced and versioned like everything else, and both are always
emitted at their current version, since there is no pin with which to request an
older one.

### Version 0 means no promises

Every shape starts at version `0`.
Version `0` states, as a contract, that there is no contract: the shape may
change in any release without notice.
It exists so the envelope can ship immediately, before a single shape has been
designed.

**Version `0` waives the `data` contract only.** `version` is the version of
`data` at `format`; the envelope itself carries no version and never will (see
[Extension policy](#extension-policy)).
A consumer that could not rely on the announcement could not perform the
announcement check, which is the entire mechanism.

A shape leaves `0` by being stabilized: given a typed `Serialize` source rather
than being derived from rendered output, reviewed, documented, and promoted to
version `1`.
This turns the work into a visible per-shape backlog rather than a prerequisite
for shipping anything.

When a shape reaches version `1`, its version `0` is **dropped**, not
maintained.
Nothing was promised, so nothing is owed.
`jp c ls --format-version=0` then exits `20`, which is the deprecation mechanism
working as intended and is why version `0` is disposable.

### What pinning promises

`--format-version` is negotiation, not merely a fail-fast check, and the
difference is a retention rule the flag's meaning depends on.

**For negotiable formats:**

- **Version `0` is disposable.** It is dropped the moment its shape reaches `1`.
- **A stabilized version (`1` or higher) stays emitted** after later versions
  ship.
  It is removed only by completing a deprecation process, never as a side effect
  of the next version arriving.

Without the second clause a future version `2` could drop version `1` on the day
it landed, and a pin would buy a clean error instead of continued operation —
which is the weaker of the two things this flag could mean.

**For non-negotiable formats**, retention does not apply and cannot: there is no
pin, so there is no way to request an older version and no point in emitting
one.
They emit exactly their current version, announced in the envelope like anything
else.

That has a consequence worth stating rather than discovering.
Because a consumer cannot pin `error`, a breaking change to it after
stabilization is unmitigable — there is no older version to fall back to.
Two things follow.
`error` is stabilized once, deliberately, and evolves additively from then on
under the extension policy's rules.
And a consumer must never parse the `error` payload to decide *whether* a
command failed: the exit code answers that at every version, forever, and the
payload only answers *why*.

`message` never stabilizes, because it is deleted rather than promoted.

How retained versions are emitted side by side, and what the deprecation process
is, is deferred (see [Non-Goals](#non-goals)).
The retention invariant is not, because it is what `--format-version` means to a
consumer.

### Negotiation happens before the command runs

`run_inner` loads the workspace and sanitizes it — which can repair persisted
state — before dispatching the command, and `jp query` calls the LLM and
persists events long before any emitter writes.
Validating a pin when the first record is written is too late: JP would exit
`20` or `21` having already done the work, possibly with partial stdout.

So each command **declares its addresses statically**: the set it may emit, and
which one is primary.
Immediately after argument parsing, and before the workspace is loaded, a
preflight step resolves that declaration against the invocation and validates
every `--format-version` pin.
All version rejections happen there and nowhere else.

A runtime branch must stay inside the declared set.
An empty result uses the same address as a non-empty one — `attachment ls`
returning zero attachments emits `attachment.summary_list` with an empty
payload, not a `message`.

The declaration is a **pure function of the parsed invocation**, not a
per-command constant:

```text
parsed command + resolved output options -> emitted address set + optional primary
```

Arguments change which contract is primary.
`jp c grep` emits `conversation.match_list`; `jp c grep -l` emits
`conversation.id_list`.
`jp query` is primarily `conversation.event`; `jp query --schema` is primarily
`query.result`, because the result is what the caller asked for and the event
stream is the byproduct — [RFD 029] treats it as noise in scripted use.
The event stream stays in the emitted set and remains pinnable by explicit
address.
Those are two commands with two primaries each, so a constant cannot resolve the
bare pin.
Output mode matters for the same reason: an invocation that produces only a
human rendering emits no machine record and has no primary at all.

The function must stay independent of workspace data and runtime results, which
is what keeps it callable during preflight.
A command emitting two addresses names one primary and requires the explicit
form for the other.

### Pinning a version

```sh
jp c ls -F json --format-version=0                # command-scoped shorthand
jp query --format-version=conversation.event=0    # explicit address
```

The bare form pins the invoked command's primary format and covers the ordinary
case; most users never type an address.
The explicit form exists for commands that emit more than one format.

| Situation                                                   | Behavior                                                         |
| ----------------------------------------------------------- | ---------------------------------------------------------------- |
| Flag omitted                                                | Emit current versions (the default)                              |
| `-F auto` / `text` / `text-pretty` / `json` / `json-pretty` | Identical version semantics                                      |
| `json` vs `json-pretty`                                     | Same version — the number names the shape, not the serialization |
| Line-oriented text machine formats                          | Covered; the version is out-of-band only                         |
| Invocation emits only a human rendering                     | Usage error, exit `2`                                            |
| Requested version is higher than any JP emits               | Exit `21`                                                        |
| Requested version was emitted once, now dropped             | Exit `20`                                                        |
| Address unknown, or not emitted by this invocation          | Usage error, exit `2`                                            |
| Command emits no machine format in any invocation           | Usage error, exit `2`                                            |
| Explicit pins                                               | Repeatable, one per address                                      |
| Bare pin                                                    | At most once; duplicate or conflicting pins rejected at parse    |

The flag never changes the output mode.
It is orthogonal to `-F`: it versions the machine record wherever that record is
emitted, including `print_json` under a text format.
What it cannot do is turn a human rendering into a machine record — `jp c ls
--format-version=0` without `-F json` is a usage error, because the table it
would otherwise print is presentation.
Erroring is deliberate: a pin that silently succeeded against a human table
would tell a script it was protected when it was not.
`jp c grep -l --format-version=0` stays valid in text mode, because the
line-oriented ID list *is* a machine record there.

`json-pretty` records span multiple lines and are therefore not NDJSON, even
though compact `json` is.
The version does not distinguish them.

**The `error` format is announced but not negotiable.** Pinning the format that
reports a failed negotiation is circular.
It carries a `format` and `version` like everything else, and `--format-version`
never applies to it.

### Exit codes

| Code | Meaning                                                      |
| ---- | ------------------------------------------------------------ |
| `20` | The requested format version is too old (no longer emitted). |
| `21` | The requested format version is too new (not yet supported). |

The numbers match notmuch's convention.
Both are reserved by this RFD; only `21` can fire until a second version of any
shape exists.

Reserving them requires narrowing JP's exit-code namespace.
`cmd/plugin/dispatch.rs` forwards a plugin's exit code verbatim via
`cmd::Error::from(exit.code)`, so a plugin exiting `21` is indistinguishable
from a version rejection.
Any nonzero plugin exit is mapped to `1`, with the original recorded in the
error metadata:

```json
{"format":"error","version":0,"data":{"message":"plugin failed","metadata":{"plugin_exit_code":20},"code":1}}
```

This is a deliberate narrowing, not a free win.
The numeric value survives as diagnostic data, but the plugin's **process-status
contract** does not: a script matching `case $? in 10) … 11) …` against a
plugin can no longer recover that value without parsing stderr.
JP takes full ownership of its exit-code namespace in exchange.

### Plugins

Plugin output carries no format address, and `--format-version` is **rejected
before the plugin is spawned**.
The reason is worth stating, because it is not the one a reader might assume.

JP owns the channel here, not the meaning.
A plugin's own stdout is the protocol: `message_loop` parses every line as
`PluginToHost`, so arbitrary plugin stdout is a protocol error, and user-facing
text reaches the terminal only because JP relays a `PrintMessage`.
What is missing is a *semantic* contract — `PrintMessage` cannot declare a
format address and `DescribeResponse` cannot declare which formats a plugin
emits.
Supplying one means extending the protocol, which is [RFD 072] and [RFD D19]
scope, not this RFD's.

[RFD 072] states that JP handles plugin output formatting.
That RFD is in Discussion, so this is not a blocking conflict, but implementing
that clause reopens this boundary and should be designed together with the
protocol extension above.
What this RFD owns is the exit-code mapping.

### Extension policy

Stated up front, because without it every added key is a version bump and the
scheme collapses under its own ceremony.

**The envelope is frozen.** Three *required* keys — `format`, `version`, `data`
— these names, this nesting, permanently.
There is no envelope version and never will be — a consumer would have to read
a version to learn how to read the version, so some layer has to be fixed, and
this is it.
Optional root keys may be *added*, and only if a consumer that ignores them
stays correct.
Removing or renaming a required key is not a versionable change; it is out of
bounds.
JP owns the root namespace; payload fields never appear there, flattened or
otherwise.
A change that would need a *required* root key is answered with a new format
address or a field inside `data`, never with an envelope change — the versioned
layer sits directly underneath, which is why the freeze costs so little.

The payoff is that the *mechanism* is permanent: on any JP release, forever,
including a stream from a version the script has never seen, `.format` and
`.version` are readable and `.data` holds the payload.
A specific address literal is only as durable as the address — a version `0`
address may still be renamed (see [Risks](#risks-and-open-questions)), so `jq
'select(.format == "conversation.event")'` becomes permanent when that address
reaches version `1`.

**Payloads are versioned** by `version` at `format`:

- **Adding a key** to a payload object is **not** breaking.
- **Consumers must ignore keys they do not recognize.**
- **Breaking** is: removing a key, renaming a key, changing a value's type, or
  changing documented semantics — the meaning of a value, a guaranteed
  ordering, or whether `null`, an absent field, and an empty array are
  equivalent.
- For a non-JSON encoding, **breaking** also covers its parse-relevant grammar:
  delimiters, quoting, and records per line.

Changing `jp c ls`'s activity column from `"3 minutes ago"` to an RFC 3339
timestamp breaks consumers without changing a single JSON type.
So does switching the default ordering from creation time to activity time.
Those are version bumps.

**Open enums are opt-in per format, not a global guarantee.** A format may
declare that unknown variants inside `data` are safely ignorable, and consumers
of that format may then match with a default arm.
`conversation.event` is the counterexample: a future redaction or replacement
event would change how *earlier* records must be interpreted, so a consumer
reconstructing a transcript cannot skip it and stay correct.

This is the same compatibility statement `cargo metadata` publishes, minus the
blanket enum clause.
It is also what lets a later root-level addition — a deprecation notice, for
one — land as an ignorable key rather than as a change to a frozen envelope.

### Nested payload versions

A payload that is *also* an interchange format in its own right may carry its
own inner version, governing that inner layer only.

Two numbers over one document only works if the outer one treats part of `data`
as **opaque**, so a format that does this declares the delegation explicitly:

- A format may declare all or part of `data` an independently versioned
  document.
- The **outer** version governs the container: the placement of that document,
  the fields around it, and how it is to be interpreted.
- The **inner** version governs the document's own fields and semantics.
- Changing only the opaque document does **not** bump the outer version.

Without the opacity clause the inner document's fields are also outer wire
shape, every inner change bumps both numbers, and the second number buys
nothing.

The case that forces this is a payload round-tripped as input — a conversation
export consumed by a matching import command has a reader on the other side, and
that reader's compatibility rules are not the CLI's.
`query.result` is the degenerate case: its `data` is the caller-supplied
schema's result, wholly opaque to JP, so the outer version promises only that
`data` *is* the requested result and says nothing about its fields.

A payload that is only ever output and wholly owned by JP does not get a second
number; the envelope's is the only one.

## Drawbacks

**It is a breaking change to every JSON consumer, in one shot.** Anything
parsing `jp c ls -F json` today gets an object where it expected an array.
The mitigation is timing: JP is pre-1.0 and the known consumers are the [RFD
086] pipelines and our own tooling.
The alternative is a trickle of smaller breaks with no announcement mechanism at
all.

**Envelope overhead on NDJSON.** ~50 bytes per line on a `jp query` stream.
This is the reason the envelope is three keys and not six, and the reason the
timestamp and `jp` version discussed during design were cut.

**`--format-version` is a promise to keep old versions alive.** The retention
invariant is a stated contract, not an implication: once a shape is stabilized,
JP owes its consumers that version until a deprecation completes.
Nothing is owed today because version `0` is disposable, but the obligation
lands in full the first time a shape reaches version `2`, and it arrives as
multi-version emission machinery.
That cost is deferred, not avoided — choosing the fail-fast reading of the flag
instead would be the way to avoid it, and this RFD declines to.

**Version 0 is honest but unhelpful.** A consumer pinning `--format-version=0`
pins nothing.
The flag only starts paying the day a shape reaches `1`, and scripts written
before then get a clean failure rather than continued operation.

**Plugins lose their exit-code channel.** Covered above; it is a real narrowing
of an interface that works today.

## Alternatives

### Version field only, no flag

Add `version` to the output, skip negotiation.
Rejected: it is a receipt, not insurance.
A consumer learns after the fact that it received something it cannot parse; it
still cannot ask for what it can parse.

### Version by access point instead of by format

Version the *command* rather than the shape: all output from `jp c ls` is at
version N, and `--format-version=N` pins the command.
No addresses needed.

Rejected on two existing cases.
The `error` format is emitted by every command, so it would either be versioned
once per access point — changing the error shape bumps ~30 commands on the same
day — or exempted, which builds format addressing for one unnamed special case.
And `jp query -F json` already emits two unrelated shapes from one access point.

Shared payload types make it worse rather than better: if `ls`, `show`, and
`grep` all emit a stabilized conversation summary, changing that summary forces
three renumberings for one change, and the multi-version machinery still has to
keep one old summary type alive for all three.

The ergonomics of the access-point model are worth keeping, which is why the
bare `--format-version=N` form is command-scoped.
Only the addressing moves into the payload.

### A single global version for all machine output

Rejected because it is incompatible with incremental stabilization: a single
number cannot express "`conversation.summary_list` is stable at `1` while
`conversation.event` is still `0`," which is the entire migration strategy.

### Remap only the plugin exit codes that collide

Forward plugin exit codes as today, except `20` and `21`.
Narrower, and preserves most of the plugin interface.

Rejected because partial forwarding is a stranger contract than either extreme:
a plugin's exit `19` passes through while `20` becomes `1`, so a downstream
script cannot reason about *any* code without knowing JP's reserved set, and the
reservation has to be relitigated the next time JP needs a code.
Full namespace ownership is the smaller long-term surprise.

### Full multi-version emission now

Ship `--format-version` with JP maintaining every historical version from day
one, notmuch-style.
Rejected as the Second-System trap: two emitters per shape, forever, for formats
we already know are wrong.
The flag reserves the option; the obligation is bought when a real second
version exists.

## Non-Goals

- **Designing the stable shapes.** No shape is promoted past version `0` here.
  The typed-`Serialize` rewrite of `list_json` / `details_json`, the
  stabilization procedure, and per-format compatibility policies (including
  which formats declare open enums) are separate, per-shape work that this RFD's
  version-`0` baseline exists to unblock.
- **Multi-version emission and deprecation.** How JP emits versions `1` and `2`
  side by side, and how it warns before dropping a retained version, is designed
  when a second stabilized version first exists.
  Version `0` never coexists with `1` — it is dropped on stabilization.
  The extension policy reserves envelope keys for the deprecation notice; exit
  code `20` is reserved for the removal.
- **Unifying the plugin and server protocols with this envelope.** Those are RPC
  transports with correlation, handshakes, and verb-shaped messages; this
  envelope frames one-way documents.
  The registry is built so a later RFD can share *payload types* across
  transports without sharing framing or version namespaces — but that RFD is
  not this one, and `jp_plugin::PROTOCOL_VERSION` stays independent of format
  versions.
- **Versioning JSON input.** Accepting a machine-readable request format is a
  different axis with a different number.
- **Versioning observability output.** Tracing events and the log-path
  announcement belong to [RFD D15] and [RFD D32].
  If those RFDs give tracing a typed, committed shape, whether it earns a format
  address is their decision; this RFD neither claims nor forecloses it.
- **Versioning chrome.** Chrome is not a machine record and gets no envelope.
  How it is serialized under each output mode is [RFD 048]'s contract, which
  this RFD extends rather than amends.

## Risks and Open Questions

1. **Envelope shape for NDJSON.** Repeating `format` and `version` on every line
   keeps each line self-contained and `jq`-friendly, at a per-line cost.
   A single header line would be cheaper and would break `jq -c` streaming and
   `tail -f`.
   Per-line is the proposal; the cost should be measured on a long `jp query`
   stream before it is settled.

2. **Element identity before stabilization.** The naming convention assumes `jp
   c ls` and `jp c show` will share a `conversation.summary` element, which is
   why the list is `conversation.summary_list`.
   If stabilization finds the two genuinely need different elements, that
   address is renamed — cheap at version `0`, impossible at version `1`.

3. **Exit-code narrowing is user-visible.** Plugins that today exit with a
   meaningful code lose it.
   `jp-path` and `jp-serve-web` do not appear to rely on this, but external
   plugins might.

4. **The `message` address is a holding pen.** It keeps the invariant honest,
   but a command whose only output is `message` is not scriptable.
   If it becomes a permanent home for output that should have been typed, it has
   failed at its purpose.

5. **[RFD 085] embeds `"schema_version": 1` inside `QueryPreview`** and sends it
   through `print_json`.
   Under this RFD that output is also an envelope payload at version `0`, so one
   document would carry two numbers governing the same contract.
   The nested-version rule covers payloads that are independently interchange
   formats; a preview is not one, so RFD 085 should drop its inner number in
   favor of the envelope's.
   That edit belongs in RFD 085.

## Implementation Plan

The three phases are separately reviewable but **ship in one release**.
Announcement without negotiation is the half-measure the Motivation rejects: a
release carrying only Phase 1 would break every JSON consumer and give them no
way to pin what replaced it.
And the exit-code guarantees are not true until Phase 3 — while plugin codes
are forwarded verbatim, a plugin exiting `20` or `21` is indistinguishable from
a version rejection.

### Phase 1: Envelope, addresses, and migration

Introduce the envelope type and the address registry.
Route `print_json`, `print_table`, `print_details`, `JsonEmitter`,
`Printer::wrap_json`, and `parse_error` through it.

Then inventory every machine-output path, not only the ones that go through a
helper: all of stdout in every output format, plus the `error` record on stderr.
The audit cannot stop at the `Printer` — `run()` writes to stderr directly with
`eprintln!` after the command returns, and a helper-only sweep would miss any
such path.
Direct writes and empty-result branches are in scope.
Every address registers at version `0`.
Existing non-empty payloads keep their version-`0` shape and are wrapped.
Empty and status-message branches are normalized to the command's declared
address where the declaration requires it — `attachment ls` with zero
attachments emits `attachment.summary_list` with an empty payload instead of a
`message`, because the declared address set cannot depend on workspace data.
The inventory will surface more of these; each one is listed in the migration
notes.

Ship the migration material in the same phase, because this is the breaking
change and a consumer should not have to infer the fix after release: document
the envelope, the extension policy, and the meaning of version `0` in
`docs/usage.md`; show old and new `jq` filters (`.data` for singletons,
`.data[]` for arrays); add *format address*, *machine record*, and *format
version* to `docs/architecture/ubiquitous-language.md`, defined as data-shape
concepts rather than stdout concepts.

Any typed payload type introduced here lives in its domain crate
(`jp_conversation` and friends), not in `jp_cli`, so a future plugin host or
server can use it without depending on the CLI.

Independently reviewable and mergeable.
This is the breaking change; everything after it is additive.

### Phase 2: Preflight, `--format-version`, and exit codes

Add the address-declaration function and the preflight validation step, placed
after argument parsing and before workspace load.
Add the global flag with the bare and explicit forms.
Accept only currently-emitted versions; exit `21` for anything higher, usage
error for unknown or non-emitted addresses.
Reserve `20`.
Wire the `error` format out of negotiation and reject the flag for external
subcommands.

Depends on Phase 1.
Exit `20` is unreachable until a shape reaches version `1`, and exit `21` is
only unambiguous once Phase 3 lands.

### Phase 3: Plugin exit-code mapping

Map any nonzero plugin exit to `1` in `cmd/plugin/dispatch.rs`, recording
`plugin_exit_code` in the error metadata.
Update [RFD 072]'s Lifecycle section, which currently states that JP exits with
the plugin-provided code.

Depends on Phase 2: the narrowing is only justified once the reserved codes
exist.

## References

- [RFD 048: Four-Channel Output Model][RFD 048] — establishes stdout as the
  data channel; this RFD defines what that channel carries.
- [RFD 050: Scripting Ergonomics for Conversation Management][RFD 050] —
  defines the line-oriented / JSON-array ID output convention.
- [RFD 086: Line-oriented stdin input for CLI arguments][RFD 086] — the
  pipelines that consume that convention and depend on JP's machine output being
  stable.
- [RFD 029: Scriptable Structured Output][RFD 029] — the `jp query -F json`
  noise problem, and why JSON output is not gated on `OutputFormat::Json`.
- [RFD 085: Query Explain][RFD 085] — carries a nested `schema_version` that
  this RFD's envelope supersedes.
- [RFD 072: Command Plugin System][RFD 072] and [RFD D19: Structured Plugin Help
  Protocol][RFD D19] — where plugin output formats would be defined, if ever.
- [RFD D15: Structured Logging Infrastructure][RFD D15] and [RFD D32: JP Tracing
  Infrastructure][RFD D32] — own the observability channel, including the
  log-path announcement and any future typed trace event shape.
- [notmuch structured output versioning] — `--format-version`, exit `20`/`21`.
- [`cargo metadata` compatibility] — the "adding fields is not breaking" policy
  this RFD adopts.

[RFD 029]: ../029-scriptable-structured-output.md
[RFD 048]: ../048-four-channel-output-model.md
[RFD 050]: ../050-scripting-ergonomics-for-conversation-management.md
[RFD 072]: ../072-command-plugin-system.md
[RFD 085]: ../085-query-explain.md
[RFD 086]: ../086-line-oriented-stdin-input-for-cli-arguments.md
[RFD D15]: D15-structured-logging-infrastructure.md
[RFD D19]: D19-structured-plugin-help-protocol.md
[RFD D32]: D32-jp-tracing-infrastructure.md
[`cargo metadata` compatibility]: https://doc.rust-lang.org/cargo/commands/cargo-metadata.html#compatibility
[notmuch structured output versioning]: https://notmuchmail.org/doc/latest/man1/notmuch-show.html
[ubiquitous language]: ../../architecture/ubiquitous-language.md
