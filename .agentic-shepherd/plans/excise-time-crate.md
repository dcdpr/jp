# Plan: Excise `time` crate, replace with `chrono`

## Context

The `time` crate is an unallowed dependency (RUSTSEC-2026-0009 and supply-chain policy). It must be completely removed from `cargo tree`. Currently it appears via:

- **8 first-party crates** that directly depend on it
- **`bat` -> `plist` -> `time`** (transitive, no feature flag to disable)
- **`octocrab` -> `jsonwebtoken` -> `simple_asn1` -> `time`** (transitive, no feature flag to disable)

`chrono` is already in the dependency tree transitively (via octocrab, openai_responses, rmcp).

## Approach

Three workstreams, all required for `cargo tree` to stop listing `time`:

### 1. Migrate all first-party code from `time` to `chrono`

**Type mapping:**

| `time` | `chrono` |
|--------|----------|
| `UtcDateTime` | `DateTime<Utc>` |
| `OffsetDateTime` | `DateTime<FixedOffset>` or `DateTime<Utc>` |
| `Date` | `NaiveDate` |
| `UtcOffset` | `FixedOffset` or `Local` |
| `time::Duration` | `chrono::Duration` (or `TimeDelta`) |

**Method mapping:**

| `time` | `chrono` |
|--------|----------|
| `UtcDateTime::now()` | `Utc::now()` |
| `.unix_timestamp_nanos()` | `.timestamp_nanos_opt().unwrap()` (returns i64, not i128) |
| `UtcDateTime::from_unix_timestamp_nanos(n)` | `DateTime::from_timestamp_nanos(n)` |
| `UtcOffset::current_local_offset()` | `Local::now().offset().clone()` |
| `format_description!("[year]-[month]...")` | `"%Y-%m-..."` strftime strings |
| `.format(&desc)` | `.format("%Y-%m-...").to_string()` |
| `.parse(s, &desc)` | `NaiveDateTime::parse_from_str(s, fmt)` |
| `utc_datetime!(2024-01-01 0:00)` | `Utc.with_ymd_and_hms(2024,1,1,0,0,0).unwrap()` |
| `date!(2024-01-01)` | `NaiveDate::from_ymd_opt(2024,1,1).unwrap()` |
| `datetime!(2024-01-01 0:00 utc)` | `Utc.with_ymd_and_hms(2024,1,1,0,0,0).unwrap()` |
| `#[serde(with = "time::serde::timestamp")]` | `#[serde(with = "chrono::serde::ts_seconds")]` |
| `1.hours()` (NumericalDuration ext) | `chrono::Duration::hours(1)` |
| `.to_offset(offset)` | `.with_timezone(&offset)` |
| `.nanosecond()` | `.nanosecond()` (same) |
| `.replace_nanosecond(n)` | `.with_nanosecond(n).unwrap()` |

**Crates to modify (Cargo.toml + source):**

| Crate | Files | Key changes |
|-------|-------|-------------|
| **jp_id** | `Cargo.toml`, `src/lib.rs`, `src/serde.rs` | `UtcDateTime` → `DateTime<Utc>`, custom serde (decisecond precision — note: chrono `timestamp_nanos_opt()` returns `i64` not `i128`, adjust divisor logic) |
| **jp_conversation** | `Cargo.toml`, `src/event.rs`, `src/conversation.rs`, `src/stream.rs` | `UtcDateTime` → `DateTime<Utc>`, serde annotations |
| **jp_format** | `Cargo.toml`, `src/datetime.rs`, `src/conversation.rs` | `UtcDateTime`/`UtcOffset` → `DateTime<Utc>`/`FixedOffset`, format strings, local offset |
| **jp_openrouter** | `Cargo.toml`, `src/types/response.rs` | `OffsetDateTime` → `DateTime<Utc>`, `time::serde::timestamp` → `chrono::serde::ts_seconds` |
| **jp_llm** | `Cargo.toml`, `src/model.rs`, `src/provider/openai.rs`, `src/provider/anthropic.rs`, `src/test.rs` | `Date` → `NaiveDate`, macro literals → constructor calls |
| **jp_storage** | `Cargo.toml`, `src/lib.rs` | `UtcDateTime` → `DateTime<Utc>`, format/parse strings |
| **jp_workspace** | `Cargo.toml`, `src/lib.rs` | `UtcDateTime` → `DateTime<Utc>` |
| **jp_cli** | `Cargo.toml`, `src/cmd/conversation/ls.rs`, `src/editor.rs`, `src/ctx.rs`, `src/cmd/conversation/fork.rs`, `src/cmd/conversation/edit.rs`, `src/cmd/query.rs` | `UtcDateTime`/`UtcOffset` → chrono types |
| **tools** | `.config/jp/tools/Cargo.toml`, `src/github/pulls.rs`, `src/github/issues.rs` | `OffsetDateTime` → `DateTime<FixedOffset>` or `DateTime<Utc>` |

**Workspace root `Cargo.toml`:** Remove `time` from `[workspace.dependencies]`, add `chrono` with needed features (`serde`).

**Special attention — `jp_id/src/serde.rs`:** The custom decisecond serializer uses `i128` from `unix_timestamp_nanos()`. Chrono's `timestamp_nanos_opt()` returns `Option<i64>`. This is fine for dates within ~292 years of epoch. The divisor math stays the same but types change from `i128` to `i64`.

### 2. Replace `bat` with `syntect` (eliminates `bat` -> `plist` -> `time`)

**Scope:** bat is used in exactly one place: `crates/jp_term/src/code.rs` via `PrettyPrinter`. It syntax-highlights code blocks for terminal output.

