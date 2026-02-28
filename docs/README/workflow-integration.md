# Your Workflow, Your Way

JP integrates seamlessly with your existing workflows. It is a single binary
that you run from anywhere, either interactively or headlessly. It respects
basic Unix convetions such as pipes, stdin, stdout/stderr, and exit codes:

```sh
# Interactive usage, can prompt for input
jp query "..."

# Force non-interactive mode
jp --no-prompt ...

# Auto selects non-interactive mode based on stdout redirection
jp query "..." > out.txt

# Pipe data into and out of jp
echo "Hello" | jp query | less

# Even with piped input, you can still edit the query
cat in.txt | jp query --edit

# You can detach a query from the terminal. Output is still sent to stdout,
# unless you redirect it.
jp --detach query "..."
```

You can choose which output format you want, from rendered markdown, to compact
JSONL:

```sh
jp --format
a value is required for '--format <FORMAT>'
  [possible values: auto, text, text-pretty, json, json-pretty]
```

You can enable and increase log verbosity:

```sh
# Enable logging
jp --verbose

# Increase verbosity
jp --verbose --verbose

# Maximum verbosity
jp -vvvvv
```

Logging always happens to stderr, so you can pipe it to a file or send it to
another process:

```sh
jp -v 2> log.txt
```

JP is a single binary you call from wherever you already work; shell scripts,
editor terminals, `git` hooks, CI pipelines, Makefiles. It stores state in a
`.jp/` directory alongside your code, so conversations, configuration, and tools
travel with the project and can be committed to git.

Each conversation has a unique ID and contains the full message history:

```sh
jp init .
jp query --new "What is the purpose of this module?"
jp query "How would you refactor the error handling?"
jp query "Write the implementation."

# One-off query, nothing persisted
jp -! query --new "..."

# Temporary conversation, persisted until you start a new one
jp query --new --tmp "..."

# Or persist for a given duration
jp query --new --tmp=1d "..."

# If you change your mind, mark the conversation as non-temporary
jp conversation edit --no-tmp
```

Conversations are text files. Commit them alongside your code changes:

```sh
git add .jp/conversations/2024-01-15-143022/
git commit -m "feat: add user authentication"
```

Your teammates clone the repo and get the full context of how you arrived at the
implementation. Switch conversations, fork from any point in history, or grep
across all of them with standard Unix tools.

[back to README](../../README.md)
