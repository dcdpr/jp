use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use chrono::NaiveDate;
use pretty_assertions::assert_eq;
use regex::Regex;

use super::*;
use crate::options::{Pack, Reference};

/// Answers each command with a canned result after a delay, and records the
/// commands it ran.
#[derive(Default)]
struct FakeRunner {
    answers: BTreeMap<String, (Duration, Result<String, String>)>,
    ran: Mutex<Vec<String>>,
}

impl FakeRunner {
    fn answer(mut self, command: &str, delay_ms: u64, result: Result<&str, &str>) -> Self {
        self.answers.insert(
            command.to_owned(),
            (
                Duration::from_millis(delay_ms),
                result.map(ToOwned::to_owned).map_err(ToOwned::to_owned),
            ),
        );
        self
    }
}

impl Run for FakeRunner {
    fn run(&self, command: &[String]) -> Output {
        let command = command.join(" ");
        self.ran.lock().unwrap().push(command.clone());
        let (delay, result) = self
            .answers
            .get(&command)
            .cloned()
            .unwrap_or_else(|| (Duration::ZERO, Err(format!("no answer for `{command}`"))));

        Box::pin(async move {
            tokio::time::sleep(delay).await;
            result
        })
    }
}

fn args(command: &str) -> Vec<String> {
    command.split(' ').map(ToOwned::to_owned).collect()
}

fn options() -> InjectOptions {
    InjectOptions {
        threshold: 0.5,
        instructions: "Which of these?".to_owned(),
        max_chars: 8000,
        packs: BTreeMap::from([("oldest".to_owned(), Pack {
            when: "The message asks for the oldest tickets.".to_owned(),
            title: "Oldest tickets".to_owned(),
            command: args("jp ticket list --order filed"),
        })]),
        references: vec![
            Reference {
                pattern: Regex::new(r"\bT-[0-9a-z]{7}\b").unwrap(),
                command: args("jp ticket show {0}"),
            },
            Reference {
                pattern: Regex::new(r"\bRFD ([0-9]{3})\b").unwrap(),
                command: args("jp rfd show {1}"),
            },
        ],
    }
}

fn injector(runner: FakeRunner) -> (Injector, Arc<FakeRunner>) {
    let runner = Arc::new(runner);
    let injector = Injector::new(options(), Arc::clone(&runner) as Arc<dyn Run>);

    (injector, runner)
}

fn pick(name: &str, probability: f64) -> Pick {
    Pick {
        name: name.to_owned(),
        probability,
    }
}

fn lookup(title: &str, command: &str) -> Lookup {
    Lookup {
        title: title.to_owned(),
        command: args(command),
    }
}

#[test]
fn a_message_looks_up_each_id_it_names_then_the_picked_pack() {
    let (injector, _) = injector(FakeRunner::default());

    let lookups = injector.lookups(
        "Ada Client: is T-1e92tft blocked by RFD 127, or by T-0n0pjkk? T-1e92tft first.",
        Some(&pick("oldest", 0.8)),
    );

    assert_eq!(lookups, [
        lookup("T-1e92tft", "jp ticket show T-1e92tft"),
        lookup("T-0n0pjkk", "jp ticket show T-0n0pjkk"),
        lookup("RFD 127", "jp rfd show 127"),
        lookup("Oldest tickets", "jp ticket list --order filed"),
    ]);
}

#[test]
fn a_message_looks_up_at_most_five_ids() {
    let (injector, _) = injector(FakeRunner::default());

    let lookups = injector.lookups(
        "T-0000001 T-0000002 T-0000003 T-0000004 T-0000005 T-0000006",
        None,
    );

    assert_eq!(
        lookups.iter().map(|l| l.title.as_str()).collect::<Vec<_>>(),
        [
            "T-0000001",
            "T-0000002",
            "T-0000003",
            "T-0000004",
            "T-0000005"
        ]
    );
}

