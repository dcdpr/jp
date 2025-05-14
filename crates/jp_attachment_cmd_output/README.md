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
