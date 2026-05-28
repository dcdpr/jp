//! Assistant-callable debug tools for `jp`.
//!
//! Each tool in this family runs `jp` inside an isolated sandbox so a
//! destructive command approved by mistake cannot reach the user's real
//! workspace or conversation store. The sandbox combines a detached git
//! worktree (for the source tree) with an alternate `JP_USER_DATA_DIR` (for
//! the user-global data directory) — see [`util::sandbox`] for details.
//!
//! Tools currently exposed:
//!
//! - `debug_jp_profile_sampling` — macOS `sample(1)` wall-clock profile.
//! - `debug_jp_profile_heap` — dhat heap profile.
//! - `debug_jp_trace` — `JP_DEBUG=1` trace log capture and render.

use crate::{
    Context, Tool,
    util::{ToolResult, unknown_tool},
};

pub(crate) mod profile_heap;
pub(crate) mod profile_sampling;
pub(crate) mod trace;
pub(crate) mod util;

pub async fn run(ctx: Context, t: Tool) -> ToolResult {
    match t.name.trim_start_matches("debug_jp_") {
        "profile_heap" => profile_heap::debug_jp_profile_heap(&ctx, &t).await,
        "profile_sampling" => profile_sampling::debug_jp_profile_sampling(&ctx, &t).await,
        "trace" => trace::debug_jp_trace(&ctx, &t).await,
        _ => unknown_tool(t),
    }
}
