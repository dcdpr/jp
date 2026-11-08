# RFD D11: VFS Tool Protocol

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-01
- **Requires**: [RFD D09], [RFD D10], [RFD 075], [RFD 058], [RFD 009]
- **Summary**: Runs `runtime = "vfs"` tool subprocesses inside an OS sandbox
  that mediates all host I/O over stdio JSON-RPC.

## Summary

This RFD defines the `runtime = "vfs"` tool execution mode, where an
OS-sandboxed subprocess performs all filesystem, network, and process operations
over a stdin/stdout JSON-RPC protocol.
The protocol mirrors the `jp:host/filesystem`, `jp:host/http`, and
`jp:host/process` interfaces from [RFD 016] over stdio instead of WASM imports,
and the host resolves every request through `ProjectFiles` ([RFD D09]) and
`AccessPolicy` ([RFD 075]).

## Motivation

JP's default `runtime = "stdio"` tools run as subprocesses with direct access to
the host filesystem, network, and process table.
[RFD 075] restricts these subprocesses with OS-level sandboxing, but OS
enforcement is coarse-grained and platform-dependent: `sandbox-exec` on macOS is
deprecated, Landlock on Linux requires kernel 5.13+, and Windows restricted
tokens cannot easily express path-prefix allowlists.

[RFD 016] solves this for WASM plugins: WASM components have no ambient
capabilities, and all host interaction goes through typed `jp:host/*` imports
checked against a sandbox policy.
Compiling a tool to a WASM component, however, imposes a significant authoring
burden on tools that are otherwise simple native executables.

`runtime = "vfs"` sits between these two models.
We want tools that:

1. Compile as native executables rather than WASM components.
2. Have no ambient access to the filesystem, network, or process table.
3. Access host resources through JP's mediated API under the same access policy
   as WASM plugins.
4. Run unchanged against any `ProjectFiles` backend (real filesystem, in-memory,
   browser storage, or database).

Under `runtime = "vfs"`, the tool runs as a subprocess inside a restrictive OS
sandbox ([RFD 075]) that blocks direct system access and forces all I/O through
a JSON-RPC protocol over stdin and stdout.
JP mediates every request through `ProjectFiles` and `AccessPolicy`, providing
WASM's policy enforcement and backend independence without the WASM toolchain.

### Why not just use WASM?

WASM is the ideal long-term model for sandboxed tools, but it has practical
barriers:

- **Toolchain friction.** Building a WASM component requires `cargo component`,
  `wit-bindgen`, and familiarity with the component model.
  JP's existing tools are shell commands or small Rust binaries, so the jump in
  complexity is steep.
- **Ecosystem maturity.** Many Rust crates do not target `wasm32-wasip2`.
  Tools that depend on `tokio`, `reqwest`, or OS-specific APIs cannot compile as
  WASM components.
- **Binary size.** `wasmtime` adds ~15-20 MB to the JP binary, a fixed cost
  until `wasmi` supports the component model.
- **Debugging.** WASM stack traces and debuggers are less mature than native
  tooling.

