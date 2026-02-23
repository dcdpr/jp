# JP Explainer: Introduction & Getting Started

**Visual:** Speaker face in top-right. Screen share of a browser open to
`jp.computer`. Terminal window ready in background.

---

## (0:00) Introduction

Hey everyone. I want to show you Jean-Pierre, or JP for short.

JP is a command-line toolkit designed to support you as a software programmer.
It’s built to integrate seamlessly into your existing workflow, providing a
flexible and powerful pair-programming experience.

What makes JP distinct from tools like Claude Code, OpenAI Codex, or Gemini CLI
is that it is an actual CLI, not a TUI. It doesn't lock you into a bespoke chat
interface. Instead, it respects standard streams like stdin and stdout, meaning
you can pipe data into it, script it, and use it alongside standard Unix tools.
It adheres to platform best practices, behaving exactly how you expect a
command-line tool to behave.

It is also provider-agnostic. Whether you use OpenAI, Anthropic, Google, or run
local models with Ollama, JP works with them all.

## (1:00) Getting Started

Let me show you how fast you can get up and running.

**Visual:** Switch focus to browser showing `jp.computer`.

First, head over to `jp.computer`. We'll just grab the single binary for our
system here.

**Visual:** Click download button. Switch to Terminal.

Once you have it in your path, you can verify it's working by running the help
command.

```bash
jp -h
```

This lists all the available options. You can see the core commands here: `init`
to start a workspace, `config` to manage settings, `query` to ask questions, and
others for managing attachments and conversations. You also have global options
to control verbosity, color, or even override configuration values on the fly.

It's a standard CLI application, ready for automation or manual use.

## (1:30) Initialization

Now, let's set it up in a project. I'll create an empty directory for this demo.

```bash
mkdir jp-demo
cd jp-demo
```

To configure JP for this specific workspace, we run:

```bash
jp init
```

Be sure to export your API key environment variable, such as `OPENAI_API_KEY` or
`ANTHROPIC_API_KEY`, depending on the model you want to use.

This sets up the necessary configuration files so JP knows how to behave in this
context.

## (2:00) First Query

Finally, let's make sure everything is connected.

I'll ask it a simple question.

```bash
jp query "hello world?"
```

And there we go. JP responds immediately.

It’s that simple to install and start talking to your new pair programmer. In
the next videos, we'll dive deeper into how to use JP to write code, refactor
files, and leverage its extensibility features.

Thanks for watching.
