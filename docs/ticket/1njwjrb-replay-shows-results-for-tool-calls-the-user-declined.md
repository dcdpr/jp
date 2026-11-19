# Replay shows results for tool calls the user declined

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-19
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: type=bug

`jp conversation print` shows a result for a tool call the user declined at its
approval prompt.
The live run shows nothing for it.

With two calls in one response, approved and then declined (`y`, `n`), the live
run ends with:

```
Calling tool cargo_check(n: 0)
Run local cargo_check tool? y
Calling tool cargo_test(n: 1)
Run local cargo_test tool? n

cargo_check result

result
```

Replay of the same turn adds a block the user never saw:

```
cargo_test result

Tool skipped by user.
```

## Cause

The live path settles a declined call without rendering its response
(`ToolCoordinator::on_permission`, the `Err(response)` arm goes to `settle`).
The recorded `ToolCallResponse` still carries the skip message, and
`TurnRenderer::render_turn` renders every `ToolCallResponse` whose chrome is
visible, with nothing to tell it the call never ran.

The same likely holds for other calls settled without running and without
rendering live: a "no" remembered for the turn, `result = "skip"`, and the user
cancelling a question.
Each needs checking.

## Fix

Replay needs to know a response is a settlement rather than a result.
Matching the skip message text would break as soon as `cancellation_response` is
configured, so the response or its event metadata has to record it.

- [ ] Replay renders no result for a call declined at its approval prompt
- [ ] Each other way a call is settled without running is checked against the
      live output, and matches it
- [ ] A replay test pins the `['y', 'n']` case with exact output
