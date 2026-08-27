# Configuration

Jean-Pierre's configuration system leans into the "[progressive complexity]":

- **Progressive Fallback**: Systems should be designed to maintain core
  functionality even as sophisticated technologies or resources become
  increasingly unavailable.
  This ensures continued operation, albeit at a reduced capacity, under adverse
  conditions.
- **[Progressive Disclosure]**: Systems should reveal their functionality in
  layers, starting with a minimal set of touchpoints for basic operation and
  expanding to offer more complex interactions as needed or as the user's
  proficiency increases.
- **Progressive Abstraction**: Systems should be built with multiple layers of
  abstraction, allowing users to interact with them at a level of detail
  appropriate to their needs and Understanding, while hiding unnecessary
  complexity but not preventing access to it.
- **Progressive Enhancement**: Systems should begin with a solid foundation of
  core functionality that works in even the most basic environments, and then
  progressively enhance the user experience and add features as more advanced
  technologies become available.
- **Progressive Control**: Systems should offer a range of control mechanisms,
  from simple, direct manipulation to more complex, automated, or AI-driven
  interactions, allowing users to choose the level of detail that suits their
  needs and abilities.
- **Progressive Operation**: Systems should be designed to operate across a
  spectrum of operational modes, from fully manual to fully autonomous, adapting
  to different contexts and user preferences.
- **Progressive Change**: Systems should be designed with feedback loops to
  evolve and adapt over time, accommodating new technologies, user needs, and
  environmental changes, without disrupting core functionality.

## Loading And Ordering

Configuration options can be set in multiple ways, the order in which they are
loaded is as follows, with later options overriding earlier ones:

1. Program defaults
2. Configuration files
3. Environment variables
4. Conversation configuration
5. Configuration options or files loaded via `--cfg`
6. Command-line arguments

This page describes what you write and where it goes.
For the step-by-step loading sequence, including how the pipeline resolves the
conversation layer, see [RFD 079].

### Program Defaults

The application sets a number of defaults for the best user experience.
Run `jp config show --defaults` to print them as TOML.

Defaults are static: they do not change based on your environment.
The one place JP inspects your machine is `jp init`, which detects available
providers and local models, asks which one to use, and writes the answer to your
configuration file.
From then on the model comes from that file like any other option.

### Configuration Files

Configuration files are loaded in the following order, with later files
overriding earlier ones.

1. `$XDG_CONFIG_HOME/jp/config.toml` (user-global)
2. `<workspace path>/.jp/config.toml` (workspace)
3. `$CWD/.jp.toml` (current directory, recursively upwards)
4. `$XDG_DATA_HOME/jp/workspace/<workspace-name>-<workspace-id>/config.toml`
   (user-workspace)

*The `$XDG_CONFIG_HOME` and `$XDG_DATA_HOME` variables are not used on all
platforms, but suitable alternatives are used instead, see [the directories
crate] for more details.*