VFS tools are native executables that can use any crate and standard debuggers.
The trade-off is a less hermetic sandbox (OS-level enforcement rather than
WASM's architectural isolation), though the access policy enforces the same
logical permissions for cooperative tools.

### Why not just use `runtime = "stdio"` with OS sandboxing?

OS sandboxing is a coarse enforcement layer: it can block filesystem access, but
it cannot mediate individual operations.
For example, `sandbox-exec` cannot allow reads under `src/` while denying reads
to `.env` with path-level granularity.
Landlock comes closer on Linux, but network restrictions require kernel 6.7+ and
subprocess controls remain limited.

The VFS protocol enforces policy per request: the host checks each `read`,
`write`, `http_get`, or `run` call against the tool's `SandboxConfig` before
executing it.
The OS sandbox prevents the tool from bypassing the protocol, while the protocol
enforces the fine-grained policy.

## Design

### Protocol overview

The VFS protocol is a bidirectional JSON-RPC exchange over the tool subprocess's
stdin (host → tool) and stdout (tool → host).
JP captures the tool's stderr separately for diagnostics.

```text
┌──────────┐         stdin          ┌──────────┐
│          │ ◄───── responses ───── │          │
│   Tool   │                        │    JP    │
│ (subprocess)                      │  (host)  │
│          │ ───── requests ──────► │          │
│          │         stdout         │          │
└──────────┘                        └──────────┘
```

The tool writes JSON-RPC requests to stdout.
JP reads each request, checks the access policy, resolves the operation through
`ProjectFiles`, and writes the JSON-RPC response to the tool's stdin.
When finished, the tool writes a final result notification and exits.

This flow inverts the usual stdio JSON-RPC server model, where the subprocess
reads requests from stdin and writes responses to stdout.
Here, the tool is the client requesting host capabilities, and JP is the host
fulfilling them.

### Message framing

Messages use newline-delimited JSON (NDJSON): each message is a single-line JSON
object terminated by `\n`.

```text
{"jsonrpc":"2.0","id":1,"method":"read","params":{"path":"src/main.rs"}}\n
```

Compared to Content-Length framing (used by LSP and MCP's stdio transport),
NDJSON:

- Uses a single `BufRead::read_line` call on the host and `println!` in the
  tool.
- Needs no HTTP-style header parser.
- Handles binary payloads without framing issues because binary data is
  base64-encoded inside JSON strings.

NDJSON messages cannot contain unescaped newlines, which JSON serialization
already guarantees.

### Request and response format

The protocol follows JSON-RPC 2.0 without batch requests: each message is a
single request, response, or notification.

**Request** (tool → host, via stdout):

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "read",
  "params": { "path": "src/main.rs" }
}
```

**Success response** (host → tool, via stdin):

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": { "content": "fn main() {}" }
}
```

**Error response** (host → tool, via stdin):

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "error": { "code": -32001, "message": "Access denied: path '.env' is in the sensitive paths list" }
}
```

**Final result** (tool → host, via stdout):

```json
{
  "jsonrpc": "2.0",
  "method": "result",
  "params": { "content": [{ "type": "text", "text": "Modified 2 files." }] }
}
```

The `result` method is a JSON-RPC notification (no `id` field) that signals
completion before the process exits.
`params.content` uses the typed content block array from [RFD 058], or a plain
string that JP wraps in a single `text` block:

```json
{
  "jsonrpc": "2.0",
  "method": "result",
  "params": { "content": "Modified 2 files." }
}
```

**Error result** (tool → host, via stdout):

```json
{
  "jsonrpc": "2.0",
  "method": "error",
  "params": { "message": "Failed to parse input", "trace": ["..."], "transient": false }
}
```

The `error` notification reports a tool failure; its `params` fields match
`ToolError` from [RFD 009].

### Protocol methods

The protocol exposes three capability groups matching the `jp:host` interfaces
from [RFD 016], with method names of the form `{capability}.{operation}`.

#### Filesystem methods

The `ProjectFiles` trait ([RFD D09]) backs all `fs.*` methods.
All paths are relative to the project root; the host rejects absolute paths and
`..` traversal.

##### `fs.read`

Read a file's contents.

```json
// Request
{ "method": "fs.read", "params": { "path": "src/main.rs" } }

// Response
{ "result": { "content": "fn main() {}", "size": 13 } }
```

The host checks whether the file is valid UTF-8.
Text files are returned as plain strings with no `encoding` field; non-UTF-8
files are base64-encoded with `"encoding": "base64"`:

```json
{ "result": { "content": "iVBORw0KGgo=", "encoding": "base64", "size": 1234 } }
```

##### `fs.write`

Write content to a file, creating parent directories as needed.

```json
// Request
{ "method": "fs.write", "params": { "path": "src/main.rs", "content": "fn main() { println!(\"hello\"); }" } }

// Response
{ "result": {} }
```

For binary content:

```json
{ "method": "fs.write", "params": { "path": "image.png", "content": "iVBORw0KGgo=", "encoding": "base64" } }
```

##### `fs.exists`

Check whether a path exists.

```json
{ "method": "fs.exists", "params": { "path": "src/main.rs" } }

{ "result": { "exists": true } }
```

##### `fs.list_dir`

List the immediate entries of a directory (non-recursive).

```json
{ "method": "fs.list_dir", "params": { "path": "src" } }

{ "result": { "entries": [
  { "path": "main.rs", "kind": "file" },
  { "path": "lib.rs", "kind": "file" },
  { "path": "cmd", "kind": "dir" }
] } }
```

##### `fs.metadata`

Get metadata for a path.

```json
{ "method": "fs.metadata", "params": { "path": "src/main.rs" } }

{ "result": { "kind": "file", "size": 1234 } }
```

##### `fs.delete`

Delete a file.

```json
{ "method": "fs.delete", "params": { "path": "tmp/scratch.txt" } }

