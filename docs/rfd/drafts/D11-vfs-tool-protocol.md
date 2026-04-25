# RFD D11: VFS Tool Protocol

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-01

## Summary

This RFD specifies the IPC protocol and sandboxed execution model for
`runtime = "vfs"` tools. A VFS tool is a subprocess that performs all host
interaction — filesystem access, network requests, subprocess spawning — through
a JSON-RPC protocol over stdin/stdout rather than direct system calls. The
protocol exposes the same logical capabilities as the `jp:host/filesystem`,
`jp:host/http`, and `jp:host/process` interfaces from [RFD 016], but over a
stdio transport instead of WASM host imports. The host side resolves every
request through the `ProjectFiles` trait from [RFD D09] and the access policy
system from [RFD 075]. Combined with OS-level sandboxing, the subprocess has no
direct access to the filesystem or network, making the IPC protocol the only
path to host resources.

## Motivation

JP's `runtime = "stdio"` tools (the current default) run as subprocesses with
direct access to the filesystem and whatever else the OS allows. [RFD 075]
introduces OS-level sandboxing to restrict what these subprocesses can reach,
but OS-level enforcement is coarse-grained and platform-dependent:
`sandbox-exec` on macOS is deprecated, Landlock on Linux requires kernel 5.13+,
and Windows restricted tokens cannot easily express path-prefix allowlists.

[RFD 016] solves this for WASM plugins by design: WASM components have no
ambient capabilities, and all host interaction goes through typed
`jp:host/*` imports checked against a sandbox policy. But WASM requires
compiling tools to a WASM component, which is a significant authoring burden
for tools that are naturally written as native executables.

There is a gap between these two extremes. We want tools that:

1. Are written as normal native programs (not WASM components).
2. Have no ambient access to the filesystem, network, or process table.
3. Access host resources through JP's mediated API, subject to the same access
   policy as WASM plugins.
4. Work identically against any `ProjectFiles` backend — real filesystem,
   in-memory, browser storage, database.

The `runtime = "vfs"` model fills this gap. The tool is still a subprocess, but
it runs under a restrictive OS sandbox ([RFD 075]) that blocks direct system
access, forcing all I/O through a JSON-RPC protocol on stdin/stdout. JP
mediates every request through `ProjectFiles` and the access policy, giving the
same security and backend-agnosticism guarantees as WASM — without the WASM
toolchain.

### Why not just use WASM?

WASM is the ideal long-term solution for sandboxed tools, but it has practical
barriers today:

- **Toolchain friction.** Building a WASM component requires `cargo component`,
  `wit-bindgen`, and familiarity with the component model. JP's current tools
  are shell commands or simple Rust binaries — the gap is large.
- **Ecosystem maturity.** Many Rust crates don't compile to `wasm32-wasip2` yet.
  Tools that depend on `tokio`, `reqwest`, or OS-specific APIs cannot be WASM
  components today.
- **Binary size.** `wasmtime` adds ~15-20 MB to the JP binary. Until `wasmi`
  gains component model support, this is a fixed cost.
- **Debugging.** WASM stack traces and debugging tooling are less mature than
  native debugging.