*Set `JP_GLOBAL_CONFIG_DIR` to override the user-global directory. The value is
tilde-expanded, and moves both the `config.toml` lookup and the user-global
search root used by [fuzzy matching](#fuzzy-matching-configuration-file-name).*

*The user-workspace directory is located by its workspace-ID suffix, so a
directory created by an older version may be named `<workspace-id>` without the
name prefix.*

*Note that `$CWD/.jp.toml` behaves differently, depending on if you are in a
workspace or not. If you are in a workspace, recursion ends at the workspace
root, whereas outside of a workspace it will continue upwards until `/`.*

*Additionally, `.jp.toml` files stack across directories, with deeper files
overriding shallower ones. Meaning if you are in `/path/to/project` and two
files exist at `/path/.jp.toml` and `/path/to/.jp.toml`, both will be loaded,
with any duplicate configuration options being overridden by the latter file.*

This load order is designed to allow for the most flexibility when using JP on
your system, both inside and outside of a workspace:

- You can define user-global configuration options that apply to any use of
  Jean-Pierre on your system,
- unless you are working in a specific workspace, for which your team has set
  specific configuration defaults,
- unless you are in a directory that has a `.jp.toml` file in it or in one of
  its parent directories, to define custom options specific to that group of
  files,
- unless you are in a workspace and override any of the above options with your
  user-specific configuration options for that specific workspace,

#### File Formats

A configuration file can be TOML, JSON, or YAML.
The examples on this page use TOML, but the same applies to the other formats.

At every location above, JP probes for the file stem with each of these
extensions in order, and uses the first one it finds:

```
toml, json, json5, yaml, yml
```

**`.json5` is probed but cannot be parsed.** The loader recognizes TOML, JSON,
JSONC, and YAML only, so a `config.json5` file is found and then fails to load.
Because `json5` is probed before `yaml`, it also shadows a `config.yaml` sibling
in the same directory.
Do not name your configuration files `.json5`.

#### Stopping The Chain

A configuration file can set `inherit = false` to declare itself authoritative:

```toml
inherit = false
```

Files later in the load order are not merged on top.
A workspace config that sets `inherit = false` prevents both `.jp.toml` files
and the user-workspace config from overriding it.

This is checked against the accumulated configuration, so the option affects
everything that would be loaded *after* the declaring file, not the files loaded
before it.

#### Extending Configuration Files

Any configuration file can pull in additional files using the `extends` field:

```toml
extends = [
    "config.d/tools.toml",
    { path = "config.d/model.toml", strategy = "after" },
]
```

Paths are resolved relative to the directory of the file that declares them, and
may contain glob patterns.
For example, `extends = ["config.d/**/*"]` loads every file in a `config.d/`
directory next to the declaring file.
The field must be declared explicitly to have any effect.

Each entry is merged *before* the declaring file by default, so the declaring
file wins on conflicting options.
Use `strategy = "after"` to give the extended file precedence instead.
Extending is recursive: extended files can declare their own `extends`.

A missing target logs a warning and is skipped.
Cycles are rejected, and recursion is capped at 255 levels deep.

Note that `extends` resolution is strictly file-relative.
It does not search `config_load_paths`, and it does not look for same-named
files in the other configuration roots the way `--cfg <name>` does (see [fuzzy
matching](#fuzzy-matching-configuration-file-name) below).
Concretely: if a workspace file extends `../skill/web.toml`, only the
workspace's own `skill/web.toml` is loaded, even when a file with the same name
exists in your user-global config directory.
To have a personal file contribute alongside such a file, give it the same
`--cfg`-resolvable name (e.g. `skill/web.toml`) under one of the other
configuration roots; both files are then found and merged when you pass `--cfg
skill/web`.
Which file wins on a conflicting option depends on the root you choose, as
described under [fuzzy matching](#fuzzy-matching-configuration-file-name).

### Environment Variables

Every configuration option can be set via an environment variable, with the
prefix `JP_CFG_` followed by the configuration option name in uppercase, using
`_` between option groups.

For example, to set the `max_tokens` model parameter, use
`JP_CFG_ASSISTANT_MODEL_PARAMETERS_MAX_TOKENS`.

**The `CFG_` part matters.** It keeps configuration options separate from JP's
other environment variables.
`JP_CFG_EDITOR_ENVS` sets the `editor.envs` configuration option; `JP_EDITOR`
names the editor JP opens, and is not a configuration option at all.

Run `jp config show` to list every configuration key.
The matching variable is the key uppercased, with `.` replaced by `_`, prefixed
with `JP_CFG_`.

You can use `=:` to set raw JSON values, and `=+` to merge into a list:

```sh
JP_CFG_EDITOR_ENVS=+MY_EDITOR jp query "..."
JP_CFG_EDITOR_ENVS=:'["MY_EDITOR"]' jp query "..."
```

### Conversation Configuration

Any conversation can have configuration options attached to it, which will be
used to override any file- or environment-level configuration.

Some configuration options are attached automatically, such as the model to use.
This means the following command re-uses the same model for every turn in the
conversation: `jp query --new --model <provider>/<model>`, unless a new model is
specified using CLI arguments, or the conversation's configuration is changed.

A conversation stores its configuration in two files, next to its event history:

- `base_config.json`: the configuration snapshot the conversation started with.
- `events.json`: subsequent changes, recorded as config delta events.

To change the configuration of an existing conversation:

```bash
$ jp config set --id <conversation id> --cfg assistant.model.id=anthropic/claude-sonnet-5
Set configuration in conversation <conversation id>
```

This appends a delta to the conversation's event stream, leaving earlier turns
untouched.

To edit the files directly, use `jp conversation edit`:

```bash
$ jp conversation edit --base-config <conversation id>   # or -b
$ jp conversation edit --events <conversation id>        # or -e
```

Run `jp conversation path --base-config <conversation id>` to print a path
instead of opening an editor.

Note that `metadata.json` holds the conversation's title, labels, pin state, and
expiry, not its configuration.
Editing it has no effect on which model or tools a conversation uses.

See [RFD 054] for how the two files relate.

### Configuration Options Or Files Loaded Via `--cfg`

The `jp` command can take one or more `--cfg` flags to load configuration
options.
Flags are applied left to right, so a later `--cfg` overrides an earlier one.
A value can take one of five forms.

#### Dot-Delimited Configuration Option

Similar to [environment variables], the `--cfg` flag can be used to set specific
configuration options.
If the value contains a `=` character, it is considered to be a dot-delimited
configuration option.

These options are expected to be in the form of `path.to.option=value`.
Four operators are available:

| Operator | Meaning                             |
| -------- | ----------------------------------- |
| `=`      | Set the option to a string value.   |
| `:=`     | Set the option to a raw JSON value. |
| `+=`     | Merge a string value into a list.   |
| `:+=`    | Merge a raw JSON value into a list. |

#### Path To An Existing Configuration File

If the value is a path to an existing configuration file, it will be loaded and
merged with the other configuration sources.
Paths are resolved against the current working directory, not the workspace
root.

Prefix the value with `@` to force path interpretation, which is how you load a
file whose name would otherwise be read as a keyword:

```bash
$ jp query --cfg @./NONE.toml "..."
```

#### Fuzzy Matching Configuration File Name

If the provided value is not an existing file, it will be searched for in any
configured `config_load_paths` directories.
If the file name does not have an extension, any file with the extension
`.toml`, `.json`, `.json5`, `.yaml`, or `.yml` will be loaded, in that order.
The value can contain a nested file path, such as `path/to/my_file`, in which
case any directory in `config_load_paths` will be searched for sub-directories
named `path/to`, containing the file `my_file` with one of the above extensions.

Directories in `config_load_paths` must be relative.
Each entry is resolved against three roots, searched in this order:

1. The user-global config directory, e.g. `~/.config/jp/config/` (or the
   platform equivalent).
2. The workspace root, which is the closest directory containing a `.jp`
   directory.
3. The user-workspace data directory, e.g.
   `$XDG_DATA_HOME/jp/workspace/<workspace-name>-<workspace-id>/config/`.
   The directory is located by its workspace-ID suffix, so a directory created
   by an older version may be named `<workspace-id>` without the name prefix.

Within a single root, the first `config_load_paths` entry that produces a match
wins.
Across roots, all matches are loaded and merged, with later roots taking
precedence.
So a private file in your user-global config directory contributes *beneath* a
workspace file of the same name: it can add options the workspace file leaves
unset, but the workspace file wins wherever the two conflict.
To override a workspace-provided file, put yours in the user-workspace data
directory, which has the highest precedence of the three.

Concretely, if I have a file `<workspace root>/.config/persona/dev.toml`, and my
`config_load_paths` contains `.config`, then the `--cfg persona/dev` flag will
load the `dev.toml` configuration file.
This makes it easy to load specific configuration overrides quickly through the
CLI.

#### JSON Object

A value starting with `{` is parsed as a JSON object and merged key by key at
the root of the configuration:

```bash
$ jp query --cfg '{"assistant": {"model": {"parameters": {"temperature": 0.2}}}}' "..."
```

#### Reset Keywords

Two reserved uppercase keywords discard everything loaded so far, and layer
whatever follows on top of the reset point:

| Keyword     | Resets to                               |
| ----------- | --------------------------------------- |
| `NONE`      | Program defaults.                       |
| `WORKSPACE` | The workspace's resolved configuration. |

```bash
$ jp query --cfg NONE --cfg assistant.model.id=ollama/llama3 "..."
```

`--no-cfg` is shorthand for `--cfg NONE`.

`NONE` also skips implicit config loading for the whole invocation, whatever its
position in the directive list.
The two keywords are therefore mutually exclusive: `NONE` skips the loading step
that `WORKSPACE` restores, so combining them fails with an error.

Both are matched exactly, before any other resolution.
To load a file literally named `NONE`, use the `@` prefix or a path-style prefix
such as `./NONE`.

When you use a reset keyword on a continuing conversation, the reset is recorded
in the conversation's event stream, so later turns see the same configuration.
See [RFD 038] for the details.

#### Resetting From A Loaded File

A file loaded through `--cfg` can declare its own reset, which does the same
thing as the `NONE` keyword at that position in the directive list:

```toml
# .jp/personas/solo.toml
[loader]
reset = "none"
```

```bash
$ jp query --cfg solo "..."
```

Configuration accumulated up to this point is thrown away, and the file applies
on top of program defaults.
Anything after it on the command line still layers on top.

Three rules keep this predictable:

- The section is read only from files loaded through `--cfg`.
  A `[loader]` section in `.jp/config.toml` or a `.jp.toml` file is ignored.
- The section counts only in the file named on the command line.
  A file reached through that file's `extends` has its `[loader]` section
  ignored.
- The section never becomes part of the resolved configuration, and is never
  persisted to a conversation.

See [RFD 038] for the full design.

### Command-line Arguments

Any non `--cfg` CLI arguments that manipulate configuration will be merged with
the configuration loaded from the above sources, with the CLI-provided
configuration taking precedence over the other sources.
For example, the `--model` flag for the `query` command will override any model
configuration specified in other sources.

## Inspecting And Editing Configuration

### Show

```
jp config show [--defaults] [--themes]
```

Without flags, prints a commented TOML skeleton of every available configuration
key:

```bash
$ jp config show
# config_load_paths =
# extends =
# inherit =

[assistant]
# instructions =
# name =
# system_prompt =
# system_prompt_sections =
# tool_choice =

[assistant.model]
# id =
...
```

Use `--defaults` to print the resolved default values instead, and `--themes` to
list the available syntax highlighting themes.

### Set

```
jp config set --cfg <KEY=VALUE> [--user-global | --user-workspace | --cwd]
jp config set --cfg <KEY=VALUE> --id <conversation id>
```

Writes the values given with `--cfg` to a configuration file.
Only the keys you set are touched; comments, whitespace, and key order are
preserved.
The workspace configuration file is the default target:

```bash
$ jp config set --cfg assistant.model.id=anthropic/claude-sonnet-5
Set configuration in /home/you/project/.jp/config.toml
```

Pick a different file with `--user-global`, `--user-workspace`, or `--cwd`.

The target file must already exist, and must be TOML.
Writing to a JSON or YAML configuration file fails with `format-preserving merge
is only supported for TOML files`.

With `--id`, the values are appended to a conversation's event stream instead of
a file.
See [Conversation Configuration](#conversation-configuration).

### Format

```
jp config fmt [--check] [--user-global | --user-workspace | --cwd]
```

Rewrites a configuration file in a normalized form.
Without a target flag, every configuration file that applies to the current
workspace is formatted.
Use `--check` to report unformatted files without writing anything, which is
useful in CI.

```bash
$ jp config fmt --check
Checked configuration file: /home/you/project/.jp/config.toml
```

[RFD 038]: ./rfd/038-config-reset-keywords.md
[RFD 054]: ./rfd/054-split-conversation-config-and-events.md
[RFD 079]: ./rfd/079-config-sources-and-load-order.md
[environment variables]: #environment-variables
[progressive complexity]: https://benefuture.miraheze.org/wiki/Progressive_complexity
[progressive disclosure]: https://en.wikipedia.org/wiki/Progressive_disclosure
[the directories crate]: https://docs.rs/directories/6.0.0/directories/struct.ProjectDirs.html#method.config_dir