{ "result": {} }
```

##### `fs.rename`

Rename or move a file.

```json
{ "method": "fs.rename", "params": { "from": "old.rs", "to": "new.rs" } }

{ "result": {} }
```

##### `fs.grep`

Search file contents for a pattern.
`fs.grep` is a dedicated method rather than a client-side composition of
`fs.list_dir` and `fs.read` so `ProjectFiles` implementations can optimize it
directly (for example, ripgrep-style parallel search on a local filesystem or
indexed search on a database backend).

```json
{ "method": "fs.grep", "params": {
  "pattern": "fn main",
  "paths": ["src"],
  "extensions": ["rs"],
  "context": 2
} }

{ "result": { "matches": [
  { "path": "src/main.rs", "lines": [
    { "line_number": 1, "content": "fn main() {", "is_match": true },
    { "line_number": 2, "content": "    println!(\"hello\");", "is_match": false },
    { "line_number": 3, "content": "}", "is_match": false }
  ] }
] } }
```

Only `pattern` is required.
When omitted, `paths` defaults to the entire project, `extensions` matches all
files, and `context` defaults to `0`.

#### HTTP methods

JP's HTTP client backs the `http.*` methods.
Before sending a request, the access policy checks the target URL against the
tool's `sandbox.network.allow` configuration.

##### `http.get`

```json
{ "method": "http.get", "params": {
  "url": "https://api.github.com/repos/owner/repo",
  "headers": [
    { "name": "Authorization", "value": "Bearer ${GITHUB_TOKEN}" }
  ]
} }

{ "result": { "status": 200, "body": "{\"id\": 123, ...}" } }
```

Header values support `${VAR}` substitution: if `VAR` is listed in the tool's
`sandbox.network.envs` config, the host substitutes the value from its own
environment without exposing the resolved secret to the tool.
Unknown or disallowed variables return an error response.
Binary response bodies are base64-encoded with `"encoding": "base64"`.

##### `http.post`

```json
{ "method": "http.post", "params": {
  "url": "https://api.example.com/data",
  "headers": [{ "name": "Content-Type", "value": "application/json" }],
  "body": "{\"key\": \"value\"}"
} }

{ "result": { "status": 201, "body": "{\"id\": 456}" } }
```

#### Process methods

Process methods spawn subprocesses on the host after checking the program name
and arguments against `sandbox.commands`.

##### `process.run`

Run a command and return its output.

```json
{ "method": "process.run", "params": {
  "program": "cargo",
  "args": ["check", "--message-format=json"],
  "envs": ["CARGO_TERM_COLOR"]
} }