VFS tools avoid all of these: they are native executables, debugged with
standard tools, using any crate they want. The cost is a less hermetic sandbox
(OS-level enforcement vs. WASM's architectural isolation), but the access
policy layer provides equivalent logical security for cooperative tools.

### Why not just use `runtime = "stdio"` with OS sandboxing?

Because OS sandboxing is an all-or-nothing enforcement layer. It can block
filesystem access, but it cannot mediate it — there is no way for
`sandbox-exec` to say "allow reads to `src/` but deny reads to `.env`" with
path-level granularity. Landlock comes closer, but network restrictions require
kernel 6.7+ and subprocess restrictions are limited.

The VFS protocol provides fine-grained, per-request policy enforcement. Each
`read`, `write`, `http_get`, or `run` request is checked against the tool's
`SandboxConfig` before execution. The OS sandbox is the backstop that prevents
the tool from bypassing the protocol; the protocol is the policy enforcement
layer.

## Design

### Protocol overview

The VFS protocol is a bidirectional JSON-RPC exchange over the tool process's
stdin (host → tool) and stdout (tool → host). The tool sends requests; the host
sends responses. The tool's stderr is captured separately for diagnostics but
is not part of the protocol.

```text
┌──────────┐         stdin          ┌──────────┐
│          │ ◄───── responses ───── │          │
│   Tool   │                        │    JP    │
│ (subprocess)                      │  (host)  │
│          │ ───── requests ──────► │          │
│          │         stdout         │          │
└──────────┘                        └──────────┘
```

The tool writes JSON requests to stdout. JP reads them, resolves through
`ProjectFiles` and the access policy, and writes JSON responses to the tool's
stdin. When the tool is finished, it writes a final result message and exits.

This is the inverse of the typical JSON-RPC server model (where the server
reads from stdin and writes to stdout). The inversion is intentional: the
tool is the *requester* of host capabilities, not the provider. JP is the
host that fulfills those requests.

### Message framing

Messages are newline-delimited JSON (NDJSON). Each message is a single JSON
object followed by a newline (`\n`). This is the simplest framing that avoids
partial-read issues and is widely supported across languages.

```text
{"jsonrpc":"2.0","id":1,"method":"read","params":{"path":"src/main.rs"}}\n
```

NDJSON is chosen over Content-Length framing (used by LSP and MCP's stdio
transport) because:

- It is simpler to implement — `BufRead::read_line` on the host,
  `println!` on the tool.
- It does not require a parser for HTTP-style headers.
- Binary content is base64-encoded in JSON, so newlines in content do not
  break framing.

The trade-off is that messages cannot contain raw newlines — all content must
be JSON-escaped. This is a non-issue for JSON-RPC, where all payloads are valid
JSON objects.

### Request and response format

The protocol follows JSON-RPC 2.0 conventions with one simplification: no
batch requests. Each message is a single request or response.

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

The `result` method is a JSON-RPC notification (no `id` field). It signals that
the tool is done and the process will exit. The `params.content` array uses the
typed content block format from [RFD 058]. Tools that don't use typed content
blocks can return a plain string:

```json
{
  "jsonrpc": "2.0",
  "method": "result",
  "params": { "content": "Modified 2 files." }
}
```

JP accepts both forms: if `params.content` is a string, it is wrapped in a
single `text` block.

**Error result** (tool → host, via stdout):

```json
{
  "jsonrpc": "2.0",
  "method": "error",
  "params": { "message": "Failed to parse input", "trace": ["..."], "transient": false }
}
```

The `error` notification signals that the tool failed. The `params` fields match
`ToolError` from [RFD 009].

### Protocol methods

The protocol exposes three capability groups, matching the `jp:host` interfaces
from [RFD 016]. The method names use the format `{capability}.{operation}`.

#### Filesystem methods

These methods are backed by the `ProjectFiles` trait from [RFD D09]. All paths
are relative to the project root. Absolute paths and `..` traversal are
rejected by the host.

##### `fs.read`

Read a file's contents.

```json
// Request
{ "method": "fs.read", "params": { "path": "src/main.rs" } }

// Response
{ "result": { "content": "fn main() {}", "size": 13 } }
```

For binary files, content is base64-encoded:

```json
{ "result": { "content": "iVBORw0KGgo=", "encoding": "base64", "size": 1234 } }
```

The host determines encoding based on the file's content (UTF-8 validity
check). The `encoding` field is absent for text content and `"base64"` for
binary.

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

List entries in a directory. Non-recursive.

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

Search file contents for a pattern. This is a first-class method (not built
from `list_dir` + `read`) because `ProjectFiles` implementations can optimize
it significantly — ripgrep-style parallel search on a real filesystem, indexed
search on a database backend.

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

