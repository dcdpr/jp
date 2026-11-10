# RFD D34: Versioned Machine Output Envelope

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-28
- **Extends**: [RFD 048]
- **Summary**: Wraps JSON machine output in a versioned `{"format", "version",
  "data"}` envelope and adds `--format-version` pinning.

## Summary

This RFD wraps every JSON machine record JP emits in a `{"format", "version",
"data"}` envelope that names the record format and its schema version.
Line-oriented ID outputs remain raw lines.
Existing JSON shapes start at version `0` until stabilized, and a
`--format-version` flag lets callers pin the version they expect, exiting with
code `20` or `21` on a version mismatch.

## Motivation

`jp -F json` is not a single format.
It produces seven unrelated shapes, three of which are byproducts of human
rendering:

| Producer                      | Shape                         | Source                                                                                |
| ----------------------------- | ----------------------------- | ------------------------------------------------------------------------------------- |
| `jp c ls`                     | array of objects              | rendered table headers (`ID`, `#`, `Activity ▼`); columns appear and vanish with data |
| `jp c show`, `attachment ls`  | object                        | rendered detail labels, ANSI-stripped                                                 |
| `jp c grep`                   | array of objects              | hand-built `json!` in `render_json`                                                   |
| `jp c grep -l`, `new`, `fork` | array of ID strings           | deliberate, per [RFD 050]                                                         |
| `jp query`                    | NDJSON of `ConversationEvent` | direct serialization of the persisted event type                                      |
| errors                        | `{message, metadata, code}`   | `parse_error`, on stderr                                                              |
| `Printer::print` / `println`  | `{"message":"…"}` per call    | `Printer::wrap_json`, on stdout                                                       |

`list_json` keys each object by `strip_ansi_escapes::strip_str(header_cell)`.
Renaming a column or adding a sort arrow silently changes the JSON, and columns
appear and vanish with the data (`expires_at` and `local` are emitted only when
some row has a value).
The `(3 hidden)` summary row also becomes an object.
Anyone parsing `jp c ls -F json` depends on all of these details.

`Printer::print` and `Printer::println` are the widest gap.
Both wrap their content as `{"message":"…"}` and write it to **stdout**, so any
command that prints through either method emits unannounced JSON on the data
channel.
Commands also emit primary data this way: `conversation path` prints filesystem
paths, `config show` prints TOML, and `attachment ls` prints a message when
empty but a details object when non-empty.

[RFD 086] builds `jp c grep -l … | jp c archive -` pipelines on this output as
an intended workflow.
Without a version contract, any change to a table column silently breaks
documented pipelines, and consumers only notice when their parsers produce wrong
answers.

A version contract requires two parts:

- **Announcement**: the output states its format and version so a consumer can
  detect an unfamiliar shape.
- **Negotiation**: the consumer requests the version it expects, and JP honors
  it or fails before doing any work.

## Design

### Machine records

A **machine record** is a structured command result on the stdout data channel,
or a structured post-parse `error` record on stderr.
Records are classified by producer intent, not by whether the bytes parse as
JSON or which `OutputFormat` is active.
`print_json` emits JSON under `Text` and `TextPretty` to support the no-flag
pipelines in [RFD 029], so JSON machine output is not limited to
`OutputFormat::Json`.

Every machine record has a registry address.
Every **JSON** machine record also carries that address in band inside an
envelope:

- every JSON machine record on **stdout**, in every output format;
- the `error` record on **stderr**, under JSON output modes.

Line-oriented text encodings have a registry address but no in-band
announcement; they are versioned out-of-band by `--format-version` alone.

The following outputs are **not machine-readable** and carry no envelope:

- **Chrome.** Progress, tool headers, status, and countdowns are human-facing
  output on stderr.
  [RFD 048] defines how they serialize per output mode, and this RFD does not
  change that behavior.
- **Human renderings.** The pretty and plain tables produced by `-F text-pretty`
  and `-F text` are presentation, not data, and cannot be pinned.
- **Argument-parsing diagnostics.** `Cli::try_parse` failures exit through
  `e.exit()` before the output format is resolved, so clap's usage errors,
  `--help`, and `--version` never use the envelope.

The following outputs **are** machine-readable, but belong to separate contracts
with their own version namespaces:

