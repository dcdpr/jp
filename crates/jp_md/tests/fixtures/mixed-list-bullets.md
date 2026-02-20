JP is a Rust-based command-line toolkit designed to augment software development workflows. You can initialize the
environment by running `jp init` in your terminal. This tool bridges the gap between local codebases and LLMs.
Visit the [Rust website](https://www.rust-lang.org/) for more information on the language.

The architecture focuses on low latency and extensibility. Users can define custom prompts within the
`config.toml` file to suit specific project needs. For further details, check the [repository](https://github.com/).

- Core functionalities:
  * Automated code review.
  * Unit test generation.
    + Support for `cargo test`.
    + Mocking complex dependencies.
  * Refactoring suggestions.