{ "result": {
  "stdout": "...",
  "stderr": "...",
  "exit_code": 0
} }
```

The subprocess starts with a clean environment and `cwd` set to the project root
(resolved through `ProjectFiles`).
The host forwards only variables listed in `envs` and permitted by the command's
allowlist (such as `sandbox.commands.cargo.envs`).
Before returning the response, the host scrubs resolved environment variable
values from `stdout` and `stderr`, replacing them with `[REDACTED]` ([RFD 016]).

### Initial context

When the VFS runtime spawns the tool, it writes an `init` notification to the
tool's stdin before the request-response loop begins.
This supplies the context that `StdioRuntime` passes via command-line arguments
and template rendering:

```json
{
  "jsonrpc": "2.0",
  "method": "init",
  "params": {
    "tool": {
      "name": "fs_modify_file",
      "arguments": { "path": "src/main.rs", "patterns": [...] },
      "answers": {},
      "options": { "confirmation_mode": true }
    },
    "protocol_version": "0.1.0"
  }
}
```

`init` is a JSON-RPC notification (no `id` and no response).
The tool reads `init`, issues requests as needed, and finishes with a `result`
or `error` notification.
The `protocol_version` field lets the tool check the host's protocol version and
degrade gracefully when versions differ.

### Tool lifecycle

A VFS tool invocation follows this sequence:

```text
1. JP spawns the subprocess under OS sandbox (RFD 075)
2. JP writes `init` notification to stdin
3. Tool reads init, begins work
4. Tool writes request to stdout (e.g., fs.read)
5. JP reads request, checks access policy, resolves through ProjectFiles
6. JP writes response to stdin
7. Steps 4-6 repeat as needed
8. Tool writes `result` or `error` notification to stdout
9. Tool exits
10. JP reads the final notification, maps to ExecutionOutcome
```

A typical one-shot tool issues a few requests before returning a result; a bulk
refactoring tool may issue hundreds of requests within the same invocation.

### Cancellation

When the user cancels a tool execution (Ctrl+C), JP writes a `cancel`
notification to the tool's stdin:

```json
{ "jsonrpc": "2.0", "method": "cancel" }
```

On receiving `cancel`, the tool should clean up and exit.
Because the notification is best-effort (a tool blocked on computation may not
read stdin), JP waits for a configurable grace period (default: 5 seconds)
before sending SIGTERM, followed by SIGKILL after a second timeout.

### Integration with the stateful tool protocol

[RFD 009] defines the stateful tool lifecycle: `spawn` → `fetch` → `apply` →
`abort`.

For **one-shot VFS tools** (no `action` field in arguments), JP spawns the
process, writes `init`, fulfills requests until the tool writes `result`, and
maps the outcome to `ExecutionOutcome::Completed`.
This matches `StdioRuntime`, but with mediated I/O.

For **stateful VFS tools** (tools that declare stateful support per [RFD 009]),
the subprocess and its IPC channel remain open across `fetch`/`apply` cycles.
Each `apply` from the assistant becomes an `apply` notification on the tool's
stdin:

```json
{ "jsonrpc": "2.0", "method": "apply", "params": { "input": "y" } }
```

The tool processes the input, issues any needed host requests, and writes a
`state` notification:

```json
{ "jsonrpc": "2.0", "method": "state", "params": { "type": "running", "content": "Next hunk: ..." } }
```

Or, if the tool needs structured input:

```json
{ "jsonrpc": "2.0", "method": "state", "params": { "type": "waiting", "content": "Confirm?", "question": { ... } } }
```

The `state` notification uses the `ToolState` types from [RFD 009], which JP
maps to the handle registry.
When the tool finishes:

```json
{ "jsonrpc": "2.0", "method": "state", "params": { "type": "stopped", "result": "Staged 3 hunks." } }
```

### `VfsRuntime` implementation

`VfsRuntime` implements `ToolRuntime` ([RFD D10]) and holds the dependencies
required to serve protocol requests:

```rust
pub struct VfsRuntime {
    project: Arc<dyn ProjectFiles>,
    policy: Arc<AccessPolicy>,
    http_client: reqwest::Client,
}
```

The `execute` method:

1. Resolves `MaterializedView` from `project` (for `process.run` `cwd`).
2. Generates the OS sandbox profile from the tool's `SandboxConfig` ([RFD 075]).
3. Spawns the subprocess under the sandbox with stdin and stdout piped.
4. Writes the `init` notification.
5. Runs the request-response loop, reading requests from stdout, fulfilling
   them, and writing responses to stdin.
6. Maps a `result` or `error` notification to `ExecutionOutcome` and returns.
7. If the process exits without a final notification, returns
   `ExecutionOutcome::Completed` with the process's stderr as an error message.

The request-response loop runs on a dedicated async task using
`tokio::io::BufReader` and `tokio::io::BufWriter`, with the cancellation token
wired to abort the loop.

### Access policy enforcement

Every protocol request passes through the tool's `AccessPolicy`, which is
derived from `SandboxConfig` ([RFD 075]):

```rust
pub struct AccessPolicy {
    filesystem: FilesystemSandbox,
    network: NetworkSandbox,
    commands: HashMap<String, CommandRule>,
    sensitive_paths: Vec<String>,
}