All `params` fields except `pattern` are optional. When omitted: `paths`
defaults to the entire project, `extensions` defaults to all files, `context`
defaults to `0`.

#### HTTP methods

These methods are backed by JP's HTTP client. The access policy checks the URL
against the tool's `sandbox.network.allow` configuration.

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

Header values support `${VAR}` substitution: the host replaces `${VAR}` with
the value from its own environment, if the variable is listed in the tool's
`sandbox.network.envs` config. The tool never sees the resolved value. Unknown
or disallowed variables produce an error response.

For binary response bodies, the body is base64-encoded with
`"encoding": "base64"`.

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

These methods allow the tool to spawn subprocesses on the host. The access
policy checks the program name and arguments against `sandbox.commands`.

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

The subprocess runs with a clean environment. Only variables listed in `envs`
(and allowed by the tool's `sandbox.commands.cargo.envs`) are forwarded from
the host. The `cwd` is the project root (resolved through `ProjectFiles`).

The host applies secret scrubbing to `stdout` and `stderr` before returning
them, following the same approach as [RFD 016]: resolved env var values are
replaced with `[REDACTED]`.

### Initial context

When the VFS runtime spawns the tool, it writes an initial context message to
the tool's stdin before the request-response loop begins. This provides the
tool with the same information currently passed via command-line arguments and
template rendering in `StdioRuntime`:

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

The `init` message is a JSON-RPC notification (no `id`, no response expected).
The tool reads it, performs its work by issuing requests, and eventually writes
a `result` or `error` notification.

The `protocol_version` field enables forward-compatible evolution. The tool can
check it and degrade gracefully if the host supports a newer version with
methods the tool doesn't know about.

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

For one-shot tools (the common case), the lifecycle is short — a few requests,
then a result. For tools that do extensive work (a large refactoring tool that
reads and writes many files), the lifecycle may involve hundreds of requests.

### Cancellation

When the user cancels a tool execution (Ctrl+C), JP sends a `cancel`
notification to the tool's stdin:

```json
{ "jsonrpc": "2.0", "method": "cancel" }
```

The tool should clean up and exit promptly. If the tool does not exit within a
timeout (configurable, default 5 seconds), JP kills the process with SIGTERM,
then SIGKILL after another timeout.

The `cancel` notification is best-effort — the tool may not read it if it's
blocked on a long-running computation. The process kill is the hard backstop.

### Integration with the stateful tool protocol

[RFD 009] defines a stateful tool lifecycle: `spawn` → `fetch` → `apply` →
`abort`. VFS tools integrate with this model naturally.

For **one-shot VFS tools** (no `action` field in arguments), the lifecycle is:
JP spawns the process, writes `init`, the tool issues requests and writes a
`result`, and JP maps the result to `ExecutionOutcome::Completed`. This is
identical to a `StdioRuntime` tool, just with mediated I/O.

For **stateful VFS tools** (tool declares stateful support per [RFD 009]), the
process stays alive across `fetch`/`apply` cycles. The IPC channel remains
open. Each `apply` from the assistant translates to an `apply` notification on
the tool's stdin:

```json
{ "jsonrpc": "2.0", "method": "apply", "params": { "input": "y" } }
```

The tool processes the input, issues any necessary requests (reads, writes),
and writes a state update:

```json
{ "jsonrpc": "2.0", "method": "state", "params": { "type": "running", "content": "Next hunk: ..." } }
```

Or, if the tool needs structured input:

```json
{ "jsonrpc": "2.0", "method": "state", "params": { "type": "waiting", "content": "Confirm?", "question": { ... } } }
```

The `state` notification uses the `ToolState` types from [RFD 009]. JP maps
these to the handle registry's state tracking.

When the tool finishes:

```json
{ "jsonrpc": "2.0", "method": "state", "params": { "type": "stopped", "result": "Staged 3 hunks." } }
```

### `VfsRuntime` implementation

`VfsRuntime` implements the `ToolRuntime` trait from [RFD D10]. It captures the
dependencies needed to fulfill protocol requests:

```rust
pub struct VfsRuntime {
    project: Arc<dyn ProjectFiles>,
    policy: Arc<AccessPolicy>,
    http_client: reqwest::Client,
}
```

The `execute` method:

1. Resolves `MaterializedView` from `project` (needed for `process.run` `cwd`).
2. Generates the OS sandbox profile from the tool's `SandboxConfig` ([RFD 075]).
3. Spawns the subprocess under the sandbox with stdin/stdout piped.
4. Writes the `init` notification.
5. Enters the request-response loop, reading requests from stdout, fulfilling
   them, and writing responses to stdin.
6. On `result` or `error` notification, maps to `ExecutionOutcome` and returns.
7. On process exit without a final notification, returns
   `ExecutionOutcome::Completed` with the process's stderr as an error message.

The request-response loop runs on a dedicated async task. JP uses
`tokio::io::BufReader` for reading and `tokio::io::BufWriter` for writing, with
the cancellation token wired to abort the loop.

### Access policy enforcement

Every protocol request passes through the tool's `AccessPolicy` before
execution. The policy is derived from the tool's `SandboxConfig` ([RFD 075]):

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

Denied requests return a JSON-RPC error response with a clear message
explaining what was denied and why. The tool receives the error and can surface
it to the user via its result content.

For requests that are not covered by the sandbox config (neither explicitly
allowed nor denied), the host triggers an inquiry prompt to the user, following
the inquiry model from [RFD 075]. The tool's request blocks until the user
responds. If the user approves, the request proceeds. If denied, the tool
receives an error response.

### Deadlock prevention

The bidirectional stdin/stdout protocol has an inherent deadlock risk: the tool
blocks writing a request to stdout while JP blocks writing a response to stdin,
and neither side makes progress because OS pipe buffers are full.

Mitigations:

1. **Async I/O on the host side.** JP reads from the tool's stdout and writes
   to stdin on separate async tasks. The read and write sides never block each
   other.

2. **Reasonable message sizes.** File content is the largest payload. For files
   larger than a configurable threshold (default: 10 MB), the host returns an
   error rather than attempting to serialize the entire file into a JSON
   message. Tools that need large files should process them in chunks or use
   `process.run` with a streaming command.

3. **Request timeout.** If the tool does not write a request or final result
   within a configurable timeout (default: 60 seconds), JP assumes the tool is
   hung and kills it. This catches infinite loops and unexpected blocking.

4. **Response timeout.** If JP does not write a response within 30 seconds (due
   to a slow `ProjectFiles` backend or a long-running inquiry prompt), the tool
   can write a `cancel` request to abandon the pending operation.

### Error codes

The protocol defines the following JSON-RPC error codes:

| Code | Name | Meaning |
|------|------|---------|
| -32600 | Invalid Request | Malformed JSON-RPC message |
| -32601 | Method Not Found | Unknown method name |
| -32602 | Invalid Params | Missing or invalid parameters |
| -32001 | Access Denied | Policy check failed |
| -32002 | Not Found | File or resource does not exist |
| -32003 | Already Exists | Write target already exists (when applicable) |
| -32004 | Timeout | Operation timed out |
| -32005 | Cancelled | Operation cancelled by host or user |

Standard JSON-RPC error codes (-32600 through -32603) are used for protocol
errors. Application-specific codes start at -32001.

### Configuration

VFS tools are configured with `runtime = "vfs"` in the tool config:

```toml
[tools.smart_editor]
source = "local"
command = ".config/jp/tools/target/release/jp-tools fs modify_file"
runtime = "vfs"

[tools.smart_editor.sandbox]
filesystem.allow = ["."]
filesystem.writable = true
```

The `sandbox` section from [RFD 075] controls what the tool can do through the
protocol. The OS-level sandbox ([RFD 075] Phase 2-4) ensures the tool cannot
bypass the protocol.

A VFS tool with no `sandbox` section gets the default policy: workspace
read-only, no network, no subprocess spawning. This is sufficient for read-only
tools like `grep_files`, `list_files`, and `read_file`.

### Tool SDK support

JP's tool SDK (`jp_tool` crate) will provide a VFS client library that handles
the protocol automatically:

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

The `VfsHost` struct wraps the JSON-RPC protocol. `read`, `write`, `list_dir`,
etc. send requests and block on responses. `result` writes the final
notification and returns.

For tools not written in Rust, the protocol is simple enough to implement
directly: read NDJSON from stdin, write NDJSON to stdout, follow the method
schemas.

## Drawbacks

- **Protocol overhead.** Every file read and write is a JSON-RPC round-trip
  through stdin/stdout. For tools that process many files, this adds latency
  compared to direct filesystem access. The `fs.grep` method mitigates this for
  the most common bulk operation, but tools that read hundreds of individual
  files will be slower than their `stdio` equivalents.

- **Binary content encoding.** Binary files must be base64-encoded in JSON
  messages, inflating their size by ~33%. This is acceptable for the expected
  use case (source code, config files, text documents) but inefficient for
  tools that process large binary files.

- **Complexity for tool authors.** Writing a VFS tool requires understanding the
  JSON-RPC protocol and using the SDK (or implementing the protocol manually).
  This is more work than writing a shell script that reads files directly. The
  SDK reduces this burden, but there is still a gap compared to `runtime =
  "stdio"` where `cat file.txt` just works.

- **Two runtime code paths.** `StdioRuntime` and `VfsRuntime` are separate
  implementations of `ToolRuntime`. Both must be maintained, tested, and kept
  in sync for shared behaviors (argument validation, cancellation, result
  parsing). The `ToolRuntime` trait from [RFD D10] provides the shared
  interface, but the internal logic diverges.

- **Inquiry latency.** When a protocol request triggers an inquiry prompt (an
  action not covered by the sandbox config), the tool blocks until the user
  responds. For interactive use this is fine, but in `--no-interaction` mode the
  request must be denied immediately, which may cause the tool to fail in ways
  it wasn't designed for.

## Alternatives

### Content-Length framing (LSP/MCP style)

Use `Content-Length: N\r\n\r\n` headers instead of NDJSON. This is what LSP
and MCP's stdio transport use.

Rejected because it adds parsing complexity for minimal benefit. The only
advantage of Content-Length framing is supporting raw newlines in payloads, but
JSON-RPC payloads are JSON objects where newlines are escaped. NDJSON is simpler
to implement (one `read_line` call) and debug (messages are human-readable in
terminal output).

### gRPC or Cap'n Proto

Use a binary RPC protocol for lower overhead and schema enforcement.

Rejected because the protocol is internal to JP and its tools. The message
count per tool invocation is small (typically 1-50 requests). JSON-RPC's
simplicity, debuggability, and cross-language support outweigh the performance
benefits of binary protocols at this scale. If profiling shows JSON
serialization as a bottleneck, a binary framing layer can be added later
without changing the logical protocol.

### FUSE-based virtual filesystem

Mount a FUSE filesystem that mediates access. Tools see a real filesystem and
use standard I/O, but all operations go through JP's VFS layer.

Rejected because FUSE is Linux/macOS only (no Windows support), requires
elevated privileges on some configurations, adds significant complexity (kernel
module interaction, mount lifecycle management), and has known performance
issues for metadata-heavy workloads. The IPC protocol is simpler, portable, and
does not require kernel support.

### Shared memory for large files

Use shared memory (`mmap`, `shm_open`) to transfer large file contents without
JSON serialization overhead.

Rejected as premature optimization. The expected payloads are source code files
(kilobytes to low megabytes). The base64 overhead for text files is zero (text
is transmitted as-is in JSON strings). Shared memory adds platform-specific
complexity and security considerations (the tool could read beyond its allocated
region). If large binary file transfer becomes a bottleneck, this can be added
as an optional transport optimization in a future RFD.

### Make `runtime = "vfs"` the default

Default to VFS for all local tools, with `runtime = "stdio"` as the opt-out
for tools that need direct filesystem access.

Rejected because it would break the "shell script just works" property. A tool
that does `cat file.txt` would fail under VFS because the subprocess cannot
access the filesystem directly. The default must remain `stdio` to preserve
backward compatibility and simplicity. VFS is opt-in for tools that want (or
need) mediated access.

## Non-Goals

- **Streaming file content.** The protocol transfers file content as complete
  JSON values. Chunked or streaming file transfer is not supported. Tools that
  need to process files larger than the message size limit should use
  `process.run` with appropriate commands.

- **File watching.** The protocol does not support filesystem event
  notifications. Tools that need to react to file changes should poll or use
  `process.run` with a file watcher command.

- **Inter-tool communication.** VFS tools cannot communicate with each other
  through the protocol. Each tool has an independent IPC channel to the host.

- **Custom protocol extensions.** The protocol methods are fixed. Tools cannot
  register custom host capabilities. If a tool needs a capability not covered
  by the protocol, it should request it through `process.run` or it should be
  proposed as a protocol extension in a future RFD.

- **WASM transport.** This RFD specifies the stdio transport only. The WASM
  transport (host function imports) is defined by [RFD 016]. Both transports
  expose the same logical capabilities, but the wire format and framing differ.

## Risks and Open Questions

### Protocol versioning and evolution

The `protocol_version` field in the `init` message enables version negotiation,
but the RFD does not define a formal versioning policy. Questions:

- Can the host add new methods in a minor version bump, or is that a major
  change?
- Should tools declare which protocol version they require, or should they
  discover available methods dynamically?
- How does the host handle requests for methods it doesn't recognize (forward
  compatibility)?

