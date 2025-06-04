# Case Studies

<style>
.features {
  font-size: 0.75rem
}
</style>

Jean-Pierre is designed to be used in a wide range of contexts, from simple
command-line invocations to sophisticated tools-driven agentic workflows. The
goal of the project is to integrate seamlessly into your existing workflow, be
useful in your daily work, but stay out of your way when unneeded.

Let's explore some of the use cases that Jean-Pierre can help you with. These
cases come from real-world scenarios, used in production environments by your
peers in the industry.

<br />

::: tip Share Your Experience
Want to inspire others with your real-world experience? [Get in
touch](https://github.com/dcdpr/jp/issues/new?labels=use-case&template=use-case.yml&title=)
:::

## Git Commit Message Generator

<span class="features">features: [send-query][],
[custom-context][custom-context], [ephemeral-queries][], [hide-reasoning][],
[disable-tool-use][], [command attachments][]</span>

Jean-Pierre can be used to generate accurate and well-formatted commit messages
for your project. For this to work, we need the following pieces of data:

1. [A diff of the staged changes](#git-diff), to help inform the model of what
   the commit is about.
2. [Instructions for the model to follow](#model-instructions), such as best
   practices for commit messages, or the exact format of the commit message
   (e.g. conventional commits).
3. [The final `jp` command to run](#cli-usage), piping the output to the `git`
   command.

Let's walk through these steps one by one.

### 1. Git Diff Attachment {#git-diff}

First, we need to add [context][] to the query we're sending to the model. JP
has support for many different [attachment types][attachments], but for this use
case, we want to attach the diff of the staged changes to the query, so we'll
use the [`command attachment`][] type. We can do this by
using the [`--attachment` flag][attachment-feature], but since we're going to be
adding more contextual information later, and we want to re-use this context,
we'll go ahead and create a new context file:

```sh
touch .jp/contexts/commit.json
```

Now let's populate the file with our attachment handler:

```json
{
  "attachments": {
    "cmd": {
      "type": "cmd",
      "value": [
        {
          "cmd": "git",
          "args": [
            "diff",
            "--cached"
          ]
        }
      ]
    }
  }
}
```

This tells JP, if we enable the `commit` context, to attach the output of the
`git diff --cached` command to the query.

### 2. Commit Persona {#model-instructions}

Next, we need to create a [persona][] for the model to use. A persona, as the
name implies, is a set of properties that shape the behavior of the model. In
our case, we are interested in four properties:

- The model to send the query to.
- The parameters to use for the model (specifically, the `reasoning` parameter).
- The system prompt to tell the model what we want it to do.
- The instructions to follow on how to write a commit message.

Let's create a new persona file:

```sh
touch .jp/personas/commit.json
```

Now let's populate the file with the properties we want:

```json
{
  "name": "Commit",
  "model": "anthropic/claude-sonnet-4-0",
  "parameters": {
    "reasoning": {
      "effort": "high"
    }
  },
  "system_prompt": "You are an expert at following the Commit specification. Generate a commit message, using the `git diff` output available to you.",
  "instructions": [
    {
      "title": "How to format your response",
      "items": [
        "ONLY respond with a commit message, nothing else.",
        "DO NOT provide any explanations or justifications.",
        "DO NOT add fenced code blocks around the commit message.",
      ]
    },
    {
      "title": "The seven rules of a great Git commit message",
      "items": [
        "Separate subject from body with a blank line",
        "Limit the subject line to 50 characters",
        "Capitalize the subject line",
        "Do not end the subject line with a period",
        "Use the imperative mood in the subject line",
        "Wrap the body at 72 characters",
        "Use the body to explain what and why vs. how"
      ]
    }
  ]
}
```

There are many more instructions you can add to the persona, depending on how
structured you want your commit messages to be. For example, [here is the
persona
file](https://github.com/dcdpr/jp/blob/febbce945b821879f637f970dab4c971e9c95ddd/.jp/personas/commit.json)
we use for the JP project itself. Some additional instruction sets we use, are:

- Project Structure
- Why Commit Messages Matter
- Commit Message Format
- Commit Message Header Format
- Commit Message Header Types
- Commit Message Scope Types
- Commit Message Body Format
- Commit Message Footer Format

Experiment with different instructions sets to find the ones that works best for
your project.

Next, we add this persona to the context we created earlier:

```json
{
  "persona_id": "commit", // [!code ++]
  "attachments": {
    ...
  }
}
```
### 3. Running The Query {#cli-usage}

Now that we have our context and persona set up, we can run the query. We can do
this by using the `query` command.

```sh
jp query \
   --no-persist \
   --new \
   --hide-reasoning \
   --no-tool \
   --context=commit \
   "Write a commit message."
```

<br />

::: info Structured Output
An alternative approach is to use JP's [structured output][]. This allows you to
get a JSON response from the model, separating different parts of the commit
message. You can then use tools such as [jq](https://stedolan.github.io/jq/) to
reconstruct the final commit message.

However, this approach is more complex, and hasn't proven to be more effective
than the approach outlined in this use case. This alternative approach would be
useful if you have a need for a more formal "commit message AST" to work with.
:::

[send-query]: ./features.md#send-query
[custom-context]: ./features.md#custom-context
[ephemeral-queries]: ./features.md#ephemeral-queries
[hide-reasoning]: ./features.md#hidden-reasoning
[disable-tool-use]: ./features.md#tool-use
[structured output]: ./features.md#structured-output
[command attachments]: ./features.md#command-attachments
[attachment-feature]: ./features.md#attachments
[attachments]: ./features/attachments.md
[persona]: ./features/personas.md
[context]: ./features/contexts.md
[config-reasoning]: ./configuration.md#reasoning
