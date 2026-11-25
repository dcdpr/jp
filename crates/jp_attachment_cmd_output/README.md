# Attachment: Command Output

An attachment handler for retrieving the output of a command.

It returns the standard output, standard error, and exit code of the command.

## Usage

Get the `git diff` of the current project:

```sh
jp attachment add "cmd://git?arg=diff"
```

List all added URIs:

```sh
jp attachment ls

Attachments:
  cmd://git?arg=diff
```

## Environment

The command does not inherit the parent `jp` environment.
It receives `PATH` and `HOME` (plus `USERPROFILE` and `SystemRoot` on Windows),
along with any variable named by an `env` query parameter:

```sh
jp attachment add "cmd://printenv?arg=TERM&env=TERM"
```

Repeat `env` to forward multiple variables.
Each variable receives the value set in the parent environment when the
attachment is resolved; unset variables are omitted.
`env` accepts only variable names, so `env=TERM=xterm` is rejected.

The parameter can also be spelled `envs`, `env[]`, or `envs[]`, matching the
spellings of `arg`.
In a config file, list variable names under `params.envs`:

```toml
[[conversation.attachments]]
type = "cmd"
path = "printenv"
params.args = ["TERM"]
params.envs = ["TERM"]
```
