# Define deny-preserving `access.env` matching on case-insensitive platforms

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-09-05
- **Implements**: 076

`EnvRule::matches` is case-sensitive on every platform.
That keeps a rule set meaning the same thing wherever it is read, but it does
not preserve a deny where the OS resolves variable names case-insensitively.

Given this policy:

```toml
[[conversation.tools.my_tool.access.env]]
name = "*"
read = true

[[conversation.tools.my_tool.access.env]]
name = "GITHUB_TOKEN"
read = false
```

a request for `github_token` misses the exact deny (`"github_token" !=
"GITHUB_TOKEN"`), selects the `*` grant, and then resolves through
`std::env::var` to the same OS variable on Windows.
On Unix the same request usually finds nothing.
Same config, same request, different outcome per platform — the opposite of
what case-sensitive matching is supposed to buy.

Windows is a supported target: `.github/workflows/rust.yml` runs the suite on
`windows-latest`.

## Why it isn't fixed yet

There is no in-tree `env` consumer, so there is no caller to design the rule
against, and the obvious half-measures do not work:

- Canonicalizing only the requested name leaves an exact rule written in another
  case unmatched.
- Matching case-insensitively everywhere widens grants on Unix, where `foo` and
  `FOO` are genuinely different variables.

## Candidates

- Canonicalize the requested name *and* every rule name to one form, on
  platforms where the OS is case-insensitive.
- Match denies case-insensitively while keeping grants exact, so the strict
  direction always wins.

Recorded as an open question in RFD 076's env-rules section.
Settle it alongside the first consumer, and pin it with a case-variant test
then.