The recommended starting point: the host ignores unknown methods from the tool
(returns `Method Not Found` error) and the tool ignores unknown fields in
responses. New methods are added in minor versions. Removing methods requires a
major version bump. This is standard JSON-RPC forward-compatibility practice.

### Large directory listings

`fs.list_dir` returns all entries in a single response. For directories with
thousands of entries, this could produce very large messages. A pagination
mechanism (offset/limit) or streaming approach may be needed. For the initial
implementation, no pagination is provided — the response includes all entries.
If this proves problematic, pagination can be added as optional parameters in
a backward-compatible way.

### Concurrent requests

The protocol as specified is strictly sequential: one request, one response, one
request, one response. The tool cannot issue multiple requests in parallel. This
simplifies the host implementation but may limit performance for tools that
could benefit from concurrent file reads.

A future protocol version could support concurrent requests by allowing
multiple outstanding requests with different `id` values. The host would
process them concurrently and respond in any order. This is a natural extension
of JSON-RPC's `id`-based correlation, but it adds complexity to both sides and
is deferred until a concrete performance need arises.

### Inquiry prompt blocking

When a protocol request triggers an inquiry prompt, the tool blocks. If the
tool has its own timeout logic, it may abort before the user responds. The
protocol should define how the host signals that a request is being held for
user input, so the tool can adjust its timeout. A possible mechanism: the host
sends a `pending` notification before prompting:

