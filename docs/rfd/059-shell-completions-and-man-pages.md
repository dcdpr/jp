# RFD 059: Shell Completions and Man Pages

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-21
- **Summary**: Adds `jp completions` and `jp manpage` subcommands backed by
  `clap_complete` and `clap_mangen`.

## Summary

This RFD adds a `jp completions <shell>` subcommand that prints shell completion
scripts to stdout and a `jp manpage` subcommand that renders man pages at
runtime with `clap_mangen`.
Completions use `clap_complete`'s dynamic `CompleteEnv` mode, in which the `jp`
binary itself serves context-aware completions.

## Motivation

JP has a large CLI surface: global flags, subcommands with their own flags,
value parsers with custom formats (`KEY=VALUE`, `KEY:=JSON`, `provider/model`),
and a layered configuration system.
Without shell completions or man pages, users discover this surface through
`--help`, the documentation site, or trial and error.

Shell completions let users discover flags, subcommands, and values directly in
their shell by pressing Tab, without switching to a browser or memorizing exact
flag names.
Completions are especially useful for JP because:

- Flag names are not always guessable (e.g. `-!` for `--no-persist` or `-%` for
  `--template`).
- Several flags accept structured values that benefit from completion hints
  (`--cfg KEY=VALUE`, `--model provider/name`).
- Subcommands have short aliases (`q` for `query`, `c` for `conversation`) that
  are hard to discover without completions.

Man pages provide offline, terminal-native documentation through standard Unix
tools (`man jp`, `man jp-query`), matching the expected documentation format for
CLI tools on Unix systems.

## Design

### `jp completions`

`jp completions` is a top-level subcommand that generates shell completion
scripts and prints them to stdout:

```sh
jp completions bash
jp completions zsh
jp completions fish
jp completions powershell
jp completions elvish
jp completions nushell
```

The user pipes the output to the appropriate location for their shell.
This is the standard pattern among CLI tools (`starship`, `rustup`, `gh`, `uv`,
`mise`), and it keeps JP from needing to know shell-specific installation paths.

#### Supported shells

| Shell      | Crate                   |
| ---------- | ----------------------- |
| Bash       | `clap_complete`         |
| Zsh        | `clap_complete`         |
| Fish       | `clap_complete`         |
| Elvish     | `clap_complete`         |
| PowerShell | `clap_complete`         |
| Nushell    | `clap_complete_nushell` |

#### Installation instructions

`jp completions --help` prints static per-shell installation instructions:

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
<shell>` outputs a small script that registers `jp` as its own completer.

When the shell requests completions, it invokes `jp` with completion environment
variables.
`CompleteEnv::with_factory(Cli::command).complete()` runs at the start of
`main()`, before any other initialization, inspects the partial command line,
and returns matching candidates.

Because the completer runs inside the binary, the registration script stays the
same across all implementation phases; only the completer logic inside `jp`
changes.
Compared to static script generation, this approach has several advantages:

- Completions always match the installed binary's exact flags and subcommands.
- Value-level completions can inspect runtime state (config files, workspace
  index, schema).
- Command aliases and plugin-defined subcommands are picked up automatically.
- `clap_complete` is a regular dependency of `jp_cli`, with no build-time code
  generation.

`CompleteEnv` and `ArgValueCandidates` sit behind the `unstable-dynamic` feature
flag in `clap_complete`.
Despite the flag name, the API has been stable since v4.4 and is the recommended
approach for new projects.

#### Why a subcommand, not a flag

Some tools expose completions through a flag on the root command (e.g., `bat
--completion bash`).
JP uses a subcommand because:

- It appears in `--help` output alongside other subcommands.
- It avoids polluting the global flag namespace.
- It gives `jp completions --help` its own help page for installation
  instructions.
- It matches the convention of most modern CLI tools.

#### Visibility

The `completions` subcommand is visible in `--help` output after the primary
commands (`query`, `config`, etc.) and does not have a short alias.

### `jp manpage`

`jp manpage` is a top-level subcommand that renders roff-formatted man pages to
stdout:

```sh
# Print the root man page
jp manpage

# Print a subcommand man page
jp manpage query
jp manpage conversation ls
```

Users can view a page directly with `man`:

```sh
jp manpage query | man -l -
```

Or install it system-wide:

```sh
jp manpage query > /usr/local/share/man/man1/jp-query.1
```

#### Man page structure

`clap_mangen` generates one man page per subcommand.
Running `jp manpage` with no arguments generates the top-level `jp.1` page, `jp
manpage <subcommand>` generates `jp-<subcommand>.1`, and nested subcommands use
hyphenated names (`jp manpage conversation ls` generates
`jp-conversation-ls.1`).

#### Bulk generation

Package maintainers can generate all man pages at once:

```sh
jp manpage --all --output-dir /usr/local/share/man/man1/
```

This writes one `.1` file per subcommand into the target directory.
The `--output-dir` flag is only valid with `--all`.

### CLI changes

The CLI is built with clap's builder API.
Each command implements `CommandArgs` to declare its arguments on a
`clap::Command` and read them back from the matches.
`Commands::augment` mounts every command with its help text, and
`Commands::from_matches` dispatches on the matched subcommand name.
The two new commands follow the same pattern:

```rust
pub(crate) enum Commands<S: Stage> {
    // ... existing variants ...

