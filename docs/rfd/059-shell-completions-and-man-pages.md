# RFD 059: Shell Completions and Man Pages

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-21

## Summary

Add a `jp completions <shell>` subcommand that prints shell completion scripts
to stdout, and a `jp manpage` subcommand that prints man page content.
Completions use `clap_complete`'s dynamic `CompleteEnv` system — the binary
itself acts as the completer, enabling context-aware value completions in later
phases. Man pages are generated at runtime using `clap_mangen`.

## Motivation

JP has a large CLI surface — global flags, subcommands with their own flags,
value parsers with unique formats (`KEY=VALUE`, `KEY:=JSON`, `provider/model`),
and a layered configuration system. Users currently discover this surface
through `--help`, the documentation site, or trial and error.

Shell completions and man pages are table-stakes for any serious CLI tool. They
meet users where they already are — in their shell, pressing Tab — without
requiring them to context-switch to a browser or remember exact flag names.
Completions are particularly valuable for JP because:

- Flag names are not always guessable (e.g. `-!` for `--no-persist` or `-%` for
  `--template`).
- Several flags accept structured values that benefit from completion hints
  (`--cfg KEY=VALUE`, `--model provider/name`).
- Subcommands have short aliases (`q` for `query`, `c` for `conversation`) that
  are hard to discover without completions.

Man pages provide offline, terminal-native documentation that integrates with
the Unix help ecosystem (`man jp`, `man jp-query`). They are the expected
documentation format for CLI tools on Unix systems.

## Design

### `jp completions`

A new top-level subcommand that generates shell completion scripts and prints
them to stdout:

```sh
jp completions bash
jp completions zsh
jp completions fish
jp completions powershell
jp completions elvish
jp completions nushell
```

The user pipes the output to the appropriate location for their shell. This is
the dominant pattern in the ecosystem — used by `starship`, `rustup`, `gh`,
`uv`, `mise`, and many others. It is the simplest, most portable approach and
avoids JP needing to know shell-specific installation paths.

#### Supported shells

| Shell      | Crate                    |
|------------|--------------------------|
| Bash       | `clap_complete`          |
| Zsh        | `clap_complete`          |
| Fish       | `clap_complete`          |
| Elvish     | `clap_complete`          |
| PowerShell | `clap_complete`          |
| Nushell    | `clap_complete_nushell`  |

#### Installation instructions

`jp completions --help` prints brief per-shell installation instructions. This
is static text, not generated — each shell has a different mechanism:

```sh
$ jp completions --help

Generate shell completions for jp.

The output is printed to stdout. Pipe it to the appropriate file
for your shell:

  bash:
    jp completions bash > ~/.local/share/bash-completion/completions/jp

  zsh:
    jp completions zsh > ~/.zfunc/_jp
    # Then add: fpath+=~/.zfunc; compinit

  fish:
    jp completions fish > ~/.config/fish/completions/jp.fish

  powershell:
    jp completions powershell >> $PROFILE

  elvish:
    jp completions elvish >> ~/.config/elvish/rc.elv

  nushell:
    jp completions nushell | save -f ~/.config/nushell/jp.nu
    # Then add: source ~/.config/nushell/jp.nu
```

#### Dynamic completions via `CompleteEnv`

`clap_complete` v4.4+ provides `CompleteEnv`, a dynamic completion system where
the shell calls back into the binary itself when the user presses Tab. Instead
of generating a large static script with all options baked in, `jp completions
<shell>` generates a small shell script that registers `jp` as its own
completer.

When the shell requests completions, it invokes `jp` with special environment
variables. JP inspects the partial command line and returns candidates. This
happens via `CompleteEnv::with_factory(Cli::command).complete()`, called at the
very start of `main()` before any other initialization.

This architecture enables value-level completions — the binary can load config
files, read the workspace index, and return context-aware candidates. The same
shell integration script works for all implementation phases; only the completer
logic inside the binary changes.

`CompleteEnv` and `ArgValueCandidates` are behind the `unstable-dynamic` feature
flag in `clap_complete`. Despite the name, the API has been stable since v4.4
and is the recommended approach for new projects.

