# T0002: Persona recipes outrank a command-line --cfg

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-08-10

`_shape-args` takes a leading `CFG` parameter naming the recipe's persona, and
emits it *after* the user's arguments. `--cfg` layers compose in declaration
order and the last one wins, so the persona now outranks anything passed on the
command line.

## Why

`just rfd-this -c .jp/config.toml -mopus` silently disabled every tool and
replaced the persona's system prompt.

`.jp/config.toml` extends `mcp/tools/**/*.toml`, where every tool ships
`enable = false`, and `config/personas/default.toml`, whose
`system_prompt.strategy` is `replace`. Re-passing a file already in the load
order promotes its whole `extends` graph above the persona. The assistant
reported "my file tools are gone this turn", which was true.

A recipe's persona is its identity, not a default. `just rfd-this` without the
author persona is not `rfd-this`.

## What changes for callers

A `-c KEY=VALUE` no longer overrides a key the persona sets. The command-level
flags still win, because they are applied after the whole config pipeline:
`-m`, `--hide-reasoning`, `--edit`.

`stage-and-commit` relied on the old behaviour (`-c
style.reasoning.display=hidden` against `personas/stager`'s `full`) and was
switched to `--hide-reasoning`.

## Recipes converted

`commit`, `stage`, `rfd-this`, `rfd-write`, `pr-review`, `pr-triage`,
`rfd-review`, `rfd-triage`, `rfd-prose`, `rfd-implement`.

Left alone on purpose: `issue-bug` and `issue-feat`, whose `{{ARGS}}` is prompt
text rather than flags; `review-triager`, which its own persona file documents
as a `--cfg` delta applied onto an existing conversation.

The persona is passed bare (`personas/rfd-author`, not
`--cfg=personas/rfd-author`) so no leading `--` reaches `just`'s own argument
parser on the nested call.

## Why a ticket and not part of the RFD pipeline work

It arrived alongside that work but is not part of it. It changes `-c` semantics
for every persona recipe in the project, which is its own contract with its own
blast radius.

## Unverified

Nothing here was executed. Worth checking that

    just _shape-args personas/rfd-author "msg" -c foo.toml -mopus

prints `-c foo.toml -mopus --cfg=personas/rfd-author -- msg`, and that no other
recipe depends on a `-c` override of a key its persona sets.