```json
{ "jsonrpc": "2.0", "method": "pending", "params": { "request_id": 1, "reason": "awaiting user approval" } }
```

The tool can then extend its timeout or display a message to its own stderr.
This is not specified in the initial protocol version but noted as a likely
addition.

### Migration path for existing tools

JP's current local tools are `StdioRuntime` shell commands. Migrating a tool
to `runtime = "vfs"` requires rewriting it to use the IPC protocol instead of
direct file access. The `jp_tool::vfs` SDK reduces the effort, but it is still
a per-tool migration. The migration can be incremental — tools are migrated
one at a time, and both runtimes coexist indefinitely.

## Implementation Plan

### Phase 1: Protocol types and NDJSON framing

Define the JSON-RPC request, response, and notification types in a new
`jp_vfs_protocol` crate (or module within `jp_tool`). Implement NDJSON
reading and writing with `tokio::io::BufReader`/`BufWriter`. Unit tests for
serialization round-trips and framing edge cases.

**Depends on:** Nothing.
**Mergeable:** Yes.

### Phase 2: Host-side request handler

Implement the host-side request dispatcher that reads requests from the tool's
stdout, resolves them through `ProjectFiles` ([RFD D09]) and `AccessPolicy`
([RFD 075]), and writes responses to stdin. Cover all `fs.*` methods first,
since those are the most commonly needed.

