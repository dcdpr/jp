# Configuration

## Loading And Ordering

Configuration options can be set in multiple ways, and the order in which they
are loaded is as follows, with later options overriding earlier ones.

1. Hard-coded defaults
2. Configuration files
3. Environment variables
4. Conversation metadata
5. Configuration options or files loaded via `--cfg`
6. Command-line arguments

### Hard-coded Defaults

The application sets a number of hard-coded defaults for the best user
experience. You can use `jp config show --defaults` to show the defaults.

### Configuration Files

Configuration files are loaded in the following order, with later files
overriding earlier ones.

1. `$XDG_CONFIG_HOME/jp/config.toml` (user-global)
2. `<workspace path>/.jp/config.toml` (workspace)
3. `$CWD/.jp.toml` (current directory, recursively upwards)
4. `$XDG_CONFIG_HOME/jp/<workspace id>/config.toml` (user-workspace)

_A configuration file can be either a TOML, JSON, or YAML file, the above
example uses TOML, but the same applies to JSON and YAML._

_The `$XDG_CONFIG_HOME` variable is not used on all platforms, but a suitable
alternative is used instead, see [the directories crate] for more details._

_Note that `$CWD/.jp.toml` behaves differently, depending on if you are in a
workspace or not. If you are in a workspace, recursion ends at the workspace
root, whereas outside of a workspace it will continue upwards until `/`._

_Additionally, `.jp.toml` files can inherit from each other, with the ones
higher in the directory hierarchy overriding the lower ones. Meaning if you are
in `/path/to/project` and two files exist at `/path/.jp.toml` and
`/path/to/.jp.toml`, both will be loaded, with any duplicate configuration
options being overridden by the latter file._

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

[the directories crate]: https://docs.rs/directories/6.0.0/directories/struct.ProjectDirs.html#method.config_dir

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
by the application, such as the `model` and `provider` options. Others are not,
but can be added manually by editing the conversation metadata file in
`<workspace path>/.jp/conversations/<conversation id>/metadata.json`.

This means that if you start a new conversation with `jp query --new --model
<provider>/<model>`, the same model will be re-used for every turn in the
conversation, unless a new model is specified using CLI arguments, or when the
conversation metadata is manually edited.

### Configuration Options Or Files Loaded Via `--cfg`

The `jp` command can take one or more `--cfg` flags to load configuration
options. These options can be specified in one of three ways.

#### Dot-Delimited Configuration Option

Similar to [environment variables], the `--cfg` flag can be used to set specific
configuration options. If the value contains a `=` character, it is considered
to be a dot-delimited configuration option.

These options are expected to be in the form of `path.to.option=value`. You can
use `:=` to set raw JSON values, and `+=` to merge arrays.

#### Path To An Existing Configuration File

If the value is a path to an existing configuration file, it will be loaded
and merged with the other configuration sources.

#### Fuzzy Matching Configuration File Name

If the provided value is not an existing file, it will be searched for in any
configured `config_load_paths` directories. If the file name does not have an
extension, any file with the extension `.toml`, `.json`, or `.yaml` will be
loaded, in that order. The value can contain a nested file path, such as
`path/to/my_file`, in which case any directory in `config_load_paths` will be
searched for sub-directories named `path/to`, containing the file `my_file` with
one of the above extensions.

Note that directories in `config_load_paths` must be relative, and are appended
to the workspace path, which is the closest directory containing a `.jp`
directory.

Concretely, if I have a file `<workspace path>/.config/persona/dev.toml`, and my
`config_load_paths` contains `.config`, the the `--cfg persona/dev` flag will
load the `dev.toml` configuration file. This makes it easy to load specific
configuration overrides quickly through the CLI.

### Command-line Arguments

Any non `--cfg` CLI arguments that manipulate configuration will be merged with
the configuration loaded from the above sources, with the CLI-provided
configuration taking precedence over the other sources. For example, the
`--model` flag for the `query` command will override any model configuration
specified in other sources.
