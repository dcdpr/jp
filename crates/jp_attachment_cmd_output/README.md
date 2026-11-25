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

The command does not inherit the environment `jp` runs in.
It receives `PATH` and `HOME`, and every variable named by an `env` parameter:

```sh
jp attachment add "cmd://printenv?arg=TERM&env=TERM"
```

Repeat `env` to pass more than one variable.
A variable is passed with the value it has when the attachment is resolved, and
one that is unset then is not passed.
`env` takes a name only, so `env=TERM=xterm` is rejected.

The parameter can also be spelled `envs`, `env[]`, or `envs[]`, matching the
spellings of `arg`.
In a config file, list the names under `params.envs`:

```toml
[[conversation.attachments]]
type = "cmd"
path = "printenv"
params.args = ["TERM"]
params.envs = ["TERM"]
```

On Windows the command also receives `USERPROFILE` and `SystemRoot`.