**Depends on:** Phase 1, [RFD D09] Phase 2 (`FsProjectFiles`), [RFD 075]
Phase 1 (`SandboxConfig`).
**Mergeable:** Yes.

### Phase 3: `VfsRuntime` implementation

Implement `VfsRuntime` as a `ToolRuntime` ([RFD D10]) implementation. Wire up
subprocess spawning with OS sandbox, `init` notification, the request-response
loop, and `result`/`error` notification handling. Map outcomes to
`ExecutionOutcome`.

**Depends on:** Phase 2, [RFD D10] Phase 1 (`ToolRuntime` trait).
**Mergeable:** Yes.

### Phase 4: HTTP and process methods

Add `http.get`, `http.post`, and `process.run` to the host-side dispatcher.
Wire through the access policy. Implement secret scrubbing for `process.run`
output.

**Depends on:** Phase 2.
**Mergeable:** Yes (parallel with Phase 3).

### Phase 5: Tool SDK (`jp_tool::vfs`)

Implement the `VfsHost` client library in `jp_tool`. Provide synchronous
wrappers for all protocol methods. Add integration tests that exercise a
real tool subprocess against the host.

**Depends on:** Phase 3.
**Mergeable:** Yes.

### Phase 6: Migrate one tool as proof of concept

Migrate a single tool (candidate: `fs_read_file` or `fs_list_files`) from
`runtime = "stdio"` to `runtime = "vfs"`. Validate that the tool produces
identical results. Benchmark the performance difference.

