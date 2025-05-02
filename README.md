# Jean-Pierre, An LLM-based Programming Assistant

Jean-Pierre is a command-line toolkit to support you in your daily work as a
software programmer. It is built to integrate into your existing workflow, uses
powerful concepts such as _workspaces_, _contexts_, _attachments_ and _personas_
to provide a flexible and powerful pair-programming experience with LLMs.

## Features

- Single `jp` command for all interactions, no installation required.
- Use multiple LLM _providers_ (support for local LLMs coming soon).
- Integrate existing Model-Context-Protocol servers using _mcp_ configurations.
- Switch between different _conversations_ during your work.
- Attach files or notes to conversations using _attachments_.
- Use _models_ to use specific providers, models and parameters.
- Define multiple _personas_ to customize the LLM's behavior.
- Use _contexts_ to limit/expand the LLM's knowledge base per conversation.
- Persist JP state in your VCS of choice.
- (soon) Encrypted conversation history.
- (soon) Private (local) conversations excluded from VCS.
- (soon) Text-to-speech integration.
- (soon) More attachments types (e.g. header files, external apps, etc.).
- (soon) Sync server to store data in a central local location.
- (soon) API server to expose data to other devices.
- (soon) Mobile web app to continue conversations on mobile devices.
- (soon) Agentic workflows with budget constraints and milestones.
- (soon) Directly integrate into your VCS, allowing LLM to edit files.

## Command Line Interface

```sh
jp init .
jp <...> <-h|--help>
---
jp <q|query>        ...
jp <p|persona>      ...
jp <c|conversation> ...
jp <a|attachment>   ...
jp <m|mcp>          ...
```
