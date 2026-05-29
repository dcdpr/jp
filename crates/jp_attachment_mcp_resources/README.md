# Attachment: MCP Resources

An attachment handler for retrieving MCP resources.

You can use it by using the URI's as specified by the MCP server resource API,
but prefix the URI with `mcp+` to ensure this attachment handler is used.

## Usage

As an example, for the [`github-mcp-server`], for any of its listed [resources]:

```sh
jp attachment add "mcp+repo://{owner}/{repo}/contents{/path*}"
```

[`github-mcp-server`]: https://github.com/github/github-mcp-server
[resources]: https://github.com/github/github-mcp-server#resources