Benefits over static script generation:

- Completions always match the installed binary's exact flags and subcommands.
- Value-level completions can use runtime state (config, workspace, schema).
- If JP later gains command aliases or plugin-defined subcommands, completions
  pick them up automatically.
- No build-time dependency — `clap_complete` is a regular dependency of
  `jp_cli`.

#### Why a subcommand, not a flag

Some tools use `--completions <shell>` as a flag on the root command (e.g., `bat
--completion bash`). We use a subcommand because:

- It groups naturally with `--help` output — users looking for help see
  `completions` in the subcommand list.
- It avoids polluting the global flag namespace.
- It allows `--help` on the subcommand itself to show installation
  instructions.
- It is the more common pattern among modern CLI tools.

#### Visibility

The `completions` subcommand is visible in `--help` output but placed after the
primary commands (query, config, etc.). It does not have a short alias.

### `jp manpage`

A new top-level subcommand that generates man pages:

```sh
# Print the root man page
jp manpage

# Print a subcommand man page
jp manpage query
jp manpage conversation ls
```

Output is roff-formatted text printed to stdout. Users can view it directly:

```sh
jp manpage query | man -l -
```

Or install it system-wide:

```sh
jp manpage query > /usr/local/share/man/man1/jp-query.1
```

#### Man page structure

`clap_mangen` generates one man page per subcommand. The root `jp manpage` (no
arguments) generates the top-level `jp.1` page. `jp manpage <subcommand>`
generates `jp-<subcommand>.1`. Nested subcommands use hyphenated names: `jp
manpage conversation ls` generates `jp-conversation-ls.1`.

#### Bulk generation

For package maintainers who want to install all man pages at once:

```sh
jp manpage --all --output-dir /usr/local/share/man/man1/
```

This writes one `.1` file per subcommand into the specified directory. The
`--output-dir` flag is only valid with `--all`.

### CLI changes

```rust
#[derive(Debug, clap::Subcommand)]
enum Commands {
    // ... existing variants ...

    /// Generate shell completions for jp.
    Completions(Completions),

    /// Generate man pages for jp.
    Man(Man),
}
```

The `Completions` and `Man` subcommands are handled early in `run_inner()`,
before workspace loading — they don't need a workspace, configuration, or
runtime, similar to `Init`.

### Completion performance

Every Tab press invokes the `jp` binary. The completion codepath must be fast.
`CompleteEnv::complete()` runs before `Cli::parse()`, before workspace loading,
before config resolution — it only needs the `clap::Command` structure.

For Phase 1 (structural completions), performance is a non-issue — building the
`Command` and returning candidates is sub-millisecond.

For Phases 3-4 (value-level completions), the `ValueCandidates` closures need to
load data. The approach is to keep these lightweight:

- `AppConfig::fields()` — static, zero-cost.
- Schema enum variants — static, zero-cost.
- Model aliases — requires loading config files only.
- Conversation IDs — requires reading the workspace directory index.

All of these complete in under 50ms. No network calls, no MCP server
initialization.

## Drawbacks

- **`unstable-dynamic` feature flag**: `CompleteEnv` and `ArgValueCandidates`
  are behind `clap_complete`'s `unstable-dynamic` feature. The API has been
  stable in practice since v4.4, but the flag name is a risk signal. If the API
  changes, our completion logic needs updating. This is mitigated by the clap
  team's commitment to the dynamic completion approach as the future direction.

## Alternatives

### Build-time generation

Generate completion scripts and man pages in `build.rs` and embed them in the
binary (or write them to `OUT_DIR` for packaging). This is what `clap_mangen`'s
README suggests.

Rejected because: it adds build-time dependencies, and it doesn't support
dynamic content (future command aliases, plugins).

### Separate generation binary

A separate `jp-generate` binary (or `xtask`) that produces completions and man
pages. Used for release packaging only, not shipped to end users.

Rejected because: end users can't regenerate completions after updates, and it
adds packaging complexity. The subcommand approach serves both end users and
package maintainers.

### No man pages

