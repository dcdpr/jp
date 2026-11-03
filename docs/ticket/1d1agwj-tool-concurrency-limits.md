# Tool Concurrency Limits

- **Status**: Todo
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-11-03
- **Label**: tracking-rfd=128
- **Label**: type=tracking

Tracking ticket for [RFD 128](../rfd/128-tool-concurrency-limits.md).

## Implementation plan

- **Add tool-level concurrency limits**: Adds `concurrency` to `ToolConfig` with its accessor, validation (one limit per key), and tests, plus Host scheduling in `ToolCoordinator::release` and the step handler. `fan_out.concurrency` stays for now. Coordinator tests cover order and limit within a key, parallel keys, questions keeping the slot, failures releasing the next call, cancellation of parked calls, fan-out operations counting against the tool's key, and restart order.
- **Remove fan_out.concurrency**: Removes `concurrency` from `FanOutConfig` and `FanOut`, strips the server `Gate` down to the `stop` check, and adds the `on_error = "stop"` validation (config error with an unbounded limit). Updates the tests and schema snapshot that use it and closes ticket `0ya5qnf`. Depends on phase 1.
- **Declare concurrency in jp-rfd tools**: Declares `concurrency = { key = "rfd" }` on the writing tools (`draft`, `promote`, `rename`, `renumber`, `abandon`, `extend`, `require`, `supersede`) in the shipped definitions and in `.jp/mcp/tools/rfd/`. Depends on phase 1.