**Files to modify:**
- `Cargo.toml` (workspace): Remove `bat`, add `syntect`
- `crates/jp_term/Cargo.toml`: Replace `bat` with `syntect`
- `crates/jp_term/src/code.rs`: Rewrite `format()` using syntect directly
- `crates/jp_cli/Cargo.toml`: Remove `bat` dependency (only needed it for error type)
- `crates/jp_cli/src/error.rs`: Remove `Bat` error variant, add syntect equivalent
- `crates/jp_cli/src/cmd.rs`: Remove bat error conversion macro

**Implementation sketch for `code.rs`:**
```rust
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::html::ClassedHTMLGenerator; // or terminal output
use syntect::util::as_24bit_terminal_escaped;
use syntect::easy::HighlightLines;

pub fn format(content: &str, buf: &mut String, config: &Config) -> Result<bool, Error> {
    let ss = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();
    let syntax = match config.language.as_deref() {
        Some(lang) => match ss.find_syntax_by_token(lang) {
            Some(s) => s,
            None => return Ok(false), // unknown syntax
        },
        None => return Ok(false),
    };
    let theme = config.theme.as_deref()
        .and_then(|t| ts.themes.get(t))
        .unwrap_or_else(|| &ts.themes["base16-ocean.dark"]);
    let mut h = HighlightLines::new(syntax, theme);
    for line in LinesWithEndings::from(content) {
        let ranges = h.highlight_line(line, &ss)?;
        let escaped = as_24bit_terminal_escaped(&ranges, config.theme.is_some());
        buf.push_str(&escaped);
    }
    Ok(true)
}
```

**Note:** Need to check if syntect itself pulls in plist. If it does (for loading .tmTheme/.tmLanguage files), use `syntect` with `default-syntaxes` and `default-themes` features which embed assets and may not need plist at runtime. Will verify during implementation.

### 3. Replace `octocrab` with raw `reqwest` + `serde` (eliminates `octocrab` -> `jsonwebtoken` -> `simple_asn1` -> `time`)

**Scope:** octocrab is used only in `.config/jp/tools/src/github/`. reqwest is already a dependency of the tools crate.

**Files to modify:**
- `Cargo.toml` (workspace): Remove `octocrab`
- `.config/jp/tools/Cargo.toml`: Remove `octocrab` dep + `github` feature
- `.config/jp/tools/src/lib.rs`: Remove `#[cfg(feature = "github")]` gates (added during step 1)
- `.config/jp/tools/src/github.rs`: Replace auth init with reqwest client setup
- `.config/jp/tools/src/github/pulls.rs`: Replace octocrab calls with reqwest; define local `DiffEntryStatus` enum
- `.config/jp/tools/src/github/issues.rs`: Replace octocrab calls with reqwest
- `.config/jp/tools/src/github/create_issue_bug.rs`: Replace octocrab calls with reqwest
- `.config/jp/tools/src/github/create_issue_enhancement.rs`: Replace octocrab calls with reqwest
- `.config/jp/tools/src/github/repo.rs`: Replace octocrab calls with reqwest + GraphQL

**Approach:**
- Create a thin `GitHubClient` struct wrapping `reqwest::Client` with base URL + auth token
- Implement pagination helper that follows GitHub `Link` headers
- Define minimal serde response structs for: PullRequest, Issue, Label, Collaborator, FileContent, DiffEntry, SearchResult, Code
- GraphQL: POST to `https://api.github.com/graphql` with JSON body

**REST endpoint mapping:**

| octocrab call | REST endpoint |
|---|---|
| `.current().user()` | `GET /user` |
| `.issues(o,r).get(n)` | `GET /repos/{o}/{r}/issues/{n}` |
| `.issues(o,r).list()` | `GET /repos/{o}/{r}/issues?per_page=100` |
| `.issues(o,r).create(...)` | `POST /repos/{o}/{r}/issues` |
| `.issues(o,r).list_labels_for_repo()` | `GET /repos/{o}/{r}/labels?per_page=100` |
| `.pulls(o,r).get(n)` | `GET /repos/{o}/{r}/pulls/{n}` |
| `.pulls(o,r).list().state(s)` | `GET /repos/{o}/{r}/pulls?state={s}&per_page=100` |
| `.pulls(o,r).list_files(n)` | `GET /repos/{o}/{r}/pulls/{n}/files?per_page=100` |
| `.repos(o,r).list_collaborators()` | `GET /repos/{o}/{r}/collaborators?per_page=100` |
| `.repos(o,r).get_content().path(p).ref(r)` | `GET /repos/{o}/{r}/contents/{p}?ref={r}` |
| `.search().code(q)` | `GET /search/code?q={q}&per_page=100` |
| `.graphql(&query)` | `POST /graphql` |

**Auth pattern:**
```rust
struct GitHubClient {
    client: reqwest::Client,
    token: String,
}
impl GitHubClient {
    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client.get(format!("https://api.github.com{path}"))
            .bearer_auth(&self.token)
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "jp-tools")
    }
}
```

## Verification

1. `cargo build` — compiles without errors
2. `cargo test` — all tests pass
3. `cargo tree -i time` — **must return nothing** (no crate depends on time)
4. `cargo tree | grep time` — only matches like "timeout" not "time v0.3"
5. `cargo vet` — all new dependencies pass supply-chain audit
6. Manually verify syntax highlighting still works in CLI
7. Verify GitHub tool operations still work (list issues, get PR, etc.)

## Order of Operations

1. Migrate first-party `time` → `chrono` (largest, most mechanical change)
2. Replace `bat` → `syntect` (small, isolated)
3. Replace `octocrab` → `reqwest` (moderate, isolated to tools crate)
4. Remove `time` from workspace deps, clean up `Cargo.lock`
5. Run verification steps