impl AccessPolicy {
    fn check_fs_read(&self, path: &str) -> Result<(), PolicyDenied>;
    fn check_fs_write(&self, path: &str) -> Result<(), PolicyDenied>;
    fn check_http(&self, url: &str) -> Result<(), PolicyDenied>;
    fn check_process(&self, program: &str, args: &[String]) -> Result<(), PolicyDenied>;
}
```

When a policy check fails, the host returns a JSON-RPC error response explaining
what was denied and why, which the tool can include in its result.

When a request is neither explicitly allowed nor denied by `SandboxConfig`, the
host prompts the user following the inquiry model in [RFD 075].
The request blocks until the user responds: approval executes the request, and
denial returns an error response.

### Deadlock prevention

The bidirectional stdio protocol risks deadlock if both OS pipe buffers fill at
once, blocking the tool on a stdout write while JP blocks on a stdin write.
Four mechanisms prevent this:

1. **Separate read and write tasks.** JP reads the tool's stdout and writes to
   its stdin on independent async tasks, so host reads and writes never block
   each other.

2. **Payload size limit.** File content is the largest payload.
   When a file exceeds a configurable threshold (default: 10 MB), the host
   returns an error instead of serializing the file into JSON.
   Tools that process larger files should read them in chunks or run a streaming
   command via `process.run`.

3. **Request timeout.** If the tool writes neither a request nor a final result
   within a configurable timeout (default: 60 seconds), JP treats the process as
   hung and kills it.

4. **Response timeout.** If JP does not respond within 30 seconds (for example,
   during a slow `ProjectFiles` operation or an unanswered inquiry prompt), the
   tool can write a `cancel` request to abandon the pending operation.

### Error codes

The protocol uses standard JSON-RPC error codes (-32600 through -32603) for
protocol errors and application-specific codes starting at -32001:

| Code   | Name             | Meaning                                       |
| ------ | ---------------- | --------------------------------------------- |
| -32600 | Invalid Request  | Malformed JSON-RPC message                    |
| -32601 | Method Not Found | Unknown method name                           |
| -32602 | Invalid Params   | Missing or invalid parameters                 |
| -32001 | Access Denied    | Policy check failed                           |
| -32002 | Not Found        | File or resource does not exist               |
| -32003 | Already Exists   | Write target already exists (when applicable) |
| -32004 | Timeout          | Operation timed out                           |
| -32005 | Cancelled        | Operation cancelled by host or user           |

### Configuration

Set `runtime = "vfs"` in the tool config to run a tool under the VFS runtime:

```toml
[tools.smart_editor]
source = "local"
command = ".config/jp/tools/target/release/jp-tools fs modify_file"
runtime = "vfs"

[tools.smart_editor.sandbox]
filesystem.allow = ["."]
filesystem.writable = true
```

The `sandbox` section ([RFD 075]) governs both the protocol's `AccessPolicy` and
the OS-level sandbox ([RFD 075] Phase 2-4) that prevents the subprocess from
bypassing the protocol.

A VFS tool without a `sandbox` section gets the default policy: read-only
workspace access, no network access, and no subprocess spawning.
This policy suffices for read-only tools such as `grep_files`, `list_files`, and
`read_file`.

### Tool SDK support

JP's tool SDK (`jp_tool` crate) provides a `VfsHost` client that wraps the
JSON-RPC protocol:

```rust
// Tool author's code
use jp_tool::vfs::{self, VfsHost};

