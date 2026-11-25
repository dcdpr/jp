# Attachment: Command Output

An attachment handler for retrieving the output of a command.

It returns the standard output, standard error, and exit code of the command.

## Usage

Attach the `git diff` of the current project to a query:

```sh
jp query --attach "cmd://git?arg=diff" "Review this diff"
```

The attachment is persisted with the conversation, and the command re-runs on
each subsequent query.

Preview what an attachment sends without attaching it:

```sh
jp attachment print "cmd://git?arg=diff"
```

## Environment

The command does not inherit the parent `jp` environment.
It receives `PATH` and `HOME` (plus `USERPROFILE` and `SystemRoot` on Windows),
along with any variable named by an `env` query parameter:

```sh
jp attachment print "cmd://printenv?arg=TERM&env=TERM"
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

This config entry and the URL above resolve to the same canonical URL, so
attaching both adds the attachment only once.
