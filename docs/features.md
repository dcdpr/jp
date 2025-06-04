# Features

A list of the features that Jean-Pierre offers, and how they can be used.

## Send Query

You can initiate a new conversation, or continue an existing one, by using the
`query` command. The command takes a prompt, and will return a response from the
LLM.

```sh
# Start a new conversation.
jp query --new "What is the capital of France?"
jp query -n "How many planets are in our solar system?"

# Continue an existing conversation.
jp query "How many of those planets are inhabitable?"
```

Alternatively, you can omit the inline query, and have JP open your editor to
provide the query. The editor to open is configurable via one of the following
ways:

- The `editor.cmd` config key (see [configuring JP][./configuration.md]).
- Any environment variable listed in the `editor.env_vars` config key (defaults
  to `JP_EDITOR`, `VISUAL`, `EDITOR`, in that order).
- The `--edit` flag.

  ```sh
  # Open the query in the default editor, even if an inline query is provided.
  jp query --edit "How high is the highest mountain in the world?"

  # Open the query in a specific editor.
  jp query --edit=vim "Will the moon ever crash into the earth?"

  # Do not open the editor, even if no inline query is provided.
  jp query --edit=false
  jp query --no-edit
  jp query -E
  ```

<br />

::: tip Empty Queries
Not providing a query (by omitting the inline query and using `--no-edit`) can
be useful if your [context](./features.md#custom-context) is sufficient to
instruct the model to a useful answer. For example, if you have a custom context
to instruct the model to generate a commit message, you do not need to ask it to
do so in the query.
:::

## Custom Context

You can attach pre-defined context to your queries to provide the right amount
of information to JP. Pass in the `--context <name>` flag to the `query` command
to attach a context to your query.

The `<name>` must match any named file in your workspace's `.jp/contexts`
directory.

```sh
# Override any configured context for this query.
jp query --cfg conversation.context=commit "Give me a commit message"

# Similar, but using the convenience flag `--context`.
jp query --context commit "Give me a commit message"

# Or the short-hand flag `-x`.
jp query -x commit "Give me a commit message"
```

## Ephemeral Queries

You can use the `--no-persist` flag to run _ephemeral commands_. Ephemeral
commands load data from the workspace, apply your command to it, but don't
persist any mutations made to the workspace state. You can think of this as a
_dry run_ mode for JP.

While this is in general useful for testing, experimentation or debugging your
workflows, it can also be used to run **one-off queries**.

One-off queries are useful for when you want to send a message, but don't plan
on starting a multi-turn conversation with JP.

```sh
# Do not persist the query.
jp --no-persist query "I'm in focus mode, give me a song recommendation."

# Or the short-hand flag `-!`.
jp -! query "Hit me with your best dad joke!"

# Global flags can be positioned anywhere in the command.
jp query "Time for my mid-day snack, any healthy recommendations?" -!
```

Keep in mind that ephemeral queries are **part of the active conversation**.
This means that you can leverage the feature to try out different approaches to
the next _turn_ in the conversation, without persisting that turn in the
conversation history.

If you need a one-off query without any existing conversation history, combine
the `--no-persist` flag with the `--new` flag.

```sh
# Start a new conversation, but don't persist it.
jp query -! --new "Any movie recommendations?"
```

## Hidden Reasoning

You can use the `--hide-reasoning` flag to hide the reasoning behind the LLM's
response. This is useful when you want to focus on the outcome of the query,
without getting distracted by the reasoning process, or if you want to use the
output of the query as a tool input, but don't use [structured
output](#structured-output).

The flag is intentionally named `--hide-reasoning`, as this does **NOT** disable
JP's ability to generate reasoning tokens. If [reasoning is
enabled](#reasoning), and you use a model capable of generating reasoning
tokens, those tokens will still be generated and used to improve the quality of
JP's response, but they will not be displayed to you.

```sh
# Hide the reasoning behind the response.
jp query --hide-reasoning "What physical exercises can I do by my desk?"
```

## Tool Use

JP supports the use of tools in conversations through the [Model Context
Protocol](https://modelcontextprotocol.io). You can [configure MCP
servers](#configure-mcp-servers) or write [embedded tools](#embedded-tools) to
use.

By default, JP will instruct models to optionally use any of the available
tools, but tools tend to be used sparingly, unless explicitly requested by the
user in the query.

However, most model providers have API capabilities to explicitly guide the use
of tools by the model. You can leverage this feature by using the `--tool` flag.

::: info Best Effort
If a model provider does not support explicit tool usage instructions via their
API, JP will try to guide the model to the correct tool usage by injecting
explicit instructions into the prompt.
:::

### Disable Tool Use

You can disable the use of tools by using the `--tool=false` flag.
Alternatively, use the `--no-tool` flag, or the short-hand `-T` flag.

```sh
jp query --tool=false "How many hours are there in a day?"
jp query --no-tool "How many days are there in a week?"
jp query -T "How many weeks are there in a year?"
```

::: info Guaranteed Support
JP removes any tools from the request to the provider if this flag is set, so
even if a provider does not support explicit tool usage instructions via their
API, this flag will still work.
:::

### Require Any Tool

You can require the use of any tool by using the `--tool=true` flag (or just
`--tool`). This means that the model will be forced to use a tool, but its still
free to choose which one, if more than one is available.

```sh
jp query --tool=true "At what time does the sun rise tomorrow?"
jp query --tool "When can I see the northern lights?"
```

### Force Specific Tool Use

You can force the use of a specific tool by using the `--tool=<name>` flag.

```sh
jp query --tool=cargo_test "Any idea what causes this test to fail?"
```

## Structured Output

TODO

## Configure MCP Servers

TODO

## Embedded Tools

TODO
