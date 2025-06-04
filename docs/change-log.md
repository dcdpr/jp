# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog], and this project adheres to
[Semantic Versioning]. The file is auto-generated using [Conventional Commits].

[keep a changelog]: https://keepachangelog.com/en/1.0.0/
[semantic versioning]: https://semver.org/spec/v2.0.0.html
[conventional commits]: https://www.conventionalcommits.org/en/v1.0.0/

## Overview

- [unreleased](#unreleased)
- [`0.1.0`](#010) – _2025.05.02_

## _[Unreleased]_

- build: improve build reproducibility (#130) ([`bd642d2`])
- build: Enhance development workflow with improved justfile recipes (#129) ([`510a744`])
- build: tweak deployment process (#128) ([`f637719`])
- build: ensure `just` is installed (#127) ([`9eb6b0e`])
- build: do not require yarn.lock (#126) ([`91d3fc0`])
- build: do not require yarn.lock (#125) ([`81cfc84`])
- build: fix working-directory (#124) ([`3092918`])
- build: disable build caching (#123) ([`bad7dc8`])
- build: fix github workflows directory (#122) ([`d63a130`])
- build: fix docs deploy branch name (#121) ([`714ac4a`])
- build: Add documentation infrastructure with VitePress (#120) ([`e8800a0`])
- chore: Refactor commit scope guidelines for clarity (#114) ([`6eb984b`])
- feat: Add embedded tools system with TOML-based configuration (#113) ([`db9f418`])
- feat: Add convenience `--hide-reasoning` flag to `query` command (#111) ([`05a8350`])
- fix: Restore tool call recursion in query (#106) ([`d9a7f91`])
- feat: Improved reasoning support (#105) ([`c3e845f`])
- chore: Add GitHub issues/PRs tools and restructure code (#103) ([`7a94e66`])
- build: Add MCP server for development tooling (#100) ([`b80e7c8`])
- fix: Prevent early return in tool call completion handling (#98) ([`8be4e34`])
- feat: Improve assistant message formatting in editor (#97) ([`5a2fd94`])
- fix: Allow forward slashes in `ModelId` validation (#96) ([`51fe290`])
- refactor: Reorganize MCP server configs into subdirectory (#95) ([`85a74dc`])
- test: Refresh LLM provider test fixtures (#94) ([`be427d9`])
- feat: Add `--tool` flag to control tool choice behavior (#93) ([`a8112bf`])
- feat: Add `--from` flag to `conversation rm` command (#90) ([`34417a2`])
- fix: ensure `--model` flag is applied correctly (#89) ([`35fd2c4`])
- chore: add missing crates to log output (#88) ([`da4dc7d`])
- chore: Improve `commit` persona model and instructions (#87) ([`61f293f`])
- fix: Correctly set context window size for Ollama (#86) ([`9893602`])
- feat: Improve extended thinking/reasoning support (#85) ([`34afef5`])
- fix: Remove hardcoded default model from `Persona` (#84) ([`b2deea1`])
- fix: Preserve empty lines in markdown formatting (#82) ([`538f417`])
- refactor: Error message improvements (#81) ([`c411abc`])
- refactor: migrate model parameters to typed struct (#80) ([`1e52932`])
- refactor: Simplify model handling (#79) ([`ce1ab70`])
- ci: Add `bacon.toml` for CI task management (#78) ([`c5856db`])
- build: Add `cargo-deny` to the build process (#77) ([`e20c97c`])
- fix: fix string indentation in conversation_titles query (#75) ([`c292644`])
- feat: Add Ollama provider for local LLMs (#74) ([`6146e57`])
- build: Add commit recipe to justfile (#70) ([`6cea9b5`])
- build: clippy fixes (#69) ([`75a6d95`])
- refactor: Extract storage into `jp_storage` crate (#68) ([`e0aa3f9`])
- build: Update Rust toolchain to `nightly-2025-05-19` (#67) ([`da456fc`])
- fix: Skip title generation for non-persistent sessions (#66) ([`47c06a4`])
- refactor: Replace `backoff` with `backon` for retry logic (#65) ([`4a97f39`])
- refactor: Rename `LocalState` to `UserState` (#64) ([`3214bcf`])
- refactor: Extract `TombMap` to `jp_tombmap` crate (#63) ([`3633e6c`])
- feat: set target workspace with `--workspace` flag (#62) ([`6dbe6b5`])
- test: Improve VCR-based snapshot testing with insta (#60) ([`f2369a0`])
- feat: Add provider model listing capabilities (#59) ([`a6145fc`])
- feat: Add MCP resources handler (#58) ([`b226b0f`])
- ci: Add commit message tooling (#54) ([`1e03075`])
- refactor: Refactor query flow and extract helper methods (#50) ([`786b0bf`])
- fix: handle duplicate workspace IDs and unsupported platforms (#48) ([`cb492ef`])
- feat: add command output handler (#47) ([`f336253`])
- feat: add Anthropic provider integration (#45) ([`f07c5fc`])
- feat: enhance verbosity control with all-crate tracing (#44) ([`60b31b2`])
- refactor: empty strings in config setters unset optionals (#41) ([`a414405`])
- test: add missing tests (#39) ([`6fe61da`])
- feat: support structured output (#36) ([`011761f`])
- feat: add Jinja2 template support for queries (#35) ([`8cdcb17`])
- feat: support external config files via `--cfg @<path>` flag (#34) ([`7180a75`])
- refactor: rename `--config` flag to `--cfg` (#33) ([`7f86aeb`])
- refactor: allow disabling inheritance from CLI (#32) ([`72e036e`])
- chore: update project configuration and documentation (#31) ([`b75fd14`])
- refactor: improve configuration handling and parsing (#30) ([`5c1fde9`])
- feat: add persona and context configuration options (#29) ([`e69db47`])
- refactor: adjust log levels and enhance configuration (#21) ([`515c5e7`])
- feat: add conversation title generation settings (#20) ([`2f12759`])
- perf: improveo overall performance of CLI commands (#19) ([`f12e6f3`])
- feat: automated title generation (#18) ([`7ffc2cc`])
- feat: improve MCP loading and configuration editing (#16) ([`ca1c1df`])
- fix: clean up `QUERY_MESSAGES.md` file when empty (#11) ([`c63c590`])
- refactor: rename `private` flag to `local` (#10) ([`548f948`])
- feat: add `--no-persist` flag to disable state persistence (#7) ([`328f0aa`])
- feat: automated llm-based conversation title generation (#6) ([`5129eb9`])
- refactor: Improve help formatting (#4) ([`683d43f`])
- fix: ensure consistent EOF newlines in workspace files (#3) ([`932cdd0`])
- feat: edit conversation details (#2) ([`ee0f359`])
- feat: add support for private conversations (#1) ([`f56e837`])

## [0.1.0] – _v0.1.0_

_2025.05.02_

### Contributions

This release is made possible by the following people (in alphabetical order).
Thank you all for your contributions. Your work – no matter how significant – is
greatly appreciated by the community. 💖

- Jean Mertz (<git@jeanmertz.com>)

### Changes

#### Features

- **officially hook Jean-Pierre into the project!!!!** ([`3e59e1b`])

- **return error when trying to delete active conversation** ([`f2f5d27`])

- **create missing directory on `init`** ([`e50590e`])

- **add workspace management crate** ([`6dda179`])

  Introduces `jp_workspace` crate for managing (persisted) state of JP.

  - Workspace state management and persistence
  - Support for personas, models, conversations and messages
  - Local and shared storage handling
  - Atomic file operations
  - Unique workspace ID generation

- **add new crate for LLM provider integrations** ([`e0fb3b9`])

  Adds a new `jp_llm` crate that handles interactions with LLM providers:

  - Support for OpenAI and Openrouter providers
  - Future support for Anthropic, Google, Deepseek, and Xai
  - Streaming chat completion functionality
  - Tool/function calling capabilities
  - Error handling and type conversions
  - Message formatting and thread management

- **implement OpenRouter API client** ([`cd47a14`])

  - Add OpenRouter API client with streaming chat completion support
  - Implement error handling and retry mechanisms
  - Add request/response type definitions
  - Include test fixtures and recording infrastructure
  - Support conventional OpenRouter API features like tool calls

  At the time of writing, there are no good alternative OpenRouter clients
  in Rust. When one is found, we can switch to using it, if it makes
  sense.

- **implement MCP client crate** ([`8a16d23`])

  The new `jp_mcp` crate provides a client implementation for the Model
  Context Protocol (MCP).

  The client supports the following operations:

  - Start/stop multiple MCP servers in parallel
  - List available tools
  - Call a tool with given parameters

  Currently, the client only supports the `Stdio` transport type.

- **improve tag filtering** ([`940f8ce`])

  Previously, you either searched for a specific query using the `search`
  endpoint, or you could filter notes by tag using the `tagged` endpoint.

  Now, those two endpoints are combined into a single `search` endpoint
  that accepts a query and an optional list of tags to filter by.

  Before:

  ```sh
  jp attachment add "bear://search/my query"
  jp attachment add "bear://tagged/my/tag"
  ```

  After:

  ```sh
  jp attachment add "bear://search/my query"
  jp attachment add "bear://search/?tag=my/tag"
  jp attachment add "bear://search/my query?tag=my/tag"
  ```

- **add Bear Notes integration** ([`9998d31`])

  Implements a new attachment handler for Bear Notes that allows:

  - Fetching single notes by ID
  - Searching notes by content
  - Filtering notes by tags

  The handler interfaces with Bear's SQLite database to retrieve note
  content and metadata in XML format.

- **implement URI-based attachment system** ([`2600658`])

  **jp_attachment**

  This crate provides a trait for handling attachments and a registry for
  handling attachments in a generic way. The trait supports adding,
  removing and listing attachments. Each attachment handler "owns" a
  single URI scheme, and can handle attachments of that scheme.

  **jp_attachment_file_content**

  This crate provides an attachment handler for the full content of a
  file. It supports glob patterns for file inclusion and exclusion.

  More handlers will be added in the future, such as "file headers", "file
  summaries", "web pages", etc.

- **add configuration management crate** ([`d86b311`])

  adds a new `jp_config` crate that handles configuration loading and
  management.

  Configuration is loaded from three sources:

  - file system (lowest precedence)
  - environment variables
  - command-line arguments (highest precedence)

  Configuration files can be in multiple formats, including toml, json and
  yaml. Additionally, multiple configuration files can be merged together,
  until a configuration file sets the `inherit` field to `false`.

  The following tree of configuration files is loaded:

  ```
  /path/to/workspace/{jp, .jp}.{toml, json, json5, yaml, yml}
  /path/to/{jp, .jp}.{toml, json, json5, yaml, yml}
  /path/{jp, .jp}.{toml, json, json5, yaml, yml}
  /{jp, .jp}.{toml, json, json5, yaml, yml}
  $XDG_CONFIG_HOME/jp/config.{toml, json, json5, yaml, yml}
  ```

  For each configuration property, an environment variable can be set to
  override the default value, or the value set through the configuration
  file(s). The environment variable name follows the following pattern:

  ```
  JP_{group}_{property}
  ```

  For example, the `llm.provider.openrouter.api_key_env` property can be
  set with the `JP_LLM_PROVIDER_OPENROUTER_API_KEY_ENV` environment
  variable.

  Additionally, the CLI allows overriding configuration properties using
  the `--config` or `-c` command-line argument. The flag can be passed
  multiple times, and follows the following pattern:

  ```
  --config KEY=VALUE
  ```

  Where `KEY` is the configuration property name, similar to the
  environment variable name, but `.` as a separator instead of `_`, and
  all lowercase. `VALUE` is the string representation of the value, so a
  bool can be set with `true` or `false`. For example:

  ```
  jp --config llm.provider.openrouter.api_key_env=MY_API_KEY ...
  ```

  If a configuration property is not set, a default value is used.

#### Miscellaneous Tasks

- **add initial README** ([`0fd5b96`])

- **add missing files** ([`33ca0dd`])

- **minor terminal output tweak** ([`147a24d`])

- **cargo tweaks** ([`6383eb9`])

- **clippy fixes** ([`c54ecf4`])

- **update MSRV** ([`407bc4d`])

- **remove unneeded files** ([`b84ab79`])

- **update dependencies** ([`d17488c`])

- **remove unused dependencies** ([`6f4f866`])

- **fix typos.toml config** ([`be90120`])

- **update project configuration and tooling** ([`5ab9d80`])

  - add workspace schema for dependency validation
  - update gitignore and add ripgrep ignore patterns
  - refine rustfmt and taplo formatting rules
  - remove docker configuration
  - add llvm-tools to rust toolchain

- **project tooling** ([`37ba3f4`])

#### Bug Fixes

- **don't store new conversations without messages** ([`ee6ab21`])

#### Refactoring

- **more fine-grained deletion/modification tracking** ([`1e8d05f`])

  Before, the storage system used a temporary directory to copy the
  workspace state to upon initialization of the CLI. This was done to
  ensure atomic replacement of the state directory, but this came with
  downsides:

  - Any storage files *not* loaded into memory (e.g. due to
    deserialization errors) were removed once the new state was persisted
    and the entire storage directory was replaced.
  - When tracking the storage directory in a VCS, every run of the CLI
    would change the storage directory metadata, potentially causing
    non-relevant changes to be committed.
  - The wholesale copying/replacing of the storage directory was
    inefficient.

  With this commit, the storage system is no longer *atomic* (with some
  additional work, it could be made to be), but we do gain other benefits:

  - The workspace state is loaded from the storage directory directly,
    without the need for a temporary copy.
  - Internally, modifications and deletions are tracked, to allow
    fine-grained control over what is persisted or removed.
  - VCS metadata is no longer updated for every run of the CLI, only when
    a file has actually changed.

  A new `TombMap` type is introduced to track deletions and modifications
  to the workspace state. This is a direct copy of Rust's `HashMap`, with
  minor modifications to track deletions/modifications.

  Additionally, the different `*Id` types have been updated to correctly
  convert between paths/filenames and IDs.

- **restructure project into workspace architecture** ([`a4f9b58`])

  - Convert project into a Cargo workspace with multiple crates
  - Add workspace-level dependency management
  - Configure workspace-level lints and settings
  - Update and standardize dependency versions
  - Add workspace metadata (license, docs, etc.)
  - Remove direct dependencies in favor of workspace dependencies

- **several tweaks and fixes to existing crates** ([`7a24104`])

- **remove legacy codebase** ([`93aa603`])

  Removes the entire legacy codebase in `src/`. The project is now a
  "virtual workspace" that moves all logic into individual `jp-*` crates
  in `crates/`.

  A few old half working features are removed, such as the "server", but
  these will be re-added in a future commit.

<!-- [releases] -->

[unreleased]: https://github.com/dcdpr/jp/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/dcdpr/jp/releases/tag/v0.1.0

<!-- [commits] -->

[`bd642d2`]: https://github.com/dcdpr/jp/commit/bd642d2a2b9ca7f63866e87ac0a11ca9d3a2a989
[`510a744`]: https://github.com/dcdpr/jp/commit/510a7441df3fa6b5e25932c8dc17c9dbced2a531
[`f637719`]: https://github.com/dcdpr/jp/commit/f6377191515932862346623b2f8f046a99801624
[`9eb6b0e`]: https://github.com/dcdpr/jp/commit/9eb6b0ed2bba1a44af9dad21704706a722a32a45
[`91d3fc0`]: https://github.com/dcdpr/jp/commit/91d3fc008a901a78902e0f0991436ce031d04ff6
[`81cfc84`]: https://github.com/dcdpr/jp/commit/81cfc847931d81252cc0e2c69b23d0ef177d894b
[`3092918`]: https://github.com/dcdpr/jp/commit/30929188e2ac1d6e81698f6323122252c2638f4d
[`bad7dc8`]: https://github.com/dcdpr/jp/commit/bad7dc8aaec6fe6bd0489ebad414d239942ec697
[`d63a130`]: https://github.com/dcdpr/jp/commit/d63a13025a4893f8df77ce54ee1eaac9c3917bb9
[`714ac4a`]: https://github.com/dcdpr/jp/commit/714ac4a7595b7a79e96c75d266632619e8faade4
[`e8800a0`]: https://github.com/dcdpr/jp/commit/e8800a0ea21769ed3e41f2bd01e2640c0af711f3
[`6eb984b`]: https://github.com/dcdpr/jp/commit/6eb984b2d438084e8c0a781f7cecdf451ddfdd88
[`db9f418`]: https://github.com/dcdpr/jp/commit/db9f4182ebbbee09b7a5d1bb910d2cc1942600c7
[`05a8350`]: https://github.com/dcdpr/jp/commit/05a83507c6392ce24d99a75d5bc7071f07e5c36e
[`d9a7f91`]: https://github.com/dcdpr/jp/commit/d9a7f9153f2a1fdc15396754bf3073c131f7dd97
[`c3e845f`]: https://github.com/dcdpr/jp/commit/c3e845f37543fdb8956910ece05b13072ab548ff
[`7a94e66`]: https://github.com/dcdpr/jp/commit/7a94e6620f49899d4242e355862f7e227012504f
[`b80e7c8`]: https://github.com/dcdpr/jp/commit/b80e7c86c92ff90ce34a7176ef1b3933a97d6dba
[`8be4e34`]: https://github.com/dcdpr/jp/commit/8be4e34520f76622e582eb0617d04305e4350578
[`5a2fd94`]: https://github.com/dcdpr/jp/commit/5a2fd941fb8a797727482b5ad3525d8b3aabf6f0
[`51fe290`]: https://github.com/dcdpr/jp/commit/51fe290c6f76df50f35a1797c5a0d25e5850d17d
[`85a74dc`]: https://github.com/dcdpr/jp/commit/85a74dcc9d9f38670c15f026e128c438b1416b30
[`be427d9`]: https://github.com/dcdpr/jp/commit/be427d914204ca50200183ce849741ed29d8ad97
[`a8112bf`]: https://github.com/dcdpr/jp/commit/a8112bf8d4dbc225da8f1e589343ee66cb4b4584
[`34417a2`]: https://github.com/dcdpr/jp/commit/34417a2de670301a57ea22e8e2ff711aba77754b
[`35fd2c4`]: https://github.com/dcdpr/jp/commit/35fd2c4cf4d50c96b84d1513ed7fc18da881b685
[`da4dc7d`]: https://github.com/dcdpr/jp/commit/da4dc7d0bdf1c8e014be158eabee0da7ae321a74
[`61f293f`]: https://github.com/dcdpr/jp/commit/61f293f1877548e6254ec1d42b4ba9103bff7e49
[`9893602`]: https://github.com/dcdpr/jp/commit/9893602a5af257ec76775e0f8a10921640a2271d
[`34afef5`]: https://github.com/dcdpr/jp/commit/34afef5af8364150e1efc86ecc4cd0887a05c4b5
[`b2deea1`]: https://github.com/dcdpr/jp/commit/b2deea1d4feb52e3f67a2b44633bdf2522f941b2
[`538f417`]: https://github.com/dcdpr/jp/commit/538f417d4234e32137a3832c659edd000835eb77
[`c411abc`]: https://github.com/dcdpr/jp/commit/c411abc7ffff8950307bd94c1064dc41d04587e7
[`1e52932`]: https://github.com/dcdpr/jp/commit/1e52932112110c6fde09ace212ef93a2a746f5bb
[`ce1ab70`]: https://github.com/dcdpr/jp/commit/ce1ab70ba387f86907cd557e6db21eb9ddb473bd
[`c5856db`]: https://github.com/dcdpr/jp/commit/c5856db8840f999279b04c3446122799ccbe83b2
[`e20c97c`]: https://github.com/dcdpr/jp/commit/e20c97c340286a6d9144bf651801b06931ddc36e
[`c292644`]: https://github.com/dcdpr/jp/commit/c292644b2077948f18645eec6cd31bebc095b603
[`6146e57`]: https://github.com/dcdpr/jp/commit/6146e57ed7ca7a61c8902929858a2069df1c8228
[`6cea9b5`]: https://github.com/dcdpr/jp/commit/6cea9b5052fb109a97a520e7737a6a8753554b28
[`75a6d95`]: https://github.com/dcdpr/jp/commit/75a6d95520a3ae3d322c05c161e551b9f474e891
[`e0aa3f9`]: https://github.com/dcdpr/jp/commit/e0aa3f9bec445f763a237af6d82355f6f7400cd5
[`da456fc`]: https://github.com/dcdpr/jp/commit/da456fcb8f9c9d25f20c0bfb8c7e90977a5ee0ce
[`47c06a4`]: https://github.com/dcdpr/jp/commit/47c06a44c9a181d704e33f19e6f6cf0197ea9347
[`4a97f39`]: https://github.com/dcdpr/jp/commit/4a97f3941b7939dc7ea6ae6178ed86118ac41176
[`3214bcf`]: https://github.com/dcdpr/jp/commit/3214bcf1c60aafec43f147a52d3d845136f75f67
[`3633e6c`]: https://github.com/dcdpr/jp/commit/3633e6c96cceed988ad59f4e03c01d399459bf2c
[`6dbe6b5`]: https://github.com/dcdpr/jp/commit/6dbe6b55c7d5208059bd26121207f38047395212
[`f2369a0`]: https://github.com/dcdpr/jp/commit/f2369a0a29776eefa766d3ea4e5b7963869affbd
[`a6145fc`]: https://github.com/dcdpr/jp/commit/a6145fcbc100a9c75848b3c2da3f9cd4f2d92229
[`b226b0f`]: https://github.com/dcdpr/jp/commit/b226b0f25e24499da03735187b73f71ae83e854c
[`1e03075`]: https://github.com/dcdpr/jp/commit/1e03075f1ca8e18ac1fcc8b56289f405f7c0bb16
[`786b0bf`]: https://github.com/dcdpr/jp/commit/786b0bfe86aa5579bb77ccd2833ed096f7fe016f
[`cb492ef`]: https://github.com/dcdpr/jp/commit/cb492efcd44663a7446503ba4f8a0433c45e0cfa
[`f336253`]: https://github.com/dcdpr/jp/commit/f33625389d0770982bd54bc8930d23900e53e580
[`f07c5fc`]: https://github.com/dcdpr/jp/commit/f07c5fcc08160c89412f2f0d61eebd33886091ab
[`60b31b2`]: https://github.com/dcdpr/jp/commit/60b31b28cbb5dd6ea44b68db86cbc0051becc900
[`a414405`]: https://github.com/dcdpr/jp/commit/a414405efb5f4217a2b58269da687a1a0c3bc15e
[`6fe61da`]: https://github.com/dcdpr/jp/commit/6fe61da57453bc8b769c7204cf1fd43e6f8e1aa3
[`011761f`]: https://github.com/dcdpr/jp/commit/011761ff435b7af43b2f0ee0bd7be71e62983ebb
[`8cdcb17`]: https://github.com/dcdpr/jp/commit/8cdcb172f0a28182ffb6815342153647dd7ee584
[`7180a75`]: https://github.com/dcdpr/jp/commit/7180a753dc7beb4a825342e5311bb6c3b916c153
[`7f86aeb`]: https://github.com/dcdpr/jp/commit/7f86aeb1ff8ef3b5cecba2ac410dd08a125f4676
[`72e036e`]: https://github.com/dcdpr/jp/commit/72e036eac4445620bfb7a3e28942544791927230
[`b75fd14`]: https://github.com/dcdpr/jp/commit/b75fd140820c66d51d2cbe3ac001f3375a4960f3
[`5c1fde9`]: https://github.com/dcdpr/jp/commit/5c1fde957841cf120748247d0886fb923ef5e28b
[`e69db47`]: https://github.com/dcdpr/jp/commit/e69db4751834fd2a54078c6270428d7ee904e47a
[`515c5e7`]: https://github.com/dcdpr/jp/commit/515c5e7f202471109bf31471bad6cc87b740d209
[`2f12759`]: https://github.com/dcdpr/jp/commit/2f12759ceb040dfdf3b45d0a015e503f64e65758
[`f12e6f3`]: https://github.com/dcdpr/jp/commit/f12e6f3b0b682e60c3f2a47f48b48ed037b8a784
[`7ffc2cc`]: https://github.com/dcdpr/jp/commit/7ffc2cc8b519c243e3ff017d9caeb4dcd14383e6
[`ca1c1df`]: https://github.com/dcdpr/jp/commit/ca1c1df0f92941d9ec418f7f941da011072e1ce8
[`c63c590`]: https://github.com/dcdpr/jp/commit/c63c5900706d4d3b760870a8f8ef79e942406a5e
[`548f948`]: https://github.com/dcdpr/jp/commit/548f94840b0a2ea7e9ec62febbd7ed59b9e755c3
[`328f0aa`]: https://github.com/dcdpr/jp/commit/328f0aa5dd1281f12911643d2008eecac2352909
[`5129eb9`]: https://github.com/dcdpr/jp/commit/5129eb9e4da30fa7cb4830785ce648b12b3b20fd
[`683d43f`]: https://github.com/dcdpr/jp/commit/683d43fca695120ef9121e75d02db56453fb8843
[`932cdd0`]: https://github.com/dcdpr/jp/commit/932cdd0eec190fc1bc46f7695de28aaa77ca4a5b
[`ee0f359`]: https://github.com/dcdpr/jp/commit/ee0f359e3d3c4f11a3e71e5829f0205ac4d7f990
[`f56e837`]: https://github.com/dcdpr/jp/commit/f56e83708fb5c1fd52999ed8f599e03bcfed2dd3
[`3e59e1b`]: https://github.com/dcdpr/jp/commit/3e59e1b76e5297ab86ab29d9f292be6672096a64
[`0fd5b96`]: https://github.com/dcdpr/jp/commit/0fd5b96fcf6ac4983b4d6d42d55961db85696586
[`33ca0dd`]: https://github.com/dcdpr/jp/commit/33ca0dd77efb7ec93dec4a9d718387dbd9531fa5
[`147a24d`]: https://github.com/dcdpr/jp/commit/147a24db399420b13dfd50fd4f2baf84bcecaa3c
[`ee6ab21`]: https://github.com/dcdpr/jp/commit/ee6ab21e2e75144ac5a5d212161cce5fc0744d0e
[`f2f5d27`]: https://github.com/dcdpr/jp/commit/f2f5d279e1cc47e9f9fd4ecc72e6e33ffc1db906
[`1e8d05f`]: https://github.com/dcdpr/jp/commit/1e8d05f3391bf33a6f9c758c1d8d00a98ecddbd5
[`6383eb9`]: https://github.com/dcdpr/jp/commit/6383eb9e161e709d637b971e9dda5ed44f8406b4
[`e50590e`]: https://github.com/dcdpr/jp/commit/e50590e33cbba7f4f178555ba967684f3f32d650
[`c54ecf4`]: https://github.com/dcdpr/jp/commit/c54ecf4035ebd31fed2dea12d685e09ff858b736
[`407bc4d`]: https://github.com/dcdpr/jp/commit/407bc4d746e4a18f879a2ab4b6c9798bcaadd881
[`b84ab79`]: https://github.com/dcdpr/jp/commit/b84ab792fed0c27d38c05fd96f49f64ebec44d8d
[`d17488c`]: https://github.com/dcdpr/jp/commit/d17488c0710e6992c53ea9daf0e4c1253d156e6e
[`6f4f866`]: https://github.com/dcdpr/jp/commit/6f4f866f5e15c22d2fed8a52219e1a8d99ce6398
[`be90120`]: https://github.com/dcdpr/jp/commit/be9012085c839b3e4ba311d2e182aa180b997956
[`a4f9b58`]: https://github.com/dcdpr/jp/commit/a4f9b5856620ef41d1b837c5fcc8b711d1ecfdfc
[`5ab9d80`]: https://github.com/dcdpr/jp/commit/5ab9d80f1a85d147a659b261b1d94dd8cbbae75e
[`7a24104`]: https://github.com/dcdpr/jp/commit/7a2410475c388b8a4fb213b3cb8eaa49bd4145ee
[`93aa603`]: https://github.com/dcdpr/jp/commit/93aa6033270268347bed220d94d35866f7d8bc7f
[`6dda179`]: https://github.com/dcdpr/jp/commit/6dda179e4722b6bc9ae31b57068ce95b86d41d70
[`e0fb3b9`]: https://github.com/dcdpr/jp/commit/e0fb3b906a65add1576b67891f6247572fe9651d
[`cd47a14`]: https://github.com/dcdpr/jp/commit/cd47a14061ad41ec1ab4c397af22af8e79317573
[`8a16d23`]: https://github.com/dcdpr/jp/commit/8a16d2378ead10aff02a18719c838dd3143f21c8
[`940f8ce`]: https://github.com/dcdpr/jp/commit/940f8ceabc692f5f177387a61984adf3e41e90b6
[`9998d31`]: https://github.com/dcdpr/jp/commit/9998d31f9e80cf6e61c77d2f32e18d520cdc6acd
[`2600658`]: https://github.com/dcdpr/jp/commit/2600658e2a382a8ca453780e25625aa4d8223574
[`d86b311`]: https://github.com/dcdpr/jp/commit/d86b3111c8b87da2b9c15050a3d9c3b2a97a0013
[`37ba3f4`]: https://github.com/dcdpr/jp/commit/37ba3f44703188c7b29f7472d75a8e3378a03f94
