# Bookworm

An MCP server (and accompanying CLI utilities) for working with [docs.rs] Rust
crate documentation.

The `bookworm` binary exposes three subcommands:

```sh
bookworm mcp [--jp]              # run the MCP server on stdio
bookworm download <crate> [-v VERSION] [-r ROOT]
bookworm index <SOURCE> [-o OUTPUT]
```

## MCP server

Run the server locally with:

```sh
cargo run -p bookworm -- mcp
```

To enable the JP tool protocol (responses wrapped in `jp_tool::Outcome` JSON):

```sh
cargo run -p bookworm -- mcp --jp
```

Adding the server to your MCP client depends on the client, but the following
example works for Claude.ai:

```json
{
  "mcpServers": {
    "bookworm": {
      "command": "/path/to/bookworm",
      "args": ["mcp"]
    }
  }
}
```

### Tools

The following tools are available to an LLM with MCP client capabilities:

#### `crates_search`

Search for crates matching the given query.

The returned list contains a list of URIs for each crate to fetch additional
crate information.

#### `crate_search_items`

Search for item definitions within a crate.

Each item contains:

- Item Path (e.g. `serde_json::value::Value`)
- Item Type (e.g. `enum`)
- Type Signature
- Documentation
- Related Resource URIs

#### `crate_resource`

Fetch a resource for a crate, by URI.
Supported URIs:

- `crate://{crate_name}` — list crate versions
- `crate://{crate_name}/{crate_version}` — get metadata
- `crate://{crate_name}/{crate_version}/readme` — get readme content
- `crate://{crate_name}/{crate_version}/items` — list item resources
- `crate://{crate_name}/{crate_version}/src` — list source code resources
- `crate://{crate_name}/{crate_version}/{path}` — get item/src resource

#### `crate_versions`

Get a list of most recent versions of a crate.

#### `crate_readme`

Get the README for a specific crate version, converted to Markdown.

### URI parameters

- `{crate_name}` is the exact name of the crate.
- `{crate_version}` is either a (partial) semver-compatible version number, or
  `latest` for the latest published crate version.

## Operator CLI

### `bookworm download`

Download a crate's documentation from docs.rs into a local directory:

```sh
cargo run -p bookworm -- download regex
```

### `bookworm index`

Index a previously-downloaded documentation tree into a SQLite database used by
the MCP server's search:

```sh
cargo run -p bookworm -- index /tmp/regex/...
```

The default output path is `./index.sqlite`.

[docs.rs]: https://docs.rs