#[test]
fn only_a_confident_pick_of_a_configured_pack_is_looked_up() {
    let (injector, _) = injector(FakeRunner::default());

    assert_eq!(
        injector.lookups("what's old?", Some(&pick("oldest", 0.5))),
        [lookup("Oldest tickets", "jp ticket list --order filed")]
    );
    assert_eq!(
        injector.lookups("what's old?", Some(&pick("oldest", 0.49))),
        []
    );
    assert_eq!(
        injector.lookups("what's old?", Some(&pick("newest", 0.9))),
        []
    );
    assert_eq!(injector.lookups("what's old?", None), []);
}

#[tokio::test(start_paused = true)]
async fn lookups_run_concurrently_and_keep_their_order() {
    let (injector, runner) = injector(
        FakeRunner::default()
            .answer("jp ticket show T-1e92tft", 300, Ok("# T-1e92tft\n"))
            .answer("jp ticket show T-0n0pjkk", 100, Err("No T-0n0pjkk."))
            .answer("jp ticket list --order filed", 200, Ok("T-005zd00 Done\n")),
    );

    let injected = injector
        .inject(vec![
            lookup("T-1e92tft", "jp ticket show T-1e92tft"),
            lookup("T-0n0pjkk", "jp ticket show T-0n0pjkk"),
            lookup("Oldest tickets", "jp ticket list --order filed"),
        ])
        .await;

    assert_eq!(injected.sections, [
        Section {
            lookup: lookup("T-1e92tft", "jp ticket show T-1e92tft"),
            output: "# T-1e92tft\n".to_owned(),
        },
        Section {
            lookup: lookup("Oldest tickets", "jp ticket list --order filed"),
            output: "T-005zd00 Done\n".to_owned(),
        },
    ]);
    assert_eq!(injected.failures, [(
        lookup("T-0n0pjkk", "jp ticket show T-0n0pjkk"),
        "No T-0n0pjkk.".to_owned()
    )]);
    assert_eq!(injected.took, Duration::from_millis(300));
    assert_eq!(runner.ran.lock().unwrap().len(), 3);
}

fn november(day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, day).unwrap()
}

fn section(title: &str, command: &str, output: &str) -> Section {
    Section {
        lookup: lookup(title, command),
        output: output.to_owned(),
    }
}

#[test]
fn render_appends_a_dated_section_per_lookup() {
    let rendered = render(
        "Ada Client: what's the oldest ticket?",
        november(16),
        &[
            section(
                "Oldest tickets",
                "jp ticket list --order filed",
                "T-005zd00 Done         2026-08-06 Prepare ticket board\n",
            ),
            section(
                "T-005zd01",
                "jp ticket show T-005zd01",
                "# T-005zd01: Shell mode\n",
            ),
        ],
        8000,
    );

    assert_eq!(
        rendered,
        "Ada Client: what's the oldest ticket?

# Auto-Injected Context

The bridge looked this up on 2026-11-16 because the message above seemed to need it. The sender \
         didn't write it, and it may be incomplete or irrelevant.

## Oldest tickets

Output of `jp ticket list --order filed`:

```text
T-005zd00 Done         2026-08-06 Prepare ticket board
```

## T-005zd01

Output of `jp ticket show T-005zd01`:

```text
# T-005zd01: Shell mode
```"
    );
}

#[test]
fn render_truncates_long_output_and_says_so() {
    let rendered = render(
        "hi",
        november(16),
        &[section("Big", "cat big", "abcdéfgh")],
        5,
    );

    assert!(
        rendered.ends_with("```text\nabcdé\n[Truncated at 5 characters.]\n```"),
        "{rendered}"
    );
}

#[test]
fn render_fences_output_holding_a_fence_with_a_longer_one() {
    let rendered = render(
        "hi",
        november(16),
        &[section(
            "T-1",
            "show",
            "before\n````toml\nx = 1\n````\nafter",
        )],
        8000,
    );

    assert!(
        rendered.ends_with("`````text\nbefore\n````toml\nx = 1\n````\nafter\n`````"),
        "{rendered}"
    );
}