**Depends on:** Phase 5.
**Mergeable:** Yes.

### Phase 7: Stateful VFS tool support

Add `apply`, `cancel`, and `state` notification handling to the host-side
loop. Wire into the handle registry from [RFD 009]. Integration test with a
stateful VFS tool.

**Depends on:** Phase 3, [RFD 009] implementation.
**Mergeable:** Yes.

## References

- [RFD 009] — Stateful tool protocol. Defines `ToolState`, the
  spawn/fetch/apply/abort lifecycle, and the handle registry. VFS tools
  integrate as a transport layer beneath this lifecycle.
- [RFD 016] — WASM plugin architecture. Defines `jp:host/filesystem`,
  `jp:host/http`, and `jp:host/process` — the same logical capabilities
  exposed by this protocol over a stdio transport.
- [RFD 058] — Typed content blocks for tool responses. The `result`
  notification uses the content block format defined there.
- [RFD 075] — Tool sandbox and access policy. Provides the OS-level sandbox
  that prevents VFS tools from bypassing the protocol, and the `SandboxConfig`
  / `AccessPolicy` types that govern per-request policy enforcement.
- [RFD D09] — Project filesystem abstraction. The `ProjectFiles` trait backs
  all `fs.*` protocol methods.
- [RFD D10] — Unified tool execution model. `VfsRuntime` implements the
  `ToolRuntime` trait defined there.
- [JSON-RPC 2.0 Specification][jsonrpc]

[RFD 009]: 009-stateful-tool-protocol.md
[RFD 016]: 016-wasm-plugin-architecture.md
[RFD 058]: 058-typed-content-blocks-for-tool-responses.md
[RFD 075]: 075-tool-sandbox-and-access-policy.md
[RFD D09]: D09-project-filesystem-abstraction.md
[RFD D10]: D10-unified-tool-execution-model.md
[jsonrpc]: https://www.jsonrpc.org/specification
