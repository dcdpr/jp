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

1. Hard-coded defaults
2. Configuration files
3. Environment variables
4. Conversation metadata
5. Configuration options or files loaded via `--cfg`
6. Command-line arguments

### Hard-coded Defaults

The application sets a number of hard-coded defaults for the best user
experience.
You can use `jp config show --defaults` to show the defaults.

Note that defaults can be context-specific.
For example, the `assistant.model` option depends on whether specific local
`ollama` models are available, or which cloud-model environment variables (such
as `OPENAI_API_KEY`) are set.

### Configuration Files

Configuration files are loaded in the following order, with later files
overriding earlier ones.

1. `$XDG_CONFIG_HOME/jp/config.toml` (user-global)
2. `<workspace path>/.jp/config.toml` (workspace)
3. `$CWD/.jp.toml` (current directory, recursively upwards)
4. `$XDG_CONFIG_HOME/jp/<workspace id>/config.toml` (user-workspace)

*A configuration file can be either a TOML, JSON, or YAML file, the above
example uses TOML, but the same applies to JSON and YAML.*

*The `$XDG_CONFIG_HOME` variable is not used on all platforms, but a suitable
alternative is used instead, see [the directories crate] for more details.*

*Note that `$CWD/.jp.toml` behaves differently, depending on if you are in a
workspace or not. If you are in a workspace, recursion ends at the workspace
root, whereas outside of a workspace it will continue upwards until `/`.*

*Additionally, `.jp.toml` files can inherit from each other, with the ones
higher in the directory hierarchy overriding the lower ones. Meaning if you are
in `/path/to/project` and two files exist at `/path/.jp.toml` and
`/path/to/.jp.toml`, both will be loaded, with any duplicate configuration
options being overridden by the latter file.*

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
prefix `JP_` followed by the configuration option name in uppercase, using `_`
between option groups.

For example, to set the `max_tokens` model parameter, use
`JP_ASSISTANT_MODEL_PARAMETERS_MAX_TOKENS`.

You can use `jp config show --envs` to list all environment variables.

You can use `=:` to set raw JSON values, and `=+` to merge arrays:
`JP_EDITOR_ENV_VARS=+MY_EDITOR`, or `JP_EDITOR_ENV_VARS=:'["MY_EDITOR"]'`.

### Conversation Metadata

Any conversation can have configuration options attached to it, which will be
used to override any file- or environment-level configuration.

Some configuration options are automatically added to the conversation metadata
by the application, such as the `model` and `provider` options.
Others are not, but can be added manually by editing the conversation's
`metadata.json` file (run `jp conversation path --metadata <conversation id>` to
print its location).

This means the following command will re-use the same model for every turn in
the conversation: `jp query --new --model <provider>/<model>`, unless a new
model is specified using CLI arguments, or when the conversation metadata is
manually edited.

### Configuration Options Or Files Loaded Via `--cfg`

The `jp` command can take one or more `--cfg` flags to load configuration
options.
These options can be specified in one of three ways.

#### Dot-Delimited Configuration Option

Similar to [environment variables], the `--cfg` flag can be used to set specific
configuration options.
If the value contains a `=` character, it is considered to be a dot-delimited
configuration option.

These options are expected to be in the form of `path.to.option=value`.
You can use `:=` to set raw JSON values, and `+=` to merge arrays.

#### Path To An Existing Configuration File

If the value is a path to an existing configuration file, it will be loaded and
merged with the other configuration sources.

#### Fuzzy Matching Configuration File Name

If the provided value is not an existing file, it will be searched for in any
configured `config_load_paths` directories.
If the file name does not have an extension, any file with the extension
`.toml`, `.json`, `.yaml`, or `.yml` will be loaded, in that order.
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

### Command-line Arguments

Any non `--cfg` CLI arguments that manipulate configuration will be merged with
the configuration loaded from the above sources, with the CLI-provided
configuration taking precedence over the other sources.
For example, the `--model` flag for the `query` command will override any model
configuration specified in other sources.

[progressive complexity]: https://benefuture.miraheze.org/wiki/Progressive_complexity
[progressive disclosure]: https://en.wikipedia.org/wiki/Progressive_disclosure
[the directories crate]: https://docs.rs/directories/6.0.0/directories/struct.ProjectDirs.html#method.config_dir