Ship only completions and rely on `--help` plus the documentation site for
reference material.

Considered but rejected: man pages are nearly free given `clap_mangen`, and
their absence is noticeable on Unix systems where `man <tool>` is the reflexive
help gesture.

## Non-Goals

- **Installation automation**: JP does not write to shell config files or manage
  completion installation. It prints to stdout; the user decides where it goes.

## Risks and Open Questions

- **Man page quality**: `clap_mangen` generates man pages from clap's help text.
  If our `--help` descriptions are terse or poorly formatted, the man pages will
  be too. This is an incentive to keep help text high-quality, which is good
  pressure.

## Implementation Plan

### Phase 1: `CompleteEnv` infrastructure + structural completions

1. Add `clap_complete` (with `unstable-dynamic` feature) and
   `clap_complete_nushell` as dependencies of `jp_cli`.
2. Add `CompleteEnv::with_factory(Cli::command).complete()` at the start of
   `main()`, before any other initialization.
3. Add the `Completions` subcommand to `Commands` with a `Shell` enum argument.
   This subcommand prints the shell-specific registration script (the small
   script that tells the shell to call back into `jp` for completions).
4. Handle `Commands::Completions` early in `run_inner()`, before workspace
   loading.
5. Write installation instructions as the `--help` long description.
6. Test that completions work for each shell (flag names, subcommand names,
   aliases).

### Phase 2: Man pages

1. Add `clap_mangen` as a dependency of `jp_cli`.
2. Add the `Manpage` subcommand to `Commands` with optional subcommand name and
   `--all`/`--output-dir` flags.
3. Handle `Commands::Manpage` early in `run_inner()`, before workspace loading.
4. Test that generated roff is valid (renders without errors in `man -l -`).

### Phase 3: Schema-derived value completions

1. Add `ArgValueCandidates` to the `--cfg` argument with a completer that
   returns `AppConfig::fields()` paths.
2. Add `ArgValueCandidates` to `--model` with a completer that returns
   `provider/name` patterns based on `ProviverId` and `ModelDetails` (without
   net work calls).
3. Add `ArgValueCandidates` to enum-valued flags (`--reasoning`, `--tool-use`)
   using schema `EnumType` variants.
4. For `--cfg KEY=VALUE`, implement prefix-aware completion: if the user has
   typed `--cfg assistant.tool_choice=`, return the enum values for that field
   from the schema.

### Phase 4: Workspace-aware completions

1. Add `ArgValueCandidates` to `--tool` / `--no-tools` with a completer that
   loads tool names from config files.
2. Add `ArgValueCandidates` to `--model` with a completer that reads model
   aliases from config files.
3. Add completions for `conversation use <TAB>` that returns conversation IDs
   from the workspace index.
4. Add completions for `--cfg @<TAB>` that returns files in `config_load_paths`
   directories.

Phases 1-2 can be merged independently. Phases 3-4 add incremental value without
changing the shell integration mechanism.

## References

- [`clap_complete` docs](https://docs.rs/clap_complete)
- [`clap_complete::env::CompleteEnv`](https://docs.rs/clap_complete/latest/clap_complete/env/struct.CompleteEnv.html)
  — dynamic completion system
- [`clap_complete::engine::ValueCandidates`](https://docs.rs/clap_complete/latest/clap_complete/engine/trait.ValueCandidates.html)
  — custom value completion trait
- [`clap_complete::ArgValueCandidates`](https://docs.rs/clap_complete/latest/clap_complete/struct.ArgValueCandidates.html)
  — attaching custom completers to args
- [`clap_complete_nushell` docs](https://docs.rs/clap_complete_nushell)
- [`clap_mangen` docs](https://docs.rs/clap_mangen)
- `AppConfig::fields()` in `jp_config/src/lib.rs` — static field list for
  `--cfg` completions
- `SchemaType::Enum` in `schematic` — enum variant extraction for value
  completions
- [RFD D11]: Config explain — provenance infrastructure
- [RFD D12]: Interactive config — `--cfg` browser

[RFD D11]: D11-config-explain.md
[RFD D12]: D12-interactive-config.md