    /// Generate shell completions for jp.
    Completions(completions::Completions),

    /// Generate man pages for jp.
    Manpage(manpage::Manpage),
}
```

Like `Init`, `Completions` and `Manpage` are handled early in `run_inner()`,
before workspace loading, because they do not need a workspace, configuration,
or runtime.

### Flags with optional values

Several flags take an optional value, and on `jp query` the token after such a
flag may be the first word of the query instead: `jp q -t try again` enables
every tool and sends the query "try again", while `jp q -t fs_read_file explain`
enables only `fs_read_file` and sends "explain".
Each of these flags implements `OptionalValue` (`jp_cli/src/cli/flag.rs`), whose
`claim` method decides whether a following token is the flag's value or trailing
free text, using the same context that resolves the flag: the configured and
built-in tool names for `-t`, `-T`, and `-u`, and static parser values for the
remaining flags.

Completion candidates for these flags come from `OptionalValue`, keeping
completion and argument parsing consistent:

- `OptionalValue` gains a `candidates(cx)` method, which `flag_arg` attaches to
  the argument via `ArgValueCandidates`.
- Every candidate returned by `candidates(cx)` is a token that `claim` accepts
  as the flag's value.
  Selecting a completion candidate never turns it into query text or triggers
  the confirmation prompt that bare `-t` shows before enabling every tool.
- The default `candidates(cx)` implementation returns the possible values of the
  flag's parser.
  When an argument has `ArgValueCandidates`, `clap_complete` offers only those
  candidates and skips the parser's possible values, so the default
  implementation must forward them explicitly.
- On a command that accepts trailing free text after its flags, the candidates
  also include `--`, which ends flag parsing so words after `--` become free
  text while any value before `--` stays with the flag: `jp q -t <TAB>` offers
  the tool names and `--` (for `jp q -t -- read the file`).
- Values attached to a flag (`--tool=x`, `-t=x`, `-tx`) complete from the same
  candidates.
  `clap_complete` completes an attached long value only for the argument's
  primary long name (`--tool=<TAB>` completes, whereas `--tools=<TAB>` does
  not).

Conversation target arguments (`-i`/`--id` and the positional IDs accepted by
most `conversation` subcommands) offer the conversation IDs from the workspace
index alongside the keyword and sigil forms (`recent`, `pinned`, `?`, ...)
accepted by that target instantiation.
Because `jp query` does not accept a session target, its completions omit
`session` and `?s`.

### Completion performance

Because every Tab press invokes the `jp` binary, the completion codepath must be
fast.
`CompleteEnv::complete()` runs before `Cli::try_parse_from()`, workspace
loading, and config resolution, and only needs the `clap::Command` structure
built by `Cli::command()`.

For Phase 1 (structural completions), building the `Command` and returning
candidates takes under a millisecond.
For Phases 3-4 (value-level completions), the `ValueCandidates` closures keep
data loading lightweight:

- `AppConfig::fields()`: static, zero-cost.
- Schema enum variants: static, zero-cost.
- Model aliases: loads config files only.
- Conversation IDs: reads the workspace directory index.

Each of these completes in under 50ms, with no network calls or MCP server
initialization.

## Drawbacks

- **`unstable-dynamic` feature flag**: `CompleteEnv` and `ArgValueCandidates`
  require `clap_complete`'s `unstable-dynamic` feature.
  Although the API has been stable since v4.4 and is the clap team's planned
  direction for dynamic completions, upstream breaking changes remain possible
  until the feature flag is stabilized.

## Alternatives

### Build-time generation

Generate completion scripts and man pages in `build.rs` and embed them in the
binary (or write them to `OUT_DIR` for packaging), as `clap_mangen`'s README
suggests.

Rejected because it adds build-time dependencies and cannot support dynamic
completions for runtime state, future command aliases, or plugins.

### Separate generation binary

Provide a separate `jp-generate` binary (or `xtask`) that produces completions
and man pages for release packaging only, without shipping it to end users.

Rejected because end users cannot regenerate completions after updates and it
adds packaging complexity.
Built-in subcommands serve both end users and package maintainers.

### Omit man pages

Ship only completions and rely on `--help` and the documentation site for
reference material.

Rejected because man pages are nearly free to generate with `clap_mangen`, and
Unix users expect `man <tool>` to work.

## Non-Goals

- **Installation automation**: JP prints completion scripts and man pages to
  stdout rather than modifying shell configuration files or managing
  installation paths.

## Risks and Open Questions

- **`--` after an attached value**: `clap_complete` does not tell a completer
  whether the value being completed is attached to the flag, and
  `ArgValueCompleter` receives the same input.
  As a result, `--tool=<TAB>` offers `--tool=--` alongside the tool names.
  Selecting `--tool=--` fails immediately as an unknown tool name, whereas
  omitting `--` from the candidates would remove it from `--tool <TAB>`, where
  it separates a bare flag from trailing query text.

- **Man page quality**: `clap_mangen` renders man pages directly from clap's
  help text, so terse or poorly formatted `--help` descriptions carry over into
  the generated man pages.
  Keeping `--help` text clear and well-structured addresses both outputs.

## Implementation Plan

### Phase 1: `CompleteEnv` infrastructure and structural completions

1. Add `clap_complete` (with the `unstable-dynamic` feature) and
   `clap_complete_nushell` as dependencies of `jp_cli`.
2. Add `CompleteEnv::with_factory(Cli::command).complete()` at the start of
   `main()`, before any other initialization.
3. Add the `Completions` subcommand to `Commands` with a `Shell` enum argument
   that prints the shell-specific registration script.
4. Handle `Commands::Completions` early in `run_inner()`, before workspace
   loading.
5. Write installation instructions in the `--help` long description.
6. Test completions for each shell (flag names, subcommand names, aliases).

### Phase 2: Man pages

1. Add `clap_mangen` as a dependency of `jp_cli`.
2. Add the `Manpage` subcommand to `Commands` with an optional subcommand name
   and `--all`/`--output-dir` flags.
3. Handle `Commands::Manpage` early in `run_inner()`, before workspace loading.
4. Test that generated roff is valid and renders without errors in `man -l -`.

### Phase 3: Schema-derived value completions

1. Add `ArgValueCandidates` to the `--cfg` argument with a completer that
   returns `AppConfig::fields()` paths.
2. Add `ArgValueCandidates` to `--model` with a completer that returns
   `provider/name` patterns based on `ProviverId` and `ModelDetails` (without
   network calls).
3. Add `ArgValueCandidates` to enum-valued flags (`--reasoning`) using schema
   `EnumType` variants.
   An `OptionalValue` flag whose parser already lists its possible values
   (`--quote`) completes them through the default `candidates` implementation
   without a custom completer.
4. For `--cfg KEY=VALUE`, implement prefix-aware completion: when the user types
   `--cfg assistant.tool_choice=`, return the schema's enum values for that
   field.

### Phase 4: Workspace-aware completions

1. Implement `OptionalValue::candidates` for `--tool`, `--no-tool`, and
   `--tool-use`, returning the configured and built-in tool names loaded from
   config files, plus `--`; `--tool-use` also offers `true` and `false`.
2. Add `ArgValueCandidates` to `--model` with a completer that reads model
   aliases from config files.
3. Implement `OptionalValue::candidates` for conversation targets, returning
   conversation IDs from the workspace index and the keyword and sigil forms
   accepted by that target.
   Positional targets on `conversation` subcommands use the same candidates.
4. Add completions for `--cfg @<TAB>` that return files in `config_load_paths`
   directories.
5. Test that completion and parsing agree: for every `OptionalValue` flag on `jp
   query` and every candidate it offers other than `--`, parsing `jp query
   <flag> <candidate> word` keeps `<candidate>` as the flag's value.

Phases 1-2 can be merged independently.
Phases 3-4 add value-level completions without changing the shell integration
script.

## References

- [`clap_complete` docs]
- [`clap_complete::env::CompleteEnv`]: dynamic completion system
- [`clap_complete::engine::ValueCandidates`]: custom value completion trait
- [`clap_complete::ArgValueCandidates`]: attaching custom completers to args
- [`clap_complete_nushell` docs]
- [`clap_mangen` docs]
- `AppConfig::fields()` in `jp_config/src/lib.rs`: static field list for `--cfg`
  completions
- `SchemaType::Enum` in `schematic`: enum variant extraction for value
  completions
- `OptionalValue` in `jp_cli/src/cli/flag.rs`: trait for flags with optional
  values and their completion candidates

[`clap_complete::ArgValueCandidates`]: https://docs.rs/clap_complete/latest/clap_complete/struct.ArgValueCandidates.html
[`clap_complete::engine::ValueCandidates`]: https://docs.rs/clap_complete/latest/clap_complete/engine/trait.ValueCandidates.html
[`clap_complete::env::CompleteEnv`]: https://docs.rs/clap_complete/latest/clap_complete/env/struct.CompleteEnv.html
[`clap_complete_nushell` docs]: https://docs.rs/clap_complete_nushell
[`clap_complete` docs]: https://docs.rs/clap_complete
[`clap_mangen` docs]: https://docs.rs/clap_mangen
