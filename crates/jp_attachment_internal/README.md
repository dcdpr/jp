# jp_attachment_internal

Attachment handler for JP-internal resources, accessed via the `jp://` scheme.

The handler dispatches on the variant character in the JP ID. Each resource
type owns its own set of query parameters.

## URL scheme

```
jp://<jp-id>[?<query>]
```

## Supported resources

### Conversations (`jp-c…`)

Attach the contents of another conversation.

#### Query parameters

| Parameter | Values | Default | Effect |
|---|---|---|---|
| `select` | DSL `CONTENT[:RANGE]` | `a:-1` | Filters which events to include |
| `raw` | (absent), `events`, `all` | absent | Toggles JSON output |

#### Selector DSL

`CONTENT[:RANGE]`

##### CONTENT

One or more of these joined with `,`:

- `a` — assistant messages (default)
- `u` — user messages
- `r` — reasoning blocks
- `t` — tool calls (request + response pairs)
- `*` — shorthand for `a,u,r,t`

##### RANGE

Selects which turns to include. 1-based; negative values count from the end.

- (omitted) — last turn only (`-1`)
- `N` — turn `N`
- `-N` — last `N` turns
- `N..M` — turns `N` through `M` (inclusive)
- `N..` — from turn `N` to the end
- `..M` — first `M` turns
- `-N..` — last `N` turns (same as `-N`)
- `..` — all turns

#### Raw mode

Without `raw`, the conversation is rendered as markdown with turn/role headers.
With `raw`, the same selected events are returned as JSON, using the same event
shape as persisted conversation events.

- `?raw` — selected events only
- `?raw=all` — selected events plus `base_config` and `metadata` fields

The selector applies to `events`. It does not affect `base_config` or
`metadata`.

#### Examples

```
jp://jp-c17013123456                            # last assistant response
jp://jp-c17013123456?select=a                   # last assistant response (explicit)
jp://jp-c17013123456?select=u,a:-1              # last turn, both sides
jp://jp-c17013123456?select=a:-3..              # last three assistant responses
jp://jp-c17013123456?select=*:..                # entire conversation
jp://jp-c17013123456?select=a:5                 # turn 5 only (assistant message)
jp://jp-c17013123456?raw                        # selected events as JSON
jp://jp-c17013123456?raw=all                    # events + base_config + metadata
jp://jp-c17013123456?select=a:-1&raw            # JSON events filtered to last assistant
```

#### CLI shorthand

The CLI parser (`--attach`) accepts these abbreviated forms:

```
jp-c17013123456                                 # → jp://jp-c17013123456
jp-c17013123456?a:-1                            # → jp://jp-c17013123456?select=a:-1
jp-c17013123456?select=a:-1                     # → jp://jp-c17013123456?select=a:-1
jp-c17013123456?raw                             # → jp://jp-c17013123456?raw
jp-c17013123456?raw=all                         # → jp://jp-c17013123456?raw=all
```

A bare value after `?` (e.g. `a:-1`) becomes the value of an implicit
`select=`. If the suffix already names a known parameter (`select` or `raw`),
it's passed through verbatim.

### Other variants

Reserved for future use. Today, requesting any non-conversation variant
(e.g. `jp-w…` for workspaces) returns a clear error. Adding new resources
is a single dispatch arm in `lib.rs` plus a renderer.

## Output

Each entry resolves to one or more text attachments. The source field of each
attachment encodes the canonical ID and what the attachment contains, so the
assistant always knows which slice of which resource it received.
