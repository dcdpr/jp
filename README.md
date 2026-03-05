# Jean-Pierre, An LLM-powered Programming Assistant

> A command-line toolkit to support you in your daily work as a software
> programmer. Built to integrate into your existing workflows, providing a
> secure, powerful and flexible pair-programming experience with LLMs.

Visit [**jp.computer**] to learn more.

> [!NOTE]
> This project is in active development. Expect breaking changes. What is
> documented here is subject to change and may not be up-to-date. Please consult
> the [installation instructions](#getting-started) to get started, and [reach
> out to us](https://jp.computer/contact) if you need any assistance, or have
> feedback.

## Philosophy

JP is built to be **[provider-agnostic][1]**, your workflow shouldn't be coupled
to any single LLM backend; **[private and secure by default][2]**, with no
implicit network access or silent tool execution; a **[proper Unix
citizen][3]**, a single static binary that composes with pipes, respects your
shell, and stays out of your way; **[extensible][4]** through sandboxed plugins
and **[configurable][5]** where it matters; **[open-source and
independent][6]**, funded without VC money, no allegiance to any LLM provider,
just software that serves its users.

[1]: docs/README/providers.md
[2]: docs/README/privacy-and-security.md
[3]: docs/README/workflow-integration.md
[4]: docs/README/extensibility.md
[5]: docs/README/configuration.md
[6]: docs/README/open-source.md

## Getting Started

JP is in active development. Install from source:

```sh
cargo install --locked --git https://github.com/dcdpr/jp.git
```

Initiate a new workspace in an existing directory:

```sh
jp init .
> Confirm before running tools?
Yes (safest option)
> Which LLM model do you want to use?
ollama/qwen3

Initialized workspace at current directory
```

Run your first query:

```sh
jp query "Is this thing on?"
Hello there! I am Jean-pierre, how can I help you today?
```

Configure your JP workspace:

```sh
open .jp/config.toml
```

See what else you can do:

```sh
jp help
```