fn main() {
    let host = vfs::connect(); // reads init from stdin

    let content = host.read("src/main.rs").unwrap();
    // ... modify content ...
    host.write("src/main.rs", &modified).unwrap();

    host.result(vec![
        ContentBlock::text("Modified src/main.rs"),
    ]);
}
```

`VfsHost` methods (`read`, `write`, `list_dir`, and others) send a request on
stdout and block until the response arrives on stdin; `result` writes the final
notification and returns.
Tools in other languages can implement the protocol directly by reading and
writing NDJSON over stdio.

## Drawbacks

- **Protocol overhead.** Every file read and write is a JSON-RPC round-trip over
  stdin/stdout.
  `fs.grep` avoids per-file round-trips for search, but tools that read hundreds
  of individual files are slower than their `stdio` equivalents.

- **Binary content encoding.** Base64 encoding inflates binary files by ~33%.
  That overhead is fine for source code, configuration, and text documents, but
  inefficient for large binary files.

- **Complexity for tool authors.** Writing a VFS tool requires using the SDK or
  implementing the JSON-RPC protocol directly, unlike `runtime = "stdio"` where
  a shell script running `cat file.txt` works directly.

- **Two runtime code paths.** `StdioRuntime` and `VfsRuntime` implement the
  shared `ToolRuntime` trait ([RFD D10]) separately, so shared behaviors such as
  argument validation, cancellation, and result parsing must stay in sync across
  both implementations.

- **Inquiry latency.** When a request triggers an inquiry prompt for an action
  not covered by `SandboxConfig`, the tool blocks until the user responds.
  In `--no-interaction` mode, the host denies those requests immediately, which
  can cause tools that assume interactive approval to fail.

## Alternatives

### Content-Length framing (LSP/MCP style)

Use `Content-Length: N\r\n\r\n` headers instead of NDJSON, as LSP and MCP's
stdio transport do.

Rejected.
Content-Length framing exists to allow raw newlines in payloads, which JSON-RPC
objects already escape.
NDJSON is simpler to implement (a single `read_line` call) and easier to inspect
in terminal output.

### gRPC or Cap'n Proto

Use a binary RPC protocol for lower serialization overhead and schema
enforcement.

Rejected.
The protocol is internal to JP and its tools, and a typical invocation exchanges
only 1 to 50 requests.
At that volume, JSON-RPC's simplicity, debuggability, and cross-language support
outweigh the performance of binary protocols.
If profiling shows JSON serialization to be a bottleneck, a binary framing layer
can be added later without changing the logical protocol.

### FUSE-based virtual filesystem

Mount a FUSE filesystem so tools use standard filesystem I/O while JP mediates
every operation.

Rejected.
FUSE is Linux/macOS only (no Windows support), requires elevated privileges on
some systems, adds kernel module and mount lifecycle complexity, and performs
poorly on metadata-heavy workloads.
The stdio IPC protocol is portable and requires no kernel support.

### Shared memory for large files

Use shared memory (`mmap`, `shm_open`) to transfer large file contents without
JSON serialization overhead.

Rejected as premature optimization.
Expected payloads are source files of kilobytes to low megabytes, and text files
pay zero base64 overhead in JSON strings.
Shared memory adds platform-specific complexity and security risks (a tool
reading past its allocated region).
If large binary file transfers become a bottleneck, a future RFD can add shared
memory as an optional transport optimization.

### Make `runtime = "vfs"` the default

Default local tools to VFS, with `runtime = "stdio"` as an opt-out for tools
that need direct filesystem access.

Rejected.
Defaulting to VFS breaks plain shell scripts: a tool running `cat file.txt`
fails when the subprocess cannot access the filesystem directly.
`stdio` remains the default for backward compatibility and simplicity, while VFS
is opt-in for tools that need mediated access.

## Non-Goals

- **Streaming file content.** The protocol transfers complete files in single
  JSON messages rather than chunks or streams.
  Tools that must process files larger than the message size limit should run a
  streaming command via `process.run`.

- **File watching.** The protocol does not push filesystem event notifications.
  Tools that react to file changes should poll or invoke a file watcher via
  `process.run`.

- **Inter-tool communication.** Each VFS tool has an independent IPC channel to
  the host and cannot communicate with other tools.

- **Custom protocol extensions.** The method set is fixed, and tools cannot
  register custom host capabilities.
  Capabilities outside the protocol must use `process.run` or be proposed in a
  future RFD.

- **WASM transport.** This RFD specifies only the stdio transport.
  [RFD 016] defines the WASM host-import transport for the same logical
  capabilities.

## Risks and Open Questions

### Protocol versioning and evolution

The `protocol_version` field in `init` enables version negotiation, but this RFD
does not define a formal versioning policy.
Open questions:

- Can the host add methods in a minor version bump, or is that a major change?
- Should tools declare a required protocol version, or discover available
  methods dynamically?
- How does the host handle requests for unrecognized methods?

Recommended starting policy: the host rejects unknown methods with `Method Not
Found` (-32601), and tools ignore unknown fields in responses.
Minor versions may add methods; removing or incompatibly changing a method
requires a major version bump, following standard JSON-RPC practice.

### Large directory listings

`fs.list_dir` returns all entries in a single unpaginated response, which can
produce large messages for directories with thousands of entries.
If this becomes a problem, optional `offset`/`limit` pagination parameters or
streaming can be added in a backward-compatible way.

### Concurrent requests

The initial protocol is strictly sequential: a tool may have only one in-flight
request at a time.
This keeps the host implementation simple, at the cost of serializing
independent operations such as concurrent file reads.

Because JSON-RPC correlates requests and responses by `id`, a future protocol
version can allow multiple outstanding requests and out-of-order responses
without changing the message format.
Concurrent request handling is deferred until sequential round-trips prove to be
a bottleneck.

### Inquiry prompt blocking

When a request triggers a user inquiry prompt, the tool blocks and may hit its
own timeout before the user responds.
To let the tool adjust its timeout, the host can send a `pending` notification
before prompting:

```json
{ "jsonrpc": "2.0", "method": "pending", "params": { "request_id": 1, "reason": "awaiting user approval" } }
```

On receiving `pending`, the tool can extend its timeout or log a status message
to stderr.
`pending` is omitted from the initial protocol version but noted as a likely
addition.

### Migration path for existing tools

JP's existing local tools are `StdioRuntime` shell commands.
Migrating a tool to `runtime = "vfs"` requires rewriting it to use the IPC
protocol (or the `jp_tool::vfs` SDK) instead of direct file access.
Because both runtimes coexist indefinitely, tools can be migrated one at a time.

## Implementation Plan

### Phase 1: Protocol types and NDJSON framing

Define the JSON-RPC request, response, and notification types in a new
`jp_vfs_protocol` crate (or module within `jp_tool`).
Implement NDJSON reading and writing with `tokio::io::BufReader`/`BufWriter`.
Add unit tests for serialization round-trips and framing edge cases.

**Depends on:** Nothing.
**Mergeable:** Yes.

### Phase 2: Host-side request handler

Implement the host-side request dispatcher that reads requests from the tool's
stdout, resolves them through `ProjectFiles` ([RFD D09]) and `AccessPolicy`
([RFD 075]), and writes responses to stdin.
Implement all `fs.*` methods first, since those are the most commonly needed.

**Depends on:** Phase 1, [RFD D09] Phase 2 (`FsProjectFiles`), [RFD 075] Phase 1
(`SandboxConfig`).
**Mergeable:** Yes.

### Phase 3: `VfsRuntime` implementation

Implement `ToolRuntime` ([RFD D10]) for `VfsRuntime`.
Wire up subprocess spawning under the OS sandbox, the `init` notification, the
request-response loop, and `result`/`error` notification handling, mapping
outcomes to `ExecutionOutcome`.

**Depends on:** Phase 2, [RFD D10] Phase 1 (`ToolRuntime` trait).
**Mergeable:** Yes.

### Phase 4: HTTP and process methods

Add `http.get`, `http.post`, and `process.run` to the host-side dispatcher.
Enforce `AccessPolicy` checks and scrub secrets from `process.run` output.

**Depends on:** Phase 2.
**Mergeable:** Yes (parallel with Phase 3).

### Phase 5: Tool SDK (`jp_tool::vfs`)

Implement the `VfsHost` client library in `jp_tool` with synchronous wrappers
for all protocol methods.
Add integration tests that exercise a real tool subprocess against the host.

**Depends on:** Phase 3.
**Mergeable:** Yes.

### Phase 6: Migrate one tool as proof of concept

Migrate a single tool (candidate: `fs_read_file` or `fs_list_files`) from
`runtime = "stdio"` to `runtime = "vfs"`.
Validate that the tool produces identical results and benchmark the performance
difference.

**Depends on:** Phase 5.
**Mergeable:** Yes.

### Phase 7: Stateful VFS tool support

Add `apply`, `cancel`, and `state` notification handling to the host-side loop
and wire it into the handle registry from [RFD 009].
Add an integration test with a stateful VFS tool.

**Depends on:** Phase 3, [RFD 009] implementation.
**Mergeable:** Yes.

## References

- [RFD 009]: Stateful tool protocol.
  Defines `ToolState`, the spawn/fetch/apply/abort lifecycle, and the handle
  registry that VFS tools integrate beneath.
- [RFD 016]: WASM plugin architecture.
  Defines `jp:host/filesystem`, `jp:host/http`, and `jp:host/process`, the
  logical capabilities this protocol exposes over stdio.
- [RFD 058]: Typed content blocks for tool responses, used by the `result`
  notification.
- [RFD 075]: Tool sandbox and access policy.
  Provides the OS-level sandbox that prevents VFS tools from bypassing the
  protocol, and the `SandboxConfig` / `AccessPolicy` types that govern
  per-request policy enforcement.
- [RFD D09]: Project filesystem abstraction.
  The `ProjectFiles` trait backs all `fs.*` protocol methods.
- [RFD D10]: Unified tool execution model.
  `VfsRuntime` implements the `ToolRuntime` trait defined there.
- [JSON-RPC 2.0 Specification][jsonrpc]

[RFD 009]: ../009-stateful-tool-protocol.md
[RFD 016]: ../016-wasm-plugin-architecture.md
[RFD 058]: ../058-typed-content-blocks-for-tool-responses.md
[RFD 075]: ../075-tool-sandbox-and-access-policy.md
[RFD D09]: D09-project-filesystem-abstraction.md
[RFD D10]: D10-unified-tool-execution-model.md
[jsonrpc]: https://www.jsonrpc.org/specification