- **Observability output.** Tracing events (written to the log file or to stderr
  via `--log-file=-`) and the log-path announcement.
  JP does not commit to a trace event shape today; if [RFD D15] and [RFD D32]
  introduce typed events, those RFDs will decide whether to register them here.
  `run()` also writes `{"trace_log": "<path>"}` to stderr for in-repo tooling,
  but [RFD D15] folds that path into a post-run log report.
  Registering `trace_log` here would freeze a shape that [RFD D15] replaces.
- **Plugin output.** JP relays text from a plugin's `PrintMessage` to stdout
  verbatim, which may itself be JSON.
  JP owns the transport channel but not the payload schema; assigning it a
  format address requires a protocol extension (see [Plugins](#plugins)).

Errors fall into three tiers, and only the third is a machine record:

| Failure                                        | Surface                       |
| ---------------------------------------------- | ----------------------------- |
| Argument-parsing errors, `--help`, `--version` | clap diagnostics, unversioned |
| Post-parse failure under a text output mode    | human error rendering         |
| Post-parse failure under a JSON output mode    | `error` record, enveloped     |

Preflight version rejections bypass clap and use the post-parse error renderer
for the resolved output mode: an enveloped `error` record under `json` and
`json-pretty`, or human error text under the text modes.
In both modes, the process exits with code `20` or `21` before loading the
workspace and writes nothing to stdout, preserving the negotiation contract for
line-oriented consumers.

No built-in command emits unannounced JSON on stdout.
Status lines that are not structured data receive the `message` address at
version `0`, matching what `Printer::wrap_json` already produces.
`message` is a temporary migration address because `Printer::print` and
`println` wrap arbitrary strings as JSON, which is how `conversation path` and
`config show` currently emit primary data.
Once every command declares its own address, `message` will be deleted rather
than stabilized (see [Preflight negotiation](#preflight-negotiation)).

### The envelope

```json
{
  "format": "conversation.summary_list",
  "version": 0,
  "data": [ … ]
}
```

The envelope has three required keys (`format`, `version`, and `data`) and is
repeated on every line of an NDJSON stream:

```jsonl
{"format":"conversation.event","version":0,"data":{"type":"chat_request","content":"…"}}
{"format":"conversation.event","version":0,"data":{"type":"chat_response","message":"…"}}
```

Array and bare-value payloads (`jp c grep`, `jp c grep -l`, `jp query --schema`)
gain a field for their schema version.

The line-oriented ID format consumed by [RFD 086] has no in-band field for a
version and keeps its raw-line representation; `--format-version` versions it
out-of-band.

### Format addresses

`format` is a dot-delimited identifier for a **semantic record contract**,
independent of the transport channel or encoding.
The version number attaches to this address.

A contract may have multiple encodings.
`conversation.id_list` is a JSON array under the JSON formats and one ID per
line under the text formats; both represent the same contract at the same
version.

A format's version governs two things: the record's semantic contract (meaning,
field set, and documented guarantees) and, for each encoding, that encoding's
**parse-relevant grammar** (delimiters, quoting, ordering, and records per
line).
Presentation-only differences are not versioned.
Compact and pretty JSON share a version number because every conforming JSON
parser accepts both.
A line-oriented encoding has no shared parser grammar, so its syntax is part of
the contract: quoting each ID or switching from newlines to commas breaks
consumers without changing a semantic field and requires a version bump.

If two encodings diverge enough that they no longer represent the same semantic
record, they receive separate format addresses rather than a version bump.

The key is named `format` rather than `type` because `EventKind` already
serializes with `#[serde(tag = "type")]`.
Keying the envelope with `type` would produce `{"type": …, "data": {"type":
"chat_request"}}`, placing two different `type` fields one level apart.

Addresses belong to the data shape, not the command that emits it.
Multiple commands can emit the same address, and one command can emit multiple
addresses.
Every CLI command can emit `error`, and `jp query -F json` emits both an event
stream (`JsonEmitter`) and a structured schema result (`print_json`).

Addresses identify semantic records, not structural coincidences.
`details_json` produces the same outer `{title, details}` structure for
conversation details and attachment listings, so those use two separate
addresses.

#### Naming convention

Addresses take the form `<subject>.<shape>` in lowercase `snake_case`:

- **Subject** is the domain entity the record describes, using the [ubiquitous
  language] term where one exists, or the domain area when the record describes
  an operation outcome (`query.result`).
- **Shape** names the data structure, never the command that produced it
  (`list`, not `ls`) and never the wire encoding (no `.json` or `.ndjson`
  suffix).
- **Collections end in `_list`,** preceded by the element name.
  When the element has its own address, the two names match:
  `conversation.summary_list` is a list of `conversation.summary`.
  Matching names encourage shared payload types between single-item and list
  commands.
- **Single-segment addresses** are reserved for cross-cutting records without a
  domain subject.
  Only `error` and `message` use single-segment names.
- **JP owns unprefixed addresses.** `plugin.*` is reserved so a future
  plugin-format namespace cannot collide with built-in addresses.

Collections use `_list` rather than an English plural because
`conversation.summary` and `conversation.summaries` differ by only two
characters in string literals that consumers match verbatim.

Two segments suffice for every address in the initial registry; deeper nesting
is allowed if a shape justifies it at stabilization.
Version `0` addresses may be renamed before stabilization, whereas version `1`
addresses are permanent.

### Version 0 registry

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

`c new` and `c fork` are listed as planned producers.
`conversation new` does not exist yet and `fork` currently prints a status
message rather than an ID; both will adopt `conversation.id_list` when [RFD 050]
is implemented.

Two payload boundaries apply:

- `query.result` versions JP's **result container**, not the caller-supplied
  JSON Schema payload inside it.
- `conversation.event` versions the emitted `ConversationEvent` record, not the
  persisted `InternalEvent` stream entry, which base64-encodes content fields at
  rest and includes `config_delta` and `compaction` entries that the emitter
  never outputs.

`error` and `message` are announced and versioned in the envelope, always emit
at their current version, have no primary status, and reject `--format-version`
pins:

- `error` covers post-parse failures under JSON output modes (see [Machine
  records](#machine-records)).
  It cannot be pinned because pinning the record used to report a failed version
  negotiation is circular.
- `message` is a temporary migration artifact scheduled for deletion.
  A command whose only output is `message` has no machine contract, so rejecting
  `--format-version` forces the command to declare a typed address before it can
  support version pinning.

### Version 0 semantics

Every shape starts at version `0`.
At version `0`, the payload shape has no stability guarantee and may change in
any release without notice.
This allows the envelope to ship immediately before individual shapes are
stabilized.

**Version `0` waives only the `data` contract.** `version` is the version of
`data` at `format`; the outer envelope has no version and is permanently fixed
(see [Extension policy](#extension-policy)), so consumers can always read the
envelope fields to check the format and version.

Stabilizing a shape promotes it from `0` to `1`: the shape is backed by a typed
`Serialize` struct rather than derived from rendered output, reviewed, and
documented.
Each shape can be stabilized independently.

When a shape reaches version `1`, version `0` is **dropped** rather than
maintained alongside `1`.
`jp c ls --format-version=0` then exits with code `20`.

### Version retention

`--format-version` negotiates a retained version rather than performing only a
fail-fast check:

- **Version `0` is disposable.** JP drops version `0` as soon as a shape reaches
  version `1`.
- **Stabilized versions (`1` and higher) remain supported** after newer versions
  ship.
  JP removes a stabilized version only after a deprecation period, never as a
  side effect of introducing the next version.

Without retention, introducing version `2` would immediately break callers
pinned to `--format-version=1`.

Retention does not apply to non-negotiable formats (`error` and `message`).
Because callers cannot pin them, JP emits only their current version.

Because callers cannot pin `error`, a breaking change to `error` after version
`1` leaves callers with no fallback version.
Therefore, `error` is stabilized once and evolves only additively under the
extension policy.
Consumers must use the process exit code to determine *whether* a command failed
and parse the `error` payload only to inspect *why*.

`message` is never stabilized; it will be deleted once all commands declare
typed addresses.

The mechanism for emitting multiple retained versions side by side and the
deprecation procedure are deferred until a shape reaches version `2` (see
[Non-Goals](#non-goals)).

### Preflight negotiation

`run_inner` loads and sanitizes the workspace (which can modify persisted state)
before dispatching a command, and `jp query` calls the LLM and persists events
before writing any output.
Validating a version pin when emitting the first record is too late: JP would
exit with code `20` or `21` after performing side effects or writing partial
stdout.

Each command **declares its addresses statically**, specifying the set of
addresses it may emit and which address is primary.
Immediately after argument parsing and before loading the workspace, a preflight
step resolves that declaration for the invocation and validates every
`--format-version` pin.

Runtime branches must stay within the declared address set.
An empty result uses the same address as a non-empty result: `attachment ls`
with zero attachments emits `attachment.summary_list` with an empty payload, not
`message`.

The declaration is a **pure function of the parsed invocation**, not a
per-command constant:

```text
parsed command + resolved output options -> emitted address set + optional primary
```

Command arguments and output options determine which contract is primary:

- `jp c grep` emits `conversation.match_list`, whereas `jp c grep -l` emits
  `conversation.id_list`.
- `jp query` has primary format `conversation.event`, whereas `jp query
  --schema` has primary format `query.result` because the caller requested the
  schema result and [RFD 029] treats the event stream as secondary in scripts.
  `conversation.event` remains in the emitted set for `jp query --schema` and
  can still be pinned by explicit address.
- An invocation that produces only human-rendered text emits no machine record
  and has no primary address.

Keeping the declaration independent of workspace data and runtime results makes
it callable during preflight.
A command that emits multiple addresses designates one as primary and requires
an explicit address pin for the others.

### Pinning a version

```sh
jp c ls -F json --format-version=0                # command-scoped shorthand
jp query --format-version=conversation.event=0    # explicit address
```

The bare form (`--format-version=N`) pins the command's primary format.
The explicit form (`--format-version=<address>=N`) pins a specific format when a
command emits more than one.

| Situation                                                   | Behavior                                                      |
| ----------------------------------------------------------- | ------------------------------------------------------------- |
| Flag omitted                                                | Emit current versions (the default)                           |
| `-F auto` / `text` / `text-pretty` / `json` / `json-pretty` | Identical version semantics                                   |
| `json` vs `json-pretty`                                     | Same version; the number names the shape, not the formatting  |
| Line-oriented text machine formats                          | Covered; the version is out-of-band only                      |
| Invocation emits only a human rendering                     | Usage error, exit `2`                                         |
| Requested version is higher than any JP emits               | Exit `21`                                                     |
| Requested version was emitted once, now dropped             | Exit `20`                                                     |
| Address unknown, or not emitted by this invocation          | Usage error, exit `2`                                         |
| Command emits no machine format in any invocation           | Usage error, exit `2`                                         |
| Explicit pins                                               | Repeatable, one per address                                   |
| Bare pin                                                    | At most once; duplicate or conflicting pins rejected at parse |

`--format-version` is orthogonal to `-F` and never changes the output mode.
It versions machine records wherever they are emitted, including `print_json`
under text modes and line-oriented ID lists under `jp c grep -l
--format-version=0`.
It does not convert human renderings into machine records: running `jp c ls
--format-version=0` without `-F json` is a usage error (exit `2`) because the
default table output is presentation, and succeeding silently would give scripts
a false guarantee.

`json-pretty` records span multiple lines and are not NDJSON, unlike compact
`json`, but both use the same format version.

**The `error` format is announced but not negotiable.** It carries `format` and
`version` in the envelope, and `--format-version` rejects pins targeting
`error`.

### Exit codes

| Code | Meaning                                                      |
| ---- | ------------------------------------------------------------ |
| `20` | The requested format version is too old (no longer emitted). |
| `21` | The requested format version is too new (not yet supported). |

These exit codes follow notmuch's convention.
Both are reserved now, though only `21` can occur until a shape reaches version
`1` and drops version `0`.

Reserving codes `20` and `21` requires narrowing JP's plugin exit-code behavior.
`cmd/plugin/dispatch.rs` currently forwards a plugin's exit code verbatim via
`cmd::Error::from(exit.code)`, making a plugin that exits `21` indistinguishable
from a format-version rejection.
JP instead maps any nonzero plugin exit code to `1` and records the original
code in the error metadata:

```json
{"format":"error","version":0,"data":{"message":"plugin failed","metadata":{"plugin_exit_code":20},"code":1}}
```

This change narrows the plugin interface: the plugin's exit code remains
available in structured error metadata on stderr, but scripts inspecting `$?`
only see `1` on plugin failure.
In exchange, JP owns its entire exit-code namespace.

### Plugins

Plugin output carries no format address, and `--format-version` is **rejected
before the plugin is spawned**.

A plugin's stdout is reserved for the host-plugin protocol: `message_loop`
parses every line as `PluginToHost`, treats arbitrary stdout as a protocol
error, and writes user-facing text to the terminal only when relaying a
`PrintMessage`.
However, the protocol has no semantic format contract: `PrintMessage` cannot
specify a format address, and `DescribeResponse` cannot declare which formats a
plugin emits.
Adding plugin format addresses requires extending the plugin protocol in [RFD
072] and [RFD D19].

[RFD 072] (currently in Discussion) states that JP formats plugin output;
implementing that behavior should be designed alongside protocol support for
plugin format addresses.
This RFD only changes how plugin exit codes are mapped.

### Extension policy

Without an explicit compatibility rule, every added JSON key would require a
version bump.

**The envelope is frozen.** It has three required root keys (`format`,
`version`, and `data`) with fixed names and nesting, and no envelope version.
Optional root keys may be added only if consumers that ignore them remain
correct.
Removing or renaming a required root key is never permitted.
JP owns the root namespace; payload fields are never flattened into the root
object.
Any change that requires a new mandatory field belongs inside `data` or under a
new format address.

Because the envelope is fixed, `.format`, `.version`, and `.data` can be parsed
on every future JP release.
A filter such as `jq 'select(.format == "conversation.event")'` becomes
permanent once `conversation.event` reaches version `1` (version `0` addresses
may still be renamed; see [Risks and Open
Questions](#risks-and-open-questions)).

**Payloads are versioned** by `version` at `format`:

- **Adding a key** to a payload object is **not** a breaking change.
- **Consumers must ignore unrecognized keys.**
- **Breaking changes** include removing or renaming a key, changing a value's
  type, or changing documented semantics (the meaning of a value, a guaranteed
  sort order, or whether `null`, an absent field, and an empty array are
  equivalent).
- For non-JSON encodings, **breaking changes** also include changes to the
  parse-relevant grammar (delimiters, quoting, and records per line).

For example, changing `jp c ls`'s activity field from `"3 minutes ago"` to an
RFC 3339 timestamp, or changing the default sort order from creation time to
activity time, is a breaking semantic change and requires a version bump.

**Open enums are opt-in per format.** A format may declare that unknown enum
variants inside `data` can be safely ignored, allowing consumers to match with a
fallback arm.
Formats do not have open enums by default: in `conversation.event`, a future
redaction or replacement event could alter the meaning of earlier records, so a
consumer reconstructing a transcript cannot safely skip unknown variants.

This compatibility policy matches `cargo metadata`, except that open enums are
opt-in per format.
Allowing optional root keys also lets future features, such as deprecation
warnings, attach to the envelope without breaking existing parsers.

### Nested payload versions

A payload that is itself a standalone interchange format may carry an inner
version number governing only that inner document.

For two version numbers to coexist without coupling, the outer format must treat
the inner document as **opaque**:

- A format may declare all or part of `data` to be an independently versioned
  document.
- The **outer** version governs the container: where the inner document is
  placed, the surrounding fields, and how the container is interpreted.
- The **inner** version governs the inner document's fields and semantics.
- Changing only the opaque inner document does **not** bump the outer version.

If the outer format does not treat the inner document as opaque, every inner
change is also an outer wire change and bumps both versions.

Nested versions are needed when a payload is round-tripped as input, such as a
conversation export consumed by an import command with its own compatibility
rules.
`query.result` is a boundary case: its `data` is the caller-defined schema
output, which is completely opaque to JP, so the outer version guarantees only
that `data` holds the schema result.

Payloads that are output-only and wholly owned by JP use only the envelope
version.

## Drawbacks

**One-time breaking change for all JSON consumers.** Any script parsing `jp c ls
-F json` receives an envelope object instead of a top-level array.
Because JP is pre-1.0, the known consumers are the [RFD 086] pipelines and
in-repo tooling, and making one explicit break now avoids repeated unannounced
breaks as individual shapes are stabilized.

**Envelope overhead on NDJSON.** Repeating the envelope adds roughly 50 bytes
per line on a `jp query` stream.
To keep that overhead small, the envelope is limited to three keys and omits the
timestamp and `jp` version fields considered during design.

**Maintaining old versions once stabilized.** Once a shape reaches version `1`,
JP must continue emitting that version until it completes a deprecation cycle.
Version `0` is disposable, so no compatibility machinery is needed immediately,
but the first version `2` shape will require multi-version emission support.

**Pinning version `0` provides no stability.** Pinning `--format-version=0` only
asserts that the shape has not yet been stabilized.
Scripts written against version `0` will fail with exit code `20` when the shape
reaches version `1` rather than continuing to run.

**Plugins lose distinct exit codes.** Mapping nonzero plugin exits to `1`
removes the ability for callers to branch on a plugin's process exit status
without reading stderr.

## Alternatives

### Version field only, no flag

Add `version` to the output envelope without `--format-version` negotiation.
Rejected because a version field only lets a consumer detect an incompatible
change after the command has already run and mutated state; it does not let the
consumer request a supported version or fail before execution.

### Version by command instead of by format

Version the command rather than the output shape, so all output from `jp c ls`
shares version `N` and `--format-version=N` pins the command without format
addresses.

Rejected for three reasons:

1. Every command can emit `error`, so changing the error shape would either bump
   the version of ~30 commands at once or require exempting `error` as a special
   case.
2. `jp query -F json` emits two unrelated shapes (`conversation.event` and
   `query.result`) from a single command.
3. Shared payload types across commands (`ls`, `show`, and `grep` emitting a
   conversation summary) would require bumping all three commands whenever the
   shared summary type changes, while still maintaining the old summary type in
   all three places.

The bare `--format-version=N` flag preserves the ergonomics of command-scoped
pinning while keeping versions attached to format addresses.

### A single global version for all machine output

Use one version number for all machine output across JP.
Rejected because it prevents incremental stabilization: a single version number
cannot express that `conversation.summary_list` is stable at version `1` while
`conversation.event` remains at version `0`.

### Remap only colliding plugin exit codes

Forward plugin exit codes unchanged except for `20` and `21`.
Rejected because partial forwarding makes the exit-code contract unpredictable:
a plugin exit code of `19` would pass through while `20` becomes `1`, forcing
scripts to track JP's reserved code set and breaking callers again whenever JP
reserves another code.

### Full multi-version emission now

Require JP to retain and emit every historical version starting from version
`0`.
Rejected because the current version `0` shapes are known to be unstable
byproducts of human rendering; maintaining multiple emitters for them before
stabilization adds permanent maintenance cost for shapes that will be replaced.

## Non-Goals

- **Designing the stable version `1` shapes.** No shape is promoted past version
  `0` in this RFD.
  Rewriting `list_json` and `details_json` to serialize typed structs, defining
  the stabilization checklist, and setting per-format compatibility rules
  (including open-enum policies) are separate per-shape tasks.
- **Multi-version emission and deprecation mechanics.** How JP emits versions
  `1` and `2` side by side and how it announces deprecations will be designed
  when the first version `2` shape is proposed.
  Version `0` never coexists with version `1`.
  The extension policy reserves optional root keys for deprecation notices, and
  exit code `20` is reserved for removed versions.
- **Unifying the plugin and server protocols with this envelope.** Those
  protocols are bidirectional RPC transports with request correlation and
  handshakes, whereas this envelope frames one-way output records.
  Domain crates can share payload types across CLI output and RPC transports
  without sharing framing or version namespaces, and
  `jp_plugin::PROTOCOL_VERSION` remains independent of format versions.
- **Versioning JSON input.** Versioning machine-readable input formats is
  separate from output versioning.
- **Versioning observability output.** Tracing events and the log-path
  announcement belong to [RFD D15] and [RFD D32].
- **Versioning chrome.** Chrome on stderr is human-facing output governed by
  [RFD 048] and does not use this envelope.

## Risks and Open Questions

1. **Envelope overhead on NDJSON streams.** Repeating `format` and `version` on
   every line keeps each line self-contained for `jq -c` streaming and `tail
   -f`, whereas a single header line would break line-by-line tools.
   The per-line overhead should be measured on long `jp query` streams before
   stabilization.

2. **Shared element identity between `ls` and `show`.** The naming convention
   assumes `jp c ls` (`conversation.summary_list`) and `jp c show`
   (`conversation.summary`) will share a summary payload type.
   If stabilization shows that they need distinct types, one address can be
   renamed while still at version `0`.

3. **Plugin exit-code narrowing.** External plugins that rely on returning
   distinct nonzero exit codes to calling scripts will now exit with `1`
   (`jp-path` and `jp-serve-web` do not rely on distinct exit codes).

4. **Lingering `message` outputs.** `message` provides an immediate envelope for
   `Printer::print` and `println`, but any command that continues to emit only
   `message` cannot be pinned with `--format-version`.
   Commands that output primary data through `message` must be migrated to typed
   addresses.

5. **Duplicate version field in [RFD 085].** [RFD 085] embeds `"schema_version":
   1` inside `QueryPreview` and emits it through `print_json`.
   Because `QueryPreview` is not a standalone round-tripped interchange format,
   [RFD 085] should drop its inner `schema_version` field and rely on the outer
   envelope version.

## Implementation Plan

All three phases are independently reviewable on a branch or in stacked PRs, but
**must ship in the same release**.
Shipping Phase 1 without Phase 2 would break existing JSON consumers without
giving them `--format-version`, and exit codes `20` and `21` are only
unambiguous once Phase 3 remaps plugin exit codes.

### Phase 1: Envelope, addresses, and migration

Introduce the envelope type and the address registry, and route `print_json`,
`print_table`, `print_details`, `JsonEmitter`, `Printer::wrap_json`, and
`parse_error` through it.

Audit every machine-output path across all output formats on stdout and the
`error` record on stderr, including direct writes outside `Printer` (such as
`eprintln!` calls in `run()`) and empty-result branches:

- Register every initial address at version `0` and wrap existing payloads in
  the envelope.
- Normalize empty-result branches to emit the command's declared address rather
  than `message` (for example, `attachment ls` with zero attachments emits
  `attachment.summary_list` with an empty list).
- Place any new typed payload structs in their domain crates (`jp_conversation`
  and sibling crates) rather than `jp_cli` so non-CLI consumers can reuse them.

Update documentation in the same phase:

- Document the envelope, extension policy, and version `0` semantics in
  `docs/usage.md`, including before-and-after `jq` examples (`.data` for
  singletons, `.data[]` for arrays) and a list of normalized empty-result
  branches.
- Add *format address*, *machine record*, and *format version* to
  `docs/wiki/00dxybd-ubiquitous-language.md` as data-contract terms.

### Phase 2: Preflight, `--format-version`, and exit codes

Add the static address-declaration function and the preflight validation step
between argument parsing and workspace loading.
Add the global `--format-version` flag supporting both bare (`N`) and explicit
(`<address>=N`) forms:

- Accept currently emitted versions.
- Exit `21` when a requested version is higher than supported, and reserve exit
  code `20` for dropped versions.
- Exit `2` (usage error) for unknown addresses, addresses not emitted by the
  invocation, `error` pins, or external plugin subcommands.

Depends on Phase 1.

### Phase 3: Plugin exit-code mapping

Map any nonzero plugin exit code to `1` in `cmd/plugin/dispatch.rs` and record
`plugin_exit_code` in the error metadata.
Update the Lifecycle section of [RFD 072] to reflect that JP exits with `1`
rather than forwarding the plugin's nonzero exit code.

Depends on Phase 2.

## References

- [RFD 048: Four-Channel Output Model][RFD 048]: establishes stdout as the data
  channel; this RFD defines the envelope for records on that channel.
- [RFD 050: Scripting Ergonomics for Conversation Management][RFD 050]: defines
  the line-oriented and JSON-array ID output conventions.
- [RFD 086: Line-oriented stdin input for CLI arguments][RFD 086]: defines the
  CLI pipelines that consume JP's machine output.
- [RFD 029: Scriptable Structured Output][RFD 029]: explains why `jp query`
  emits JSON schema results outside `OutputFormat::Json`.
- [RFD 085: Query Explain][RFD 085]: defines `QueryPreview`, whose inner
  `schema_version` field is superseded by this envelope.
- [RFD 072: Command Plugin System][RFD 072] and [RFD D19: Structured Plugin Help
  Protocol][RFD D19]: define the plugin protocol where plugin format addresses
  would be added.
- [RFD D15: Structured Logging Infrastructure][RFD D15] and [RFD D32: JP Tracing
  Infrastructure][RFD D32]: own the observability channel, including the
  log-path announcement and trace event schemas.
- [notmuch structured output versioning]: prior art for `--format-version` and
  exit codes `20` and `21`.
- [`cargo metadata` compatibility]: prior art for the additive payload
  compatibility policy.

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
[ubiquitous language]: ../../wiki/00dxybd-ubiquitous-language.md
