> [!NOTE]
> Originally the client was forked from [`anthropic-sdk`](https://github.com/Mixpeal/anthropic-sdk) which no longer seems to be maintained. There might still be some references, even though the code has been rewritten from scratch.

## async-anthropic

A client for the anthropic messages api, written in Rust. There are plenty of clients on crates.io, but we figured we needed another one. Specifically, a straightforward builder api, robust error handling, and room to grow. Tests are also nice.

### Features

- [x] Messages API
- [x] Models API
- [x] Tool use
- [x] Support all API parameters
- [x] Automatic [backoff](https://crates.io/crates/backoff)
- [x] Tracing
- [x] Streaming
- [ ] Non-text messages

### Installation

Add the project with `cargo`:

```bash
cargo add async-anthropic
```

### Usage

#### Basic Usage

For non-streaming responses, you can use the SDK as follows:

```rust
    let client = Client::default();

    let request = CreateMessagesRequestBuilder::default()
        .model("claude-3-5-sonnet-20241022")
        .messages(vec![MessageBuilder::default()
            .role(MessageRole::User)
            .content("Hello claude!!")
            .build()
            .unwrap()])
        .build()
        .unwrap();

    let response = client.messages().create(request).await?;

    println!("{:?}", response);
```

See `/examples` for more examples.

### Contributing

Contributions are welcome! This project was quickly drafted together to add anthropic support to other bosun projects, and several features are missing. If you'd like to contribute, please open an issue or a pull request.
