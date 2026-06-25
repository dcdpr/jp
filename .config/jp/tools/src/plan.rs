//! The `plan` tool: a lightweight, ordered checklist the assistant can step
//! through during a multi-step task.
//!
//! A plan is a named list of tasks with a single cursor marking the in-progress
//! task.
//! Tasks before the cursor are done, the task at the cursor is in progress, and
//! tasks after it are pending.
//! The assistant drives the cursor forward (`next`) to complete the current
//! task and start the next, or backward (`prev`) to re-open the previous one,
//! optionally moving several steps at once with `count`.
//! Reversing past the first task discards the plan entirely.
//!
//! Plans are persisted per conversation under
//! `<workspace>/.jp/mcp/state/plans/<conversation_id>/<name>.json`, so they
//! survive across tool invocations within a conversation.

use std::cmp::Ordering;

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

use crate::{
    Context, Tool,
    util::{ToolResult, error},
};

/// A named, ordered checklist.
///
/// `current` is the index of the in-progress task.
/// Indices below it are done, indices above it are pending.
/// When `current == tasks.len()` every task is complete and none is in
/// progress.
#[derive(Debug, Serialize, Deserialize)]
struct Plan {
    name: String,
    tasks: Vec<String>,
    current: usize,
}

impl Plan {
    /// Render the task list as a markdown checklist.
    fn checklist(&self) -> String {
        let mut out = String::new();
        for (index, task) in self.tasks.iter().enumerate() {
            let marker = match index.cmp(&self.current) {
                Ordering::Less => "x",
                Ordering::Equal => ">",
                Ordering::Greater => " ",
            };
            out.push_str(&format!("- [{marker}] {task}\n"));
        }
        out
    }

    /// Number of completed tasks, capped at the task count.
    fn done(&self) -> usize {
        self.current.min(self.tasks.len())
    }
}

/// Build the tool output: a one-line `header` followed by the checklist.
fn render_block(header: &str, plan: &Plan) -> String {
    format!("{header}\n\n{}", plan.checklist())
}

/// One-line header for a `next`/`prev` step, e.g. `Advancing plan "x" by 2
/// steps (3/5 complete)`.
/// The "by N steps" suffix is dropped for single-step moves.
fn step_header(verb: &str, name: &str, moved: usize, done: usize, total: usize) -> String {
    let by = if moved > 1 {
        format!(" by {moved} steps")
    } else {
        String::new()
    };
    format!("{verb} plan \"{name}\"{by} ({done}/{total} complete)")
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "consistent with other module run fns"
)]
pub fn run(ctx: Context, t: Tool) -> ToolResult {
    let action: String = t.req("action")?;
    let name: String = t.req("name")?;

    if let Err(message) = validate_name(&name) {
        return error(message);
    }

    let count = match t.opt::<usize>("count")? {
        Some(0) => return error("The \"count\" argument must be at least 1."),
        Some(n) => n,
        None => 1,
    };

    let dir = plans_dir(&ctx.root, &ctx.conversation_id);

    match action.as_str() {
        "create" => {
            let Some(tasks) = t.opt::<Vec<String>>("tasks")? else {
                return error("The \"create\" action requires a non-empty \"tasks\" array.");
            };
            create(&dir, &name, tasks)
        }
        "next" => advance(&dir, &name, count),
        "prev" => retreat(&dir, &name, count),
        other => error(format!(
            "Unknown action \"{other}\". Valid actions are \"create\", \"next\", and \"prev\"."
        )),
    }
}

/// Create (or reset) a plan, marking the first task in progress.
fn create(dir: &Utf8Path, name: &str, tasks: Vec<String>) -> ToolResult {
    let tasks: Vec<String> = tasks.into_iter().map(|t| t.trim().to_owned()).collect();

    if tasks.is_empty() {
        return error("The \"create\" action requires at least one task.");
    }
    if tasks.iter().any(String::is_empty) {
        return error("Task descriptions must not be empty.");
    }

    let plan = Plan {
        name: name.to_owned(),
        tasks,
        current: 0,
    };
    save(dir, &plan)?;
    Ok(render_block(&format!("Creating plan \"{name}\""), &plan).into())
}

/// Complete the in-progress task(s) and start the next one.
fn advance(dir: &Utf8Path, name: &str, count: usize) -> ToolResult {
    let Some(mut plan) = load(dir, name)? else {
        return error(format!(
            "No plan named \"{name}\" exists. Create one first with the \"create\" action."
        ));
    };

    let total = plan.tasks.len();
    if plan.current >= total {
        let header = format!("Advancing plan \"{name}\" ({total}/{total} complete)");
        return Ok(format!(
            "{}\nAll tasks are already complete.",
            render_block(&header, &plan)
        )
        .into());
    }

    let previous = plan.current;
    plan.current = (plan.current + count).min(total);
    save(dir, &plan)?;

    let header = step_header(
        "Advancing",
        name,
        plan.current - previous,
        plan.done(),
        total,
    );
    Ok(render_block(&header, &plan).into())
}

/// Re-open the most recently completed task(s).
/// Reversing past the first task discards the plan.
fn retreat(dir: &Utf8Path, name: &str, count: usize) -> ToolResult {
    let Some(mut plan) = load(dir, name)? else {
        return error(format!("No plan named \"{name}\" exists. Nothing to undo."));
    };

    if count > plan.current {
        std::fs::remove_file(path_for(dir, name))?;
        return Ok(format!("Plan \"{name}\" discarded.").into());
    }

    let previous = plan.current;
    plan.current -= count;
    save(dir, &plan)?;

    let total = plan.tasks.len();
    let header = step_header(
        "Reverting",
        name,
        previous - plan.current,
        plan.done(),
        total,
    );
    Ok(render_block(&header, &plan).into())
}

/// Resolve the per-conversation directory that holds this conversation's plans,
/// inside the workspace's `.jp/mcp/state` tree.
fn plans_dir(root: &Utf8Path, conversation_id: &str) -> Utf8PathBuf {
    root.join(".jp/mcp/state/plans").join(conversation_id)
}

fn path_for(dir: &Utf8Path, name: &str) -> Utf8PathBuf {
    dir.join(format!("{name}.json"))
}

fn save(dir: &Utf8Path, plan: &Plan) -> crate::Result<()> {
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(plan)?;
    std::fs::write(path_for(dir, &plan.name), json)?;
    Ok(())
}

fn load(dir: &Utf8Path, name: &str) -> crate::Result<Option<Plan>> {
    let path = path_for(dir, name);
    if !path.exists() {
        return Ok(None);
    }
    let json = std::fs::read_to_string(&path)?;
    let Ok(plan) = serde_json::from_str::<Plan>(&json) else {
        // A corrupt state file is unreadable and unrepairable here; drop it so
        // the assistant can recreate the plan from scratch.
        std::fs::remove_file(&path)?;
        return Ok(None);
    };
    Ok(Some(plan))
}

/// Reject plan names that would escape the plans directory or collide with the
/// `.json` extension.
/// Used as a filename, so the accepted set is deliberately narrow.
fn validate_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Plan name must not be empty.".to_owned());
    }

    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_'))
    {
        return Err(format!(
            "Plan name \"{name}\" is invalid. Use only letters, digits, spaces, hyphens, and \
             underscores."
        ));
    }

    Ok(())
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
